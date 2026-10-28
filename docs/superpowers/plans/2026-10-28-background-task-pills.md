# Running Work Pills Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two pills above the composer (`N subagents`, `N background`) that expand into trays, a "Background" lane in the session card, and a per-task stop for Claude background tasks.

**Architecture:** The Claude driver decodes `background_tasks_changed` into a new `AgentEvent::BackgroundTasks`. The engine keeps the set on the `Session` row, the way `running_subagents` already rides it. The UI reads it per chat. Stopping goes UI → RPC `StopBackgroundTask` → `SessionsEngine` → a new task-stop queue on `TurnControl` → the driver writes the CLI's `stop_task` control request.

**Tech Stack:** Rust workspace: `zeron-proto`, `zeron-harness`, `zeron-engine`, `zeron-rpc`, `zeron-ui` (gpui).

**Spec:** `docs/superpowers/specs/2026-10-28-background-task-pills-design.md`

## Global Constraints

- **Fork scope:** fork-only (`zeron-fork`). Every device runs this fork, so there is no capability gate for the new RPC.
- **Background sources:** only the Claude harness emits `AgentEvent::BackgroundTasks`. `task_type == "local_agent"` is never a background task.
- **Product copy:** English, exactly these strings:
  - `subagents` and `background` (pill labels)
  - `Stopping…` (row state)
  - `Stop task` (tooltip)
  - `Couldn't stop the task: {error}` (toast)
- **Loader tint:** `theme.busy` for both loaders.
- **Stop button:** `theme.danger` at 0.14 opacity, 0.30 on hover. A 20px button with a 7px square.
- **Tray motion:** height and opacity on `motion::COLLAPSE`, chevron on `motion::CHEVRON`.
- **Code style:** match the surrounding comment density and naming. Doc comments on new pub items, like their neighbours.
- **Commits:** English messages, no co-author or tool trailers.

## Review Focus

