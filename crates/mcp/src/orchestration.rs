//! Tools for orchestrator chats and the children they spawn.
//!
//! Which tools a chat sees depends on where it sits: an orchestrator chat
//! (`config.orchestrator` set) drives demands, a child (its parent is an
//! orchestrator chat) reads them and reports back. The orchestrator id is
//! always taken from the chat's own lineage, never from tool arguments.

use std::borrow::Cow;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeron_proto::{Chat, HarnessId, ReasoningLevel, SandboxLevel, Space};
use zeron_rpc::methods;

use crate::tools::{ToolDef, Tools};
use crate::zeron::{Zeron, resolve_chat_in, short};

/// One role from `ListRoles`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleInfo {
    pub name: String,
    pub description: String,
    pub model: String,
    pub access: String,
}

/// `ListRoles` reply: the loadable roles plus the files that failed to load.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleCatalog {
    pub roles: Vec<RoleInfo>,
    #[serde(default)]
    pub errors: Vec<String>,
}

/// `ResolveRoleModel` reply.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRole {
    pub model: String,
    #[serde(default)]
    pub reasoning: Option<ReasoningLevel>,
    pub sandbox: SandboxLevel,
}

/// `GetOrchestrator` reply; `projects` are space ids.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorInfo {
    pub id: String,
    pub name: String,
    pub child_harness: HarnessId,
    #[serde(default)]
    pub projects: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

/// One line of a demand plan, as `OrchWritePlan` takes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub text: String,
    pub status: PlanStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl Zeron {
    pub async fn list_roles(&self) -> anyhow::Result<RoleCatalog> {
        let value = self.call(methods::LIST_ROLES, json!({})).await?;
        serde_json::from_value(value).context("ListRoles: unexpected shape")
    }

    pub async fn resolve_role_model(
        &self,
        role: &str,
        harness: HarnessId,
        effort: Option<&str>,
        device: Option<&str>,
    ) -> anyhow::Result<ResolvedRole> {
        let mut params = json!({ "role": role, "harness": harness });
        if let Some(effort) = effort {
            params["effort"] = json!(effort);
        }
        if let Some(device) = device {
            params["targetDeviceId"] = json!(device);
        }
        let value = self.call(methods::RESOLVE_ROLE_MODEL, params).await?;
        serde_json::from_value(value).context("ResolveRoleModel: unexpected shape")
    }

    pub async fn get_orchestrator(
        &self,
        id: &str,
        device: &str,
    ) -> anyhow::Result<OrchestratorInfo> {
        let value = self
            .call(
                methods::GET_ORCHESTRATOR,
                json!({ "orchestratorId": id, "targetDeviceId": device }),
            )
            .await?;
        serde_json::from_value(value).context("GetOrchestrator: unexpected shape")
    }

    pub async fn orch_create_demand(
        &self,
        orchestrator: &str,
        device: &str,
        title: &str,
    ) -> anyhow::Result<Value> {
        self.call(
            methods::ORCH_CREATE_DEMAND,
            json!({ "orchestratorId": orchestrator, "targetDeviceId": device, "title": title }),
        )
        .await
    }

    pub async fn orch_write_spec(
        &self,
        orchestrator: &str,
        device: &str,
        demand: &str,
        text: &str,
    ) -> anyhow::Result<Value> {
        self.call(
            methods::ORCH_WRITE_SPEC,
            json!({ "orchestratorId": orchestrator, "targetDeviceId": device, "demand": demand, "text": text }),
        )
        .await
    }

    pub async fn orch_write_plan(
        &self,
        orchestrator: &str,
        device: &str,
        demand: &str,
        items: &[PlanItem],
    ) -> anyhow::Result<Value> {
        self.call(
            methods::ORCH_WRITE_PLAN,
            json!({ "orchestratorId": orchestrator, "targetDeviceId": device, "demand": demand, "items": items }),
        )
        .await
    }

    pub async fn orch_read_demand(
        &self,
        orchestrator: &str,
        device: &str,
        demand: &str,
    ) -> anyhow::Result<Value> {
        self.call(
            methods::ORCH_READ_DEMAND,
            json!({ "orchestratorId": orchestrator, "targetDeviceId": device, "demand": demand }),
        )
        .await
    }

    pub async fn orch_submit_report(
        &self,
        orchestrator: &str,
        device: &str,
        demand: &str,
        child_id: &str,
        text: &str,
    ) -> anyhow::Result<Value> {
        self.call(
            methods::ORCH_SUBMIT_REPORT,
            json!({
                "orchestratorId": orchestrator, "targetDeviceId": device,
                "demand": demand,
                "childId": child_id,
                "text": text,
            }),
        )
        .await
    }

    pub async fn orch_close_demand(
        &self,
        orchestrator: &str,
        device: &str,
        demand: &str,
        summary: &str,
    ) -> anyhow::Result<Value> {
        self.call(
            methods::ORCH_CLOSE_DEMAND,
            json!({ "orchestratorId": orchestrator, "targetDeviceId": device, "demand": demand, "summary": summary }),
        )
        .await
    }

    pub async fn orch_remember(
        &self,
        orchestrator: &str,
        device: &str,
        note: &str,
    ) -> anyhow::Result<Value> {
        self.call(
            methods::ORCH_REMEMBER,
            json!({ "orchestratorId": orchestrator, "targetDeviceId": device, "note": note }),
        )
        .await
    }
}

