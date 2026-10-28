//! Work running under the open chat, derived once for every surface that
//! lists it: the lanes card and the composer's pills. Subagents are the
//! spawn chips of the transcript with their live status; background tasks
//! share their title, stop button and stop flow.

use std::time::Duration;

use chrono::{DateTime, Utc};
use gpui::{
    App, Entity, InteractiveElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, div, prelude::*, px,
};
use zeron_doc::{MessagePart, SubagentStatus};
use zeron_proto::{BackgroundTask, ChatIndicator};
use zeron_rpc::methods;

use crate::state::{AppState, clock_now};
use crate::theme::Theme;

/// A spawn chip of the open transcript, as its lane lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubagentRow {
    pub doc_id: String,
    pub title: SharedString,
    pub status: Option<SubagentStatus>,
    /// When the latest turn that spawned (or steered) it was written — the
    /// closest thing a subagent has to a last-updated time.
    pub spawned_at: DateTime<Utc>,
    /// The model the spawn named, when it named one.
    pub model: Option<SharedString>,
}

impl SubagentRow {
    /// A settled subagent is frozen: its tab reads the uploaded blob first.
    pub fn frozen(&self) -> bool {
        matches!(
            self.status,
            Some(SubagentStatus::Done) | Some(SubagentStatus::Failed)
        )
    }

    pub fn indicator(&self) -> ChatIndicator {
        match self.status {
            Some(SubagentStatus::Running) => ChatIndicator::Working,
            Some(SubagentStatus::Done) => ChatIndicator::Completed,
            Some(SubagentStatus::Failed) => ChatIndicator::Errored,
            None => ChatIndicator::Idle,
        }
    }
}

/// The open chat's subagents, one row per subagent doc: running ones first,
/// longest-running leading, then the settled ones most recently updated first
/// (later spawns lead within one turn).
/// Only genuine spawn chips with a stamped doc ref qualify — the chip IS the
/// index (there is no listing endpoint), and a stray ref on a non-Agent tool
/// must not surface as a phantom subagent.
pub(crate) fn subagent_rows(state: &AppState, chat_id: &str) -> Vec<SubagentRow> {
    if state.selected_chat.as_deref() != Some(chat_id) {
        return Vec::new();
    }
    let mut rows: Vec<SubagentRow> = Vec::new();
    for entry in &state.transcript {
        let spawned_at =
            DateTime::<Utc>::from_timestamp_millis(entry.created_at).unwrap_or_else(Utc::now);
        for part in &entry.parts {
            let MessagePart::Tool {
                call,
                subagent_ref: Some(doc_id),
                subagent_status,
                ..
            } = part
            else {
                continue;
            };
            if !call.is_subagent_spawn() {
                continue;
            }
            let row = SubagentRow {
                doc_id: doc_id.clone(),
                title: crate::transcript::subagent_tab_title(call),
                status: *subagent_status,
                spawned_at,
                model: call
                    .subagent_model()
                    .map(|model| SharedString::from(model.to_owned())),
            };
            match rows.iter_mut().find(|r| r.doc_id == row.doc_id) {
                // A reopened (steered) subagent updates its row in place.
                Some(existing) => *existing = row,
                None => rows.push(row),
            }
        }
    }
    // Stable sort over the reversed spawn order: ties keep the later spawn
    // on top.
    rows.reverse();
    rows.sort_by_key(|row| std::cmp::Reverse(row.spawned_at));
    // Running subagents lead, longest-running first: one started long ago is
    // buried under everything spawned since, and is the one to find and steer.
    // Oldest-first also keeps the lane still as new subagents join at its
    // foot. The settled tail keeps the newest-first order above.
    let (mut running, settled): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|row| row.status == Some(SubagentStatus::Running));
    // Ties keep spawn order, like the lane's oldest-first order.
    running.reverse();
    running.sort_by_key(|row| row.spawned_at);
    running.extend(settled);
    running
}

/// The subagents of `chat_id` still running, in [`subagent_rows`] order.
pub(crate) fn running_subagent_rows(state: &AppState, chat_id: &str) -> Vec<SubagentRow> {
    let mut rows = subagent_rows(state, chat_id);
    rows.retain(|row| row.status == Some(SubagentStatus::Running));
    rows
}

/// What a background task runs, or its id when it says nothing.
pub(crate) fn background_task_title(task: &BackgroundTask) -> SharedString {
    if task.description.is_empty() {
        task.id.clone().into()
    } else {
        task.description.clone().into()
    }
}

