#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildEventKind {
    Finished,
    WaitingForAnswer,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MessageOrigin {
    Plain,
    ChildEvent {
        chat_id: String,
        title: String,
        kind: ChildEventKind,
    },
    FromChat {
        chat_id: String,
        label: String,
        body: String,
        contract_demand: Option<String>,
    },
}

const CHILD_EVENT_VERBS: [(&str, ChildEventKind); 3] = [
    (
        " finished its turn. Read it with read_chat.",
        ChildEventKind::Finished,
    ),
    (
        " is waiting for an answer. Read its question with get_chat.",
        ChildEventKind::WaitingForAnswer,
    ),
    (
        " stopped with an error. Read it with read_chat.",
        ChildEventKind::Failed,
    ),
];

const SHORT_ID_LEN: usize = 8;

pub(crate) fn classify_message(text: &str) -> MessageOrigin {
    child_event(text)
        .or_else(|| message_from_chat(text))
        .or_else(|| contract_change(text))
        .unwrap_or(MessageOrigin::Plain)
}

pub(crate) fn child_event_text(title: &str, kind: ChildEventKind) -> String {
    match kind {
        ChildEventKind::Finished => format!("{title} finished"),
        ChildEventKind::WaitingForAnswer => format!("{title} is waiting for an answer"),
        ChildEventKind::Failed => format!("{title} stopped with an error"),
    }
}

/// One-line label for a queued chat-to-chat message, or `None` for user text.
pub(crate) fn queue_label(text: &str) -> Option<String> {
    match classify_message(text) {
        MessageOrigin::Plain => None,
        MessageOrigin::ChildEvent { title, kind, .. } => Some(child_event_text(&title, kind)),
        MessageOrigin::FromChat {
            label,
            body,
            contract_demand: None,
            ..
        } => Some(format!("{label}: {body}")),
        MessageOrigin::FromChat {
            label,
            contract_demand: Some(demand),
            ..
        } => Some(format!("{label}: contract change · {demand}")),
    }
}

fn title_and_id(head: &str) -> Option<(&str, &str)> {
    let head = head.strip_suffix(')')?;
    let (title, id) = head.rsplit_once(" (")?;
    Some((title, id))
}

fn child_event(text: &str) -> Option<MessageOrigin> {
    let inner = text
        .trim_end()
        .strip_prefix("[Zeron: ")?
        .strip_suffix(']')?;
    CHILD_EVENT_VERBS.iter().find_map(|(verb, kind)| {
        let head = inner.strip_suffix(verb)?;
        let (title, id) = title_and_id(head)?;
        Some(MessageOrigin::ChildEvent {
            chat_id: id.to_owned(),
            title: title.to_owned(),
            kind: *kind,
        })
    })
}

fn message_from_chat(text: &str) -> Option<MessageOrigin> {
    let rest = text.strip_prefix("[Message from Zeron chat ")?;
    let (header, body) = rest.split_once("]\n\n")?;
    let (label, id) =
        header.rsplit_once(". Reply to it with the Zeron `send_message` tool, chat ")?;
    let id = id.strip_suffix('.')?;
    let short = id.get(..SHORT_ID_LEN).unwrap_or(id);
    let label = label
        .strip_suffix(&format!(" ({short})"))
        .or_else(|| label.strip_suffix(&format!(" ({id})")))
        .unwrap_or(label);
    Some(MessageOrigin::FromChat {
        chat_id: id.to_owned(),
        label: label.to_owned(),
        body: body.to_owned(),
        contract_demand: None,
    })
}