const ORCHESTRATOR_TOOLS: &[&str] = &[
    "list_roles",
    "create_demand",
    "write_spec",
    "update_plan",
    "read_demand",
    "close_demand",
    "remember",
];
const CHILD_TOOLS: &[&str] = &["read_demand", "submit_report", "request_contract_change"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    Plain,
    Orchestrator {
        id: String,
        device: String,
    },
    Child {
        orchestrator: String,
        device: String,
        parent: String,
    },
}

impl Scope {
    fn store(&self) -> Option<(&str, &str)> {
        match self {
            Scope::Plain => None,
            Scope::Orchestrator { id, device } => Some((id, device)),
            Scope::Child {
                orchestrator,
                device,
                ..
            } => Some((orchestrator, device)),
        }
    }
}

pub(crate) async fn scope(zeron: &Zeron) -> anyhow::Result<Scope> {
    let Some(origin) = zeron.origin().chat_id.as_deref() else {
        return Ok(Scope::Plain);
    };
    if zeron_proto::voice::is_orchestrator_chat(origin) {
        return Ok(Scope::Plain);
    }
    let chats = zeron.chats().await?;
    let chat = resolve_chat_in(&chats, origin)?;
    let orchestrator_of = |chat: &zeron_proto::Chat| {
        chat.config
            .as_ref()
            .and_then(|config| config.orchestrator.clone())
    };
    if let Some(id) = orchestrator_of(&chat) {
        return Ok(Scope::Orchestrator {
            id,
            device: chat.device_id.clone(),
        });
    }
    let parent = chat
        .parent_chat_id
        .as_deref()
        .and_then(|id| chats.iter().find(|c| c.id == id));
    Ok(
        match parent.and_then(|p| orchestrator_of(p).map(|id| (id, p))) {
            Some((orchestrator, parent)) => Scope::Child {
                orchestrator,
                device: parent.device_id.clone(),
                parent: parent.id.clone(),
            },
            None => Scope::Plain,
        },
    )
}

fn names_for(scope: &Scope) -> &'static [&'static str] {
    match scope {
        Scope::Plain => &[],
        Scope::Orchestrator { .. } => ORCHESTRATOR_TOOLS,
        Scope::Child { .. } => CHILD_TOOLS,
    }
}

pub(crate) fn offers(scope: &Scope, name: &str) -> bool {
    names_for(scope).contains(&name)
}

pub(crate) fn handles(name: &str) -> bool {
    ORCHESTRATOR_TOOLS.contains(&name) || CHILD_TOOLS.contains(&name)
}

fn demand_schema() -> Value {
    json!({ "type": "string", "description": "Demand id, as returned by create_demand." })
}

fn definitions() -> Vec<ToolDef> {
    let def = |name: &'static str, description: &'static str, input_schema: Value| ToolDef {
        name,
        description: Cow::Borrowed(description),
        input_schema,
    };
    vec![
        def(
            "list_roles",
            "Roles you can give a child through create_chat's role argument: name, description, model alias and access. Role files that failed to load are listed under errors.",
            json!({ "type": "object", "properties": {} }),
        ),
        def(
            "create_demand",
            "Start a demand: a folder holding the spec, plan and reports of one piece of work. Returns its id; pass it as demand to the other tools.",
            json!({
                "type": "object",
                "properties": { "title": { "type": "string", "description": "Short name of the work." } },
                "required": ["title"]
            }),
        ),
        def(
            "write_spec",
            "Write or replace a demand's spec (markdown): what must be built and how it is verified.",
            json!({
                "type": "object",
                "properties": {
                    "demand": demand_schema(),
                    "text": { "type": "string", "description": "The full spec." }
                },
                "required": ["demand", "text"]
            }),
        ),
        def(
            "update_plan",
            "Replace a demand's plan with the given items.",
            json!({
                "type": "object",
                "properties": {
                    "demand": demand_schema(),
                    "items": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" },
                                "status": { "type": "string", "enum": ["pending", "inProgress", "completed"] },
                                "role": { "type": "string", "description": "Role of the child doing it." },
                                "project": { "type": "string", "description": "Project it touches." }
                            },
                            "required": ["text", "status"]
                        }
                    }
                },
                "required": ["demand", "items"]
            }),
        ),
        def(
            "read_demand",
            "Read a demand's spec, plan and the reports submitted so far.",
            json!({
                "type": "object",
                "properties": { "demand": demand_schema() },
                "required": ["demand"]
            }),
        ),
        def(
            "close_demand",
            "Close a demand and record a summary of the outcome.",
            json!({
                "type": "object",
                "properties": {
                    "demand": demand_schema(),
                    "summary": { "type": "string", "description": "What was delivered." }
                },
                "required": ["demand", "summary"]
            }),
        ),
        def(
            "remember",
            "Save a short note to this orchestrator's long-term memory, kept across demands.",
            json!({
                "type": "object",
                "properties": { "note": { "type": "string" } },
                "required": ["note"]
            }),
        ),
        def(
            "submit_report",
            "Send your final report on a demand to the orchestrator: what you did, what changed, what is left.",
            json!({
                "type": "object",
                "properties": {
                    "demand": demand_schema(),
                    "text": { "type": "string", "description": "The report." }
                },
                "required": ["demand", "text"]
            }),
        ),
        def(
            "request_contract_change",
            "Ask the orchestrator to change a demand's spec or plan before you continue. Explain why the current one cannot work.",
            json!({
                "type": "object",
                "properties": {
                    "demand": demand_schema(),
                    "reason": { "type": "string" }
                },
                "required": ["demand", "reason"]
            }),
        ),
    ]
}

