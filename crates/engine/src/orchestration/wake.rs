//! Wakes an orchestrator chat when one of its children stops, so the
//! orchestrator never has to poll for results.

use std::collections::HashMap;

use tokio::sync::watch;
use zeron_proto::{Session, SessionStatus};

use crate::doc_host::DocHost;

/// Why a child stopped, which decides what the orchestrator is told to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildStop {
    Finished,
    WaitingForAnswer,
    Failed,
}

/// The agent-facing text queued to the orchestrator when a child stops.
pub fn wake_message(child_title: &str, child_id: &str, transition: ChildStop) -> String {
    let what = match transition {
        ChildStop::Finished => "finished its turn. Read it with read_chat.",
        ChildStop::WaitingForAnswer => "is waiting for an answer. Read its question with get_chat.",
        ChildStop::Failed => "stopped with an error. Read it with read_chat.",
    };
    format!("[Zeron: {child_title} ({child_id}) {what}]")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservedStop {
    pub child_id: String,
    pub stop: ChildStop,
}

#[derive(Debug, Clone)]
struct Seen {
    status: SessionStatus,
    completed_turn: Option<String>,
}

impl Seen {
    fn of(session: &Session) -> Self {
        Self {
            status: session.status,
            completed_turn: session.last_completed_turn.clone(),
        }
    }

    fn baseline() -> Self {
        Self {
            status: SessionStatus::Idle,
            completed_turn: None,
        }
    }
}

/// Turns successive session snapshots into child stops. Seeded from the first
/// snapshot without reporting anything, so a restart or a late subscribe never
/// replays turns that ended before it started looking.
pub(crate) struct StopTracker {
    seen: HashMap<String, Seen>,
    last_fired: HashMap<String, String>,
}

impl StopTracker {
    pub(crate) fn seeded(initial: &[Session]) -> Self {
        Self {
            seen: initial
                .iter()
                .map(|session| (session.chat_id.clone(), Seen::of(session)))
                .collect(),
            last_fired: HashMap::new(),
        }
    }

    pub(crate) fn observe(&mut self, snapshot: &[Session]) -> Vec<ObservedStop> {
        let mut stops = Vec::new();
        for session in snapshot {
            let previous = self
                .seen
                .insert(session.chat_id.clone(), Seen::of(session))
                .unwrap_or_else(Seen::baseline);
            for (stop, key) in transitions(&previous, session) {
                let key = format!("{}:{key}", session.chat_id);
                if self.last_fired.get(&session.chat_id) == Some(&key) {
                    continue;
                }
                self.last_fired.insert(session.chat_id.clone(), key);
                stops.push(ObservedStop {
                    child_id: session.chat_id.clone(),
                    stop,
                });
            }
        }
        stops
    }
}

fn transitions(previous: &Seen, session: &Session) -> Vec<(ChildStop, String)> {
    let change = session.updated_at.timestamp_millis();
    let turn_advanced = session
        .last_completed_turn
        .as_ref()
        .is_some_and(|turn| previous.completed_turn.as_ref() != Some(turn));
    let was_active = matches!(
        previous.status,
        SessionStatus::Working | SessionStatus::AwaitingInput
    );
    let mut found = Vec::new();
    if let Some(turn) = session
        .last_completed_turn
        .as_ref()
        .filter(|_| turn_advanced)
    {
        found.push((ChildStop::Finished, format!("finished:{turn}")));
    } else if session.status == SessionStatus::Idle && was_active {
        found.push((ChildStop::Finished, format!("finished:{change}")));
    }
    match session.status {
        SessionStatus::AwaitingInput if previous.status != SessionStatus::AwaitingInput => {
            found.push((ChildStop::WaitingForAnswer, format!("awaiting:{change}")));
        }
        SessionStatus::Errored if previous.status != SessionStatus::Errored => {
            found.push((ChildStop::Failed, format!("failed:{change}")));
        }
        _ => {}
    }
    found
}

pub(crate) async fn watch_children(host: DocHost, mut statuses: watch::Receiver<Vec<Session>>) {
    let mut tracker = StopTracker::seeded(&statuses.borrow_and_update());
    while statuses.changed().await.is_ok() {
        let snapshot = statuses.borrow_and_update().clone();
        for stop in tracker.observe(&snapshot) {
            wake_parent(&host, &stop);
        }
    }
}

fn wake_parent(host: &DocHost, stop: &ObservedStop) {
    let Some(workspace) = host.workspace() else {
        return;
    };
    let Ok(Some(child)) = workspace.chat(&stop.child_id) else {
        return;
    };
    let Some(parent_id) = child.parent_chat_id.as_deref() else {
        return;
    };
    let Ok(Some(parent)) = workspace.chat(parent_id) else {
        return;
    };
    if parent
        .config
        .is_none_or(|config| config.orchestrator.is_none())
    {
        return;
    }
    if parent.device_id != host.device_id() {
        tracing::debug!(
            child = %stop.child_id,
            parent = %parent_id,
            host = %parent.device_id,
            "waking an orchestrator hosted on another device through the synced queue"
        );
    }
    let title = child.title.as_deref().unwrap_or(&child.id);
    let text = wake_message(title, &child.id, stop.stop);
    if let Err(error) = host.queue_message(parent_id, &text, Vec::new()) {
        tracing::warn!(parent = %parent_id, child = %child.id, %error, "orchestrator wake failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn session(id: &str, status: SessionStatus, turn: Option<&str>, at: i64) -> Session {
        Session {
            last_turn_outcome: None,
            terminal_activity: None,
            last_completed_turn: turn.map(str::to_owned),
            chat_id: id.into(),
            device_id: "dev".into(),
            status,
            started_at: None,
            updated_at: Utc.timestamp_millis_opt(at).unwrap(),
            running_subagents: 0,
        }
    }

    #[test]
    fn wake_messages_name_the_child_and_the_next_tool() {
        assert_eq!(
            wake_message("Fix login", "c-1", ChildStop::Finished),
            "[Zeron: Fix login (c-1) finished its turn. Read it with read_chat.]"
        );
        assert_eq!(
            wake_message("Fix login", "c-1", ChildStop::WaitingForAnswer),
            "[Zeron: Fix login (c-1) is waiting for an answer. Read its question with get_chat.]"
        );
        assert_eq!(
            wake_message("Fix login", "c-1", ChildStop::Failed),
            "[Zeron: Fix login (c-1) stopped with an error. Read it with read_chat.]"
        );
    }

    #[test]
    fn the_first_snapshot_seeds_without_waking() {
        let mut tracker = StopTracker::seeded(&[session("c", SessionStatus::Idle, Some("t1"), 1)]);
        assert!(
            tracker
                .observe(&[session("c", SessionStatus::Idle, Some("t1"), 1)])
                .is_empty()
        );
    }

    #[test]
    fn one_completed_turn_wakes_once() {
        let mut tracker = StopTracker::seeded(&[]);
        assert!(
            tracker
                .observe(&[session("c", SessionStatus::Working, None, 1)])
                .is_empty()
        );
        let done = session("c", SessionStatus::Idle, Some("t1"), 2);
        let stops = tracker.observe(&[done.clone()]);
        assert_eq!(
            stops,
            [ObservedStop {
                child_id: "c".into(),
                stop: ChildStop::Finished
            }]
        );
        assert!(tracker.observe(&[done]).is_empty());
        assert!(
            tracker
                .observe(&[session("c", SessionStatus::Idle, Some("t1"), 9)])
                .is_empty()
        );
    }

    #[test]
    fn a_coalesced_turn_is_caught_by_its_marker() {
        let mut tracker =
            StopTracker::seeded(&[session("c", SessionStatus::Working, Some("t1"), 1)]);
        let stops = tracker.observe(&[session("c", SessionStatus::Working, Some("t2"), 3)]);
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].stop, ChildStop::Finished);
    }

    #[test]
    fn an_interrupt_wakes_as_finished_without_a_turn_marker() {
        let mut tracker = StopTracker::seeded(&[session("c", SessionStatus::Working, None, 1)]);
        let stops = tracker.observe(&[session("c", SessionStatus::Idle, None, 2)]);
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].stop, ChildStop::Finished);
    }

    #[test]
    fn awaiting_and_error_wake_once_per_state_and_again_for_a_new_one() {
        let mut tracker = StopTracker::seeded(&[session("c", SessionStatus::Working, None, 1)]);
        let waiting = session("c", SessionStatus::AwaitingInput, None, 2);
        assert_eq!(
            tracker.observe(&[waiting.clone()])[0].stop,
            ChildStop::WaitingForAnswer
        );
        assert!(tracker.observe(&[waiting]).is_empty());
        assert_eq!(
            tracker.observe(&[session("c", SessionStatus::Working, None, 3)]),
            []
        );
        assert_eq!(
            tracker.observe(&[session("c", SessionStatus::AwaitingInput, None, 4)])[0].stop,
            ChildStop::WaitingForAnswer
        );
        let failed = session("c", SessionStatus::Errored, None, 5);
        assert_eq!(
            tracker.observe(&[failed.clone()])[0].stop,
            ChildStop::Failed
        );
        assert!(tracker.observe(&[failed]).is_empty());
    }
}