/// The stop button's square; a row insets it as far from its edges.
pub(crate) const STOP_SIZE: f32 = 20.0;
const STOP_SQUARE: f32 = 7.0;

/// A background task's stop button, `radius` concentric with its row. While
/// the stop is in flight (`stopping`) it is inert.
pub(crate) fn background_stop_button(
    key: &str,
    stopping: bool,
    radius: f32,
    theme: &Theme,
    on_stop: impl Fn(&mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let danger = theme.danger;
    let accent = theme.accent;
    div()
        .id(SharedString::from(format!("{key}-stop")))
        .role(gpui::Role::Button)
        .aria_label("Stop task")
        .size(px(STOP_SIZE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius))
        .bg(danger.opacity(0.14))
        .when(stopping, |el| {
            el.cursor(gpui::CursorStyle::Arrow).opacity(0.45)
        })
        .when(!stopping, |el| {
            el.cursor_pointer()
                .hover(move |s| s.bg(danger.opacity(0.30)))
                .tab_index(0)
                .focus_visible(move |s| s.bg(accent.opacity(0.18)))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    on_stop(cx);
                })
        })
        .tooltip(crate::settings::widgets::text_tooltip("Stop task"))
        .tooltip_show_delay(Duration::from_millis(350))
        .child(div().size(px(STOP_SQUARE)).rounded(px(1.5)).bg(danger))
}

/// Ask the chat's host to stop one of its background tasks. A refusal says
/// so only while the task still runs — one that ended on its own in the
/// meantime needs no toast. Each surface keeps its own in-flight mark and
/// clears it on `Err`.
pub(crate) fn stop_background_task(
    state: &Entity<AppState>,
    chat_id: String,
    task_id: String,
    cx: &mut App,
) -> Task<Result<(), String>> {
    let (engine, host) = {
        let state = state.read(cx);
        let host = state
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .map(|chat| chat.device_id.clone());
        (state.engine().cloned(), host)
    };
    let Some(engine) = engine else {
        return Task::ready(Err("not connected".into()));
    };
    let mut params = serde_json::json!({ "chatId": chat_id, "taskId": task_id });
    if let Some(host) = host {
        params["targetDeviceId"] = serde_json::Value::String(host);
    }
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Err(err) = engine
            .client()
            .call(methods::STOP_BACKGROUND_TASK, params)
            .await
        else {
            return Ok(());
        };
        tracing::warn!(error = %err, "stopping a background task failed");
        cx.update(|cx| {
            let running = state
                .read(cx)
                .background_tasks_for(&chat_id, clock_now())
                .iter()
                .any(|task| task.id == task_id);
            if running {
                crate::toast::error(cx, format!("Couldn't stop the task: {err}"));
            }
        });
        Err(err.to_string())
    })
}