pub(crate) fn definitions_for(scope: &Scope) -> Vec<ToolDef> {
    let names = names_for(scope);
    definitions()
        .into_iter()
        .filter(|def| names.contains(&def.name))
        .collect()
}

pub(crate) async fn roles_summary(zeron: &Zeron) -> String {
    let Ok(catalog) = zeron.list_roles().await else {
        return String::new();
    };
    if catalog.roles.is_empty() {
        return String::new();
    }
    let roles: Vec<String> = catalog
        .roles
        .iter()
        .map(|role| format!("{} ({})", role.name, role.description))
        .collect();
    format!("Roles: {}.", roles.join("; "))
}

pub(crate) struct ChildRequest<'a> {
    pub role: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub demand: Option<&'a str>,
    pub has_prompt: bool,
    pub parent_chat_id: Option<&'a str>,
}

pub(crate) struct Child {
    pub role: Option<String>,
    effort: Option<String>,
    demand: Option<String>,
    orchestrator: Option<OrchestratorInfo>,
    scope: Scope,
}

pub(crate) async fn prepare_child(
    zeron: &Zeron,
    request: ChildRequest<'_>,
) -> anyhow::Result<Child> {
    let scope = scope(zeron).await?;
    if matches!(scope, Scope::Orchestrator { .. }) {
        anyhow::ensure!(
            request.parent_chat_id.is_some()
                && request.parent_chat_id == zeron.origin().chat_id.as_deref(),
            "Chats created from an orchestrator are its children: omit kind chat and parent."
        );
    } else {
        anyhow::ensure!(
            request.demand.is_none(),
            "demand is only available in an orchestrator chat"
        );
    }
    let role = request
        .role
        .map(str::trim)
        .filter(|role| !role.is_empty())
        .map(str::to_owned);
    let effort = request.effort.map(str::trim).map(str::to_owned);
    if let Some(effort) = effort.as_deref() {
        anyhow::ensure!(
            matches!(effort, "low" | "medium" | "high"),
            "effort must be one of: low, medium, high"
        );
        anyhow::ensure!(role.is_some(), "effort needs a role");
    }
    let demand = request
        .demand
        .map(str::trim)
        .filter(|demand| !demand.is_empty())
        .map(str::to_owned);
    anyhow::ensure!(
        demand.is_none() || request.has_prompt,
        "demand needs a prompt"
    );
    Ok(Child {
        role,
        effort,
        demand,
        orchestrator: None,
        scope,
    })
}

pub(crate) fn ensure_started_by_owner(zeron: &Zeron, parent: &Chat) -> anyhow::Result<()> {
    anyhow::ensure!(
        parent
            .config
            .as_ref()
            .is_none_or(|c| c.orchestrator.is_none())
            || zeron.origin().chat_id.as_deref() == Some(parent.id.as_str()),
        "Only the orchestrator can start its own children."
    );
    Ok(())
}

impl Child {
    pub(crate) async fn attach_project(
        &mut self,
        zeron: &Zeron,
        space: Option<&Space>,
    ) -> anyhow::Result<()> {
        if let Scope::Orchestrator { id, device } = &self.scope {
            let orchestrator = zeron.get_orchestrator(id, device).await?;
            ensure_project_associated(zeron, &orchestrator, space).await?;
            self.orchestrator = Some(orchestrator);
        }
        Ok(())
    }

