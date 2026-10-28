use std::collections::HashSet;

use gpui::{AnyElement, App, EntityId, SharedString, div, prelude::*, px};
use zeron_doc::{CompactionStatus, MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_proto::invocation::{invocation_prompt, leading_command};

use crate::theme::Theme;

pub(crate) fn short_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }
    let thousands = tokens.saturating_add(500) / 1_000;
    if thousands < 1_000 {
        return format!("{thousands}K");
    }
    let tenths = tokens.saturating_add(50_000) / 100_000;
    if tenths % 10 == 0 {
        format!("{}M", tenths / 10)
    } else {
        format!("{}.{}M", tenths / 10, tenths % 10)
    }
}

pub(crate) fn label(
    status: CompactionStatus,
    streaming: bool,
    before: Option<u64>,
    after: Option<u64>,
) -> String {
    match (status, before, after) {
        (CompactionStatus::Running, ..) if streaming => "Compacting context".into(),
        (CompactionStatus::Running, ..) => "Compaction stopped".into(),
        (CompactionStatus::Done, Some(before), Some(after)) => format!(
            "Compacted · {} → {} tokens",
            short_tokens(before),
            short_tokens(after)
        ),
        (CompactionStatus::Done, Some(before), None) => {
            format!("Compacted · from {} tokens", short_tokens(before))
        }
        (CompactionStatus::Done, None, Some(after)) => {
            format!("Compacted · down to {} tokens", short_tokens(after))
        }
        (CompactionStatus::Done, None, None) => "Compacted".into(),
    }
}

pub(crate) fn running(status: CompactionStatus, streaming: bool) -> bool {
    status == CompactionStatus::Running && streaming
}

pub(crate) fn is_compact_command(raw: &str) -> bool {
    leading_command(&invocation_prompt(raw)).is_some_and(|(name, _)| name == "compact")
}

fn has_compaction(entry: &SessionMessageEntry) -> bool {
    entry
        .parts
        .iter()
        .any(|part| matches!(part, MessagePart::Compaction { .. }))
}