/// How long `since` has run as of `now`: `41s`, `1m 12s` or `1h 03m`.
pub(crate) fn elapsed_label(since: DateTime<Utc>, now: DateTime<Utc>) -> SharedString {
    let secs = now.signed_duration_since(since).num_seconds().max(0);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::{MessageRole, MessageStatus, SessionMessageEntry};
    use zeron_proto::ToolCall;

    fn spawn(
        id: &str,
        name: &str,
        doc: Option<&str>,
        status: Option<SubagentStatus>,
    ) -> MessagePart {
        MessagePart::Tool {
            id: id.into(),
            call: ToolCall::Unknown {
                name: name.into(),
                input: Some(serde_json::json!({ "description": "verify the marker pipeline" })),
            },
            is_error: false,
            resolved: true,
            output: None,
            diff: None,
            output_ref: None,
            output_bytes: None,
            diff_ref: None,
            diff_stats: None,
            subagent_ref: doc.map(str::to_owned),
            subagent_status: status,
            subagent_tail: None,
        }
    }

    fn entry(parts: Vec<MessagePart>) -> SessionMessageEntry {
        SessionMessageEntry {
            duration_ms: None,
            id: "e1".into(),
            role: MessageRole::Assistant,
            parts,
            created_at: (Utc::now() - chrono::Duration::minutes(3)).timestamp_millis(),
            device_id: "dev".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
        }
    }

    fn at(id: &str, minutes_ago: i64, parts: Vec<MessagePart>) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            created_at: (Utc::now() - chrono::Duration::minutes(minutes_ago)).timestamp_millis(),
            ..entry(parts)
        }
    }

    fn sub(id: &str, status: SubagentStatus) -> MessagePart {
        spawn(
            id,
            &format!("Agent: {id}"),
            Some(&format!("main--sub--{id}")),
            Some(status),
        )
    }

    #[test]
    fn subagent_rows_list_only_stamped_spawn_chips_of_the_selected_chat() {
        let mut state = AppState::new();
        state.selected_chat = Some("main".into());
        state.transcript = vec![entry(vec![
            spawn(
                "t1",
                "Agent: verify",
                Some("main--sub--t1"),
                Some(SubagentStatus::Running),
            ),
            // No doc ref yet: the engine stamps it asynchronously.
            spawn("t2", "Agent: later", None, None),
            // A stray ref on a non-spawn tool never surfaces.
            spawn(
                "t3",
                "Read",
                Some("main--sub--t3"),
                Some(SubagentStatus::Done),
            ),
            spawn(
                "t4",
                "Agent: done",
                Some("main--sub--t4"),
                Some(SubagentStatus::Done),
            ),
        ])];
        let rows = subagent_rows(&state, "main");
        assert_eq!(
            rows.iter().map(|r| r.doc_id.as_str()).collect::<Vec<_>>(),
            ["main--sub--t1", "main--sub--t4"]
        );
        // The bare task, genus stripped — the same title the tab wears.
        assert_eq!(rows[0].title.as_ref(), "verify");
        assert!(rows[1].frozen());
        assert!(!rows[0].frozen());
        // Spawn time comes from the turn that carried the chip.
        assert!((Utc::now() - rows[0].spawned_at).num_minutes() >= 2);
        // Another chat sees nothing of this transcript.
        assert!(subagent_rows(&state, "other").is_empty());
    }

    #[test]
    fn a_steered_subagent_moves_up_with_its_newer_turn() {
        let mut state = AppState::new();
        state.selected_chat = Some("main".into());
        state.transcript = vec![
            at(
                "e1",
                30,
                vec![
                    sub("a", SubagentStatus::Done),
                    sub("b", SubagentStatus::Done),
                ],
            ),
            at("e2", 20, vec![sub("c", SubagentStatus::Done)]),
            at("e3", 10, vec![sub("a", SubagentStatus::Done)]),
        ];
        let ids: Vec<_> = subagent_rows(&state, "main")
            .into_iter()
            .map(|r| r.doc_id)
            .collect();
        assert_eq!(ids, ["main--sub--a", "main--sub--c", "main--sub--b"]);
    }

    #[test]
    fn running_subagents_lead_longest_running_first() {
        let mut state = AppState::new();
        state.selected_chat = Some("main".into());
        state.transcript = vec![
            at("e1", 90, vec![sub("old-run", SubagentStatus::Running)]),
            at("e2", 60, vec![sub("old-done", SubagentStatus::Done)]),
            at("e3", 30, vec![sub("mid-run", SubagentStatus::Running)]),
            at("e4", 20, vec![sub("mid-fail", SubagentStatus::Failed)]),
            at("e5", 10, vec![sub("new-done", SubagentStatus::Done)]),
            at("e6", 5, vec![sub("new-run", SubagentStatus::Running)]),
        ];
        let (finished, active): (Vec<_>, Vec<_>) = subagent_rows(&state, "main")
            .into_iter()
            .partition(SubagentRow::frozen);
        let ids = |rows: &[SubagentRow]| -> Vec<String> {
            rows.iter().map(|r| r.doc_id.clone()).collect()
        };
        // The running lead, longest-running first; the settled follow
        // newest first.
        assert_eq!(
            ids(&active),
            [
                "main--sub--old-run",
                "main--sub--mid-run",
                "main--sub--new-run"
            ]
        );
        assert_eq!(
            ids(&finished),
            [
                "main--sub--new-done",
                "main--sub--mid-fail",
                "main--sub--old-done"
            ]
        );
    }

    #[test]
    fn running_rows_drop_settled_subagents() {
        let mut state = AppState::new();
        state.selected_chat = Some("main".into());
        state.transcript = vec![entry(vec![
            sub("run", SubagentStatus::Running),
            sub("done", SubagentStatus::Done),
        ])];
        let rows = running_subagent_rows(&state, "main");
        assert_eq!(
            rows.iter().map(|r| r.doc_id.as_str()).collect::<Vec<_>>(),
            ["main--sub--run"]
        );
    }

    #[test]
    fn elapsed_label_formats() {
        let now = Utc::now();
        let label = |secs: i64| elapsed_label(now - chrono::Duration::seconds(secs), now);
        assert_eq!(label(41).as_ref(), "41s");
        assert_eq!(label(72).as_ref(), "1m 12s");
        assert_eq!(label(3780).as_ref(), "1h 03m");
        assert_eq!(label(-5).as_ref(), "0s");
    }
}
