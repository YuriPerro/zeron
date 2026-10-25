//! Orchestration RPCs against a real engine: the store, roles and model
//! resolution that the MCP server and CLI call.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};

use zeron_doc::{MessagePart, MessageRole};
use zeron_engine::orchestration::wake::{ChildStop, wake_message};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode, UserInputQuestion,
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
    fixture_with(Arc::new(CatalogHarness)).await
}

async fn fixture_with(harness: Arc<dyn Harness>) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let registry = HarnessRegistry::new();
    registry.register(harness);
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
    assert_eq!(chat.parent_chat_id, None);
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
async fn create_orchestrator_refuses_harnesses_that_drop_instructions() {
    let fx = fixture().await;

    let err = fx
        .client
        .call(
            methods::CREATE_ORCHESTRATOR,
            json!({ "name": "Loja", "harness": "hermes", "projects": [] }),
        )
        .await
        .expect_err("hermes drops instructions");
    assert!(
        err.to_string().contains(
            "Orchestrators run on Claude Code or Codex. hermes can drop the orchestrator prompt when its session restarts."
        ),
        "{err}"
    );

    assert!(!fx.tmp.path().join("data/orchestrators").exists());
    assert!(fx.core.workspace.read_chats().unwrap().is_empty());
    fx.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_orchestrator_requires_a_name_and_dedupes_projects() {
    let fx = fixture().await;
    create_space(&fx, "space-1").await;
    create_space(&fx, "space-2").await;

    for blank in ["", "   \t"] {
        let err = fx
            .client
            .call(
                methods::CREATE_ORCHESTRATOR,
                json!({ "name": blank, "harness": "mock", "projects": [] }),
            )
            .await
            .expect_err("blank name");
        assert!(err.to_string().contains("name is required"), "{err}");
    }
    assert!(fx.core.workspace.read_chats().unwrap().is_empty());

    let created = create_orchestrator(
        &fx,
        "  Loja  ",
        &["space-2", "space-1", "space-2", "space-1"],
    )
    .await;
    let stored = fx
        .client
        .call(
            methods::GET_ORCHESTRATOR,
            json!({ "orchestratorId": created["orchestratorId"] }),
        )
        .await
        .unwrap();
    assert_eq!(stored["name"], "Loja");
    assert_eq!(stored["projects"], json!(["space-2", "space-1"]));
    let chat = fx
        .core
        .workspace
        .chat(created["chatId"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(chat.title.as_deref(), Some("Loja"));
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

    std::fs::write(
        fx.tmp.path().join("data/roles/quebrado.md"),
        "no frontmatter",
    )
    .unwrap();
    let missing = fx
        .client
        .call(
            methods::RESOLVE_ROLE_MODEL,
            json!({ "role": "quebrado", "harness": "mock" }),
        )
        .await
        .expect_err("role file that failed to parse");
    let message = missing.to_string();
    assert!(message.contains("unknown role `quebrado`"), "{message}");
    assert!(message.contains("quebrado.md"), "{message}");
    fx.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn resolve_role_model_loads_the_role_locally_before_asking_a_target_for_its_catalog() {
    let fx = fixture().await;

    let unknown = fx
        .client
        .call(
            methods::RESOLVE_ROLE_MODEL,
            json!({ "role": "arquiteto", "harness": "mock", "targetDeviceId": "device-elsewhere" }),
        )
        .await
        .expect_err("unknown role");
    assert!(unknown.to_string().contains("arquiteto"), "{unknown}");

    let offline = fx
        .client
        .call(
            methods::RESOLVE_ROLE_MODEL,
            json!({ "role": "executor", "harness": "mock", "targetDeviceId": "device-elsewhere" }),
        )
        .await
        .expect_err("catalog comes from the target, which is unreachable");
    assert!(
        offline.to_string().contains("remote routing unavailable"),
        "{offline}"
    );
    fx.core.shutdown().await;
}

/// Runs hold until the test releases them by prompt, so which chat is busy
/// and which has stopped is always the test's call. Wake prompts finish at once.
struct GatedHarness {
    gates: Mutex<HashMap<String, Arc<tokio::sync::Notify>>>,
    prompts: Mutex<Vec<String>>,
    ask: Mutex<Option<String>>,
}

impl GatedHarness {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            gates: Mutex::new(HashMap::new()),
            prompts: Mutex::new(Vec::new()),
            ask: Mutex::new(None),
        })
    }

    fn gate(&self, prompt: &str) -> Arc<tokio::sync::Notify> {
        self.gates
            .lock()
            .unwrap()
            .entry(prompt.to_string())
            .or_default()
            .clone()
    }

    fn release(&self, prompt: &str) {
        self.gate(prompt).notify_one();
    }

    fn started(&self, prompt: &str) -> bool {
        self.prompts.lock().unwrap().iter().any(|p| p == prompt)
    }

    fn wakes(&self) -> Vec<String> {
        self.prompts
            .lock()
            .unwrap()
            .iter()
            .filter(|prompt| prompt.starts_with("[Zeron:"))
            .cloned()
            .collect()
    }
}

#[async_trait]
impl Harness for GatedHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Gated"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let prompt = request
            .prompt
            .split_once("</instructions>\n\n")
            .map_or(request.prompt.as_str(), |(_, rest)| rest)
            .to_string();
        self.prompts.lock().unwrap().push(prompt.clone());
        let asks = self.ask.lock().unwrap().as_deref() == Some(prompt.as_str());
        let answer = asks.then(|| {
            (controls.request_input)(vec![UserInputQuestion {
                id: "q1".into(),
                header: "Choose".into(),
                question: "which one?".into(),
                options: vec!["a".into(), "b".into()],
                prefill: None,
                multiline: false,
                multi_select: false,
            }])
        });
        let immediate = prompt.starts_with("[Zeron:");
        let gate = self.gate(&prompt);
        let started = futures::stream::iter(vec![Ok(AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock-1".into(),
            tools: vec![],
            cwd: request.cwd.clone(),
            session_id: "sess-gated".to_string(),
            assistant_message_id: format!("a-{prompt}"),
        })]);
        let done = futures::stream::once(async move {
            if !immediate {
                gate.notified().await;
            }
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("sess-gated".to_string()),
            })
        });
        Ok(started
            .chain(done)
            .inspect(move |_| {
                let _ = &answer;
            })
            .boxed())
    }
}

