//! Command output cut down for display: each line capped in chars, and a
//! long output reduced to its tail.

use gpui::SharedString;

/// Chars kept per output line. Lines clip rather than wrap, but each one is
/// still shaped whole for selection — a minified megabyte must not be.
pub(crate) const OUTPUT_LINE_MAX_CHARS: usize = 8 * 1024;

/// One verbatim output line, cut to [`OUTPUT_LINE_MAX_CHARS`] with an ellipsis.
pub(crate) fn output_line(line: &str) -> SharedString {
    match line.char_indices().nth(OUTPUT_LINE_MAX_CHARS) {
        Some((cut, _)) => SharedString::from(format!("{}…", &line[..cut])),
        None => SharedString::from(line.to_owned()),
    }
}

/// The last `max_lines` lines of `output`, trailing newlines dropped, and how
/// many lines came before them.
pub(crate) fn output_tail(output: &str, max_lines: usize) -> (Vec<SharedString>, usize) {
    let output = output.trim_end_matches(['\n', '\r']);
    if output.is_empty() {
        return (Vec::new(), 0);
    }
    let mut tail: Vec<SharedString> = output
        .lines()
        .rev()
        .take(max_lines)
        .map(output_line)
        .collect();
    tail.reverse();
    let skipped = output.lines().count() - tail.len();
    (tail, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_line_is_cut_on_a_char_boundary() {
        let line = output_line(&"é".repeat(OUTPUT_LINE_MAX_CHARS + 10));
        assert_eq!(line.chars().count(), OUTPUT_LINE_MAX_CHARS + 1);
        assert!(line.ends_with("é…"));
    }

    #[test]
    fn tail_keeps_the_last_lines_and_counts_the_rest() {
        let output: String = (0..30).map(|ix| format!("line {ix}\n")).collect();
        let (lines, skipped) = output_tail(&output, 24);
        assert_eq!(lines.len(), 24);
        assert_eq!(skipped, 6);
        assert_eq!(lines[0].as_ref(), "line 6");
        assert_eq!(lines[23].as_ref(), "line 29");
    }

    #[test]
    fn tail_of_short_or_empty_output() {
        assert_eq!(
            output_tail("a\nb\n\n", 24),
            (vec!["a".into(), "b".into()], 0)
        );
        assert_eq!(output_tail("\n\n", 24), (vec![], 0));
    }

    #[test]
    fn tail_caps_each_line() {
        let output = format!("{}\nshort", "x".repeat(OUTPUT_LINE_MAX_CHARS * 2));
        let (lines, _) = output_tail(&output, 24);
        assert_eq!(lines[0].chars().count(), OUTPUT_LINE_MAX_CHARS + 1);
        assert_eq!(lines[1].as_ref(), "short");
    }
}
