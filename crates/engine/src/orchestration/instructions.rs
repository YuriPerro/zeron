use std::path::Path;

use zeron_proto::{Chat, HarnessId, RunRequest};

use super::roles::load_roles;
use super::store::Store;

/// Standing rules appended to a role prompt when the chat works for an orchestrator.
pub const CHILD_RULES: &str = "You are working for a Zeron orchestrator. Your brief names a demand: read it with read_demand before starting. Finish with submit_report: what you did, how you proved it, what is left. If the contract in the spec must change, call request_contract_change instead of changing it.";

/// Resolves the instructions a chat's runs start with. An orchestrator chat
/// gets its `prompt.md` plus its memory; a role chat gets the role prompt, with
/// [`CHILD_RULES`] when `parent` is an orchestrator chat, and a roleless child of
/// an orchestrator gets [`CHILD_RULES`] alone; any other chat gets none.
pub fn instructions_for(chat: &Chat, parent: Option<&Chat>, data_dir: &Path) -> Option<String> {
    let config = chat.config.as_ref()?;
    if let Some(id) = &config.orchestrator {
        return orchestrator_instructions(id, data_dir);
    }
    let under_orchestrator =
        parent.is_some_and(|p| p.config.as_ref().is_some_and(|c| c.orchestrator.is_some()));
    let role_prompt = config
        .role
        .as_deref()
        .and_then(|name| role_prompt(chat, name, data_dir));
    match (role_prompt, under_orchestrator) {
        (Some(prompt), true) => Some(format!("{prompt}\n\n{CHILD_RULES}")),
        (Some(prompt), false) => Some(prompt),
        (None, true) => Some(CHILD_RULES.to_owned()),
        (None, false) => None,
    }
}

fn role_prompt(chat: &Chat, name: &str, data_dir: &Path) -> Option<String> {
    let (roles, _) = load_roles(&data_dir.join("roles"));
    let role = roles.into_iter().find(|role| role.name == name);
    if role.is_none() {
        tracing::warn!(chat = %chat.id, role = name, "chat role not found; running without instructions");
    }
    role.map(|role| role.prompt)
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

/// Folds run instructions into the first prompt of a fresh session on harnesses
/// that have no system-prompt channel of their own.
pub fn fold_into_first_prompt(harness: HarnessId, request: &mut RunRequest) {
    if matches!(harness, HarnessId::ClaudeCode | HarnessId::Codex) || request.resume.is_some() {
        return;
    }
    if let Some(instructions) = &request.instructions {
        request.prompt = prefix_first_prompt(instructions, &request.prompt);
    }
}

fn prefix_first_prompt(instructions: &str, prompt: &str) -> String {
    format!("<instructions>\n{instructions}\n</instructions>\n\n{prompt}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::store::{OrchestratorConfig, Store};
    use zeron_proto::{ChatConfig, SandboxLevel};

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
            access: Default::default(),
            plan: false,
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
    fn roleless_child_of_orchestrator_gets_child_rules() {
        let dir = data_dir();
        let parent = chat("p", None, Some("o1"));
        let child = chat("c1", None, None);

        assert_eq!(
            instructions_for(&child, Some(&parent), dir.path()),
            Some(CHILD_RULES.to_owned())
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

    fn request(resume: Option<&str>) -> RunRequest {
        RunRequest {
            instructions: Some("A".into()),
            mcp: None,
            prompt: "B".into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: "/tmp".into(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            access: Default::default(),
            plan: false,
            resume: resume.map(str::to_owned),
            attachments: Vec::new(),
            worktree: None,
            env: Default::default(),
        }
    }

    #[test]
    fn instructions_fold_into_a_fresh_prompt_only_where_the_harness_has_no_channel() {
        let folded = "<instructions>\nA\n</instructions>\n\nB";

        let mut hermes = request(None);
        fold_into_first_prompt(HarnessId::Hermes, &mut hermes);
        assert_eq!(hermes.prompt, folded);

        let mut resumed = request(Some("thread"));
        fold_into_first_prompt(HarnessId::Hermes, &mut resumed);
        assert_eq!(resumed.prompt, "B");

        for harness in [HarnessId::ClaudeCode, HarnessId::Codex] {
            let mut native = request(None);
            fold_into_first_prompt(harness, &mut native);
            assert_eq!(native.prompt, "B");
        }
    }

    #[test]
    fn prefix_wraps_instructions_before_the_prompt() {
        assert_eq!(
            prefix_first_prompt("A", "B"),
            "<instructions>\nA\n</instructions>\n\nB"
        );
    }
}
