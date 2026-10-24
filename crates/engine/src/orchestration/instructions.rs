use std::path::Path;

use zeron_proto::Chat;

use super::roles::load_roles;
use super::store::Store;

/// Standing rules appended to a role prompt when the chat works for an orchestrator.
pub const CHILD_RULES: &str = "You are working for a Zeron orchestrator. Your brief names a demand: read it with read_demand before starting. Finish with submit_report: what you did, how you proved it, what is left. If the contract in the spec must change, call request_contract_change instead of changing it.";

/// Resolves the instructions a chat's runs start with. An orchestrator chat
/// gets its `prompt.md` plus its memory; a role chat gets the role prompt, with
/// [`CHILD_RULES`] when `parent` is an orchestrator chat; any other chat gets none.
pub fn instructions_for(chat: &Chat, parent: Option<&Chat>, data_dir: &Path) -> Option<String> {
    let config = chat.config.as_ref()?;
    if let Some(id) = &config.orchestrator {
        return orchestrator_instructions(id, data_dir);
    }
    let name = config.role.as_deref()?;
    let (roles, _) = load_roles(&data_dir.join("roles"));
    let Some(role) = roles.into_iter().find(|role| role.name == name) else {
        tracing::warn!(chat = %chat.id, role = name, "chat role not found; running without instructions");
        return None;
    };
    let under_orchestrator =
        parent.is_some_and(|p| p.config.as_ref().is_some_and(|c| c.orchestrator.is_some()));
    Some(if under_orchestrator {
        format!("{}\n\n{CHILD_RULES}", role.prompt)
    } else {
        role.prompt
    })
}

fn orchestrator_instructions(id: &str, data_dir: &Path) -> Option<String> {
    let store = Store::new(data_dir);
    let mut text = match store.prompt(id) {
        Ok(prompt) => prompt,
        Err(err) => {
            tracing::warn!(orchestrator = id, error = %err, "orchestrator prompt unreadable; running without instructions");
            return None;
        }
    };
    match store.memory(id) {
        Ok(memory) if !memory.trim().is_empty() => {
            text.push_str("\n\n## Memory\n");
            text.push_str(&memory);
        }
        Ok(_) => {}
        Err(err) => {
            tracing::warn!(orchestrator = id, error = %err, "orchestrator memory unreadable; skipping it");
        }
    }
    Some(text)
}

/// Folds run instructions into the first prompt for harnesses that have no
/// system-prompt channel of their own.
pub fn prefix_first_prompt(instructions: &str, prompt: &str) -> String {
    format!("<instructions>\n{instructions}\n</instructions>\n\n{prompt}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::store::{OrchestratorConfig, Store};
    use zeron_proto::{ChatConfig, HarnessId, SandboxLevel};

    fn chat(id: &str, role: Option<&str>, orchestrator: Option<&str>) -> Chat {
        let mut chat: Chat = serde_json::from_value(serde_json::json!({
            "id": id,
            "title": null,
            "deviceId": "mac",
            "archived": false,
            "cwd": null,
            "branch": null,
            "checkoutId": null,
            "config": null,
            "lastMessagePreview": null,
            "lastMessageAt": null,
            "createdAt": chrono::Utc::now(),
        }))
        .unwrap();
        chat.config = Some(ChatConfig {
            harness: HarnessId::Mock,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            sandbox: SandboxLevel::WorkspaceWrite,
            role: role.map(str::to_owned),
            orchestrator: orchestrator.map(str::to_owned),
        });
        chat
    }

    fn data_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store
            .create(&OrchestratorConfig {
                id: "o1".into(),
                name: "Orchestrator".into(),
                avatar: None,
                harness: HarnessId::Mock,
                model: None,
                reasoning: None,
                child_harness: HarnessId::Mock,
                projects: Vec::new(),
            })
            .unwrap();
        let roles = dir.path().join("roles");
        std::fs::create_dir_all(&roles).unwrap();
        std::fs::write(
            roles.join("executor.md"),
            "+++\nname = \"executor\"\ndescription = \"d\"\nmodel = \"@medio\"\n+++\n\nRole prompt.\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn orchestrator_chat_gets_prompt_and_memory() {
        let dir = data_dir();
        let prompt = Store::new(dir.path()).prompt("o1").unwrap();
        let chat = chat("c1", None, Some("o1"));

        assert_eq!(
            instructions_for(&chat, None, dir.path()),
            Some(prompt.clone())
        );

        std::fs::write(
            dir.path().join("orchestrators/o1/memoria.md"),
            "Remember X.\n",
        )
        .unwrap();
        assert_eq!(
            instructions_for(&chat, None, dir.path()),
            Some(format!("{prompt}\n\n## Memory\nRemember X.\n"))
        );
    }

    #[test]
    fn whitespace_only_memory_adds_no_section() {
        let dir = data_dir();
        std::fs::write(dir.path().join("orchestrators/o1/memoria.md"), " \n\n").unwrap();
        let prompt = Store::new(dir.path()).prompt("o1").unwrap();

        assert_eq!(
            instructions_for(&chat("c1", None, Some("o1")), None, dir.path()),
            Some(prompt)
        );
    }

    #[test]
    fn orchestrator_wins_over_role() {
        let dir = data_dir();
        let prompt = Store::new(dir.path()).prompt("o1").unwrap();

        assert_eq!(
            instructions_for(&chat("c1", Some("executor"), Some("o1")), None, dir.path()),
            Some(prompt)
        );
    }

    #[test]
    fn role_child_of_orchestrator_gets_role_prompt_and_child_rules() {
        let dir = data_dir();
        let parent = chat("p", None, Some("o1"));
        let child = chat("c1", Some("executor"), None);

        assert_eq!(
            instructions_for(&child, Some(&parent), dir.path()),
            Some(format!("Role prompt.\n\n{CHILD_RULES}"))
        );
    }

    #[test]
    fn role_chat_without_orchestrator_parent_gets_only_role_prompt() {
        let dir = data_dir();
        let child = chat("c1", Some("executor"), None);
        let plain_parent = chat("p", None, None);

        assert_eq!(
            instructions_for(&child, None, dir.path()),
            Some("Role prompt.".into())
        );
        assert_eq!(
            instructions_for(&child, Some(&plain_parent), dir.path()),
            Some("Role prompt.".into())
        );
    }

    #[test]
    fn plain_chat_gets_no_instructions() {
        let dir = data_dir();

        assert_eq!(
            instructions_for(&chat("c1", None, None), None, dir.path()),
            None
        );

        let mut bare = chat("c2", None, None);
        bare.config = None;
        assert_eq!(instructions_for(&bare, None, dir.path()), None);
    }

    #[test]
    fn missing_role_or_orchestrator_files_give_none() {
        let dir = data_dir();

        assert_eq!(
            instructions_for(&chat("c1", Some("ghost"), None), None, dir.path()),
            None
        );
        assert_eq!(
            instructions_for(&chat("c2", None, Some("ghost")), None, dir.path()),
            None
        );
    }

    #[test]
    fn prefix_wraps_instructions_before_the_prompt() {
        assert_eq!(
            prefix_first_prompt("A", "B"),
            "<instructions>\nA\n</instructions>\n\nB"
        );
    }
}
