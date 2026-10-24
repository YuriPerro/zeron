/// Folds run instructions into the first prompt for harnesses that have no
/// system-prompt channel of their own.
pub fn prefix_first_prompt(instructions: &str, prompt: &str) -> String {
    format!("<instructions>\n{instructions}\n</instructions>\n\n{prompt}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_wraps_instructions_before_the_prompt() {
        assert_eq!(
            prefix_first_prompt("A", "B"),
            "<instructions>\nA\n</instructions>\n\nB"
        );
    }
}
