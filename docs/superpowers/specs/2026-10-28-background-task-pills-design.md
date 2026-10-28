# Running work pills: subagents and background tasks above the composer

Fork-only (`zeron-fork`). Harness, engine, proto, RPC and UI change together;
every device the UI talks to must run this fork.

## Problem

A chat's running subagents only show in the lanes card under the session card
(`crates/ui/src/shell/session_lanes.rs`) and as the sidebar's "● N" pill. With
the session card closed, nothing near the composer says that work is still
running.

Background tasks (a `Bash` with `run_in_background`, a `Monitor` watching CI)
show nowhere. The Claude CLI reports them: `background_tasks_changed` carries
the full live set (`task_id`, `task_type`, `description`) on every change. The
harness reads only its length into `TurnControl::set_background`
(`crates/harness/src/claude/mod.rs`), which keeps the runtime from being
reaped. Nothing reaches proto or the UI, and the user cannot stop one task
without stopping the whole runtime.

## Goals

1. Two pills sit above the composer, in the same tray stack as the queue and
   the todo panel: `N subagents` and `N background`. Each pill shows only
   while its count is above zero.
2. Clicking a pill opens its list as a tray that slides out from behind the
   composer (variant A of the mockup). Only one tray is open at a time. A
   second click closes it.
3. The subagents tray lists the running subagents. A row opens that subagent
   in the right pane, like a lanes card row does.
4. The background tray lists background tasks with their description and
   elapsed time. Each row ends with a red stop button that stops that one
   task.
5. The lanes card gets a "Background" lane with the same rows.
6. New loaders:
   - `echo_loader` leads the subagents pill and its rows.
   - `halo_loader` leads the background pill and its rows.

Non-goals:
- Background tasks for harnesses other than Claude. No other driver reports
  them, so their background pill never shows.
- Showing subagents in the background pill. `local_agent` tasks are already
  counted as subagents.
- Background task output or logs.
- Stopping subagents from the tray.

## Design

```
claude CLI ── system/background_tasks_changed {tasks:[{task_id, task_type, description}]}
   ▼
harness/claude/mod.rs
   ├─ turn.set_background(len)                (unchanged: reap guard)
   └─ AgentEvent::BackgroundTasks { tasks: Vec<BackgroundTask> }
   ▼
engine/sessions.rs: set_background_tasks(chat_id, tasks)
   │  keeps first-seen started_at per task id, drops local_agent
   ▼
Session.background_tasks: Vec<BackgroundTask>   (rides the session row,
   │                                             like running_subagents)
   ▼
ui/state.rs: background_tasks_for(chat_id, now)
   ├─ composer: running pills + trays
   └─ shell/session_lanes.rs: "Background" lane

stop:  UI ─RPC StopBackgroundTask{chatId, taskId}─▶ engine/rpc.rs
         ─▶ Sessions::stop_background_task ─▶ TurnControl::stop_task(id)
         ─▶ claude driver writes control_request {subtype:"stop_task", task_id}
```

### Proto

- `zeron_proto::BackgroundTask { id: String, kind: BackgroundTaskKind,
  description: String, started_at: DateTime<Utc> }`.
- `BackgroundTaskKind`:
  - `Shell` covers `local_bash`.
  - `Other(String)` covers any other non-agent `task_type`. This includes the
    Monitor tool's type, which no fixture shows yet.
- `AgentEvent::BackgroundTasks { tasks: Vec<BackgroundTask> }`. It always
  carries the full set, never a delta.
- `Session.background_tasks: Vec<BackgroundTask>`, with `#[serde(default)]` so
  older peers keep parsing it.
- `view::background_tasks(session, now)` returns the set only while the session
  row is fresh. It uses the same staleness window as `view::running_subagents`.

### Harness (Claude)

- `background_tasks_changed` keeps calling `set_background(len)`.
- It also emits `AgentEvent::BackgroundTasks`, decoded from `tasks` values:
  - Entries with `task_type == "local_agent"` are dropped.
  - An entry without `task_id` is skipped.
  - A missing `description` becomes an empty string.
- The driver never sets `started_at`. The engine owns it.
- Stopping a task:
  - `TurnControl` gets a host-to-driver queue for task stops: `stop_task(id)`
    on the host side, `stop_task_requested()` on the driver side.
  - The driver writes `{"type":"control_request","request_id":…,"request":{"subtype":"stop_task","task_id":…}}`.
    This shape was checked against the installed CLI binary (the SDK's
    `stopTask`).
  - A failed response (`stop_task_refused` or any error) is logged and sent
    back to the RPC caller as an error.

### Engine

- `set_background_tasks(chat_id, tasks)` mirrors `set_running_subagents`:
  - An unchanged set is a no-op.
  - A change updates `updated_at`, publishes `sessions_tx` and records the
    session.
  - It keeps a task's first-seen `started_at` across updates, keyed by id.
