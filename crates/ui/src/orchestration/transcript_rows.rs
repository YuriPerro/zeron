use gpui::{
    AnyElement, App, Entity, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use zeron_doc::SessionMessageEntry;
use zeron_proto::{Chat, HarnessId};

use super::messages::{ChildEventKind, MessageOrigin, child_event_text, classify_message};
use super::sidebar::{is_orchestrator, orchestrator_avatar};
use crate::state::AppState;
use crate::theme::Theme;
use crate::transcript::{Row, RowKind};

pub(crate) fn row_for_message(
    entry: &SessionMessageEntry,
    text: &str,
    raw_len: usize,
    pending: bool,
) -> Option<Row> {
    let mut timestamp = None;
    let mut copy_text = None;
    let kind = match classify_message(text) {
        MessageOrigin::Plain => return None,
        MessageOrigin::ChildEvent {
            chat_id,
            title,
            kind,
        } => RowKind::ChildEvent {
            chat_id: chat_id.into(),
            title: title.into(),
            kind,
        },
        MessageOrigin::FromChat {
            chat_id,
            label,
            body,
            contract_demand,
        } => {
            timestamp = Some(entry.created_at);
            copy_text = (!body.trim().is_empty()).then(|| SharedString::from(body.clone()));
            RowKind::FromChat {
                chat_id: chat_id.into(),
                label: label.into(),
                body: body.into(),
                contract_demand: contract_demand.map(Into::into),
            }
        }
    };
    Some(Row {
        id: entry.id.clone().into(),
        version: (raw_len as u64) << 1 | pending as u64,
        turn_start: true,
        kind,
        entry_id: entry.id.clone().into(),
        timestamp,
        copy_text,
        compact_fold: None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Avatar {
    Orchestrator,
    Harness(HarnessId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sender {
    pub name: SharedString,
    pub avatar: Option<Avatar>,
}

pub(crate) fn sender_for(chats: &[Chat], chat_id: &str, label: &str) -> Sender {
    let Some(chat) = chats.iter().find(|chat| chat.id == chat_id) else {
        return Sender {
            name: label.to_owned().into(),
            avatar: None,
        };
    };
    let name = chat
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(label);
    let avatar = if is_orchestrator(chat) {
        Some(Avatar::Orchestrator)
    } else {
        chat.config
            .as_ref()
            .map(|config| Avatar::Harness(config.harness))
    };
    Sender {
        name: name.to_owned().into(),
        avatar,
    }
}

fn chat_exists(chats: &[Chat], chat_id: &str) -> bool {
    chats.iter().any(|chat| chat.id == chat_id)
}

pub(crate) fn child_event_line(
    row_id: &SharedString,
    chat_id: &SharedString,
    title: &SharedString,
    kind: ChildEventKind,
    state: &Entity<AppState>,
    theme: &Theme,
    cx: &App,
) -> AnyElement {
    let tone = match kind {
        ChildEventKind::Finished => theme.text_muted,
        ChildEventKind::WaitingForAnswer => theme.warning,
        ChildEventKind::Failed => theme.danger,
    };
    let open = chat_exists(&state.read(cx).chats, chat_id).then(|| {
        let state = state.clone();
        let chat_id = chat_id.to_string();
        div()
            .id(SharedString::from(format!("{row_id}-open")))
            .flex_none()
            .cursor_pointer()
            .text_color(theme.accent)
            .on_click(move |_, _, cx| {
                state.update(cx, |state, cx| state.select_chat(Some(chat_id.clone()), cx));
            })
            .child("Open")
    });
    div()
        .py(px(6.0))
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(crate::typography::ui_rems(13.0))
        .child(div().flex_none().size(px(6.0)).rounded_full().bg(tone))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(theme.text_muted)
                .child(SharedString::from(child_event_text(title, kind))),
        )
        .children(open)
        .into_any_element()
}

pub(crate) fn from_chat_bubble(
    sender: &Sender,
    chat_id: &SharedString,
    contract_demand: Option<&SharedString>,
    body: AnyElement,
    max_width: f32,
    theme: &Theme,
) -> AnyElement {
    let avatar = sender.avatar.map(|avatar| match avatar {
        Avatar::Orchestrator => div()
            .flex_none()
            .size(px(16.0))
            .child(orchestrator_avatar(&sender.name, chat_id, false, theme))
            .into_any_element(),
        Avatar::Harness(harness) => {
            let (path, tint) = crate::pickers::harness_brand_icon(harness);
            crate::icons::icon(path)
                .size(px(15.0))
                .flex_none()
                .text_color(tint.unwrap_or(theme.text_muted))
                .into_any_element()
        }
    });
    let tag = contract_demand.map(|demand| {
        div()
            .flex_none()
            .px(px(6.0))
            .rounded(px(4.0))
            .bg(crate::theme::ink(0.07))
            .text_size(crate::typography::ui_rems(11.0))
            .text_color(theme.text_muted)
            .child(SharedString::from(format!("Contract change · {demand}")))
    });
    let header = div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.0))
        .pb(px(4.0))
        .children(avatar)
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(crate::typography::ui_rems(13.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(sender.name.clone()),
        )
        .children(tag);
    div()
        .w_full()
        .flex()
        .justify_start()
        .child(
            div()
                .min_w_0()
                .max_w(px(max_width))
                .bg(crate::theme::user_bubble_bg())
                .rounded(px(Theme::BUBBLE_RADIUS))
                .px(px(16.0))
                .py(px(10.0))
                .text_size(crate::typography::ui_rems(14.0))
                .text_color(theme.text)
                .child(header)
                .child(body),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use zeron_doc::{MessagePart, MessageRole, MessageStatus};

    use super::*;
    use crate::markdown::parser::parse_full;
    use crate::orchestration::sidebar::test_support::chat;
    use crate::transcript::rows_for_entry;

    const ID: &str = "9db29bd0-7723-4660-bc6e-c7230a3afbb6";

    fn user_entry(text: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: "u1".into(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.into(),
            }],
            created_at: 1,
            device_id: "dev".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn rows(text: &str, pending: bool) -> Vec<Row> {
        rows_for_entry(&user_entry(text), pending, false, &mut |_, text| {
            Arc::new(parse_full(text))
        })
    }

    fn wake(title: &str) -> String {
        format!("[Zeron: {title} ({ID}) finished its turn. Read it with read_chat.]")
    }

    #[test]
    fn wake_text_becomes_child_event_row() {
        let rows = rows(&wake("Fix login"), false);
        assert_eq!(rows.len(), 1);
        match &rows[0].kind {
            RowKind::ChildEvent {
                chat_id,
                title,
                kind,
            } => {
                assert_eq!(chat_id.as_ref(), ID);
                assert_eq!(title.as_ref(), "Fix login");
                assert_eq!(*kind, ChildEventKind::Finished);
            }
            _ => panic!("expected a child event row"),
        }
        assert_eq!(rows[0].id.as_ref(), "u1");
        assert!(rows[0].turn_start);
    }

    #[test]
    fn event_for_missing_chat_still_renders_title() {
        let rows = rows(&wake("Gone chat"), false);
        let RowKind::ChildEvent { title, kind, .. } = &rows[0].kind else {
            panic!("expected a child event row");
        };
        assert_eq!(child_event_text(title, *kind), "Gone chat finished");
        assert!(!chat_exists(&[], ID));
    }

    #[test]
    fn child_event_version_tracks_pending_like_user_rows() {
        let confirmed = rows(&wake("x"), false);
        let echoed = rows(&wake("x"), true);
        assert_eq!(confirmed[0].id, echoed[0].id);
        assert_ne!(confirmed[0].version, echoed[0].version);
    }

    #[test]
    fn near_miss_stays_user_row() {
        let rows = rows("[Zeron: x (y) finished its turn.]", false);
        assert!(matches!(rows[0].kind, RowKind::User { .. }));
    }

    fn from_chat_text(body: &str) -> String {
        format!(
            "[Message from Zeron chat Plataforma (9db29bd0). Reply to it with the Zeron `send_message` tool, chat {ID}.]\n\n{body}"
        )
    }

    #[test]
    fn message_from_chat_becomes_from_chat_row() {
        let rows = rows(&from_chat_text("Goal: ship"), true);
        assert_eq!(rows.len(), 1);
        match &rows[0].kind {
            RowKind::FromChat {
                chat_id,
                label,
                body,
                contract_demand,
            } => {
                assert_eq!(chat_id.as_ref(), ID);
                assert_eq!(label.as_ref(), "Plataforma");
                assert_eq!(body.as_ref(), "Goal: ship");
                assert!(contract_demand.is_none());
            }
            _ => panic!("expected a from-chat row"),
        }
        assert_eq!(rows[0].copy_text.as_deref(), Some("Goal: ship"));
        assert_eq!(rows[0].timestamp, Some(1));
    }

    #[test]
    fn contract_change_row_carries_demand() {
        let text = format!(
            "[Contract change requested by Kid ({ID}) for demand 2026-10-24-login]\n\nThe API differs."
        );
        let rows = rows(&text, false);
        let RowKind::FromChat {
            label,
            body,
            contract_demand,
            ..
        } = &rows[0].kind
        else {
            panic!("expected a from-chat row");
        };
        assert_eq!(label.as_ref(), "Kid");
        assert_eq!(body.as_ref(), "The API differs.");
        assert_eq!(contract_demand.as_deref(), Some("2026-10-24-login"));
    }

    #[test]
    fn plain_prompt_stays_user_row() {
        let rows = rows("fix the login bug", false);
        assert!(matches!(rows[0].kind, RowKind::User { .. }));
    }

    #[test]
    fn sender_prefers_current_title_and_orchestrator_avatar() {
        let mut orchestrator = chat(ID, None, true, 1);
        orchestrator.title = Some("Renamed".into());
        let sender = sender_for(&[orchestrator], ID, "Old label");
        assert_eq!(sender.name.as_ref(), "Renamed");
        assert_eq!(sender.avatar, Some(Avatar::Orchestrator));
    }

    #[test]
    fn sender_that_is_a_plain_chat_uses_its_harness_icon() {
        let mut worker = chat(ID, Some("o"), false, 1);
        worker.config.as_mut().unwrap().harness = HarnessId::Codex;
        let sender = sender_for(&[worker], ID, "label");
        assert_eq!(sender.avatar, Some(Avatar::Harness(HarnessId::Codex)));
    }

    #[test]
    fn missing_or_untitled_sender_falls_back_to_the_label() {
        let sender = sender_for(&[], ID, "Plataforma");
        assert_eq!(sender.name.as_ref(), "Plataforma");
        assert_eq!(sender.avatar, None);
        let mut untitled = chat(ID, None, false, 1);
        untitled.title = Some("  ".into());
        assert_eq!(
            sender_for(&[untitled], ID, "9db29bd0").name.as_ref(),
            "9db29bd0"
        );
    }

    #[test]
    fn chat_exists_matches_by_id() {
        assert!(chat_exists(&[chat(ID, None, false, 1)], ID));
        assert!(!chat_exists(&[chat("other", None, false, 1)], ID));
    }
}
