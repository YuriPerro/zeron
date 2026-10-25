//! `zeron orchestrator` and `zeron roles`: create orchestrators and check that
//! every role resolves to a real model, through the same engine client and
//! project resolution the MCP server uses.

use anyhow::{Context, bail};
use clap::Subcommand;
use serde_json::{Value, json};
use zeron_mcp::{Origin, Zeron, default_harness};
use zeron_rpc::methods;

#[derive(Subcommand)]
pub enum OrchestratorCommand {
    /// Create an orchestrator and its chat.
    Create {
        #[arg(long)]
        name: String,
        /// Space id, path or name; repeat for several projects.
        #[arg(long, required = true)]
        project: Vec<String>,
        /// Harness of the orchestrator chat (default: the one `create_chat` picks).
        #[arg(long)]
        harness: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        reasoning: Option<String>,
        /// Harness for the chats it spawns (default: the orchestrator's own).
        #[arg(long)]
        child_harness: Option<String>,
    },
    /// List orchestrators with their chat and projects.
    List,
}

#[derive(Subcommand)]
pub enum RolesCommand {
    /// Resolve every role on every connected harness; exits non-zero when a
    /// role fails on all of them or a role file does not parse.
    Check,
}

struct RoleCheck {
    role: String,
    harness: String,
    outcome: Result<String, String>,
}

pub async fn orchestrator(
    command: OrchestratorCommand,
    ipc_port: u16,
    data_dir: std::path::PathBuf,
) -> anyhow::Result<()> {
    let zeron = Zeron::new(ipc_port, Some(data_dir), Origin::default());
    match command {
        OrchestratorCommand::Create {
            name,
            project,
            harness,
            model,
            reasoning,
            child_harness,
        } => {
            let name = name.trim().to_owned();
            let projects = resolve_projects(&zeron, &project).await?;
            let harness = match harness {
                Some(harness) => harness,
                None => local_default_harness(&zeron).await?,
            };
            let mut params = json!({ "name": name, "harness": harness, "projects": projects });
            for (key, value) in [
                ("model", model),
                ("reasoning", reasoning),
                ("childHarness", child_harness),
            ] {
                if let Some(value) = value {
                    params[key] = json!(value);
                }
            }
            let reply = zeron.call(methods::CREATE_ORCHESTRATOR, params).await?;
            let chat_id = reply
                .get("chatId")
                .and_then(Value::as_str)
                .context("CreateOrchestrator reply is missing chatId")?;
            println!("{}", created_line(&name, chat_id));
            Ok(())
        }
        OrchestratorCommand::List => {
            let orchestrators = zeron.call(methods::LIST_ORCHESTRATORS, json!({})).await?;
            let (chats, spaces) = tokio::try_join!(zeron.chats(), zeron.spaces())?;
            for config in orchestrators.as_array().into_iter().flatten() {
                let id = config["id"].as_str().unwrap_or_default();
                let chat_id = chats
                    .iter()
                    .find(|chat| {
                        chat.config
                            .as_ref()
                            .and_then(|config| config.orchestrator.as_deref())
                            == Some(id)
                    })
                    .map(|chat| chat.id.as_str());
                let projects: Vec<String> = config["projects"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|space_id| {
                        spaces
                            .iter()
                            .find(|space| space.id == space_id)
                            .map_or_else(|| space_id.to_owned(), |s| s.display_name().to_owned())
                    })
                    .collect();
                println!(
                    "{}",
                    list_line(config["name"].as_str().unwrap_or(id), chat_id, &projects)
                );
            }
            Ok(())
        }
    }
}

