# Composer Bash Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Typing `!` on an empty composer enters bash mode. Enter runs the command on the chat's host and stages its output above the composer. The next message carries the staged outputs as tagged blocks, and the transcript renders those blocks as collapsed shell rows.

**Architecture:**
- **Engine:** a new `ShellRuns` service runs one-shot login-shell commands in its own process groups. It sits behind two forwardable RPCs.
- **UI, pure logic:** a new `shell_blocks` module formats the tagged prompt and parses it back.
- **UI, composer:** owns bash mode, the per-chat staged runs, and the tray.
- **UI, transcript:** peels shell blocks off user messages.

**Tech Stack:** Rust, tokio (`process`, `time`), gpui, serde.

**Spec:** `docs/superpowers/specs/2026-10-28-composer-bash-mode-design.md`

## Global Constraints

- Timeout: 120 s. Output cap: 65_536 bytes, keeping the tail. On cancel or timeout, SIGTERM goes to the process group, then SIGKILL after 2 s.
- The shell is the user's login shell (`$SHELL -lc '<command>'`), with stdout and stderr merged in order and decoded lossily as UTF-8.
- The cwd comes from `resolve_open_terminal_cwd(None, chat.cwd, space path, scratch_root)`, the same resolver `OpenTerminal` uses.
- RPCs, both forwardable by `targetDeviceId`:
  - `RunShellCommand {runId, chatId, command}` → `{exitCode: i32|null, output, truncated, timedOut}`
  - `CancelShellCommand {runId}` → `{}`
- Prompt tags, one group per run, groups joined by `\n`, then a blank line before the user's text:
  - `<bash-input>…</bash-input>`
  - `<bash-stdout>…</bash-stdout>`
  - `<bash-exit-code>N</bash-exit-code>`
  - `<bash-note>…</bash-note>`, optional
- Notes: `timed out after 120s` and `output truncated to the last 64 KB`.
- Copy: `bash`, `Run a command`, `exit {code}`, `timed out`, `Couldn't run the command: {error}`.
- Bash mode applies only to the main chat composer, not to side chats.
- Commits are in English, with no trailers. Match the surrounding comment density, with no new inline `//` where the file has none.

## Review Focus

1. **Binary or non-UTF-8 output** (`cat` of an image). Expected: lossy text, no panic, the cap still applies. Covered by a test in Task 1.
2. **A command that leaves a background child holding the pipe** (`sleep 300 & echo hi`). Expected: the run returns `hi` promptly when the shell exits, and never hangs until the child dies. Covered by a test in Task 1.
3. **Output that contains a literal `</bash-stdout>`.** Expected: the prompt escapes it, and the transcript round-trips the block intact. Covered by a test in Task 3.
4. **Switching chats while a command runs.** Expected: the result lands in the chat that started it, not the selected one. Covered by a test in Task 5.
5. **A draft that starts with `!` because of paste or a restored draft.** Expected: no bash mode. Only typing `!` into an empty draft enters it. Covered by a test in Task 4.

---

### Task 1: Engine `ShellRuns` service

**Files:**
- Create: `crates/engine/src/shell_runs.rs`
- Modify: `crates/engine/src/lib.rs` (add `mod shell_runs;`)

**Interfaces:**
- Produces:
  - `pub(crate) struct ShellRuns`, which is `Clone + Default` and holds an `Arc<Mutex<HashMap<String, RunHandle>>>`.
  - `pub(crate) async fn run(&self, run_id: String, cwd: &Path, command: &str) -> ShellRunResult`
  - `pub(crate) fn cancel(&self, run_id: &str)`
  - `#[derive(Serialize)] #[serde(rename_all = "camelCase")] pub(crate) struct ShellRunResult { exit_code: Option<i32>, output: String, truncated: bool, timed_out: bool }`
  - The constants `SHELL_RUN_TIMEOUT: Duration = 120s`, `SHELL_OUTPUT_CAP: usize = 65_536` and `SHELL_KILL_GRACE: Duration = 2s`, plus `#[doc(hidden)]` test overrides (`with_limits(timeout, cap)`) so the tests don't wait 120 s.

