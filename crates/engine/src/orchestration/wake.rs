//! Wakes an orchestrator chat when one of its children stops, so the
//! orchestrator never has to poll for results.

use std::collections::HashMap;

use tokio::sync::watch;
use zeron_proto::{DoneStatus, NativeTurnOutcome, Session, SessionStatus};

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
/// replays turns that ended before it started looking. A stop is a change
/// between two observed snapshots, so a repeated snapshot never repeats it.
pub(crate) struct StopTracker {
    seen: HashMap<String, Seen>,
}

impl StopTracker {
    pub(crate) fn seeded(initial: &[Session]) -> Self {
        Self {
            seen: initial
                .iter()
                .map(|session| (session.chat_id.clone(), Seen::of(session)))
                .collect(),
        }
    }

    pub(crate) fn observe(&mut self, snapshot: &[Session]) -> Vec<ObservedStop> {
        let mut stops = Vec::new();
        for session in snapshot {
            let previous = self
                .seen
                .insert(session.chat_id.clone(), Seen::of(session))
                .unwrap_or_else(Seen::baseline);
            stops.extend(
                transitions(&previous, session)
                    .into_iter()
                    .map(|stop| ObservedStop {
                        child_id: session.chat_id.clone(),
                        stop,
                    }),
            );
        }
        stops
    }
}

fn transitions(previous: &Seen, session: &Session) -> Vec<ChildStop> {
    let turn_advanced = session
        .last_completed_turn
        .as_ref()
        .is_some_and(|turn| previous.completed_turn.as_ref() != Some(turn));
    let was_active = matches!(
        previous.status,
        SessionStatus::Working | SessionStatus::AwaitingInput
    );
    let interrupted = session
        .last_turn_outcome
        .is_some_and(|outcome| outcome.status == DoneStatus::Interrupted);
    let mut found = Vec::new();
    if turn_advanced || (session.status == SessionStatus::Idle && was_active && interrupted) {
        found.push(ChildStop::Finished);
    }
    match session.status {
        SessionStatus::AwaitingInput if previous.status != SessionStatus::AwaitingInput => {
            found.push(ChildStop::WaitingForAnswer);
        }
        SessionStatus::Errored if previous.status != SessionStatus::Errored => {
            found.push(ChildStop::Failed);
        }
        _ => {}
    }
    found
}

enum Delivery {
    Done,
    ChildMissing,
}

/// Keeps stops whose child row had not synced yet for exactly one more
/// attempt on the next snapshot.
#[derive(Default)]
struct Retries {
    pending: Vec<ObservedStop>,
}

impl Retries {
    fn deliver(
        &mut self,
        fresh: Vec<ObservedStop>,
        mut attempt: impl FnMut(&ObservedStop) -> Delivery,
    ) {
        for stop in std::mem::take(&mut self.pending) {
            if let Delivery::ChildMissing = attempt(&stop) {
                tracing::warn!(child = %stop.child_id, "orchestrator wake dropped: child row never appeared");
            }
        }
        for stop in fresh {
            if let Delivery::ChildMissing = attempt(&stop) {
                self.pending.push(stop);
            }
        }
    }
}

pub(crate) async fn watch_children(host: DocHost, mut statuses: watch::Receiver<Vec<Session>>) {
    let mut tracker = StopTracker::seeded(&statuses.borrow_and_update());
    let mut retries = Retries::default();
    while statuses.changed().await.is_ok() {
        let snapshot = statuses.borrow_and_update().clone();
        retries.deliver(tracker.observe(&snapshot), |stop| wake_parent(&host, stop));
    }
}

