//! Tools for orchestrator chats and the children they spawn.
//!
//! Which tools a chat sees depends on where it sits: an orchestrator chat
//! (`config.orchestrator` set) drives demands, a child (its parent is an
//! orchestrator chat) reads them and reports back. The orchestrator id is
//! always taken from the chat's own lineage, never from tool arguments.

use std::borrow::Cow;

use anyhow::Context;
use serde::Deserialize;
use serde_json::{Value, json};
use zeron_proto::Space;

use crate::tools::{ToolDef, Tools};
use crate::zeron::{OrchestratorInfo, PlanItem, Zeron, resolve_chat_in, short};

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
