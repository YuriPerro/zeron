//! Orchestration RPCs against a real engine: the store, roles and model
//! resolution that the MCP server and CLI call.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::{Value, json};

use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel, SteeringMode,
};
use zeron_rpc::{RpcClient, methods};

struct CatalogHarness;

#[async_trait]
impl Harness for CatalogHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Catalog"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Low, ReasoningLevel::Medium]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![Model {
            id: "mock-pro".into(),
            label: "Mock Pro".into(),
            description: None,
            reasoning_levels: vec![ReasoningLevel::Low, ReasoningLevel::Medium],
            options: vec![],
        }])
    }
    async fn run(
        &self,
        _request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        Err(HarnessError::Protocol("not runnable".into()))
    }
}

struct Fixture {
    tmp: tempfile::TempDir,
    core: EngineCore,
    client: RpcClient,
}

async fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(CatalogHarness));
    let core = EngineCore::assemble(
        &tmp.path().join("data"),
        Arc::new(registry),
        HarnessId::Mock,
        None,
    )
    .expect("engine core assembles");
    let client = zeron_rpc::memory_client(core.rpc_service());
    Fixture { tmp, core, client }
}

async fn create_space(fx: &Fixture, id: &str) {
    let path = fx.tmp.path().join(id);
    std::fs::create_dir_all(&path).unwrap();
    fx.client
        .call(
            methods::MUTATE,
            json!({
                "op": "createSpace", "spaceId": id,
                "deviceId": fx.core.device_id, "path": path,
            }),
        )
        .await
        .expect("createSpace");
}

async fn create_orchestrator(fx: &Fixture, name: &str, projects: &[&str]) -> Value {
    fx.client
        .call(
            methods::CREATE_ORCHESTRATOR,
            json!({ "name": name, "harness": "mock", "projects": projects }),
        )
        .await
        .expect("CreateOrchestrator")
}

