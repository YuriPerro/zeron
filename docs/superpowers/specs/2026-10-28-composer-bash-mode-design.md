# Composer bash mode: `!` runs a command and stages its output

Fork-only (`zeron-fork`). Engine, RPC and UI change together. Every device the
UI talks to must run this fork.

## Problem

To show the agent a command's output, the user has two options today. They can
open a terminal, copy the output and paste it into the composer, or they can ask
the agent to run the command, which spends a turn and tokens. Claude Code's `!`
prefix covers this. Zeron has no equivalent.

## Goals

1. Typing `!` as the first character of an empty draft switches the composer
   into bash mode:
   - the `!` is consumed
   - a `bash` chip shows at the right of the input
   - the placeholder reads "Run a command"
2. Enter in bash mode runs the command on the chat's host device, in the chat's
   working directory, and clears the input. The composer stays in bash mode.
3. Each run appears above the composer as a staged shell block:
   - it shows `$ command` and a spinner while running
   - it shows the exit code and the output once done
   - ✕ removes the block and, while the command runs, cancels it
4. The next message carries every staged block, ahead of the user's text, as
   `<bash-input>` / `<bash-stdout>` / `<bash-exit-code>` tags. Then the blocks
   clear. Running a command never starts a turn.
5. The transcript renders those tags inside a user message as collapsed shell
   blocks, `$ command · exit N`. Expanding a block shows the output.
6. Backspace on an empty input, or Esc, leaves bash mode.

Non-goals:
- Interactive commands such as `vim` or password prompts. They hit the timeout.
  The terminal covers them.
- Streaming output while a command runs.
- Bash mode in side chats or on devices without this fork.
- Persisting staged blocks across app restarts. They live with the composer
  draft state for the session only.

## Design

```
composer: "!" in empty draft ─▶ BashMode on
Enter ─▶ RunShellCommand{runId, chatId, command, targetDeviceId}
            ─▶ host engine: cwd = resolve_open_terminal_cwd(None, chat.cwd, space path, scratch)
               $SHELL -lc '<command>' 2>&1, own process group, login-shell env
               timeout 120s · output capped at 64 KB (tail kept, head dropped)
         ◀─ {exitCode, output, truncated, timedOut}
✕ while running ─▶ CancelShellCommand{runId} ─▶ kill the process group
send ─▶ prompt = blocks.map(tags).join("\n") + "\n\n" + text
transcript ─▶ user_markdown splits the leading tag blocks into shell rows
```

### RPC (`crates/rpc/src/lib.rs`, `crates/engine/src/rpc.rs`)

- `RUN_SHELL_COMMAND = "RunShellCommand"`:
  - params `{runId, chatId, command}`
  - returns `{exitCode: i32 | null, output: String, truncated: bool, timedOut: bool}`
  - forwardable, like the terminal methods, because it runs on the chat's host
  - the cwd comes from the same resolver as `OpenTerminal`, so a new-chat
    canvas (`space-canvas:{id}`) runs in its project folder
- `CANCEL_SHELL_COMMAND = "CancelShellCommand"`:
  - params `{runId}`, returns `{}`
  - forwardable
  - an unknown or finished `runId` is a no-op
- The engine keeps the running children in a map keyed by `runId`:
  - it removes an entry on exit
  - cancel sends SIGTERM to the group, then SIGKILL after 2s
  - a timeout does the same and returns `timedOut: true` with the output so far
- Output: stdout and stderr go into one pipe, in order, decoded lossily as UTF-8.
  Past 64 KB the engine keeps the last 64 KB and sets `truncated`.
- The shell is the user's login shell (`zeron_harness::shell_env::login_shell_path`)
  with the same environment the terminals get.

### UI

- Composer:
  - `bash_mode: bool` and `shell_runs: Vec<ShellRun>` per chat, kept next to
    the draft the same way `TodoPanelState` is kept.
  - `ShellRun { id, command, state: Running | Done { exit_code, output, truncated, timed_out } }`.
  - In bash mode the input shows:
    - a `bash` chip on the right, accent-tinted like the screenshot
    - the input border tinted with the same accent
    - monospace text
  - Enter on an empty bash input does nothing. Shift+Enter is ignored, so input
    stays single line.
  - The staged blocks render in the tray stack, under the queue, as a compact
    list:
    - each row shows `$ command`, then a spinner or `exit N`, then ✕
    - a row expands to show the last 20 output lines
  - When the user sends, `Running` blocks stay staged. Only `Done` blocks go
    out with the message.
  - A send with an empty text but with blocks is allowed.
- Prompt format, one block per run:
  ```
  <bash-input>git status</bash-input>
  <bash-stdout>…output…</bash-stdout>
  <bash-exit-code>0</bash-exit-code>
  ```
  - Timed-out runs add `<bash-note>timed out after 120s</bash-note>`.
  - Truncated runs add `<bash-note>output truncated to the last 64 KB</bash-note>`.
- Transcript:
  - `user_markdown` (`crates/ui/src/transcript.rs`) peels leading
    `<bash-input>…` groups off a user message.
  - Each group renders as a collapsed row: monospace `$ command`, then
    `exit N` (danger tint when non-zero).
  - Clicking a row toggles the output.
  - The rest of the message renders as today.
  - Tags that don't parse fall back to plain text.
- Copy (English, product): "bash", "Run a command", "exit {code}",
  "timed out", "Couldn't run the command: {error}".

## Error handling

- The RPC fails (host offline, no cwd): the block shows the error as its output
  with no exit code, and a toast shows "Couldn't run the command: {error}". The
  block stays so the user can remove it.
- The user switches chats while a command runs: the run keeps going, and its
  result lands in that chat's staged blocks.
- The app quits while a command runs: the engine kills the group when the
  engine stops.

## Testing

- Engine:
  - `RunShellCommand` returns output, the exit code and the cwd (`pwd`).
  - Output is capped and keeps the tail.
  - Timeout kills a `sleep`.
  - `CancelShellCommand` ends a running `sleep` early.
  - An unknown `runId` cancel is a no-op.
- UI:
  - `!` on an empty draft enters bash mode. `!` mid-text does not.
  - Backspace on empty and Esc leave bash mode.
  - Enter dispatches `RunShellCommand` with the chat's `targetDeviceId`.
  - Send builds the tagged prompt from the `Done` blocks only, and clears them.
  - `user_markdown` splits tag groups and leaves malformed tags as text.