async fn wait_for<F>(mut predicate: F, what: &str)
where
    F: FnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

async fn create_child(fx: &Fixture, id: &str, parent: &str) {
    fx.client
        .call(
            methods::MUTATE,
            json!({
                "op": "createChat", "chatId": id, "parentChatId": parent,
                "deviceId": fx.core.device_id,
            }),
        )
        .await
        .expect("createChat");
    fx.core.workspace.rename_chat(id, id).expect("rename chat");
}

fn start(fx: &Fixture, chat: &str, prompt: &str) {
    fx.core
        .doc_host
        .queue_message(chat, prompt, Vec::new())
        .expect("queue message");
}

fn wakes_in(fx: &Fixture, chat_id: &str) -> Vec<String> {
    user_texts(fx, chat_id)
        .into_iter()
        .filter(|text| text.starts_with("[Zeron:"))
        .collect()
}

fn user_texts(fx: &Fixture, chat_id: &str) -> Vec<String> {
    fx.core
        .doc_host
        .open(chat_id)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.role == MessageRole::User)
        .map(|entry| {
            entry
                .parts
                .iter()
                .filter_map(|part| match part {
                    MessagePart::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn child_turn_end_queues_one_message_to_orchestrator() {
    let harness = GatedHarness::new();
    let fx = fixture_with(harness.clone()).await;
    let orchestrator = create_orchestrator(&fx, "Loja", &[]).await["chatId"]
        .as_str()
        .unwrap()
        .to_string();
    create_child(&fx, "child-a", &orchestrator).await;

    start(&fx, "child-a", "work a");
    wait_for(|| harness.started("work a"), "the child turn to start").await;
    assert!(wakes_in(&fx, &orchestrator).is_empty());

    harness.release("work a");
    let expected = wake_message("child-a", "child-a", ChildStop::Finished);
    wait_for(
        || wakes_in(&fx, &orchestrator) == [expected.clone()],
        "the orchestrator to be woken",
    )
    .await;

    create_child(&fx, "child-b", &orchestrator).await;
    start(&fx, "child-b", "work b");
    harness.release("work b");
    let expected_b = wake_message("child-b", "child-b", ChildStop::Finished);
    wait_for(
        || wakes_in(&fx, &orchestrator).contains(&expected_b),
        "the sentinel wake",
    )
    .await;
    assert_eq!(
        wakes_in(&fx, &orchestrator),
        [expected, expected_b],
        "one wake per completed child turn"
    );
    wait_for(
        || harness.wakes().len() == 2,
        "both wakes to reach the harness",
    )
    .await;
    fx.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn child_of_plain_chat_wakes_nobody() {
    let harness = GatedHarness::new();
    let fx = fixture_with(harness.clone()).await;
    let orchestrator = create_orchestrator(&fx, "Loja", &[]).await["chatId"]
        .as_str()
        .unwrap()
        .to_string();
    fx.client
        .call(
            methods::MUTATE,
            json!({ "op": "createChat", "chatId": "plain", "deviceId": fx.core.device_id }),
        )
        .await
        .unwrap();
    fx.core.workspace.rename_chat("plain", "plain").unwrap();
    create_child(&fx, "plain-child", "plain").await;
    create_child(&fx, "sentinel", &orchestrator).await;

    start(&fx, "plain-child", "plain work");
    wait_for(|| harness.started("plain work"), "the plain child to start").await;
    harness.release("plain work");
    wait_for(
        || {
            fx.core
                .sessions
                .session_status("plain-child")
                .is_some_and(|s| s.last_completed_turn.is_some())
        },
        "the plain child to complete",
    )
    .await;

    start(&fx, "sentinel", "sentinel work");
    harness.release("sentinel work");
    wait_for(
        || !wakes_in(&fx, &orchestrator).is_empty(),
        "the sentinel wake",
    )
    .await;
    assert_eq!(
        wakes_in(&fx, &orchestrator),
        [wake_message("sentinel", "sentinel", ChildStop::Finished)]
    );
    assert!(wakes_in(&fx, "plain").is_empty());
    assert!(wakes_in(&fx, "plain-child").is_empty());
    fx.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn orchestrator_turn_end_wakes_nobody() {
    let harness = GatedHarness::new();
    let fx = fixture_with(harness.clone()).await;
    let orchestrator = create_orchestrator(&fx, "Loja", &[]).await["chatId"]
        .as_str()
        .unwrap()
        .to_string();
    create_child(&fx, "sentinel", &orchestrator).await;

    start(&fx, &orchestrator, "plan it");
    wait_for(
        || harness.started("plan it"),
        "the orchestrator turn to start",
    )
    .await;
    harness.release("plan it");
    wait_for(
        || {
            fx.core
                .sessions
                .session_status(&orchestrator)
                .is_some_and(|s| s.last_completed_turn.is_some())
        },
        "the orchestrator turn to complete",
    )
    .await;

    start(&fx, "sentinel", "sentinel work");
    harness.release("sentinel work");
    wait_for(
        || !wakes_in(&fx, &orchestrator).is_empty(),
        "the sentinel wake",
    )
    .await;
    assert_eq!(
        wakes_in(&fx, &orchestrator),
        [wake_message("sentinel", "sentinel", ChildStop::Finished)]
    );
    fx.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn child_waiting_for_an_answer_wakes_the_orchestrator() {
    let harness = GatedHarness::new();
    *harness.ask.lock().unwrap() = Some("ask me".into());
    let fx = fixture_with(harness.clone()).await;
    let orchestrator = create_orchestrator(&fx, "Loja", &[]).await["chatId"]
        .as_str()
        .unwrap()
        .to_string();
    create_child(&fx, "child-a", &orchestrator).await;

    start(&fx, "child-a", "ask me");
    let expected = wake_message("child-a", "child-a", ChildStop::WaitingForAnswer);
    wait_for(
        || wakes_in(&fx, &orchestrator) == [expected.clone()],
        "the awaiting-input wake",
    )
    .await;
    harness.release("ask me");
    fx.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wake_queued_while_the_orchestrator_works_is_delivered_after_its_turn_ends() {
    let harness = GatedHarness::new();
    let fx = fixture_with(harness.clone()).await;
    let orchestrator = create_orchestrator(&fx, "Loja", &[]).await["chatId"]
        .as_str()
        .unwrap()
        .to_string();
    create_child(&fx, "child-a", &orchestrator).await;

    start(&fx, &orchestrator, "plan it");
    wait_for(
        || harness.started("plan it"),
        "the orchestrator turn to start",
    )
    .await;
    start(&fx, "child-a", "work a");
    harness.release("work a");

    let expected = wake_message("child-a", "child-a", ChildStop::Finished);
    let queued = || {
        fx.core
            .doc_host
            .open(&orchestrator)
            .ok()
            .and_then(|handle| handle.doc().read_queue().ok())
            .unwrap_or_default()
    };
    wait_for(
        || queued().iter().any(|row| row.text == expected),
        "the wake to be queued",
    )
    .await;
    assert!(queued().iter().all(|row| row.hold_for_turn_end));
    assert!(harness.wakes().is_empty());
    assert!(wakes_in(&fx, &orchestrator).is_empty());

    harness.release("plan it");
    wait_for(
        || wakes_in(&fx, &orchestrator) == [expected.clone()],
        "the wake to be delivered after the turn",
    )
    .await;
    assert!(queued().is_empty());
    fx.core.shutdown().await;
}