fn contract_change(text: &str) -> Option<MessageOrigin> {
    let rest = text.strip_prefix("[Contract change requested by ")?;
    let (header, reason) = rest.split_once("]\n\n")?;
    let (who, demand) = header.rsplit_once(" for demand ")?;
    let (title, id) = title_and_id(who)?;
    Some(MessageOrigin::FromChat {
        chat_id: id.to_owned(),
        label: title.to_owned(),
        body: reason.to_owned(),
        contract_demand: Some(demand.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "9db29bd0-7723-4660-bc6e-c7230a3afbb6";

    fn event(title: &str, id: &str, verb: &str) -> String {
        format!("[Zeron: {title} ({id}){verb}]")
    }

    #[test]
    fn queue_labels_read_like_the_transcript() {
        assert_eq!(
            queue_label(&event("investiga: kit-ui", ID, CHILD_EVENT_VERBS[0].0)).as_deref(),
            Some("investiga: kit-ui finished")
        );
        assert_eq!(
            queue_label(&format!(
                "[Message from Zeron chat api (9db29bd0). Reply to it with the Zeron `send_message` tool, chat {ID}.]\n\nDone."
            ))
            .as_deref(),
            Some("api: Done.")
        );
        assert_eq!(queue_label("fix the build"), None);
    }

    #[test]
    fn classifies_each_child_event() {
        for (verb, kind) in CHILD_EVENT_VERBS {
            assert_eq!(
                classify_message(&event("arch-live: explorar", ID, verb)),
                MessageOrigin::ChildEvent {
                    chat_id: ID.into(),
                    title: "arch-live: explorar".into(),
                    kind,
                }
            );
        }
    }

    #[test]
    fn title_with_parentheses_keeps_last_id() {
        assert_eq!(
            classify_message(&event("fix (urgent)", "abc", CHILD_EVENT_VERBS[0].0)),
            MessageOrigin::ChildEvent {
                chat_id: "abc".into(),
                title: "fix (urgent)".into(),
                kind: ChildEventKind::Finished,
            }
        );
    }

    #[test]
    fn classifies_message_from_chat() {
        let id = "9e66485b-9962-439e-ae83-0d86a5f8fde2";
        let text = format!(
            "[Message from Zeron chat Plataforma (9e66485b). Reply to it with the Zeron `send_message` tool, chat {id}.]\n\nGoal: x"
        );
        assert_eq!(
            classify_message(&text),
            MessageOrigin::FromChat {
                chat_id: id.into(),
                label: "Plataforma".into(),
                body: "Goal: x".into(),
                contract_demand: None,
            }
        );
    }

    #[test]
    fn classifies_contract_change() {
        let text = "[Contract change requested by Kid (chat-kid-6) for demand 2026-10-24-login]\n\nThe API differs.";
        assert_eq!(
            classify_message(text),
            MessageOrigin::FromChat {
                label: "Kid".into(),
                chat_id: "chat-kid-6".into(),
                body: "The API differs.".into(),
                contract_demand: Some("2026-10-24-login".into()),
            }
        );
    }

    #[test]
    fn near_misses_stay_plain() {
        for text in [
            "[Zeron: x (y) finished its turn.]",
            "[Zeron: x (y) finished its turn. Read it with read_chat.] and more",
            "hello",
            "",
        ] {
            assert_eq!(classify_message(text), MessageOrigin::Plain, "{text:?}");
        }
    }

    #[test]
    fn edge_cases_classify_as_expected() {
        let verb = CHILD_EVENT_VERBS[0].0;
        let reply = "Reply to it with the Zeron `send_message` tool, chat";
        let finished = |title: &str| MessageOrigin::ChildEvent {
            chat_id: "abc".into(),
            title: title.into(),
            kind: ChildEventKind::Finished,
        };
        let from_chat = |label: &str, body: &str, demand: Option<&str>| MessageOrigin::FromChat {
            chat_id: ID.into(),
            label: label.into(),
            body: body.into(),
            contract_demand: demand.map(str::to_owned),
        };
        let cases: Vec<(String, MessageOrigin)> = vec![
            (
                event(") finished its turn. Read it with read_chat.", "abc", verb),
                finished(") finished its turn. Read it with read_chat."),
            ),
            (event("two\nlines", "abc", verb), finished("two\nlines")),
            (format!("{}\n", event("api", "abc", verb)), finished("api")),
            (
                format!(
                    "[Message from Zeron chat Plataforma (9db29bd0). {reply} {ID}.]\n\nsee [a]\n\nand ]\n\nmore"
                ),
                from_chat("Plataforma", "see [a]\n\nand ]\n\nmore", None),
            ),
            (
                format!("[Message from Zeron chat 9db29bd0. {reply} {ID}.]\n\nhi"),
                from_chat("9db29bd0", "hi", None),
            ),
            (
                format!("[Contract change requested by 9db29bd0 ({ID}) for demand d-1]\n\nwhy"),
                from_chat("9db29bd0", "why", Some("d-1")),
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(classify_message(&text), expected, "{text:?}");
        }
    }

    #[test]
    fn empty_bodies_still_classify() {
        let reply = "Reply to it with the Zeron `send_message` tool, chat";
        for (text, demand) in [
            (
                format!("[Message from Zeron chat Plataforma (9db29bd0). {reply} {ID}.]\n\n"),
                None,
            ),
            (
                format!("[Contract change requested by Kid ({ID}) for demand d-1]\n\n"),
                Some("d-1".to_owned()),
            ),
        ] {
            let MessageOrigin::FromChat {
                chat_id,
                body,
                contract_demand,
                ..
            } = classify_message(&text)
            else {
                panic!("not classified: {text:?}");
            };
            assert_eq!(chat_id, ID);
            assert_eq!(body, "");
            assert_eq!(contract_demand, demand);
        }
    }

    #[test]
    fn child_event_text_matches_copy() {
        assert_eq!(
            child_event_text("api", ChildEventKind::Failed),
            "api stopped with an error"
        );
        assert_eq!(
            child_event_text("api", ChildEventKind::Finished),
            "api finished"
        );
        assert_eq!(
            child_event_text("api", ChildEventKind::WaitingForAnswer),
            "api is waiting for an answer"
        );
    }
}