- [ ] **Step 1: Write the failing tests** in `shell_runs.rs` `#[cfg(test)] mod tests`, all `#[tokio::test]` on Unix:
  - `runs_in_cwd_and_reports_exit`: `run("a", tmp, "pwd; exit 3")` gives an `output` that contains the tmp path and `exit_code == Some(3)`.
  - `merges_stderr_in_order`: `"echo one; echo two >&2; echo three"` gives `output == "one\ntwo\nthree\n"`.
  - `caps_output_keeping_the_tail`: with `with_limits(_, 16)`, running `"printf 'aaaaaaaaaaaaaaaaaaaaZ'"` gives `truncated` true and an `output` that ends with `Z` and has length 16.
  - `non_utf8_output_is_lossy`: `"printf '\\377\\376ok'"` gives an `output` that ends with `ok` and does not panic.
  - `timeout_kills_the_group`: with `with_limits(300ms, _)`, running `"sleep 30"` gives `timed_out` true and `exit_code == None` within 3 s.
  - `cancel_ends_a_running_command`: spawn `run("c", tmp, "sleep 30")` and call `cancel("c")` after 200 ms. The run resolves within 3 s with `exit_code == None`.
  - `background_child_does_not_hold_the_run`: `"sleep 300 & echo hi"` resolves within 3 s with `output == "hi\n"`.
  - `cancel_unknown_is_noop`: `cancel("nope")` does not panic.

- [ ] **Step 2: Run them.** Command: `cargo test -p zeron-engine --lib -- shell_runs`. Expected: compile failure, because the module doesn't exist yet.

- [ ] **Step 3: Implement** in `shell_runs.rs`:
  - Spawn the shell with `tokio::process::Command::new(shell).arg("-lc").arg(command)`.
    - The shell is `$SHELL` or `/bin/sh`. Promote `selected_shell()` in `crates/engine/src/terminals.rs` to `pub(crate)` and reuse it instead of copying it.
    - Set `.current_dir(cwd)`, `.process_group(0)`, `.stdin(null)` and `.kill_on_drop(true)`.
    - Set `PATH` from `zeron_harness::shell_env::login_shell_path()` when it is `Some`.
  - Merge the output streams by giving stdout and stderr the same pipe: create an `os_pipe`-style pair with `nix::unistd::pipe` or `std::io::pipe`, whichever the workspace already depends on, and pass the write end to both. Read it with a bounded tail buffer (`VecDeque<u8>` capped at `cap`).
  - Wait on the child exit, the timeout or a cancel `oneshot`, whichever comes first.
  - When the shell exits, read whatever the pipe holds for at most 100 ms more, then stop. A background child keeps the write end open, so never wait for EOF.
  - On timeout or cancel, `killpg(SIGTERM)`, wait `SHELL_KILL_GRACE`, then `killpg(SIGKILL)`.
  - Remove the map entry on every exit path.
  - On Windows, cancelling and the group kill fall back to `child.kill()`. The tests are `#[cfg(unix)]`.

- [ ] **Step 4: Run the tests.** Command: `cargo test -p zeron-engine --lib -- shell_runs`. Expected: 8 passed.

- [ ] **Step 5: Commit.** Message: `feat(engine): one-shot shell runs with a cap, timeout and cancel`.

### Task 2: RPCs `RunShellCommand` / `CancelShellCommand`

**Files:**
- Modify: `crates/rpc/src/lib.rs`. Add the constants next to `CLOSE_TERMINAL`, add the params structs, and add both methods to `forwardable`.
- Modify: `crates/engine/src/rpc.rs`:
  - add a `shell_runs: ShellRuns` field on `EngineRpc`
  - add the dispatch arms next to `OPEN_TERMINAL` (~4510)
  - add both methods to the host-routed list next to the terminals (~2226)

**Interfaces:**
- Consumes: `ShellRuns::run` and `ShellRuns::cancel` from Task 1.
- Produces:
  - `methods::RUN_SHELL_COMMAND = "RunShellCommand"` and `methods::CANCEL_SHELL_COMMAND = "CancelShellCommand"`.
  - `RunShellCommandParams { run_id: String, chat_id: String, command: String }` and `CancelShellCommandParams { run_id: String }`, both camelCase on the wire.

