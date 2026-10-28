use zeron_doc::{CompactionStatus, MessagePart, MessageRole, SessionMessageEntry};
use zeron_proto::{AgentEvent, ContextUsage};

/// A compaction the provider reported without its "before" count gets the
/// chat's last context reading, taken before the compaction moved it.
pub(crate) fn with_before(event: AgentEvent, usage: Option<ContextUsage>) -> AgentEvent {
    match event {
        AgentEvent::Compaction {
            phase,
            tokens_before: None,
            tokens_after,
        } => AgentEvent::Compaction {
            phase,
            tokens_before: usage.and_then(|usage| usage.tokens),
            tokens_after,
        },
        other => other,
    }
}

/// Whether the chat's last reply ends a run owing a compaction its "after"
/// count: the pending state is derived from the transcript, so it survives an
/// engine restart.
pub(crate) fn awaits_after(entries: &[SessionMessageEntry]) -> bool {
    entries
        .iter()
        .rev()
        .find(|entry| entry.role == MessageRole::Assistant)
        .is_some_and(|entry| has_pending(&entry.parts))
}

/// Stamps the last pending compaction with `tokens`. Returns whether one was
/// pending.
pub(crate) fn fill_after(parts: &mut [MessagePart], tokens: u64) -> bool {
    let Some(MessagePart::Compaction { tokens_after, .. }) =
        parts.iter_mut().rev().find(|part| is_pending(part))
    else {
        return false;
    };
    *tokens_after = Some(tokens);
    true
}

/// Whether a compaction is still running in `parts`. It already holds the
/// "before" reading taken when it started, and any context reading that lands
/// while it runs belongs to the compaction itself, not to a settled one.
pub(crate) fn has_running(parts: &[MessagePart]) -> bool {
    parts.iter().any(|part| {
        matches!(
            part,
            MessagePart::Compaction {
                status: CompactionStatus::Running,
                ..
            }
        )
    })
}

pub(crate) fn has_pending(parts: &[MessagePart]) -> bool {
    parts.iter().any(is_pending)
}

fn is_pending(part: &MessagePart) -> bool {
    matches!(
        part,
        MessagePart::Compaction {
            status: CompactionStatus::Done,
            tokens_after: None,
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::MessageStatus;
    use zeron_proto::CompactionPhase;

    fn compaction(
        status: CompactionStatus,
        before: Option<u64>,
        after: Option<u64>,
    ) -> MessagePart {
        MessagePart::Compaction {
            id: "c0".into(),
            status,
            tokens_before: before,
            tokens_after: after,
        }
    }

    fn entry(role: MessageRole, parts: Vec<MessagePart>) -> SessionMessageEntry {
        SessionMessageEntry {
            id: "e".into(),
            role,
            parts,
            created_at: 1,
            device_id: "dev".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn finished(before: Option<u64>, after: Option<u64>) -> AgentEvent {
        AgentEvent::Compaction {
            phase: CompactionPhase::Finished,
            tokens_before: before,
            tokens_after: after,
        }
    }

    #[test]
    fn with_before_fills_only_a_missing_before() {
        let usage = Some(ContextUsage {
            tokens: Some(224_000),
            window: Some(1_000_000),
        });
        assert_eq!(
            with_before(finished(None, None), usage),
            finished(Some(224_000), None)
        );
        assert_eq!(
            with_before(finished(Some(5), None), usage),
            finished(Some(5), None)
        );
        assert_eq!(
            with_before(finished(None, Some(9)), usage),
            finished(Some(224_000), Some(9))
        );
        assert_eq!(
            with_before(finished(None, None), None),
            finished(None, None)
        );
        let text = AgentEvent::TextDelta { text: "hi".into() };
        assert_eq!(with_before(text.clone(), usage), text);
    }

    #[test]
    fn awaits_after_reads_only_the_last_reply() {
        let pending = compaction(CompactionStatus::Done, Some(1), None);
        let user = entry(MessageRole::User, vec![]);
        assert!(awaits_after(&[
            user.clone(),
            entry(MessageRole::Assistant, vec![pending.clone()])
        ]));
        assert!(!awaits_after(&[
            user.clone(),
            entry(MessageRole::Assistant, vec![pending.clone()]),
            entry(
                MessageRole::Assistant,
                vec![MessagePart::Text {
                    id: "t0".into(),
                    text: "later".into()
                }]
            ),
        ]));
        assert!(!awaits_after(&[
            user.clone(),
            entry(
                MessageRole::Assistant,
                vec![compaction(CompactionStatus::Done, Some(1), Some(2))]
            ),
        ]));
        assert!(!awaits_after(&[
            user,
            entry(
                MessageRole::Assistant,
                vec![compaction(CompactionStatus::Running, Some(1), None)]
            ),
        ]));
        assert!(!awaits_after(&[]));
    }

    #[test]
    fn awaits_after_looks_past_a_trailing_system_entry() {
        let pending = compaction(CompactionStatus::Done, Some(1), None);
        assert!(awaits_after(&[
            entry(MessageRole::Assistant, vec![pending]),
            entry(MessageRole::System, vec![]),
        ]));
    }

    #[test]
    fn fill_after_sets_the_last_pending_part() {
        let mut parts = vec![
            compaction(CompactionStatus::Done, Some(10), None),
            MessagePart::Text {
                id: "t1".into(),
                text: "between".into(),
            },
            compaction(CompactionStatus::Done, Some(20), None),
        ];
        assert!(fill_after(&mut parts, 31_000));
        assert_eq!(parts[0], compaction(CompactionStatus::Done, Some(10), None));
        assert_eq!(
            parts[2],
            compaction(CompactionStatus::Done, Some(20), Some(31_000))
        );
        assert!(fill_after(&mut parts, 7));
        assert_eq!(
            parts[0],
            compaction(CompactionStatus::Done, Some(10), Some(7))
        );
        assert_eq!(
            parts[2],
            compaction(CompactionStatus::Done, Some(20), Some(31_000))
        );
        assert!(!fill_after(&mut parts, 1));
    }

    #[test]
    fn fill_after_leaves_a_running_part_alone() {
        let mut parts = vec![compaction(CompactionStatus::Running, Some(10), None)];
        assert!(!fill_after(&mut parts, 5));
        assert_eq!(
            parts[0],
            compaction(CompactionStatus::Running, Some(10), None)
        );
    }

    #[test]
    fn has_running_sees_only_running_parts() {
        assert!(has_running(&[
            compaction(CompactionStatus::Done, Some(1), Some(2)),
            compaction(CompactionStatus::Running, Some(1), None),
        ]));
        assert!(!has_running(&[compaction(
            CompactionStatus::Done,
            None,
            None
        )]));
        assert!(!has_running(&[MessagePart::Text {
            id: "t0".into(),
            text: "hi".into()
        }]));
        assert!(!has_running(&[]));
    }

    #[test]
    fn has_pending_sees_only_done_parts_without_after() {
        assert!(has_pending(&[compaction(
            CompactionStatus::Done,
            None,
            None
        )]));
        assert!(!has_pending(&[compaction(
            CompactionStatus::Done,
            None,
            Some(1)
        )]));
        assert!(!has_pending(&[compaction(
            CompactionStatus::Running,
            None,
            None
        )]));
        assert!(!has_pending(&[]));
    }
}
