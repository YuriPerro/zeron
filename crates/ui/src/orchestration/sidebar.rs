use std::collections::HashMap;

use zeron_proto::{Chat, ChatIndicator};

pub(crate) const UNTITLED_ORCHESTRATOR: &str = "Orchestrator";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrchestratorGroup {
    pub chat_id: String,
    pub name: String,
    pub indicator: ChatIndicator,
    pub needs_you: bool,
    pub active_children: Vec<String>,
    pub done_children: Vec<String>,
}

pub(crate) fn is_orchestrator(chat: &Chat) -> bool {
    !chat.archived
        && chat
            .config
            .as_ref()
            .is_some_and(|config| config.orchestrator.is_some())
}

fn recency(chat: &Chat) -> chrono::DateTime<chrono::Utc> {
    chat.last_message_at.unwrap_or(chat.created_at)
}

fn is_active(indicator: ChatIndicator) -> bool {
    matches!(
        indicator,
        ChatIndicator::Working | ChatIndicator::AwaitingInput | ChatIndicator::Errored
    )
}

pub(crate) fn orchestrator_groups(chats: &[(Chat, ChatIndicator)]) -> Vec<OrchestratorGroup> {
    let mut children_of: HashMap<&str, Vec<&(Chat, ChatIndicator)>> = HashMap::new();
    for entry in chats {
        let chat = &entry.0;
        if chat.archived || is_orchestrator(chat) {
            continue;
        }
        if let Some(parent) = chat.parent_chat_id.as_deref()
            && parent != chat.id
        {
            children_of.entry(parent).or_default().push(entry);
        }
    }

    let mut orchestrators: Vec<&(Chat, ChatIndicator)> = chats
        .iter()
        .filter(|(chat, _)| is_orchestrator(chat))
        .collect();
    orchestrators.sort_by_key(|(chat, _)| std::cmp::Reverse(recency(chat)));

    orchestrators
        .into_iter()
        .map(|(chat, indicator)| {
            let mut children = children_of.remove(chat.id.as_str()).unwrap_or_default();
            children.sort_by_key(|(child, _)| std::cmp::Reverse(recency(child)));
            let (active, done): (Vec<_>, Vec<_>) = children
                .into_iter()
                .partition(|(_, indicator)| is_active(*indicator));
            let name = chat
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .unwrap_or(UNTITLED_ORCHESTRATOR);
            OrchestratorGroup {
                chat_id: chat.id.clone(),
                name: name.to_owned(),
                indicator: *indicator,
                needs_you: *indicator == ChatIndicator::AwaitingInput,
                active_children: active.into_iter().map(|(c, _)| c.id.clone()).collect(),
                done_children: done.into_iter().map(|(c, _)| c.id.clone()).collect(),
            }
        })
        .collect()
}