#[tokio::test(flavor = "multi_thread")]
async fn orchestration_rpcs_round_trip() {
    let fx = fixture().await;
    create_space(&fx, "space-1").await;

    let created = create_orchestrator(&fx, "Loja Online", &["space-1"]).await;
    let id = created["orchestratorId"].as_str().unwrap().to_string();
    let chat_id = created["chatId"].as_str().unwrap().to_string();
    assert_eq!(id, "loja-online");

    let chat = fx.core.workspace.chat(&chat_id).unwrap().unwrap();
    assert_eq!(chat.title.as_deref(), Some("Loja Online"));
    assert_eq!(chat.space_id, None);
    assert_eq!(chat.device_id, fx.core.device_id);
    let config = chat.config.unwrap();
    assert_eq!(config.orchestrator.as_deref(), Some(id.as_str()));
    assert_eq!(config.sandbox, SandboxLevel::ReadOnly);
    assert_eq!(config.role, None);
    assert_eq!(config.harness, HarnessId::Mock);

    let stored = fx
        .client
        .call(methods::GET_ORCHESTRATOR, json!({ "orchestratorId": id }))
        .await
        .unwrap();
    assert_eq!(stored["name"], "Loja Online");
    assert_eq!(stored["childHarness"], "mock");
    assert_eq!(stored["projects"], json!(["space-1"]));
    let listed = fx
        .client
        .call(methods::LIST_ORCHESTRATORS, json!({}))
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], id);

    let demand = fx
        .client
        .call(
            methods::ORCH_CREATE_DEMAND,
            json!({ "orchestratorId": id, "title": "Cupom expirado" }),
        )
        .await
        .unwrap()["demand"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(demand.ends_with("-cupom-expirado"), "{demand}");

    for (method, extra) in [
        (methods::ORCH_WRITE_SPEC, json!({ "text": "the spec" })),
        (
            methods::ORCH_WRITE_PLAN,
            json!({ "items": [{ "text": "fix it", "status": "inProgress", "role": "executor" }] }),
        ),
        (
            methods::ORCH_SUBMIT_REPORT,
            json!({ "childId": "child-1", "text": "done" }),
        ),
    ] {
        let mut params = json!({ "orchestratorId": id, "demand": demand });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        fx.client.call(method, params).await.expect(method);
    }

    let view = fx
        .client
        .call(
            methods::ORCH_READ_DEMAND,
            json!({ "orchestratorId": id, "demand": demand }),
        )
        .await
        .unwrap();
    assert_eq!(view["spec"], "the spec");
    assert!(view["plan"].as_str().unwrap().contains("fix it"));
    assert_eq!(view["reports"], json!([["child-1", "done"]]));

    fx.client
        .call(
            methods::ORCH_REMEMBER,
            json!({ "orchestratorId": id, "note": "prefers small PRs" }),
        )
        .await
        .unwrap();
    fx.client
        .call(
            methods::ORCH_CLOSE_DEMAND,
            json!({ "orchestratorId": id, "demand": demand, "summary": "shipped" }),
        )
        .await
        .unwrap();
    let err = fx
        .client
        .call(
            methods::ORCH_READ_DEMAND,
            json!({ "orchestratorId": id, "demand": demand }),
        )
        .await
        .expect_err("closed demand is archived");
    assert!(err.to_string().contains(&demand), "{err}");

    fx.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_orchestrator_rejects_unknown_project_without_side_effects() {
    let fx = fixture().await;
    create_space(&fx, "space-1").await;

    let err = fx
        .client
        .call(
            methods::CREATE_ORCHESTRATOR,
            json!({ "name": "Loja", "harness": "mock", "projects": ["space-1", "ghost-space"] }),
        )
        .await
        .expect_err("unknown project");
    assert!(err.to_string().contains("ghost-space"), "{err}");

    let listed = fx
        .client
        .call(methods::LIST_ORCHESTRATORS, json!({}))
        .await
        .unwrap();
    assert!(listed.as_array().unwrap().is_empty());
    assert!(fx.core.workspace.read_chats().unwrap().is_empty());
    fx.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn orchestrator_ids_follow_the_name_and_avoid_collisions() {
    let fx = fixture().await;

    let first = create_orchestrator(&fx, "Loja", &[]).await;
    let second = create_orchestrator(&fx, "Loja", &[]).await;
    let third = create_orchestrator(&fx, "Loja", &[]).await;
    let blank = create_orchestrator(&fx, "!!!", &[]).await;

    assert_eq!(first["orchestratorId"], "loja");
    assert_eq!(second["orchestratorId"], "loja-2");
    assert_eq!(third["orchestratorId"], "loja-3");
    assert_eq!(blank["orchestratorId"], "orquestrador");
    assert_ne!(first["chatId"], second["chatId"]);
    fx.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_roles_seeds_defaults_and_resolve_role_model_reads_the_catalog() {
    let fx = fixture().await;

    let roles = fx
        .client
        .call(methods::LIST_ROLES, json!({}))
        .await
        .unwrap();
    let names: Vec<&str> = roles["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|role| role["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["executor", "explorador", "planejador", "revisor"]);
    assert_eq!(roles["errors"], json!([]));
    let explorer = &roles["roles"][1];
    assert_eq!(explorer["access"], "read-only");
    assert_eq!(explorer["model"], "@rapido");
    assert!(explorer.get("prompt").is_none());

    std::fs::write(
        fx.tmp.path().join("data/roles/models.toml"),
        "[alias.rapido]\nmock = [\"mock-pro:low\"]\n[alias.medio]\nmock = [\"mock-pro:low\"]\n",
    )
    .unwrap();

    let readonly = fx
        .client
        .call(
            methods::RESOLVE_ROLE_MODEL,
            json!({ "role": "explorador", "harness": "mock" }),
        )
        .await
        .unwrap();
    assert_eq!(
        readonly,
        json!({ "model": "mock-pro", "reasoning": "low", "sandbox": "read-only" })
    );

    let write = fx
        .client
        .call(
            methods::RESOLVE_ROLE_MODEL,
            json!({ "role": "executor", "harness": "mock", "effort": "high" }),
        )
        .await
        .unwrap();
    assert_eq!(write["model"], "mock-pro");
    assert_eq!(write["reasoning"], "medium");
    assert_eq!(write["sandbox"], "workspace-write");

    let unknown = fx
        .client
        .call(
            methods::RESOLVE_ROLE_MODEL,
            json!({ "role": "arquiteto", "harness": "mock" }),
        )
        .await
        .expect_err("unknown role");
    assert!(unknown.to_string().contains("arquiteto"), "{unknown}");

    let unmapped = fx
        .client
        .call(
            methods::RESOLVE_ROLE_MODEL,
            json!({ "role": "planejador", "harness": "mock" }),
        )
        .await
        .expect_err("alias without a candidate");
    assert!(unmapped.to_string().contains("forte"), "{unmapped}");
    fx.core.shutdown().await;
}
