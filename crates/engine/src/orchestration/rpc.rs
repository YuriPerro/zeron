use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;
use zeron_proto::{ChatConfig, HarnessId, Model, ReasoningLevel, SandboxLevel};
use zeron_rpc::{RpcError, RpcReply, methods, parse_params};

use super::models::{RelativeEffort, parse_model_map, resolve};
use super::roles::{Role, RoleAccess, load_roles, seed_defaults};
use super::store::{OrchestratorConfig, PlanItem, Store};

const ROLES_DIR: &str = "roles";
const MODELS_FILE: &str = "models.toml";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrchestratorParams {
    orchestrator_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateDemandParams {
    orchestrator_id: String,
    title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DemandParams {
    orchestrator_id: String,
    demand: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteSpecParams {
    orchestrator_id: String,
    demand: String,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WritePlanParams {
    orchestrator_id: String,
    demand: String,
    items: Vec<PlanItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitReportParams {
    orchestrator_id: String,
    demand: String,
    child_id: String,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloseDemandParams {
    orchestrator_id: String,
    demand: String,
    summary: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RememberParams {
    orchestrator_id: String,
    note: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolveRoleModelParams {
    role: String,
    pub(crate) harness: HarnessId,
    #[serde(default)]
    effort: Option<RelativeEffort>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateOrchestratorParams {
    pub(crate) name: String,
    #[serde(default)]
    avatar: Option<String>,
    harness: HarnessId,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning: Option<ReasoningLevel>,
    #[serde(default)]
    child_harness: Option<HarnessId>,
    projects: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RolesReply<'a> {
    roles: &'a [Role],
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedRoleReply {
    model: String,
    reasoning: Option<ReasoningLevel>,
    sandbox: SandboxLevel,
}

fn failed(err: impl std::fmt::Display) -> RpcError {
    RpcError::Failed(err.to_string())
}

fn empty() -> Result<RpcReply, RpcError> {
    RpcReply::value(&json!({}))
}

pub(crate) fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::LIST_ROLES
            | methods::GET_ORCHESTRATOR
            | methods::LIST_ORCHESTRATORS
            | methods::ORCH_CREATE_DEMAND
            | methods::ORCH_WRITE_SPEC
            | methods::ORCH_WRITE_PLAN
            | methods::ORCH_READ_DEMAND
            | methods::ORCH_SUBMIT_REPORT
            | methods::ORCH_CLOSE_DEMAND
            | methods::ORCH_REMEMBER
    )
}

pub(crate) fn dispatch(
    data_dir: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<RpcReply, RpcError> {
    let store = Store::new(data_dir);
    match method {
        methods::LIST_ROLES => list_roles(data_dir),
        methods::GET_ORCHESTRATOR => {
            let p: OrchestratorParams = parse_params(params)?;
            RpcReply::value(&store.get(&p.orchestrator_id).map_err(failed)?)
        }
        methods::LIST_ORCHESTRATORS => RpcReply::value(&store.list().map_err(failed)?),
        methods::ORCH_CREATE_DEMAND => {
            let p: CreateDemandParams = parse_params(params)?;
            let today = chrono::Local::now().date_naive();
            let demand = store
                .create_demand(&p.orchestrator_id, &p.title, today)
                .map_err(failed)?;
            RpcReply::value(&json!({ "demand": demand }))
        }
        methods::ORCH_WRITE_SPEC => {
            let p: WriteSpecParams = parse_params(params)?;
            store
                .write_spec(&p.orchestrator_id, &p.demand, &p.text)
                .map_err(failed)?;
            empty()
        }
        methods::ORCH_WRITE_PLAN => {
            let p: WritePlanParams = parse_params(params)?;
            store
                .write_plan(&p.orchestrator_id, &p.demand, &p.items)
                .map_err(failed)?;
            empty()
        }
        methods::ORCH_READ_DEMAND => {
            let p: DemandParams = parse_params(params)?;
            RpcReply::value(
                &store
                    .read_demand(&p.orchestrator_id, &p.demand)
                    .map_err(failed)?,
            )
        }
        methods::ORCH_SUBMIT_REPORT => {
            let p: SubmitReportParams = parse_params(params)?;
            store
                .submit_report(&p.orchestrator_id, &p.demand, &p.child_id, &p.text)
                .map_err(failed)?;
            empty()
        }
        methods::ORCH_CLOSE_DEMAND => {
            let p: CloseDemandParams = parse_params(params)?;
            let today = chrono::Local::now().date_naive();
            store
                .close_demand(&p.orchestrator_id, &p.demand, &p.summary, today)
                .map_err(failed)?;
            empty()
        }
        methods::ORCH_REMEMBER => {
            let p: RememberParams = parse_params(params)?;
            store
                .remember(&p.orchestrator_id, &p.note)
                .map_err(failed)?;
            empty()
        }
        _ => Err(RpcError::UnknownMethod(method.to_string())),
    }
}

fn list_roles(data_dir: &Path) -> Result<RpcReply, RpcError> {
    let dir = data_dir.join(ROLES_DIR);
    seed_defaults(&dir).map_err(failed)?;
    let (roles, errors) = load_roles(&dir);
    RpcReply::value(&RolesReply {
        roles: &roles,
        errors: errors.iter().map(ToString::to_string).collect(),
    })
}

pub(crate) fn resolve_role_model(
    data_dir: &Path,
    p: &ResolveRoleModelParams,
    catalog: &[Model],
) -> Result<RpcReply, RpcError> {
    let dir = data_dir.join(ROLES_DIR);
    seed_defaults(&dir).map_err(failed)?;
    let (roles, _) = load_roles(&dir);
    let role = roles
        .iter()
        .find(|role| role.name == p.role)
        .ok_or_else(|| RpcError::Failed(format!("unknown role `{}`", p.role)))?;
    let text = std::fs::read_to_string(dir.join(MODELS_FILE)).map_err(failed)?;
    let map = parse_model_map(&text).map_err(failed)?;
    let resolved = resolve(&map, &role.model, p.harness, catalog, p.effort).map_err(failed)?;
    RpcReply::value(&ResolvedRoleReply {
        model: resolved.model,
        reasoning: resolved.reasoning,
        sandbox: match role.access {
            RoleAccess::ReadOnly => SandboxLevel::ReadOnly,
            RoleAccess::Write => SandboxLevel::WorkspaceWrite,
        },
    })
}

pub(crate) fn create_orchestrator_entry(
    data_dir: &Path,
    p: CreateOrchestratorParams,
    known_spaces: &[String],
) -> Result<OrchestratorConfig, RpcError> {
    if let Some(unknown) = p
        .projects
        .iter()
        .find(|project| !known_spaces.contains(project))
    {
        return Err(RpcError::Failed(format!("unknown project id `{unknown}`")));
    }
    let store = Store::new(data_dir);
    let config = OrchestratorConfig {
        id: store.available_id(&p.name),
        name: p.name,
        avatar: p.avatar,
        harness: p.harness,
        model: p.model,
        reasoning: p.reasoning,
        child_harness: p.child_harness.unwrap_or(p.harness),
        projects: p.projects,
    };
    store.create(&config).map_err(failed)?;
    Ok(config)
}

pub(crate) fn discard_orchestrator(data_dir: &Path, id: &str) -> io::Result<()> {
    Store::new(data_dir).remove(id)
}

pub(crate) fn chat_config(config: &OrchestratorConfig) -> ChatConfig {
    ChatConfig {
        harness: config.harness,
        model: config.model.clone(),
        reasoning: config.reasoning,
        model_options: Default::default(),
        sandbox: SandboxLevel::ReadOnly,
        role: None,
        orchestrator: Some(config.id.clone()),
    }
}