pub(crate) fn needing_you(groups: &[OrchestratorGroup]) -> usize {
    groups.iter().filter(|group| group.needs_you).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChatIndicator::*;
    use chrono::{TimeZone, Utc};

    fn chat(id: &str, parent: Option<&str>, orchestrator: bool, at: i64) -> Chat {
        Chat {
            id: id.into(),
            device_id: "dev".into(),
            title: Some(id.into()),
            archived: false,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: Some(zeron_proto::ChatConfig {
                harness: zeron_proto::HarnessId::ClaudeCode,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                role: None,
                orchestrator: orchestrator.then(|| "orch".to_owned()),
            }),
            last_message_preview: None,
            last_message_at: None,
            created_at: Utc.timestamp_opt(at, 0).unwrap(),
            harness_session_id: None,
            harness_session_cwd: None,
            harness_session_harness: None,
            parent_chat_id: parent.map(str::to_owned),
            space_id: None,
            last_seen_at: None,
            room_gen: None,
        }
    }

    fn orch(id: &str, at: i64) -> Chat {
        chat(id, None, true, at)
    }

    fn child(id: &str, parent: &str, at: i64) -> Chat {
        chat(id, Some(parent), false, at)
    }

    #[test]
    fn groups_orchestrators_with_their_children() {
        let chats = vec![
            (orch("o", 1), Idle),
            (child("working", "o", 5), Working),
            (child("idle", "o", 4), Idle),
            (child("completed", "o", 3), Completed),
        ];
        let groups = orchestrator_groups(&chats);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].chat_id, "o");
        assert_eq!(groups[0].active_children, ["working"]);
        assert_eq!(groups[0].done_children, ["idle", "completed"]);
    }

    #[test]
    fn errored_child_stays_active() {
        let chats = vec![
            (orch("o", 1), Idle),
            (child("bad", "o", 2), Errored),
            (child("asking", "o", 3), AwaitingInput),
        ];
        let groups = orchestrator_groups(&chats);
        assert_eq!(groups[0].active_children, ["asking", "bad"]);
        assert!(groups[0].done_children.is_empty());
    }

    #[test]
    fn needs_you_follows_awaiting_input() {
        let chats = vec![(orch("a", 1), AwaitingInput), (orch("b", 2), Working)];
        let groups = orchestrator_groups(&chats);
        assert!(groups.iter().find(|g| g.chat_id == "a").unwrap().needs_you);
        assert!(!groups.iter().find(|g| g.chat_id == "b").unwrap().needs_you);
        assert_eq!(needing_you(&groups), 1);
    }

    #[test]
    fn archived_orchestrator_is_hidden() {
        let mut archived = orch("o", 1);
        archived.archived = true;
        let chats = vec![(archived, Idle), (child("c", "o", 2), Working)];
        assert!(orchestrator_groups(&chats).is_empty());
    }

    #[test]
    fn archived_child_is_skipped() {
        let mut archived = child("gone", "o", 2);
        archived.archived = true;
        let chats = vec![
            (orch("o", 1), Idle),
            (archived, Working),
            (child("kept", "o", 3), Idle),
        ];
        let groups = orchestrator_groups(&chats);
        assert!(groups[0].active_children.is_empty());
        assert_eq!(groups[0].done_children, ["kept"]);
    }

    #[test]
    fn untitled_orchestrator_gets_fallback_name() {
        for title in [None, Some("  ".to_owned())] {
            let mut chat = orch("o", 1);
            chat.title = title;
            let groups = orchestrator_groups(&[(chat, Idle)]);
            assert_eq!(groups[0].name, UNTITLED_ORCHESTRATOR);
        }
    }

    #[test]
    fn plain_parent_is_not_an_orchestrator() {
        let chats = vec![
            (chat("plain", None, false, 1), Idle),
            (child("side", "plain", 2), Working),
        ];
        assert!(orchestrator_groups(&chats).is_empty());
    }

    #[test]
    fn orchestrator_is_never_listed_as_a_child() {
        let chats = vec![
            (orch("o", 1), Idle),
            (chat("nested", Some("o"), true, 2), Working),
            (chat("selfie", Some("selfie"), true, 3), Working),
            (child("real", "o", 4), Working),
        ];
        let groups = orchestrator_groups(&chats);
        let by_id = |id: &str| groups.iter().find(|g| g.chat_id == id).unwrap();
        assert_eq!(by_id("o").active_children, ["real"]);
        assert!(by_id("selfie").active_children.is_empty());
        assert!(by_id("selfie").done_children.is_empty());
    }

    #[test]
    fn groups_and_children_sort_by_recency() {
        let mut recent = orch("recent", 1);
        recent.last_message_at = Some(Utc.timestamp_opt(100, 0).unwrap());
        let chats = vec![
            (orch("older", 50), Idle),
            (recent, Idle),
            (child("c-old", "recent", 2), Idle),
            (child("c-new", "recent", 9), Idle),
        ];
        let groups = orchestrator_groups(&chats);
        let ids: Vec<_> = groups.iter().map(|g| g.chat_id.as_str()).collect();
        assert_eq!(ids, ["recent", "older"]);
        assert_eq!(groups[0].done_children, ["c-new", "c-old"]);
    }
}