fn prompt_text(entry: &SessionMessageEntry) -> String {
    entry
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Ids of the `/compact` prompts whose reply carries the compaction line, so
/// the line stands in for the bubble. A prompt without that reply keeps its
/// bubble.
pub(crate) fn hidden_bubbles(entries: &[SessionMessageEntry]) -> HashSet<String> {
    entries
        .windows(2)
        .filter(|pair| {
            pair[0].role == MessageRole::User
                && has_compaction(&pair[1])
                && is_compact_command(&prompt_text(&pair[0]))
        })
        .map(|pair| pair[0].id.clone())
        .collect()
}

pub(crate) fn compacting_now(entry: &SessionMessageEntry) -> bool {
    entry.status == Some(MessageStatus::Streaming)
        && entry.parts.iter().any(|part| {
            matches!(
                part,
                MessagePart::Compaction {
                    status: CompactionStatus::Running,
                    ..
                }
            )
        })
}

pub(crate) fn marker(
    label: SharedString,
    running: bool,
    view: EntityId,
    theme: &Theme,
    cx: &mut App,
) -> AnyElement {
    let rule = || div().flex_1().min_w_0().h(px(1.0)).bg(theme.border_strong);
    div()
        .py(px(14.0))
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(rule())
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.0))
                .when(running, |el| {
                    el.child(crate::loaders::gradient_spinner(
                        "compaction-spinner",
                        theme,
                        2.5,
                        view,
                        cx,
                    ))
                })
                .child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted.opacity(0.7))
                        .child(label),
                ),
        )
        .child(rule())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, role: MessageRole, parts: Vec<MessagePart>) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role,
            parts,
            created_at: 1,
            device_id: "dev".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn text(id: &str, text: &str) -> MessagePart {
        MessagePart::Text {
            id: id.into(),
            text: text.into(),
        }
    }

    fn compaction(status: CompactionStatus) -> MessagePart {
        MessagePart::Compaction {
            id: "c1".into(),
            status,
            tokens_before: Some(224_463),
            tokens_after: None,
        }
    }

    #[test]
    fn short_tokens_rounds_like_the_spec() {
        for (n, s) in [
            (850, "850"),
            (31_499, "31K"),
            (224_463, "224K"),
            (999_499, "999K"),
            (999_500, "1M"),
            (1_000_000, "1M"),
            (1_234_567, "1.2M"),
        ] {
            assert_eq!(short_tokens(n), s, "{n}");
        }
    }

    #[test]
    fn labels_cover_every_state() {
        use CompactionStatus::*;
        assert_eq!(
            label(Running, true, Some(224_463), None),
            "Compacting context"
        );
        assert_eq!(
            label(Running, false, Some(224_463), None),
            "Compaction stopped"
        );
        assert_eq!(
            label(Done, false, Some(224_463), Some(31_000)),
            "Compacted · 224K → 31K tokens"
        );
        assert_eq!(
            label(Done, true, Some(224_463), None),
            "Compacted · from 224K tokens"
        );
        assert_eq!(
            label(Done, false, None, Some(31_000)),
            "Compacted · down to 31K tokens"
        );
        assert_eq!(label(Done, false, None, None), "Compacted");
        assert!(running(Running, true));
        assert!(!running(Running, false));
        assert!(!running(Done, true));
    }

    #[test]
    fn compact_command_matches_typed_and_picked_forms() {
        let picked = zeron_proto::invocation::Invocation::Command {
            name: "compact".into(),
        }
        .link();
        assert!(is_compact_command("/compact"));
        assert!(is_compact_command("  /compact keep the plan"));
        assert!(is_compact_command(&picked));
        assert!(!is_compact_command("/review"));
        assert!(!is_compact_command("please compact"));
    }

    #[test]
    fn compact_bubble_hides_only_when_the_reply_has_the_line() {
        let user = |body: &str| entry("u1", MessageRole::User, vec![text("t", body)]);
        let reply = |parts| entry("a1", MessageRole::Assistant, parts);

        let with_line = [
            user("/compact"),
            reply(vec![compaction(CompactionStatus::Done)]),
        ];
        assert_eq!(
            hidden_bubbles(&with_line),
            HashSet::from(["u1".to_string()])
        );

        let text_only = [user("/compact"), reply(vec![text("t2", "ok")])];
        assert!(hidden_bubbles(&text_only).is_empty());

        let other_prompt = [
            user("hello"),
            reply(vec![compaction(CompactionStatus::Done)]),
        ];
        assert!(hidden_bubbles(&other_prompt).is_empty());

        let unanswered = [user("/compact")];
        assert!(hidden_bubbles(&unanswered).is_empty());
    }

    #[test]
    fn compacting_now_needs_a_streaming_entry_with_a_running_line() {
        let mut live = entry(
            "a1",
            MessageRole::Assistant,
            vec![compaction(CompactionStatus::Running)],
        );
        live.status = Some(MessageStatus::Streaming);
        assert!(compacting_now(&live));

        let mut stopped = live.clone();
        stopped.status = Some(MessageStatus::Complete);
        assert!(!compacting_now(&stopped));

        let mut done = live.clone();
        done.parts = vec![compaction(CompactionStatus::Done)];
        assert!(!compacting_now(&done));

        let mut no_line = live;
        no_line.parts = vec![text("t", "working")];
        assert!(!compacting_now(&no_line));
    }

    #[gpui::test]
    fn marker_draws_with_and_without_the_spinner(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        struct Probe(bool);
        impl Render for Probe {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                let theme = Theme::of(cx).clone();
                let label = label(CompactionStatus::Running, self.0, Some(224_463), None);
                div()
                    .w(px(400.0))
                    .child(marker(label.into(), self.0, cx.entity_id(), &theme, cx))
            }
        }
        for running in [true, false] {
            let (_, window) = cx.add_window_view(|_, _| Probe(running));
            window.update(|window, cx| window.draw(cx).clear());
        }
    }
}