fn wake_parent(host: &DocHost, stop: &ObservedStop) -> Delivery {
    let Some(workspace) = host.workspace() else {
        return Delivery::Done;
    };
    let child = match workspace.chat(&stop.child_id) {
        Ok(Some(child)) => child,
        Ok(None) => {
            tracing::debug!(child = %stop.child_id, "orchestrator wake waiting: child row not synced yet");
            return Delivery::ChildMissing;
        }
        Err(error) => {
            tracing::warn!(child = %stop.child_id, %error, "orchestrator wake dropped: child lookup failed");
            return Delivery::Done;
        }
    };
    let Some(parent_id) = child.parent_chat_id.as_deref() else {
        return Delivery::Done;
    };
    let parent = match workspace.chat(parent_id) {
        Ok(Some(parent)) => parent,
        Ok(None) => {
            tracing::debug!(child = %child.id, parent = %parent_id, "orchestrator wake skipped: parent row missing");
            return Delivery::Done;
        }
        Err(error) => {
            tracing::warn!(child = %child.id, parent = %parent_id, %error, "orchestrator wake dropped: parent lookup failed");
            return Delivery::Done;
        }
    };
    if parent
        .config
        .is_none_or(|config| config.orchestrator.is_none())
    {
        return Delivery::Done;
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
    if let Err(error) = host.queue_message_with_behavior(parent_id, &text, Vec::new(), true) {
        tracing::warn!(parent = %parent_id, child = %child.id, %error, "orchestrator wake failed");
    }
    Delivery::Done
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

    fn settled_with(mut row: Session, outcome: DoneStatus) -> Session {
        row.last_turn_outcome = Some(NativeTurnOutcome {
            status: outcome,
            finished_at: row.updated_at,
        });
        row
    }

    #[test]
    fn an_interrupt_wakes_once_as_finished_without_a_turn_marker() {
        let mut tracker = StopTracker::seeded(&[session("c", SessionStatus::Working, None, 1)]);
        let interrupted = settled_with(
            session("c", SessionStatus::Idle, None, 2),
            DoneStatus::Interrupted,
        );
        let stops = tracker.observe(&[interrupted.clone()]);
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].stop, ChildStop::Finished);
        assert!(tracker.observe(&[interrupted]).is_empty());
    }

    #[test]
    fn a_handoff_is_not_a_stop_and_its_continuation_wakes_once() {
        let mut tracker = StopTracker::seeded(&[session("c", SessionStatus::Working, None, 1)]);
        let handoff = settled_with(
            session("c", SessionStatus::Idle, None, 2),
            DoneStatus::Completed,
        );
        assert!(tracker.observe(&[handoff]).is_empty());
        assert!(
            tracker
                .observe(&[session("c", SessionStatus::Working, None, 3)])
                .is_empty()
        );
        let done = settled_with(
            session("c", SessionStatus::Idle, Some("t1"), 4),
            DoneStatus::Completed,
        );
        assert_eq!(tracker.observe(&[done]).len(), 1);
    }

    #[test]
    fn an_error_wakes_again_after_the_child_recovers_and_fails_anew() {
        let mut tracker = StopTracker::seeded(&[session("c", SessionStatus::Working, None, 1)]);
        let failed = |at| session("c", SessionStatus::Errored, None, at);
        assert_eq!(tracker.observe(&[failed(2)])[0].stop, ChildStop::Failed);
        assert!(
            tracker
                .observe(&[session("c", SessionStatus::Working, None, 3)])
                .is_empty()
        );
        assert_eq!(tracker.observe(&[failed(4)])[0].stop, ChildStop::Failed);
    }

    fn stop(child: &str) -> ObservedStop {
        ObservedStop {
            child_id: child.into(),
            stop: ChildStop::Finished,
        }
    }

    #[test]
    fn a_stop_whose_child_row_is_missing_retries_once_on_the_next_snapshot() {
        let mut retries = Retries::default();
        let mut attempts = 0;
        retries.deliver(vec![stop("c")], |_| {
            attempts += 1;
            Delivery::ChildMissing
        });
        assert_eq!(attempts, 1);
        retries.deliver(vec![], |_| {
            attempts += 1;
            Delivery::Done
        });
        assert_eq!(attempts, 2);
        retries.deliver(vec![], |_| {
            attempts += 1;
            Delivery::Done
        });
        assert_eq!(attempts, 2);
    }

    #[test]
    fn a_stop_whose_child_row_never_appears_is_dropped_after_one_retry() {
        let mut retries = Retries::default();
        let mut attempts = 0;
        for fresh in [vec![stop("c")], vec![], vec![]] {
            retries.deliver(fresh, |_| {
                attempts += 1;
                Delivery::ChildMissing
            });
        }
        assert_eq!(attempts, 2);
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
