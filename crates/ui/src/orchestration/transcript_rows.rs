use gpui::{
    AnyElement, App, Entity, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use zeron_doc::SessionMessageEntry;
use zeron_proto::Chat;

use super::messages::{ChildEventKind, MessageOrigin, child_event_text, classify_message};
use crate::state::AppState;
use crate::theme::Theme;
use crate::transcript::{Row, RowKind};

pub(crate) fn row_for_message(
    entry: &SessionMessageEntry,
    text: &str,
    raw_len: usize,
    pending: bool,
) -> Option<Row> {
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
        MessageOrigin::FromChat { .. } => return None,
    };
    Some(Row {
        id: entry.id.clone().into(),
        version: (raw_len as u64) << 1 | pending as u64,
        turn_start: true,
        kind,
        entry_id: entry.id.clone().into(),
        timestamp: None,
        copy_text: None,
        compact_fold: None,
    })
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use zeron_doc::{MessagePart, MessageRole, MessageStatus};

    use super::*;
    use crate::markdown::parser::parse_full;
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
}