    pub(crate) fn child_harness(&self) -> Option<HarnessId> {
        self.orchestrator.as_ref().map(|o| o.child_harness)
    }

    pub(crate) fn ensure_cwd(
        &self,
        space: Option<&Space>,
        cwd: Option<&str>,
    ) -> anyhow::Result<()> {
        match (&self.orchestrator, space, cwd) {
            (Some(_), Some(space), Some(cwd)) => ensure_cwd_within_project(space, cwd),
            _ => Ok(()),
        }
    }

    pub(crate) async fn resolve_role(
        &self,
        zeron: &Zeron,
        harness: HarnessId,
        device: &str,
        all_explicit: bool,
    ) -> anyhow::Result<Option<ResolvedRole>> {
        match self.role.as_deref().filter(|_| !all_explicit) {
            Some(role) => Ok(Some(
                zeron
                    .resolve_role_model(role, harness, self.effort.as_deref(), Some(device))
                    .await?,
            )),
            None => Ok(None),
        }
    }

    pub(crate) fn brief(&self, prompt: String) -> String {
        match self.demand.as_deref() {
            Some(demand) => {
                format!("Demand: {demand}. Read it with read_demand before starting.\n\n{prompt}")
            }
            None => prompt,
        }
    }
}

pub(crate) fn ensure_cwd_within_project(space: &Space, cwd: &str) -> anyhow::Result<()> {
    let root = lexical_path(&space.path);
    let inside = std::path::Path::new(cwd).is_absolute()
        && lexical_path(cwd).is_some_and(|path| root.as_ref().is_some_and(|r| path.starts_with(r)));
    anyhow::ensure!(
        inside,
        "cwd must be inside the project folder {}.",
        space.path
    );
    Ok(())
}

fn lexical_path(raw: &str) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for component in std::path::Path::new(raw).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

pub(crate) async fn ensure_project_associated(
    zeron: &Zeron,
    orchestrator: &OrchestratorInfo,
    space: Option<&Space>,
) -> anyhow::Result<()> {
    if space.is_some_and(|space| orchestrator.projects.contains(&space.id)) {
        return Ok(());
    }
    let spaces = zeron.spaces().await?;
    let associated: Vec<String> = orchestrator
        .projects
        .iter()
        .map(|id| {
            spaces
                .iter()
                .find(|space| &space.id == id)
                .map_or_else(|| id.clone(), |space| space.display_name().to_owned())
        })
        .collect();
    let subject = match space {
        Some(space) => format!("Project {}", space.display_name()),
        None => "A chat without a project".to_owned(),
    };
    anyhow::bail!(
        "{subject} is not associated with this orchestrator. Associated: {}.",
        associated.join(", ")
    )
}

#[derive(Deserialize)]
struct CreateDemandArgs {
    title: String,
}

#[derive(Deserialize)]
struct WriteSpecArgs {
    demand: String,
    text: String,
}

#[derive(Deserialize)]
struct UpdatePlanArgs {
    demand: String,
    items: Vec<PlanItem>,
}

#[derive(Deserialize)]
struct DemandArgs {
    demand: String,
}

#[derive(Deserialize)]
struct CloseDemandArgs {
    demand: String,
    summary: String,
}

#[derive(Deserialize)]
struct RememberArgs {
    note: String,
}

#[derive(Deserialize)]
struct SubmitReportArgs {
    demand: String,
    text: String,
}

