//! Staged shell runs written into the prompt as tag groups, and parsed back
//! out of the sent message for the transcript.

pub(crate) const TIMEOUT_NOTE: &str = "timed out after 120s";
const TRUNCATION_NOTE: &str = "output truncated to the last 64 KB";
const CLOSING_PREFIX: &str = "</bash-";
const ESCAPED_CLOSING_PREFIX: &str = "<\\/bash-";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ShellRun {
    pub id: String,
    pub chat_id: String,
    pub command: String,
    pub state: ShellRunState,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ShellRunState {
    Running,
    Done {
        exit_code: Option<i32>,
        output: String,
        truncated: bool,
        timed_out: bool,
    },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShellBlock {
    pub command: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub notes: Vec<String>,
}

fn escape(text: &str) -> String {
    text.replace(CLOSING_PREFIX, ESCAPED_CLOSING_PREFIX)
}

fn unescape(text: &str) -> String {
    text.replace(ESCAPED_CLOSING_PREFIX, CLOSING_PREFIX)
}

fn group(command: &str, output: &str, exit_code: Option<i32>, notes: &[String]) -> String {
    let exit_code = exit_code.map(|code| code.to_string()).unwrap_or_default();
    let mut lines = vec![
        format!("<bash-input>{}</bash-input>", escape(command)),
        format!("<bash-stdout>{}</bash-stdout>", escape(output)),
        format!("<bash-exit-code>{exit_code}</bash-exit-code>"),
    ];
    lines.extend(
        notes
            .iter()
            .map(|note| format!("<bash-note>{}</bash-note>", escape(note))),
    );
    lines.join("\n")
}

/// Done runs and Failed runs go ahead of `text`, one tag group per run.
/// Running runs are skipped. A Failed run carries its error as a note, and a
/// missing exit code is written as an empty element.
pub(crate) fn format_prompt(runs: &[ShellRun], text: &str) -> String {
    let groups: Vec<String> = runs
        .iter()
        .filter_map(|run| match &run.state {
            ShellRunState::Running => None,
            ShellRunState::Done {
                exit_code,
                output,
                truncated,
                timed_out,
            } => {
                let mut notes = Vec::new();
                if *timed_out {
                    notes.push(TIMEOUT_NOTE.to_string());
                }
                if *truncated {
                    notes.push(TRUNCATION_NOTE.to_string());
                }
                Some(group(&run.command, output, *exit_code, &notes))
            }
            ShellRunState::Failed(error) => {
                Some(group(&run.command, "", None, std::slice::from_ref(error)))
            }
        })
        .collect();
    let blocks = groups.join("\n");
    if text.is_empty() {
        blocks
    } else if blocks.is_empty() {
        text.to_string()
    } else {
        format!("{blocks}\n\n{text}")
    }
}

fn element<'a>(text: &'a str, tag: &str) -> Option<(String, &'a str)> {
    let rest = text.trim_start().strip_prefix(&format!("<{tag}>"))?;
    let end = rest.find(&format!("</{tag}>"))?;
    let body = unescape(&rest[..end]);
    Some((body, &rest[end + tag.len() + 3..]))
}

fn block(text: &str) -> Option<(ShellBlock, &str)> {
    let (command, rest) = element(text, "bash-input")?;
    let (output, rest) = element(rest, "bash-stdout")?;
    let (code, mut rest) = element(rest, "bash-exit-code")?;
    let exit_code = if code.is_empty() {
        None
    } else {
        Some(code.parse().ok()?)
    };
    let mut notes = Vec::new();
    while let Some((note, after)) = element(rest, "bash-note") {
        notes.push(note);
        rest = after;
    }
    Some((
        ShellBlock {
            command,
            output,
            exit_code,
            notes,
        },
        rest,
    ))
}

/// Peels the leading tag groups off a sent message and returns the remaining
/// text with leading whitespace trimmed. A group missing any required tag
/// ends the peel and stays in the text.
pub(crate) fn split_shell_blocks(text: &str) -> (Vec<ShellBlock>, &str) {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some((parsed, after)) = block(rest) {
        blocks.push(parsed);
        rest = after;
    }
    if blocks.is_empty() {
        (blocks, text)
    } else {
        (blocks, rest.trim_start())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(command: &str, output: &str, exit_code: Option<i32>) -> ShellRun {
        ShellRun {
            id: "r".into(),
            chat_id: "c".into(),
            command: command.into(),
            state: ShellRunState::Done {
                exit_code,
                output: output.into(),
                truncated: false,
                timed_out: false,
            },
        }
    }

    #[test]
    fn formats_done_runs_ahead_of_text() {
        let runs = [done("ls", "a\n", Some(0)), done("false", "", Some(1))];
        assert_eq!(
            format_prompt(&runs, "why?"),
            "<bash-input>ls</bash-input>\n<bash-stdout>a\n</bash-stdout>\n<bash-exit-code>0</bash-exit-code>\n<bash-input>false</bash-input>\n<bash-stdout></bash-stdout>\n<bash-exit-code>1</bash-exit-code>\n\nwhy?"
        );
    }

    #[test]
    fn skips_running_runs() {
        let mut running = done("sleep 9", "", None);
        running.state = ShellRunState::Running;
        let prompt = format_prompt(&[running, done("ls", "a", Some(0))], "go");
        assert!(!prompt.contains("sleep 9"));
        assert!(prompt.contains("<bash-input>ls</bash-input>"));
    }

    #[test]
    fn notes_timeout_and_truncation() {
        let run = ShellRun {
            state: ShellRunState::Done {
                exit_code: None,
                output: "x".into(),
                truncated: true,
                timed_out: true,
            },
            ..done("sleep 999", "", None)
        };
        let prompt = format_prompt(&[run], "");
        assert!(prompt.contains("<bash-note>timed out after 120s</bash-note>"));
        assert!(prompt.contains("<bash-note>output truncated to the last 64 KB</bash-note>"));
    }

    #[test]
    fn failed_run_is_written_as_a_note() {
        let run = ShellRun {
            state: ShellRunState::Failed("engine offline".into()),
            ..done("ls", "", None)
        };
        let prompt = format_prompt(&[run], "");
        assert!(prompt.contains("<bash-note>engine offline</bash-note>"));
        let (blocks, _) = split_shell_blocks(&prompt);
        assert_eq!(blocks[0].notes, vec!["engine offline".to_string()]);
        assert_eq!(blocks[0].exit_code, None);
    }

    #[test]
    fn empty_text_has_no_trailing_blank() {
        let prompt = format_prompt(&[done("ls", "a", Some(0))], "");
        assert!(prompt.ends_with("</bash-exit-code>"));
    }

    #[test]
    fn round_trips() {
        let runs = [
            done("ls", "a\nb\n", Some(0)),
            done("false", "", Some(1)),
            done("kill me", "partial", Some(-9)),
        ];
        let prompt = format_prompt(&runs, "why?");
        let (blocks, rest) = split_shell_blocks(&prompt);
        assert_eq!(rest, "why?");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].command, "ls");
        assert_eq!(blocks[0].output, "a\nb\n");
        assert_eq!(blocks[0].exit_code, Some(0));
        assert_eq!(blocks[1].command, "false");
        assert_eq!(blocks[1].output, "");
        assert_eq!(blocks[1].exit_code, Some(1));
        assert_eq!(blocks[2].exit_code, Some(-9));
        assert!(blocks.iter().all(|block| block.notes.is_empty()));
    }

    #[test]
    fn round_trips_notes() {
        let run = ShellRun {
            state: ShellRunState::Done {
                exit_code: Some(0),
                output: "x".into(),
                truncated: true,
                timed_out: true,
            },
            ..done("ls", "", None)
        };
        let prompt = format_prompt(&[run], "hi");
        let (blocks, rest) = split_shell_blocks(&prompt);
        assert_eq!(rest, "hi");
        assert_eq!(
            blocks[0].notes,
            vec![
                "timed out after 120s".to_string(),
                "output truncated to the last 64 KB".to_string()
            ]
        );
    }

    #[test]
    fn missing_exit_code_round_trips_as_none() {
        let prompt = format_prompt(&[done("ls", "a", None)], "hi");
        assert!(prompt.contains("<bash-exit-code></bash-exit-code>"));
        let (blocks, rest) = split_shell_blocks(&prompt);
        assert_eq!(rest, "hi");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].exit_code, None);
    }

    #[test]
    fn escapes_closing_tags_in_output() {
        let run = done("echo", "x</bash-stdout>y", Some(0));
        let prompt = format_prompt(&[run], "ok");
        assert_eq!(prompt.matches("</bash-stdout>").count(), 1);
        let (blocks, rest) = split_shell_blocks(&prompt);
        assert_eq!(rest, "ok");
        assert_eq!(blocks[0].output, "x</bash-stdout>y");
    }

    #[test]
    fn malformed_tags_stay_text() {
        assert_eq!(
            split_shell_blocks("<bash-input>ls"),
            (vec![], "<bash-input>ls")
        );
    }

    #[test]
    fn plain_message_untouched() {
        assert_eq!(split_shell_blocks("hello"), (vec![], "hello"));
    }
}