pub async fn roles(
    command: RolesCommand,
    ipc_port: u16,
    data_dir: std::path::PathBuf,
) -> anyhow::Result<()> {
    let RolesCommand::Check = command;
    let zeron = Zeron::new(ipc_port, Some(data_dir), Origin::default());
    let device = zeron.local_device_id().await?;
    let catalog = zeron.list_roles().await?;
    let harnesses: Vec<_> = zeron
        .harnesses_on(Some(&device))
        .await?
        .into_iter()
        .filter(|harness| harness.connected())
        .collect();
    if harnesses.is_empty() {
        bail!("no connected harness on this device (see `list_harnesses`)");
    }

    let mut checks = Vec::new();
    for role in &catalog.roles {
        for harness in &harnesses {
            let outcome = zeron
                .resolve_role_model(&role.name, harness.id, None, Some(&device))
                .await
                .map(|resolved| {
                    let reasoning = resolved
                        .reasoning
                        .and_then(|level| serde_json::to_value(level).ok())
                        .and_then(|value| value.as_str().map(str::to_owned));
                    resolved_text(&resolved.model, reasoning.as_deref())
                })
                .map_err(|err| format!("{err:#}"));
            let harness = serde_json::to_value(harness.id)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| harness.name.clone());
            let check = RoleCheck {
                role: role.name.clone(),
                harness,
                outcome,
            };
            println!("{}", check_line(&check));
            checks.push(check);
        }
    }
    for error in &catalog.errors {
        eprintln!("role file error: {error}");
    }

    let names: Vec<&str> = catalog.roles.iter().map(|r| r.name.as_str()).collect();
    let failing = failing_roles(&names, &checks);
    if !failing.is_empty() {
        bail!("no harness resolves: {}", failing.join(", "));
    }
    if !catalog.errors.is_empty() {
        bail!("{} role file(s) failed to load", catalog.errors.len());
    }
    Ok(())
}

async fn resolve_projects(zeron: &Zeron, keys: &[String]) -> anyhow::Result<Vec<String>> {
    let mut ids = Vec::with_capacity(keys.len());
    for key in keys {
        ids.push(zeron.resolve_space(key).await?.id);
    }
    Ok(ids)
}

async fn local_default_harness(zeron: &Zeron) -> anyhow::Result<String> {
    let device = zeron.local_device_id().await?;
    let harnesses = zeron.harnesses_on(Some(&device)).await?;
    let id = default_harness(&harnesses)?;
    serde_json::to_value(id)?
        .as_str()
        .map(str::to_owned)
        .context("harness id is not a string")
}

fn created_line(name: &str, chat_id: &str) -> String {
    format!("Created {name}: chat {chat_id}")
}

fn list_line(name: &str, chat_id: Option<&str>, projects: &[String]) -> String {
    let projects = if projects.is_empty() {
        "-".to_owned()
    } else {
        projects.join(", ")
    };
    format!("{name}  {}  {projects}", chat_id.unwrap_or("-"))
}

fn resolved_text(model: &str, reasoning: Option<&str>) -> String {
    format!("{model} · {}", reasoning.unwrap_or("default"))
}

fn check_line(check: &RoleCheck) -> String {
    let result = match &check.outcome {
        Ok(resolved) => resolved.clone(),
        Err(error) => format!("failed: {error}"),
    };
    format!("{}  {}  {result}", check.role, check.harness)
}

fn failing_roles<'a>(roles: &[&'a str], checks: &[RoleCheck]) -> Vec<&'a str> {
    roles
        .iter()
        .copied()
        .filter(|role| {
            !checks
                .iter()
                .any(|check| check.role == *role && check.outcome.is_ok())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(role: &str, harness: &str, outcome: Result<&str, &str>) -> RoleCheck {
        RoleCheck {
            role: role.into(),
            harness: harness.into(),
            outcome: outcome.map(str::to_owned).map_err(str::to_owned),
        }
    }

    #[test]
    fn formats_created_and_list_lines() {
        assert_eq!(
            created_line("Loja Online", "chat-1"),
            "Created Loja Online: chat chat-1"
        );
        assert_eq!(
            list_line("Loja", Some("chat-1"), &["web".into(), "api".into()]),
            "Loja  chat-1  web, api"
        );
        assert_eq!(list_line("Loja", None, &[]), "Loja  -  -");
    }

    #[test]
    fn formats_role_check_lines() {
        assert_eq!(resolved_text("sonnet", Some("high")), "sonnet · high");
        assert_eq!(resolved_text("gpt-5", None), "gpt-5 · default");
        assert_eq!(
            check_line(&check("executor", "codex", Ok("gpt-5 · high"))),
            "executor  codex  gpt-5 · high"
        );
        assert_eq!(
            check_line(&check("executor", "pi", Err("no model for @medio"))),
            "executor  pi  failed: no model for @medio"
        );
    }

    #[test]
    fn a_role_fails_only_when_no_harness_resolves_it() {
        let checks = [
            check("executor", "claude-code", Ok("sonnet · high")),
            check("executor", "pi", Err("boom")),
            check("revisor", "claude-code", Err("boom")),
            check("revisor", "pi", Err("boom")),
        ];
        assert_eq!(
            failing_roles(&["executor", "revisor"], &checks),
            ["revisor"]
        );
        assert!(failing_roles(&["executor"], &checks).is_empty());
    }
}