#[derive(Deserialize)]
struct ContractChangeArgs {
    demand: String,
    reason: String,
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> anyhow::Result<T> {
    serde_json::from_value(args).map_err(|e| anyhow::anyhow!("invalid arguments: {e}"))
}

fn orchestrator_store(scope: &Scope) -> anyhow::Result<(&str, &str)> {
    match scope {
        Scope::Orchestrator { id, device } => Ok((id, device)),
        _ => anyhow::bail!("this tool is only available in an orchestrator chat"),
    }
}

fn child_of(scope: &Scope) -> anyhow::Result<(&str, &str, &str)> {
    match scope {
        Scope::Child {
            orchestrator,
            device,
            parent,
        } => Ok((orchestrator, device, parent)),
        _ => anyhow::bail!("this tool is only available to a chat started by an orchestrator"),
    }
}

impl Tools {
    pub(crate) async fn call_orchestration(
        &self,
        name: &str,
        args: Value,
    ) -> Result<Value, String> {
        self.run_orchestration(name, args)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    async fn run_orchestration(&self, name: &str, args: Value) -> anyhow::Result<Value> {
        let scope = scope(&self.zeron).await?;
        anyhow::ensure!(offers(&scope, name), "{name} is not available in this chat");
        let zeron = &self.zeron;
        match name {
            "list_roles" => {
                let catalog = zeron.list_roles().await?;
                Ok(json!(catalog))
            }
            "create_demand" => {
                let args: CreateDemandArgs = parse(args)?;
                let (id, device) = orchestrator_store(&scope)?;
                zeron.orch_create_demand(id, device, &args.title).await
            }
            "write_spec" => {
                let args: WriteSpecArgs = parse(args)?;
                let (id, device) = orchestrator_store(&scope)?;
                zeron
                    .orch_write_spec(id, device, &args.demand, &args.text)
                    .await
            }
            "update_plan" => {
                let args: UpdatePlanArgs = parse(args)?;
                let (id, device) = orchestrator_store(&scope)?;
                zeron
                    .orch_write_plan(id, device, &args.demand, &args.items)
                    .await
            }
            "read_demand" => {
                let args: DemandArgs = parse(args)?;
                let (id, device) = scope
                    .store()
                    .context("read_demand is not available in this chat")?;
                zeron.orch_read_demand(id, device, &args.demand).await
            }
            "close_demand" => {
                let args: CloseDemandArgs = parse(args)?;
                let (id, device) = orchestrator_store(&scope)?;
                zeron
                    .orch_close_demand(id, device, &args.demand, &args.summary)
                    .await
            }
            "remember" => {
                let args: RememberArgs = parse(args)?;
                let (id, device) = orchestrator_store(&scope)?;
                zeron.orch_remember(id, device, &args.note).await
            }
            "submit_report" => {
                let args: SubmitReportArgs = parse(args)?;
                let (orchestrator, device, _) = child_of(&scope)?;
                let me = self.own_chat_id()?;
                zeron
                    .orch_submit_report(orchestrator, device, &args.demand, &me, &args.text)
                    .await
            }
            "request_contract_change" => {
                let args: ContractChangeArgs = parse(args)?;
                let (_, _, parent) = child_of(&scope)?;
                let me = self.own_chat_id()?;
                let chats = zeron.chats().await?;
                let title = resolve_chat_in(&chats, &me)?.title;
                let label = title
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| short(&me));
                let text = format!(
                    "[Contract change requested by {label} ({me}) for demand {}]\n\n{}",
                    args.demand, args.reason
                );
                let id = zeron.queue_message(parent, &text).await?;
                Ok(json!({ "queued": true, "id": id, "chatId": parent }))
            }
            other => anyhow::bail!("unknown tool: {other}"),
        }
    }

    fn own_chat_id(&self) -> anyhow::Result<String> {
        self.zeron
            .origin()
            .chat_id
            .clone()
            .context("this tool needs a chat to speak for")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::*;
    use crate::tools::tests::{World, origin, tools};
    use crate::zeron::Origin;

    fn orchestrated_chats() -> Vec<Value> {
        vec![
            json!({
                "id": "chat-orch-5", "deviceId": "dev-local", "title": "Plataforma",
                "archived": false, "createdAt": "2026-10-01T00:00:00Z",
                "config": { "harness": "claude-code", "model": null, "reasoning": null,
                    "sandbox": "read-only", "orchestrator": "plataforma" }
            }),
            json!({
                "id": "chat-kid-6", "deviceId": "dev-remote", "title": "Kid",
                "parentChatId": "chat-orch-5", "spaceId": "space-1",
                "archived": false, "createdAt": "2026-10-02T00:00:00Z",
                "config": { "harness": "claude-code", "model": "sonnet", "reasoning": null,
                    "sandbox": "workspace-write", "role": "executor" }
            }),
        ]
    }

    fn orchestrated() -> Arc<World> {
        World::orchestrated(None, orchestrated_chats())
    }

    fn names(defs: &[ToolDef]) -> Vec<&str> {
        defs.iter().map(|d| d.name).collect()
    }

    #[tokio::test]
    async fn orchestrator_sees_orchestrator_tools_only_in_orchestrator_chats() {
        let world = orchestrated();
        let plain = tools(world.clone(), origin("chat-alpha-1"));
        let plain_defs = plain.list().await;
        let plain_names = names(&plain_defs);
        assert!(!plain_names.contains(&"write_spec"));
        assert!(!plain_names.contains(&"submit_report"));
        assert!(!plain.has("write_spec").await);
        assert!(
            plain
                .call("write_spec", json!({ "demand": "d", "text": "t" }))
                .await
                .is_err()
        );

        let orchestrator = tools(world.clone(), origin("chat-orch-5"));
        let listed = orchestrator.list().await;
        let orchestrator_names = names(&listed);
        for tool in [
            "list_roles",
            "create_demand",
            "write_spec",
            "update_plan",
            "read_demand",
            "close_demand",
            "remember",
            "create_chat",
        ] {
            assert!(orchestrator_names.contains(&tool), "{tool}");
        }
        assert!(!orchestrator_names.contains(&"submit_report"));
        assert!(orchestrator.has("write_spec").await);

        let child = tools(world.clone(), origin("chat-kid-6"));
        let listed = child.list().await;
        let child_names = names(&listed);
        for tool in ["read_demand", "submit_report", "request_contract_change"] {
            assert!(child_names.contains(&tool), "{tool}");
        }
        assert!(!child_names.contains(&"write_spec"));
        assert!(!child_names.contains(&"update_plan"));
        assert!(!child.has("write_spec").await);
    }

    #[tokio::test]
    async fn create_chat_with_role_uses_resolved_model_and_records_role() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        tools
            .call(
                "create_chat",
                json!({ "project": "space-1", "role": "executor", "effort": "high" }),
            )
            .await
            .unwrap();
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes[0].1["config"]["model"], "sonnet");
        assert_eq!(writes[0].1["config"]["reasoning"], "high");
        assert_eq!(writes[0].1["config"]["role"], "executor");
        assert_eq!(writes[0].1["config"]["sandbox"], "workspace-write");
        let reads = world.reads.lock().unwrap();
        let (_, resolve) = reads
            .iter()
            .find(|(method, _)| method == methods::RESOLVE_ROLE_MODEL)
            .unwrap();
        assert_eq!(resolve["role"], "executor");
        assert_eq!(resolve["harness"], "claude-code");
        assert_eq!(resolve["effort"], "high");
        assert_eq!(resolve["targetDeviceId"], "dev-local");
    }