- [ ] **Step 1: Write the failing tests** in the `rpc.rs` test module, following the existing handler-test pattern there:
  - `run_shell_command_runs_in_the_chat_cwd`: seed a chat whose `cwd` is a tmp dir and call `RunShellCommand {command: "pwd"}`. The reply `output` contains the tmp dir and `exitCode == 0`.
  - `run_shell_command_on_a_canvas_uses_the_space_path`: use `chatId = "space-canvas:{id}"` with a space whose path is a tmp dir. `pwd` prints that path.
  - `cancel_shell_command_unknown_run_is_ok`: the reply is `{}`.
  - `shell_methods_are_forwardable`: `forwardable(methods::RUN_SHELL_COMMAND)` and `forwardable(methods::CANCEL_SHELL_COMMAND)` are both true. Put this one in the rpc crate tests.

- [ ] **Step 2: Run them.** Command: `cargo test -p zeron-engine --lib -- run_shell_command cancel_shell_command && cargo test -p zeron-rpc -- shell_methods`. Expected: FAIL.

- [ ] **Step 3: Implement.**
  - The `RunShellCommand` arm resolves the cwd exactly as `OPEN_TERMINAL` does (chat cwd, then the `canvas_space_id` space path, then scratch). Pull that lookup into a shared `fn chat_cwd_for(&self, chat_id) -> Result<String, RpcError>` and make `OPEN_TERMINAL` call it too. Don't copy the block.
  - Then `RpcReply::value(&self.shell_runs.run(..).await)`.
  - The `CancelShellCommand` arm calls `cancel` and returns `{}`.

- [ ] **Step 4: Run the tests.** Same command as Step 2. Expected: PASS.

- [ ] **Step 5: Commit.** Message: `feat(engine): RunShellCommand and CancelShellCommand rpcs`.

### Task 3: `shell_blocks`: prompt format and parse

**Files:**
- Create: `crates/ui/src/shell_blocks.rs`
- Modify: `crates/ui/src/lib.rs` (add `mod shell_blocks;`)

**Interfaces:**
- Produces:
  - `pub(crate) struct ShellRun { id: String, chat_id: String, command: String, state: ShellRunState }`
  - `pub(crate) enum ShellRunState { Running, Done { exit_code: Option<i32>, output: String, truncated: bool, timed_out: bool }, Failed(String) }`
  - `pub(crate) fn format_prompt(runs: &[ShellRun], text: &str) -> String`. It takes only `Done` runs, and writes `Failed` runs as output plus `<bash-note>{error}</bash-note>`. It never writes `Running` runs.
  - `pub(crate) struct ShellBlock { command: String, output: String, exit_code: Option<i32>, notes: Vec<String> }`
  - `pub(crate) fn split_shell_blocks(text: &str) -> (Vec<ShellBlock>, &str)`. It peels the leading tag groups and returns the rest of the message with leading whitespace trimmed.

- [ ] **Step 1: Write the failing tests** in `shell_blocks.rs`:
  - `formats_done_runs_ahead_of_text`: two Done runs and the text `"why?"` give exactly:
    `"<bash-input>ls</bash-input>\n<bash-stdout>a\n</bash-stdout>\n<bash-exit-code>0</bash-exit-code>\n<bash-input>false</bash-input>\n<bash-stdout></bash-stdout>\n<bash-exit-code>1</bash-exit-code>\n\nwhy?"`
  - `skips_running_runs`: a Running run is absent from the output.
  - `notes_timeout_and_truncation`: a Done run with `timed_out` and `truncated` set carries `<bash-note>timed out after 120s</bash-note>` and `<bash-note>output truncated to the last 64 KB</bash-note>`.
  - `empty_text_has_no_trailing_blank`: one run with the text `""` ends with `</bash-exit-code>`.
  - `round_trips`: `split_shell_blocks(&format_prompt(runs, "why?"))` returns the same commands, outputs and exit codes, with the rest `"why?"`.
  - `escapes_closing_tags_in_output`: the output `"x</bash-stdout>y"` round-trips intact.
  - `malformed_tags_stay_text`: `"<bash-input>ls"` returns `(vec![], "<bash-input>ls")`.
  - `plain_message_untouched`: `"hello"` returns `(vec![], "hello")`.