- When the runtime ends, the set clears, just as `set_running_subagents(…, 0)`
  does today.
- RPC `StopBackgroundTask { chatId, taskId }`:
  - It returns an error when the chat has no live runtime.
  - It returns an error when the task id is not in the current set.
  - Otherwise it resolves once the CLI acks the control request.
  - The row leaves the list only when the next `background_tasks_changed` drops
    the task. The RPC reply alone does not remove it.

### UI

- Shared rows:
  - `subagent_rows` and `SubagentRow` move out of `shell/session_lanes.rs` into
    a shared module (`crate::running_work`).
  - The lanes card and the composer tray both read from there. This is the
    second consumer, per the no-duplication rule.
- Composer:
  - New file `crates/ui/src/running_trays.rs`, rendered in the tray stack next
    to `queue.rs` and `todo_panel.rs`.
  - Pill row: up to two pills, left-aligned, 24px tall. Each is loader + count
    + label + chevron.
  - Tray: height and opacity animate on `motion::COLLAPSE`, and the chevron
    turns on `motion::CHEVRON`.
  - Open state is per chat and follows the selected chat, like
    `TodoPanelState`.
- Subagent row:
  - Shows the echo loader, the title, then the model and elapsed time.
  - A click opens the subagent in the right pane through the same path as
    `render_subagent_row`.
- Background row:
  - Shows the halo loader, the description (or the task id when the
    description is empty), then the elapsed time and the stop button.
  - Stop button: 20px, red wash (`theme.danger` at 0.14, 0.30 on hover) with a
    7px red square.
  - After a click, the row shows "Stopping…" and ignores more clicks.
  - On RPC error, the row returns to its normal state and the error shows as a
    toast.
- Lanes card: a "Background" section appears after Subagents when the set is
  non-empty. Its rows match the tray rows.
- Copy (English, product): "subagents", "background", "Stopping…", the stop
  tooltip "Stop task", and the toast "Couldn't stop the task: {error}".

### Loaders

- Both loaders go in `crates/ui/src/loaders.rs`, with their math in
  `crates/proto/src/motion.rs` as pure functions of phase.
- Both run on the shared pulse clock and draw a 5×5 dot grid.
- Under reduced motion, both rest on a static frame.
- `echo_loader`:
  - 1.5s cycle.
  - Ring = Manhattan distance from the center (0..4).
  - Each dot plays keyframes 0% 0.10, 28% 0.98, 56% 0.32, 78% 0.7824 and
    100% 0.10, eased with cubic-bezier(0.42, 0, 0.58, 1).
  - Each dot is delayed by `(ring × 0.14 + (ring mod 2) × 0.03)` of the cycle.
- `halo_loader`:
  - 5×5 grid with the corners hidden. The ring path is the 12 outer cells,
    clockwise from (0,1).
  - 1.65s cycle in 24 steps. The lead is `floor(step / 24 × 12)`.
  - A second comet runs 6 cells ahead at 0.72 strength.
  - Tail weights are 1, 0.78, 0.56, 0.36, 0.22.
  - Off-ring cells sit at 0.08, and the core at 0.16.
- Tint is `theme.busy` for both.

## Error handling

- When the CLI drops the task before the stop lands, the RPC errors with "not
  running". The row is already gone on the next update, so no toast shows.
- A stale session row (device offline) hides both pills, through the
  `view::` staleness window.
- An unknown `task_type` falls into `Other` and shows as a background task.

## Testing

- Harness:
  - Normalize `background_tasks_changed` from the live fixtures into
    `AgentEvent::BackgroundTasks`, checking that `local_agent` entries are
    dropped and `local_bash` is kept.
  - The `fake-claude.sh` stop flow writes the `stop_task` control request.
- Engine:
  - `set_background_tasks` keeps `started_at` across updates.
  - It is a no-op on an unchanged set and clears when the runtime ends.
  - The `StopBackgroundTask` RPC errors for an unknown chat or task.
- Proto: `view::background_tasks` staleness; the echo and halo math (keyframe
  sampling, comet lead and tail).
- UI:
  - The pills render only above zero, and one tray is open at a time.
  - The tray state follows the selected chat.
  - The "Background" lane appears only when non-empty.
  - The stop button dispatches the RPC and shows "Stopping…".

## Credits

`echo_loader` and `halo_loader` port the frame logic of dotmatrix's "Echo
Ring" (`dotm-square-11`) and "Plasma Veil" (`dotm-circular-3`)
(github.com/zzzzshawn/matrix). Their license allows use inside a product and
forbids redistributing the components as a standalone or reusable collection.
They stay internal to the app.