    #[tokio::test]
    async fn the_role_decides_the_sandbox_and_the_explicit_one_wins() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        tools
            .call("create_chat", json!({ "role": "explorador" }))
            .await
            .unwrap();
        tools
            .call(
                "create_chat",
                json!({ "role": "explorador", "sandbox": "workspace-write" }),
            )
            .await
            .unwrap();
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes[0].1["config"]["sandbox"], "read-only");
        assert_eq!(writes[1].1["config"]["sandbox"], "workspace-write");
    }

    #[tokio::test]
    async fn explicit_model_beats_role_model() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        tools
            .call(
                "create_chat",
                json!({ "role": "executor", "model": "opus", "reasoning": "low" }),
            )
            .await
            .unwrap();
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes[0].1["config"]["model"], "opus");
        assert_eq!(writes[0].1["config"]["reasoning"], "low");
        assert_eq!(writes[0].1["config"]["role"], "executor");
    }

    #[tokio::test]
    async fn effort_is_validated_and_needs_a_role() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        let invalid = tools
            .call(
                "create_chat",
                json!({ "role": "executor", "effort": "max" }),
            )
            .await
            .unwrap_err();
        assert!(invalid.contains("low, medium, high"), "{invalid}");
        let orphan = tools
            .call("create_chat", json!({ "effort": "low" }))
            .await
            .unwrap_err();
        assert!(orphan.contains("effort needs a role"), "{orphan}");
        assert!(world.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn orchestrator_cannot_create_children_outside_its_projects() {
        let world = orchestrated();
        let tools = tools(world.clone(), origin("chat-orch-5"));
        let outside = tools
            .call("create_chat", json!({ "project": "space-unique" }))
            .await
            .unwrap_err();
        assert!(
            outside.contains("Project unique is not associated"),
            "{outside}"
        );
        assert!(outside.contains("Associated: comet."), "{outside}");
        let projectless = tools.call("create_chat", json!({})).await.unwrap_err();
        assert!(projectless.contains("is not associated"), "{projectless}");
        assert!(world.writes.lock().unwrap().is_empty());

        let created = tools
            .call(
                "create_chat",
                json!({ "project": "space-1", "role": "executor" }),
            )
            .await
            .unwrap();
        assert_eq!(created["parentChatId"], "chat-orch-5");
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes[0].1["parentChatId"], "chat-orch-5");
        assert_eq!(writes[0].1["config"]["role"], "executor");
    }

    #[tokio::test]
    async fn an_orchestrator_cannot_detach_its_children() {
        let world = orchestrated();
        let tools = tools(world.clone(), origin("chat-orch-5"));
        for args in [
            json!({ "project": "space-1", "kind": "chat" }),
            json!({ "project": "space-1", "parent": "Alpha" }),
        ] {
            let refused = tools.call("create_chat", args).await.unwrap_err();
            assert!(refused.contains("orchestrator"), "{refused}");
        }
        assert!(world.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn children_default_to_the_orchestrators_child_harness() {
        let world = World::orchestrated(Some("codex"), orchestrated_chats());
        let tools = tools(world.clone(), origin("chat-orch-5"));
        let refused = tools
            .call("create_chat", json!({ "project": "space-1" }))
            .await
            .unwrap_err();
        assert!(refused.contains("codex"), "{refused}");
        tools
            .call(
                "create_chat",
                json!({ "project": "space-1", "harness": "claude-code" }),
            )
            .await
            .unwrap();
        assert_eq!(
            world.writes.lock().unwrap()[0].1["config"]["harness"],
            "claude-code"
        );
    }

    #[tokio::test]
    async fn demand_prefix_is_added_to_the_first_prompt() {
        let world = orchestrated();
        let tools = tools(world.clone(), origin("chat-orch-5"));
        tools
            .call(
                "create_chat",
                json!({
                    "project": "space-1", "role": "executor",
                    "demand": "2026-10-24-login", "prompt": "Build it."
                }),
            )
            .await
            .unwrap();
        let writes = world.writes.lock().unwrap();
        let (method, params) = writes.last().unwrap();
        assert_eq!(method, methods::QUEUE_COMMAND);
        assert_eq!(
            params["command"]["request"]["prompt"],
            "Demand: 2026-10-24-login. Read it with read_demand before starting.\n\nBuild it."
        );
        drop(writes);

        let without_prompt = tools
            .call(
                "create_chat",
                json!({ "project": "space-1", "demand": "2026-10-24-login" }),
            )
            .await
            .unwrap_err();
        assert!(
            without_prompt.contains("demand needs a prompt"),
            "{without_prompt}"
        );
    }

    #[tokio::test]
    async fn create_chat_description_lists_roles_in_orchestrator_chats() {
        let world = orchestrated();
        let description = |defs: Vec<ToolDef>| {
            defs.into_iter()
                .find(|d| d.name == "create_chat")
                .unwrap()
                .description
                .into_owned()
        };
        let orchestrator = description(tools(world.clone(), origin("chat-orch-5")).list().await);
        assert!(
            orchestrator.contains(
                "Roles: executor (Implements a plan item); explorador (Reads code, changes nothing)."
            ),
            "{orchestrator}"
        );
        let plain = description(tools(world.clone(), origin("chat-alpha-1")).list().await);
        assert!(!plain.contains("Roles:"), "{plain}");
    }

    #[tokio::test]
    async fn child_tools_use_the_parents_orchestrator_id() {
        let world = orchestrated();
        let child = tools(world.clone(), origin("chat-kid-6"));
        child
            .call(
                "submit_report",
                json!({ "demand": "2026-10-24-login", "text": "done", "orchestratorId": "evil" }),
            )
            .await
            .unwrap();
        child
            .call("read_demand", json!({ "demand": "2026-10-24-login" }))
            .await
            .unwrap();
        child
            .call(
                "request_contract_change",
                json!({ "demand": "2026-10-24-login", "reason": "The API differs." }),
            )
            .await
            .unwrap();
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes[0].0, methods::ORCH_SUBMIT_REPORT);
        assert_eq!(writes[0].1["orchestratorId"], "plataforma");
        assert_eq!(writes[0].1["childId"], "chat-kid-6");
        assert_eq!(writes[0].1["targetDeviceId"], "dev-local");
        assert_eq!(writes[0].1["text"], "done");
        assert_eq!(writes[1].0, methods::ORCH_READ_DEMAND);
        assert_eq!(writes[1].1["orchestratorId"], "plataforma");
        assert_eq!(writes[1].1["targetDeviceId"], "dev-local");
        assert_eq!(writes[2].0, methods::QUEUE_MESSAGE);
        assert_eq!(writes[2].1["chatId"], "chat-orch-5");
        assert_eq!(
            writes[2].1["text"],
            "[Contract change requested by Kid (chat-kid-6) for demand 2026-10-24-login]\n\nThe API differs."
        );
    }

    #[tokio::test]
    async fn update_plan_validates_items_and_writes_them_for_the_orchestrator() {
        let world = orchestrated();
        let tools = tools(world.clone(), origin("chat-orch-5"));
        tools
            .call(
                "update_plan",
                json!({ "demand": "2026-10-24-login", "items": [
                    { "text": "Add endpoint", "status": "inProgress", "role": "executor", "project": "comet" },
                    { "text": "Review", "status": "pending" }
                ]}),
            )
            .await
            .unwrap();
        let invalid = tools
            .call(
                "update_plan",
                json!({ "demand": "d", "items": [{ "text": "x", "status": "done" }] }),
            )
            .await
            .unwrap_err();
        assert!(invalid.contains("invalid arguments"), "{invalid}");
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, methods::ORCH_WRITE_PLAN);
        assert_eq!(writes[0].1["orchestratorId"], "plataforma");
        assert_eq!(writes[0].1["items"][0]["status"], "inProgress");
        assert_eq!(writes[0].1["items"][0]["role"], "executor");
        assert_eq!(writes[0].1["items"][1]["status"], "pending");
    }

    #[tokio::test]
    async fn orchestrator_tools_address_the_orchestrators_device() {
        let world = orchestrated();
        let orchestrator = tools(world.clone(), origin("chat-orch-5"));
        orchestrator
            .call("remember", json!({ "note": "small PRs" }))
            .await
            .unwrap();
        orchestrator
            .call("create_chat", json!({ "project": "space-1" }))
            .await
            .unwrap();
        assert_eq!(
            world.writes.lock().unwrap()[0].1["targetDeviceId"],
            "dev-local"
        );
        let reads = world.reads.lock().unwrap();
        let (_, get) = reads
            .iter()
            .find(|(method, _)| method == methods::GET_ORCHESTRATOR)
            .unwrap();
        assert_eq!(get["orchestratorId"], "plataforma");
        assert_eq!(get["targetDeviceId"], "dev-local");
    }

    #[tokio::test]
    async fn only_the_orchestrator_can_start_its_children() {
        let world = orchestrated();
        for from in [Origin::default(), origin("chat-alpha-1")] {
            let refused = tools(world.clone(), from)
                .call(
                    "create_chat",
                    json!({ "project": "space-unique", "parent": "Plataforma" }),
                )
                .await
                .unwrap_err();
            assert!(
                refused.contains("Only the orchestrator can start its own children."),
                "{refused}"
            );
        }
        assert!(world.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_orchestrators_cwd_must_stay_inside_the_project() {
        let world = orchestrated();
        let tools = tools(world.clone(), origin("chat-orch-5"));
        for cwd in [
            "/repo/other",
            "/repo/comet/../other",
            "/repo/comet-evil",
            "relative/path",
            "/repo/comet/../../etc",
        ] {
            let refused = tools
                .call("create_chat", json!({ "project": "space-1", "cwd": cwd }))
                .await
                .unwrap_err();
            assert!(refused.contains("cwd must be inside"), "{cwd}: {refused}");
        }
        assert!(world.writes.lock().unwrap().is_empty());

        for cwd in [
            "/repo/comet",
            "/repo/comet/packages/app",
            "/repo/comet/./a/../b",
        ] {
            tools
                .call("create_chat", json!({ "project": "space-1", "cwd": cwd }))
                .await
                .unwrap();
        }
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes.len(), 3);
        assert_eq!(writes[1].1["cwd"], "/repo/comet/packages/app");
    }

    #[tokio::test]
    async fn a_plain_chats_cwd_is_not_restricted() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        tools
            .call(
                "create_chat",
                json!({ "project": "space-1", "cwd": "/elsewhere" }),
            )
            .await
            .unwrap();
        assert_eq!(world.writes.lock().unwrap()[0].1["cwd"], "/elsewhere");
    }

    #[tokio::test]
    async fn the_role_is_not_resolved_when_everything_it_would_fill_is_explicit() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        tools
            .call(
                "create_chat",
                json!({ "role": "executor", "model": "opus", "reasoning": "low", "sandbox": "read-only" }),
            )
            .await
            .unwrap();
        tools
            .call(
                "create_chat",
                json!({ "role": "executor", "model": "opus" }),
            )
            .await
            .unwrap();
        let reads = world.reads.lock().unwrap();
        let resolves = reads
            .iter()
            .filter(|(method, _)| method == methods::RESOLVE_ROLE_MODEL)
            .count();
        assert_eq!(resolves, 1);
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes[0].1["config"]["role"], "executor");
        assert_eq!(writes[0].1["config"]["sandbox"], "read-only");
    }

    #[tokio::test]
    async fn create_chats_from_an_orchestrator_enforces_project_scope() {
        let world = orchestrated();
        let tools = tools(world.clone(), origin("chat-orch-5"));
        let batch = tools
            .call(
                "create_chats",
                json!({ "requests": [
                    { "project": "space-unique" },
                    { "project": "space-1", "kind": "chat" },
                    { "project": "space-1", "role": "executor" }
                ]}),
            )
            .await
            .unwrap();
        let results = batch["results"].as_array().unwrap();
        assert_eq!(results[0]["isError"], true);
        assert!(
            results[0]["error"]
                .as_str()
                .unwrap()
                .contains("is not associated"),
            "{batch}"
        );
        assert_eq!(results[1]["isError"], true);
        assert_eq!(results[2]["isError"], false, "{batch}");
        assert_eq!(results[2]["result"]["parentChatId"], "chat-orch-5");
        assert_eq!(world.writes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_failed_scope_lookup_does_not_hide_orchestration_tools() {
        let world = Arc::new(World::default());
        let ghost = tools(world, origin("chat-missing-9"));
        assert!(ghost.has("write_spec").await);
        assert!(!ghost.has("no_such_tool").await);
        let err = ghost
            .call("write_spec", json!({ "demand": "d", "text": "t" }))
            .await
            .unwrap_err();
        assert!(!err.contains("unknown tool"), "{err}");
    }
}