- [ ] **Step 2: Run them.** Command: `cargo test -p zeron-ui --lib -- shell_blocks`. Expected: compile failure.

- [ ] **Step 3: Implement.** In output only, escape `</bash-` as `<\/bash-`, and unescape when parsing. Parse with plain `strip_prefix` / `find`, no regex. A group is valid only when `bash-input`, `bash-stdout` and `bash-exit-code` all close, and the notes are optional.

- [ ] **Step 4: Run the tests.** Expected: 8 passed.

- [ ] **Step 5: Commit.** Message: `feat(ui): shell block prompt format and parser`.

### Task 4: Composer bash mode and running commands

**Files:**
- Modify: `crates/ui/src/composer.rs`:
  - the `ComposerInput` events (around `submit` ~3817) and the key handling
  - the `Composer` state next to `todo_panels` (~6411)
  - the placeholder (`set_placeholder`, ~10261)
  - the `Submitted` handling that calls `send` (~9248)
- Create: `crates/ui/src/shell_tray.rs` (state plus the run and cancel calls; rendering lands in Task 5)

**Interfaces:**
- Consumes: Task 2's methods and params, and Task 3's `ShellRun` / `ShellRunState`.
- Produces:
  - `pub(crate) struct ShellTrayState { bash_mode: bool, runs: Vec<ShellRun> }`, kept per chat as `shell_trays: HashMap<String, ShellTrayState>`. The key is the selected chat id, or the canvas id for the new-chat canvas.
  - `fn run_shell_command(&mut self, command: String, cx)`
  - `fn cancel_shell_run(&mut self, chat_id: &str, run_id: &str, cx)`
  - `fn enter_bash_mode(&mut self, cx)` and `fn leave_bash_mode(&mut self, cx)`

- [ ] **Step 1: Write the failing `#[gpui::test]` tests** in `shell_tray.rs`. Use the composer window helper pattern from `todo_panel.rs` tests and the scripted fake engine from `pull_request_test_support.rs`.
  - `bang_on_empty_draft_enters_bash_mode`: typing `!` into an empty input sets `bash_mode` true, leaves the input text `""`, and sets the placeholder to `"Run a command"`.
  - `bang_mid_text_is_text`: typing `a!` sets nothing; the text is `"a!"`.
  - `pasted_bang_does_not_enter`: pasting `"!ls"` into an empty draft leaves `bash_mode` false and the text `"!ls"`.
  - `backspace_on_empty_and_escape_leave`: either key in bash mode with an empty input sets `bash_mode` false and restores the placeholder to `"Do anything…"`.
  - `enter_runs_on_the_host`: in bash mode, typing `ls` and pressing Enter makes one RPC `RunShellCommand` with `{chatId, command:"ls", targetDeviceId: <chat's device>}`. The input clears, `bash_mode` stays true, and a `Running` run is staged.
  - `enter_on_empty_bash_input_does_nothing`: no RPC goes out.
  - `reply_marks_the_run_done`: the fake replies `{exitCode:0, output:"a\n", truncated:false, timedOut:false}`, and the run becomes `Done`.
  - `rpc_error_marks_failed_and_toasts`: a failed run becomes `Failed(err)`, and `toast::messages` contains `"Couldn't run the command: {err}"`.
  - `cancel_sends_cancel_and_removes`: cancelling a running run sends `CancelShellCommand {runId}` and removes the run.
  - `side_chat_has_no_bash_mode`: typing `!` in a side chat composer is plain text.

- [ ] **Step 2: Run them.** Command: `cargo test -p zeron-ui --lib -- shell_tray`. Expected: FAIL.

- [ ] **Step 3: Implement.**
  - Detecting `!`: hook the input's text-change event. When the previous text was empty and the change is the single typed character `!`, consume it. A paste arrives as `PastedText` and never triggers this.
  - Bash mode shows a `bash` chip on the right of the input row, uses monospace input text, and tints the border. Use the theme's accent color, the same family as the existing mode chips.
  - In bash mode, Enter routes to `run_shell_command` instead of `send`.
  - The RPC call follows `queue_rpc`'s targeting pattern (`queue.rs` ~1919).
  - When the reply arrives, update the run by the `chat_id` captured when it started, not by the current selection.