1. **Stop on a task that already ended.** The CLI refuses or the id is gone. The RPC errors with "not running", and the UI shows no toast once the next update has removed the row. Test in Task 6.
2. **Stop never acked** (a driver that doesn't drain the queue, or a hung CLI). The engine gives up after 10s with an error, and the RPC never hangs. Test in Task 6.
3. **Background tasks outliving the turn.** `background_tasks_changed` arrives while no turn is open. The engine still updates the row (the event is not dropped by the parked/turn gate). Test in Task 5.
4. **Switching chats with a tray open.** The tray state follows the selected chat, and another chat starts closed. Test in Task 9.
5. **A malformed `tasks` entry** (missing `task_id`, `tasks` absent). The entry is skipped, and an absent `tasks` means an empty set rather than "no event". Test in Task 3.

---

### Task 1: Proto types, Session field and view helper

**Files:**
- Modify: `crates/proto/src/entities.rs` (types, `Session` at 390-416)
- Modify: `crates/proto/src/agent.rs` (`AgentEvent`, 809-966; round-trip test near 972)
- Modify: `crates/proto/src/view.rs` (helper after `running_subagents`, 100; tests near 377-402)
- Modify, adding `background_tasks: Vec::new()` to every full `Session { … }` literal:
  - `crates/proto/src/view.rs:378`
  - `crates/engine/src/sessions.rs:2096`
  - `crates/engine/src/session_activity.rs:176,250`
  - `crates/engine/src/orchestration/wake.rs:230`
  - `crates/engine/src/workspace_host.rs:2300`
  - `crates/engine/tests/workspace_sync.rs:740`
  - `crates/doc/src/workspace.rs:817` (raw mirror struct, add the field with `#[serde(default)]`), plus its literals at 831 and 947
  - `crates/doc/src/registry/tests.rs:307`
  - `crates/sync/tests/registry_edge.rs:86,206`
  - `crates/sync/tests/registry_client.rs:123,435`
  - `crates/mcp/src/tools.rs:2317`
  - `crates/client/src/demo/fixtures.rs:475`
  - `crates/client/src/demo/mod.rs:477`
  - `crates/ui/src/composer.rs:13337`
  - `crates/ui/src/shell.rs:15010`
  - `crates/ui/src/transcript.rs:16092`
  - `crates/ui/src/pickers.rs:7650`
  - `crates/ui/src/shell/orchestrators.rs:560`
  - `crates/ui/src/state.rs:5602,5893,5927`
  - `crates/ui/examples/macos-resource-profile.rs:149`

**Interfaces:**
- Produces, in `zeron_proto` (re-exported at the crate root like `Session`):
  - `pub struct BackgroundTask { pub id: String, pub kind: BackgroundTaskKind, pub description: String, pub started_at: DateTime<Utc> }`. Derives `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`, with `#[serde(rename_all = "camelCase")]`.
  - `pub enum BackgroundTaskKind { Shell, Other(String) }`. Same derives.
  - `pub fn BackgroundTaskKind::from_wire(task_type: &str) -> Option<BackgroundTaskKind>`:
    - `"local_agent"` → `None`
    - `"local_bash"` → `Some(Shell)`
    - anything else → `Some(Other(task_type.into()))`
  - `AgentEvent::BackgroundTasks { tasks: Vec<BackgroundTask> }`, with `#[serde(rename_all = "camelCase")]`. Drivers send `started_at` as `DateTime::<Utc>::UNIX_EPOCH`, because the engine owns it.
  - `Session.background_tasks: Vec<BackgroundTask>`, with `#[serde(default, skip_serializing_if = "Vec::is_empty")]`.
  - `pub fn view::background_tasks(session: Option<&Session>, now: DateTime<Utc>) -> &[BackgroundTask]`. Same freshness filter as `running_subagents` (`SESSION_STALE_MS`). Returns `&[]` when the row is stale or missing.

- [ ] **Step 1: Write failing tests**

In `view.rs` tests, `background_tasks_show_only_while_the_session_row_is_fresh`:
- A fresh row (−10s) with one task → `len() == 1`.
- A stale row (`SESSION_STALE_MS + 1`) → empty.
- `None` → empty.

In `agent.rs` tests, `background_tasks_event_round_trips`:
- Serialize `AgentEvent::BackgroundTasks { tasks: vec![shell_task] }`.
- Assert the JSON has `"type":"backgroundTasks"`.
- Assert it deserializes back equal.

In `entities.rs` tests, `background_task_kind_reads_the_wire_type`:
- `from_wire("local_agent") == None`
- `from_wire("local_bash") == Some(Shell)`
- `from_wire("monitor") == Some(Other("monitor".into()))`

Also `session_without_background_tasks_still_parses`: JSON without the field → `background_tasks.is_empty()`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zeron-proto background_task`
Expected: compile errors for the missing types.

- [ ] **Step 3: Implement the types, the field and the helper. Add the field to every literal listed above.**

- [ ] **Step 4: Verify**

Run: `cargo test -p zeron-proto && cargo check --workspace --all-targets`
Expected: proto tests PASS. The workspace builds with no missing-field errors.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(proto): background task set on the session row"
```

### Task 2: Echo and halo loader math

**Files:**
- Modify: `crates/proto/src/motion.rs` (constants and fns after `mark_phase`, tests in the existing `mod tests`)

**Interfaces:**
- Produces:
  - `pub const ECHO_CYCLE_MS: u64 = 1_500;`
  - `pub fn echo_ring(row: usize, col: usize) -> usize`: Manhattan distance from (2,2).
  - `pub fn echo_opacity(phase: f32, row: usize, col: usize) -> f32`
  - `pub const HALO_CYCLE_MS: u64 = 1_650;`
  - `pub const HALO_STEPS: usize = 24;`
  - `pub const HALO_PATH: [(usize, usize); 12] = [(0,1),(0,2),(0,3),(1,4),(2,4),(3,4),(4,3),(4,2),(4,1),(3,0),(2,0),(1,0)];`
  - `pub const HALO_TAIL: [f32; 5] = [1.0, 0.78, 0.56, 0.36, 0.22];`
  - `pub fn halo_opacity(phase: f32, row: usize, col: usize) -> Option<f32>`: `None` for the four hidden corners.

**Echo algorithm** (not determined by the signature):
- Keyframes `[(0.0, 0.10), (0.28, 0.98), (0.56, 0.32), (0.78, 0.7824), (1.0, 0.10)]`.
- The local phase is `(phase - (ring as f32 * 0.14 + (ring % 2) as f32 * 0.03)).rem_euclid(1.0)`.
- Interpolate the segment containing the local phase with `lerp`, after easing the segment progress with cubic-bezier(0.42, 0, 0.58, 1). Use a pure bisection solve, 24 iterations, like the HTML mockup.

**Halo algorithm:**
- `step = floor(phase × 24)`; `lead_a = floor(step / 24 × 12) % 12`; `lead_b = (lead_a + 6) % 12`.
- On the path, opacity = max of:
  - `HALO_TAIL[i]` where `order == (lead_a − i) mod 12`
  - `0.72 × HALO_TAIL[i]` where `order == (lead_b − i) mod 12`
  - `0.08`
- Off the path: the core (2,2) is `0.16`, everything else `0.08`.

- [ ] **Step 1: Write failing tests**

- `echo_center_peaks_at_28_percent`: `close(echo_opacity(0.28, 2, 2), 0.98, ..)` and `close(echo_opacity(0.0, 2, 2), 0.10, ..)`.
- `echo_outer_ring_lags_by_its_delay`: ring 4 (cell (0,2)) has delay 0.56. Assert `close(echo_opacity(0.56 + 0.28, 0, 2), 0.98, ..)`.
- `halo_hides_corners`: `halo_opacity(0.0, 0, 0) == None`.
- `halo_lead_and_opposite_comet`: at phase 0.0 (lead_a = 0, lead_b = 6):
  - `(0,1)` (order 0) → `Some(1.0)`
  - `(4,1)` (order 8 = lead_a − 4, wrapped) → `Some(0.22)`
  - `(3,4)` (order 5 = lead_b − 1) → `Some(0.78 × 0.72)`
  - `(4,3)` (order 6 = lead_b) → `Some(0.72)`
  - `(4,2)` (order 7; A's tail covers orders 0, 11, 10, 9, 8 and B's covers 6, 5, 4, 3, 2) → `Some(0.08)`
- `halo_core_and_inner_rest`: `(2,2)` → `Some(0.16)`, `(1,1)` → `Some(0.08)`.

- [ ] **Step 2: Run** `cargo test -p zeron-proto motion` → FAIL (missing fns).
- [ ] **Step 3: Implement** the constants and fns above.
- [ ] **Step 4: Run** `cargo test -p zeron-proto motion` → PASS.
- [ ] **Step 5: Commit** `git commit -am "feat(proto): echo and halo loader math"`

### Task 3: Claude normalizer emits the background set

**Files:**
- Modify: `crates/harness/src/claude/normalize.rs` (`Normalizer::normalize`, 730; tests in `mod tests`, 1349)

**Interfaces:**
- Consumes: `BackgroundTaskKind::from_wire` and `AgentEvent::BackgroundTasks` (Task 1).
- Produces: for `Frame::System` with subtype `background_tasks_changed`, `normalize` returns exactly `vec![AgentEvent::BackgroundTasks { tasks }]`.
  - Entries come from `system.tasks` (`Option<Vec<Value>>`). `None` means an empty set.
  - An entry needs a string `task_id`. `task_type` goes through `from_wire`, and `None` drops the entry. A missing `description` becomes `""`.
  - `started_at` is `UNIX_EPOCH`.
  - Subagent-tagged frames (`parent_tool_use_id` present) are not decoded. Only the root session's frame counts.

`mod.rs`'s existing `turn.set_background(len)` stays unchanged. It still counts agents, which is what the reap guard needs.

- [ ] **Step 1: Write failing tests** (use `normalize_one`)

- `background_tasks_changed_keeps_shells_and_drops_agents`: tasks `[local_agent "scout", local_bash "watch ci" id "b1", monitor "tail log" id "m1"]` → one event with 2 tasks, ids `["b1","m1"]`, kinds `[Shell, Other("monitor")]`.
- `background_tasks_changed_skips_entries_without_an_id_and_reads_absent_as_empty`:
  - `{"type":"system","subtype":"background_tasks_changed"}` → `BackgroundTasks { tasks: vec![] }`
  - an entry missing `task_id` → skipped
- Replay `crates/harness/tests/fixtures/claude/live-2.1.228-background-subagent.jsonl` line 37 → `BackgroundTasks` with an empty vec (local_agent only).

- [ ] **Step 2: Run** `cargo test -p zeron-harness background_tasks_changed` → FAIL.
- [ ] **Step 3: Implement** in `normalize`.
- [ ] **Step 4: Run** `cargo test -p zeron-harness --lib claude` → PASS (existing normalize tests included).
- [ ] **Step 5: Commit** `git commit -am "feat(harness): decode claude background tasks into an event"`

### Task 4: Per-task stop through TurnControl and the Claude driver

**Files:**
- Modify: `crates/harness/src/lib.rs` (`TurnControl`, 80-108)
- Modify: `crates/harness/src/claude/wire.rs` (new line builder next to `stop_turn_request_line`, 632)
- Modify: `crates/harness/src/claude/mod.rs`:
  - select loop: new arm next to `turn.stop_requested()` (1657)
  - `Frame::ControlResponse` handling (1387-1413)
- Modify: `crates/harness/tests/fixtures/fake-claude.sh` (new scenario `stop-task`)
- Test: `crates/harness/tests/claude.rs`

**Interfaces:**
- Produces on `TurnControl`, which stays `Clone + Default`. State is shared through `Arc`: a `std::sync::Mutex<VecDeque<TaskStop>>` plus a `tokio::sync::Notify`.
  - `pub struct TaskStop { pub task_id: String, pub reply: tokio::sync::oneshot::Sender<Result<(), String>> }`
  - Host: `pub fn stop_task(&self, task_id: String) -> tokio::sync::oneshot::Receiver<Result<(), String>>` pushes and notifies.
  - Driver: `pub async fn task_stop_requested(&self) -> TaskStop` waits until the queue is non-empty and pops the front.
- Produces in `wire.rs`: `pub(crate) fn stop_task_request_line(request_id: &str, task_id: &str) -> String` → `{"type":"control_request","request_id":…,"request":{"subtype":"stop_task","task_id":…}}`.
- The driver:
  - On `task_stop_requested()`, it writes the line with id `format!("zeron_stop_task_{n}")` and keeps `reply` in a `HashMap<String, oneshot::Sender<…>>`.
  - On a `ControlResponse` with that id, it sends `Ok(())` on success. On error, it sends `Err(error text)`, e.g. `stop_task_refused` with its message.
  - On teardown, it drops pending replies, so receivers see `RecvError`.
  - It does not touch `stopping` or `stop_settle_at`. A task stop never ends the turn.

- [ ] **Step 1: Write failing tests**

- In `lib.rs` tests, `task_stops_queue_in_order` (tokio test):
  - `stop_task("a")`, `stop_task("b")`.
  - Two `task_stop_requested()` calls yield `"a"`, then `"b"`.
  - Replying `Ok(())` to `"a"` resolves its receiver to `Ok(())`.
- In `wire.rs` tests, `stop_task_request_line_shape`: parse the JSON. `request.subtype == "stop_task"`, `request.task_id == "b1"`, `request_id == "r"`.
- In `claude.rs`, `stopping_a_background_task_sends_stop_task_and_keeps_the_turn`, with `request("scenario:stop-task")`:
  - Wait for `TextDelta "working"`.
  - `let rx = turn.stop_task("b1".into())`; `rx.await == Ok(Ok(()))`.
  - The next events include `BackgroundTasks { tasks: [] }`, then `Done { status: Completed }`. No `Interrupted`.
- `stopping_an_unknown_task_reports_the_refusal`: the same scenario with `"nope"` → `Err(msg)` where `msg` contains `"stop_task_refused"`.

`fake-claude.sh` scenario `stop-task`, following the `stop-in-place` structure at 573-602:
- Require `initialized`.
- Emit `init`, then `background_tasks_changed` with `[{"task_id":"b1","task_type":"local_bash","description":"watch ci"}]`, then the text delta `working`.
- `read -r line` and require `"subtype":"stop_task"`. Extract `request_id` and `task_id`.
- If `task_id` is `b1`:
  - emit a success `control_response`
  - emit `background_tasks_changed` with `[]`
  - emit a success `result`
- Otherwise:
  - emit `{"type":"control_response","response":{"subtype":"error","request_id":"$rid","error":"stop_task_refused: not running"}}`
  - then a success `result`

- [ ] **Step 2: Run** `cargo test -p zeron-harness --test claude stop` → the new tests FAIL.
- [ ] **Step 3: Implement** `TurnControl`, the wire builder, the select arm and response routing.
- [ ] **Step 4: Run** `cargo test -p zeron-harness` → PASS, including `a_stopped_turn_keeps_the_process_and_its_background_work`.
- [ ] **Step 5: Commit** `git commit -am "feat(harness): stop one claude background task by id"`

### Task 5: Engine keeps the set on the session row

**Files:**
- Modify: `crates/engine/src/sessions.rs`:
  - `impl Inner`: new fn next to `set_running_subagents` (2049)
  - `drive_run` (3461): handle the event before the transcript writes, like `PlanUsage` at 4461
  - the runtime-end reset (5016)
- Test: `crates/engine/src/sessions.rs` unit tests (near `running_subagents_count_only_this_chats_spawn_chips`, 5914), `crates/engine/tests/e2e.rs` (near 3928)

**Interfaces:**
- Consumes: `AgentEvent::BackgroundTasks`, `Session.background_tasks` (Task 1).
- Produces: `fn set_background_tasks(&self, chat_id: &str, tasks: Vec<BackgroundTask>)` on `Inner`. It mirrors `set_running_subagents`:
  - It is a no-op without a status row.
  - Each incoming task with an id already on the row keeps that row's `started_at`. A new id gets `Utc::now()`.
  - It is a no-op when the resulting vec equals the current one. Otherwise it sets `updated_at`, publishes `sessions_tx` and records the session.
- `drive_run`:
  - `if let AgentEvent::BackgroundTasks { tasks } = &event { inner.set_background_tasks(&chat_id, tasks.clone()); continue; }`
  - This check sits before the parked/turn-status gates (4530+), so an update between turns still lands.
- Runtime end: `inner.set_background_tasks(&chat_id, Vec::new())` next to `set_running_subagents(&chat_id, 0)`.
- `drive_run` also calls `inner.touch_session(&chat_id)` while the set is non-empty, like subagents at 4264. This keeps the row fresh while only background work runs.

- [ ] **Step 1: Write failing tests**

Unit test `background_tasks_keep_their_first_seen_start`:
- Set `[b1]`, read `started_at = t1`.
- Set `[b1, m1]`. `b1.started_at == t1`, and `m1.started_at >= t1`.
- Set `[m1]` → one task.
- Setting `[m1]` again leaves `updated_at` unchanged.

e2e `session_row_lists_background_tasks_until_the_run_ends`, modelled on 3928:
- Run fake-claude `scenario:stop-task`.
- Wait for `TextDelta` and assert the session row has 1 task with id `b1` and kind `Shell`.
- Stop the run (interrupt/terminate path) → the row's `background_tasks` is empty.

e2e `background_update_between_turns_reaches_the_row` (Review Focus 3):
- Use `scenario:stop-in-place`. Its second turn emits `background_tasks_changed` with `[]` after the first turn's set.
- Its first set holds only `local_agent`, so the row stays empty. Extend that scenario's first emit with a `local_bash` entry `bg-2` (keep `bg-1`).
- Assert `bg-2` shows after turn 1, and the row clears after turn 2's empty set.

- [ ] **Step 2: Run** `cargo test -p zeron-engine background_tasks` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p zeron-engine` → PASS. Two failures are known and pre-existing, not caused by this work:
  - `uploads::tests::chat_images_read_only_raster_files_inside_the_chat_folder` (macOS `/var` symlink)
  - `fs_watch::registration_burst_applies_in_one_pass` (flaky; passes alone)
- [ ] **Step 5: Commit** `git commit -am "feat(engine): publish a chat's background tasks on its session row"`

### Task 6: StopBackgroundTask RPC

**Files:**
- Modify: `crates/rpc/src/lib.rs` (`methods`, 37)
- Modify: `crates/engine/src/rpc.rs`:
  - params struct near `ChatParams` (96)
  - dispatch near `FOCUS_CHAT` (2951)
  - `forwardable` (2123-2249)
- Modify: `crates/engine/src/sessions.rs` (`impl SessionsEngine`, next to `stop_turn`, 1437)

**Interfaces:**
- Produces:
  - `/// Stop one background task of a chat's live runtime. { chatId, taskId } → {}` above `pub const STOP_BACKGROUND_TASK: &str = "StopBackgroundTask";`
  - `struct StopBackgroundTaskParams { chat_id: String, task_id: String }`, camelCase.
  - `pub async fn SessionsEngine::stop_background_task(&self, chat_id: &str, task_id: &str) -> Result<(), EngineError>`:
    - With no run for `chat_id` → `EngineError::Other("not running".into())`.
    - When `task_id` is not in the chat's current `background_tasks` → `EngineError::Other("not running".into())`.
    - Otherwise `h.turn.stop_task(task_id.into())`, awaited with a 10s `tokio::time::timeout`:
      - timeout → `Other("stop timed out")`
      - dropped receiver → `Other("not running")`
      - `Err(msg)` → `Other(msg)`
- Dispatch: `methods::STOP_BACKGROUND_TASK => { parse; self.sessions.stop_background_task(..).await.map_err(|e| RpcError::Failed(e.to_string()))?; RpcReply::value(&json!({})) }`. Add it to `forwardable` so `targetDeviceId` routes it to the host.

- [ ] **Step 1: Write failing tests** (in `crates/engine/tests/e2e.rs`, using the fake-claude engine harness from Task 5)

- `stop_background_task_stops_one_task_and_clears_the_row`: `scenario:stop-task`, call the RPC with `b1` → `Ok({})`. The row then empties, and the run is still live until its `result`.
- `stop_background_task_rejects_unknown_tasks_and_chats`: task `nope` → `Failed("not running")`. Chat `missing` → `Failed("not running")`.
- `stop_background_task_times_out_when_nobody_answers` (Review Focus 2):
  - Add a `stop-task-silent` scenario to `fake-claude.sh`. It is identical to `stop-task` up to the `working` delta, then it reads the stop line and never answers (`sleep 30`).
  - Call the RPC for `b1` with `tokio::time::pause()` and advance 10s → `Failed("stop timed out")`.

- [ ] **Step 2: Run** `cargo test -p zeron-engine --test e2e stop_background_task` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p zeron-engine --test e2e stop_background_task && cargo test -p zeron-rpc` → PASS.
- [ ] **Step 5: Commit** `git commit -am "feat(engine): StopBackgroundTask rpc"`

### Task 7: Echo and halo loaders in the UI

**Files:**
- Modify: `crates/ui/src/loaders.rs` (after `ring_spinner`)

**Interfaces:**
- Consumes: Task 2's math.
- Produces:
  - `pub fn echo_loader(key: impl Into<SharedString>, dot: f32, tint: Hsla) -> impl IntoElement`
  - `pub fn halo_loader(key: impl Into<SharedString>, dot: f32, tint: Hsla) -> impl IntoElement`
  - Both draw a 5×5 grid. `dot` is the dot diameter and the gap is `dot × 0.5`, so the total side is `dot × 7`.
  - Both use the cached-view pattern of `ring_spinner`: `with_global_id` + `with_element_state` + `view.cached(size)`. The surrounding row doesn't repaint.
  - Phase: `motion::spin_phase(cx.entity_id(), Duration::from_millis(ECHO_CYCLE_MS | HALO_CYCLE_MS), cx)`. `None` (reduced motion) renders phase `0.0` and schedules nothing.
  - Halo cells with `None` opacity paint nothing.

- [ ] **Step 1: Write failing test** `echo_and_halo_loaders_size_to_seven_dots` (`#[gpui::test]`, like `todo_panel.rs:1070`'s window helper):
  - Render each loader with `dot = 2.0` inside a window.
  - Assert the element's laid-out bounds are 14×14.
- [ ] **Step 2: Run** `cargo test -p zeron-ui loaders` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p zeron-ui loaders` → PASS.
- [ ] **Step 5: Commit** `git commit -am "feat(ui): echo and halo loaders"`

### Task 8: Shared running-work rows

**Files:**
- Create: `crates/ui/src/running_work.rs` (declare `mod running_work;` in `crates/ui/src/lib.rs`)
- Modify: `crates/ui/src/shell/session_lanes.rs`:
  - remove `SubagentRow` and `subagent_rows` (62-180)
  - import from `crate::running_work`
  - move the tests that cover them (778-878) along
- Modify: `crates/ui/src/state.rs` (next to `running_subagents_for`, 3321)

**Interfaces:**
- Produces, in `crate::running_work`:
  - `pub(crate) struct SubagentRow { pub doc_id: String, pub title: SharedString, pub status: Option<SubagentStatus>, pub spawned_at: DateTime<Utc>, pub model: Option<SharedString> }`
    - Same `frozen()` as today. `indicator()` becomes `pub(crate)`.
    - `model` comes from `call.subagent_model()`.
  - `pub(crate) fn subagent_rows(state: &AppState, chat_id: &str) -> Vec<SubagentRow>`: unchanged behaviour.
  - `pub(crate) fn running_subagent_rows(state: &AppState, chat_id: &str) -> Vec<SubagentRow>`: `subagent_rows` filtered to `status == Some(Running)`.
  - `pub(crate) fn elapsed_label(since: DateTime<Utc>, now: DateTime<Utc>) -> SharedString`: `"41s"`, `"1m 12s"` or `"1h 03m"`.
- Produces in `AppState`: `pub fn background_tasks_for(&self, chat_id: &str, now: DateTime<Utc>) -> &[BackgroundTask]` → `view::background_tasks(self.session_for(chat_id), now)`.

- [ ] **Step 1: Write failing tests** in `running_work.rs`:
  - The moved lanes tests, still passing.
  - `running_rows_drop_settled_subagents`: two spawn chips, `Running` and `Done` → one row.
  - `elapsed_label_formats`: 41s → `"41s"`, 72s → `"1m 12s"`, 3780s → `"1h 03m"`.
- [ ] **Step 2: Run** `cargo test -p zeron-ui running_work` → FAIL.
- [ ] **Step 3: Implement the move, the new fns and the state accessor.** `session_lanes.rs` keeps compiling against the moved items.
- [ ] **Step 4: Run** `cargo test -p zeron-ui running_work session_lanes` → PASS.
- [ ] **Step 5: Commit** `git commit -am "refactor(ui): share subagent rows between the lanes card and the composer"`

### Task 9: Composer pills and trays

**Files:**
- Create: `crates/ui/src/running_trays.rs` (`impl Composer`, like `todo_panel.rs`)
- Modify: `crates/ui/src/lib.rs` (mod)
- Modify: `crates/ui/src/composer.rs`:
  - fields next to `todo_panels` (6410)
  - tray stack (11536-11555): the new element goes above the todo panel, so it is the outermost tray
  - `ComposerEvent` (5689): new variant
- Modify: `crates/ui/src/shell.rs` (composer subscription, 2599): handle the new event with `self.add_subagent_surface(..)`

**Interfaces:**
- Consumes:
  - Task 7: `echo_loader`, `halo_loader`
  - Task 8: `running_subagent_rows`, `elapsed_label`, `background_tasks_for`
  - Task 6: `methods::STOP_BACKGROUND_TASK`
- Produces:
  - `#[derive(Default)] pub(crate) struct RunningTrayState { open: Option<RunningTray>, stopping: HashSet<String> }`, with `#[derive(Clone, Copy, PartialEq, Eq)] pub(crate) enum RunningTray { Subagents, Background }`. Stored as `running_trays: HashMap<String, RunningTrayState>` keyed by chat id, like `todo_panels`.
  - `pub(crate) fn render_running_trays(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement>`. Returns `None` when both lists are empty.
  - `ComposerEvent::OpenSubagent { chat_id: String, doc_id: String, title: String, frozen: bool }`.
- Behaviour:
  - Pill row:
    - Left-aligned, 24px pills.
    - Subagents pill: `echo_loader(dot 2.0)` + count + `subagents` + chevron.
    - Background pill: `halo_loader(dot 2.0)` + count + `background` + chevron.
    - The count is the list length, so the pill and the tray never disagree.
    - A pill is absent at zero. When its list empties, its open tray closes.
  - Clicking a pill toggles its tray. Opening one closes the other. The tray animates height and opacity on `COLLAPSE`, and the chevron rotates on `CHEVRON`.
  - Subagent row: `echo_loader` + title + `model · elapsed` (model omitted when `None`). A click emits `OpenSubagent` with `frozen: false`.
  - Background row: `halo_loader` + description (or id when empty) + elapsed + the stop button. The stop button is described in Global Constraints; its tooltip is `Stop task`.
  - When the stop button is clicked:
    - Insert the id into `stopping`, and the row shows `Stopping…` in `theme.danger` with the button inert.
    - Call `STOP_BACKGROUND_TASK` with `chatId`, `taskId` and `targetDeviceId` = the chat's `device_id`, following `queue_rpc`'s (queue.rs:1919) engine/params pattern.
    - On `Err(e)`:
      - Remove the id from `stopping`.
      - If the task is still in `background_tasks_for`, call `toast::error(cx, format!("Couldn't stop the task: {e}"))`. If it's gone, show nothing (Review Focus 1).
    - Ids leave `stopping` when they leave the set.

- [ ] **Step 1: Write failing tests** (`#[gpui::test]`, reuse the window helper pattern of `todo_panel.rs:1015`; seed `AppState.sessions` with a row holding `background_tasks` and the transcript with spawn chips)

- `pills_show_only_for_nonempty_lists`: no work → `render_running_trays` is `None`. One background task → only the background pill renders.
- `one_tray_open_at_a_time`: open Subagents, then click Background → `open == Some(Background)`.
- `tray_state_follows_the_selected_chat` (Review Focus 4): open in chat A, select chat B → B's state is closed. Back to A → open.
- `stop_marks_the_row_and_calls_the_rpc`: with a fake engine client recording calls (the pattern `pull_request_test_support.rs:76` uses), a click → the recorded method is `StopBackgroundTask` with `{chatId, taskId, targetDeviceId}`, and `stopping` contains the id.
- `failed_stop_on_a_gone_task_shows_no_toast`: the RPC errors after the session row dropped the task → `toast::take_error` is `None`.

- [ ] **Step 2: Run** `cargo test -p zeron-ui running_trays` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p zeron-ui running_trays todo_panel queue` → PASS (the neighbouring trays still behave).
- [ ] **Step 5: Commit** `git commit -am "feat(ui): subagent and background pills above the composer"`

### Task 10: Background lane in the session card

**Files:**
- Modify: `crates/ui/src/shell/session_lanes.rs` (`render_session_lanes`, 337; section pattern at 437-439)

**Interfaces:**
- Consumes: `background_tasks_for`, `elapsed_label`, `halo_loader`, and the stop flow from Task 9.
  - Extract the stop handler into `pub(crate) fn stop_background_task(state: &Entity<AppState>, chat_id: String, task_id: String, cx: &mut App) -> Task<Result<(), String>>` in `running_work.rs`.
  - Both the composer row and the lane row call it, and each keeps its own `stopping` marker.
- Behaviour:
  - After the Subagents section, when the set is non-empty, push `popover::picker_divider()`, then `popover::menu_heading(theme, "Background")`, then one `picker_row` per task.
  - Each row: `halo_loader` in `picker_icon_slot`, `row_title(description)`, `time_label(elapsed)`, and the same stop button.

- [ ] **Step 1: Write failing test** `background_lane_shows_only_with_tasks` in `session_lanes.rs` tests:
  - A chat with no tasks renders no `"Background"` heading.
  - With one task, the heading and a row with the description render.
- [ ] **Step 2: Run** `cargo test -p zeron-ui session_lanes` → FAIL.
- [ ] **Step 3: Implement**, and refactor Task 9's composer stop to call the shared `stop_background_task`.
- [ ] **Step 4: Run** `cargo test -p zeron-ui` → PASS (full UI suite, 2163+ tests).
- [ ] **Step 5: Commit** `git commit -am "feat(ui): background lane in the session card"`

### Task 11: Verify in the app

- [ ] **Step 1:** `cargo clippy --workspace --all-targets` → no new warnings in touched files.
- [ ] **Step 2:** Rebuild the dev app with `ZERON_DEV_CODESIGN_IDENTITY=- ZERON_DEV_IPC_PORT=49778 bash scripts/run-macos-dev.sh` (never the Apple Development identity).
- [ ] **Step 3:** In a Claude chat in the app:
  - Run a `sleep 120` Bash with `run_in_background`, then spawn a background subagent.
  - Both pills show with their loaders, and the trays open one at a time.
  - Stop the shell from the tray → `Stopping…`, then the row and the pill go away.
  - The session card's Background lane mirrors the tray.