- [ ] **Step 4: Run the tests.** Command: `cargo test -p zeron-ui --lib -- shell_tray composer`. Expected: PASS, with the existing composer tests still green.

- [ ] **Step 5: Commit.** Message: `feat(ui): bash mode in the composer runs commands on the chat's host`.

### Task 5: Staged shell tray and send integration

**Files:**
- Modify: `crates/ui/src/shell_tray.rs` (render), and `crates/ui/src/composer.rs` (the tray stack next to `render_queue_panel` ~11545, plus `send`)

**Interfaces:**
- Consumes: Task 4's `ShellTrayState`, and Task 3's `format_prompt`.
- Produces: `fn render_shell_tray(&mut self, window, cx) -> Option<AnyElement>`.

- [ ] **Step 1: Write the failing tests.**
  - `send_prefixes_done_runs_and_clears_them`: one Done run plus one Running run, then send `"why?"`. The text that reaches `send` is `format_prompt(&[done], "why?")`, and only the Running run stays staged.
  - `send_with_only_runs_is_allowed`: one Done run and empty text. Send goes out with only the tagged block.
  - `result_lands_in_the_starting_chat`: start a run in chat A, select chat B, then the reply arrives. A's run is Done and B has no runs.
  - `tray_hidden_without_runs`: `render_shell_tray` returns `None` when there are no runs.

- [ ] **Step 2: Run them.** Command: `cargo test -p zeron-ui --lib -- shell_tray`. Expected: FAIL.

- [ ] **Step 3: Implement.**
  - The tray goes in the tray stack beneath the queue panel. Reuse `queue::tray_glass` if it exists on this base; otherwise reuse `queue::queue_panel_surface`.
  - Each row shows monospace `$ command`, then `ring_spinner` while running, or `exit N` (danger tint when non-zero) or `timed out`, then ✕.
  - Clicking a row toggles it open to show the last 20 output lines.
  - In `send`, build the text with `format_prompt` from the chat's Done or Failed runs, then drop those runs. Empty text with at least one run is a valid send.

- [ ] **Step 4: Run the tests.** Expected: PASS.

- [ ] **Step 5: Commit.** Message: `feat(ui): staged shell outputs ride the next message`.

### Task 6: Transcript shell rows

**Files:**
- Modify: `crates/ui/src/transcript.rs` (`user_markdown` ~9784 and the user message render that calls it)

**Interfaces:**
- Consumes: Task 3's `split_shell_blocks` and `ShellBlock`.

- [ ] **Step 1: Write the failing tests.**
  - `user_message_with_shell_blocks_splits`: a user entry whose text is `format_prompt([..], "why?")` renders one shell row per block, and the markdown body is `"why?"`. Assert through the user-markdown helper's return, the same way the existing `user_markdown` tests do.
  - `plain_user_message_unchanged`: `"hello"` gives no shell rows.

- [ ] **Step 2: Run them.** Command: `cargo test -p zeron-ui --lib -- user_message_with_shell plain_user_message`. Expected: FAIL.

- [ ] **Step 3: Implement.**
  - Each row is collapsed by default: monospace `$ command` · `exit N` (danger when non-zero) · notes as faint text.
  - A click toggles the full output in a monospace block. Keep the expansion state in the transcript's existing per-row UI state map, keyed by row id and block index.

- [ ] **Step 4: Run the tests.** Expected: PASS.

- [ ] **Step 5: Commit.** Message: `feat(ui): render shell blocks in user messages`.

### Task 7: Verify in the app

- [ ] **Step 1: Clippy.** Run `cargo clippy --all-targets --keep-going -p zeron-ui -p zeron-engine -p zeron-rpc`. Expected: no warnings on lines this branch added.
- [ ] **Step 2: Rebuild the dev app.** Run `ZERON_DEV_CODESIGN_IDENTITY=- ZERON_DEV_IPC_PORT=49778 bash scripts/run-macos-dev.sh`. Never use the Apple Development identity.
- [ ] **Step 3: Manual check.** In a Claude chat:
  - `!` → `git status` → the block shows `exit 0`.
  - `!` → `sleep 200` → ✕ cancels it.
  - Send "what changed?" → the agent quotes the status, and the transcript shows a collapsed `$ git status · exit 0` row.
