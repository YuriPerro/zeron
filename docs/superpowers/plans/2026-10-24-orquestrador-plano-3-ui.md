# Orchestrator UI (Plan 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show orchestrators in their own sidebar section with their active children, render chat-to-chat messages as event lines or attributed bubbles instead of user bubbles, and create orchestrators from a dialog.

**Architecture:** All new UI code lives in a new `crates/ui/src/orchestration/` module: pure functions (message classification, sidebar grouping, form validation) plus gpui render functions. Upstream files get short hooks only: one in transcript row building, one in sidebar row building, one filter for Sessions, one "+" action. Nothing changes in stored or synced data; chat-to-chat messages are recognized by the exact text formats the engine already writes.

**Tech Stack:** Rust, gpui (crates/ui, crate `zeron-ui`), engine RPC over `engine.client().call(...)`.

**Spec:** `docs/superpowers/specs/2026-10-24-orquestrador-ui-plano-3-design.md` (parent: `docs/superpowers/specs/2026-10-24-orquestrador-design.md`).

**Execution:** subagent-driven with one review per **block** (A, B, C, D), not per task. Each block's tasks run in sequence by one implementer; the block is reviewed as one diff.

## Global Constraints

- Build and test with `cargo +1.97.1` (the default toolchain cannot build gpui). Pre-existing failure `acp::devin_models::tests::timed_out_catalog_probe_is_reaped_before_returning` in zeron-harness is not ours.
- No `//` inline comments in new code; `///` only on pub items. No new dependencies.
- Thin fork: new code in `crates/ui/src/orchestration/`. Upstream files get hooks of a few lines each. Making an existing private helper `pub(crate)` is allowed; moving or rewriting upstream code is not.
- UI copy is English, exactly: `Orchestrators`, `Needs you`, `{n} done`, `{n} need you`, `Open`, `{title} finished`, `{title} is waiting for an answer`, `{title} stopped with an error`, `Contract change · {demand}`, `New orchestrator`, `Name`, `Projects`, `Provider`, `Model`, `Children`, `Same provider`, `Cancel`, `Create`, `Name is required.`, `Select at least one project.`.
- An orchestrator is a non-archived chat whose `config.orchestrator` is `Some`. Its children are non-archived chats whose `parent_chat_id` is the orchestrator chat id. Do not use `zeron_proto::is_orchestrator_chat` (that is the voice orchestrator).
- Only Claude Code and Codex can host an orchestrator (the engine refuses others).
- Do not start, stop or call any running Zeron engine. Manual testing (Task 8) uses the isolated Zeron Dev bundle signed ad-hoc (`ZERON_DEV_CODESIGN_IDENTITY=-`), never the keychain identity.

## Review Focus

1. A user types text that starts like an engine format (`[Zeron: ...`) but isn't exact: it must stay a normal user bubble (Task 1 test `near_misses_stay_plain`).
2. A child event line whose child was deleted: "Open" must do nothing harmful and the line must still render from the text (Task 4 test `event_for_missing_chat_still_renders_title`).
3. An orchestrator chat with no title: the sidebar shows `Orchestrator` and the monogram uses `O` (Task 2 test `untitled_orchestrator_gets_fallback_name`).
4. An errored child stays visible under its orchestrator instead of disappearing into "done" (Task 2 test `errored_child_stays_active`).
5. An archived orchestrator disappears from the section and does not leak back into Sessions (Task 2 test `archived_orchestrator_is_hidden`, Task 6 test `orchestrators_are_not_in_sessions`).

---

## Block A: pure logic (one review)

### Task 1: Message classification

**Files:**
- Create: `crates/ui/src/orchestration/mod.rs` (declares submodules), `crates/ui/src/orchestration/messages.rs`
- Modify: `crates/ui/src/lib.rs` (add `mod orchestration;`)

**Interfaces:**
- Produces:
  ```rust
  pub(crate) enum ChildEventKind { Finished, WaitingForAnswer, Failed }
  pub(crate) enum MessageOrigin {
      Plain,
      ChildEvent { chat_id: String, title: String, kind: ChildEventKind },
      FromChat { chat_id: String, label: String, body: String, contract_demand: Option<String> },
  }
  pub(crate) fn classify_message(text: &str) -> MessageOrigin
  pub(crate) fn child_event_text(title: &str, kind: ChildEventKind) -> String
  ```

Formats (exact, from the spec table; `<id>` is the content of the last `(...)` before the verb):
- `[Zeron: <title> (<id>) finished its turn. Read it with read_chat.]` → `ChildEvent Finished`
- `[Zeron: <title> (<id>) is waiting for an answer. Read its question with get_chat.]` → `ChildEvent WaitingForAnswer`
- `[Zeron: <title> (<id>) stopped with an error. Read it with read_chat.]` → `ChildEvent Failed`
- `[Message from Zeron chat <label>. Reply to it with the Zeron \`send_message\` tool, chat <id>.]\n\n<body>` → `FromChat { contract_demand: None }`; `label` has its trailing ` (<short id>)` removed, same as upstream `agent_message_display` in `transcript.rs`.
- `[Contract change requested by <title> (<id>) for demand <demand>]\n\n<reason>` → `FromChat { label: title, body: reason, contract_demand: Some(demand) }`
- A ChildEvent must match the whole text (after trimming trailing whitespace). Anything else → `Plain`.

- [ ] **Step 1: Write failing tests** in `messages.rs`:
  - `classifies_each_child_event`: the three formats with title `arch-live: explorar`, id `9db29bd0-7723-4660-bc6e-c7230a3afbb6` → matching kind, title and id.
  - `title_with_parentheses_keeps_last_id`: title `fix (urgent)`, id `abc` → title `fix (urgent)`, id `abc`.
  - `classifies_message_from_chat`: label `Plataforma (9e66485b)`, id `9e66485b-9962-439e-ae83-0d86a5f8fde2`, body `Goal: x` → label `Plataforma`, body `Goal: x`.
  - `classifies_contract_change`: title `Kid`, id `chat-kid-6`, demand `2026-10-24-login`, reason `The API differs.` → `FromChat { label: "Kid", chat_id: "chat-kid-6", body: "The API differs.", contract_demand: Some("2026-10-24-login") }`.
  - `near_misses_stay_plain`: `[Zeron: x (y) finished its turn.]`, `[Zeron: x (y) finished its turn. Read it with read_chat.] and more`, `hello`, `` (empty) → `Plain`.
  - `child_event_text_matches_copy`: `child_event_text("api", Failed) == "api stopped with an error"`, and the other two kinds.
- [ ] **Step 2:** Run `cargo +1.97.1 test --locked -p zeron-ui --lib orchestration::messages`. Expected: FAIL (compile).
- [ ] **Step 3:** Implement with `strip_prefix`/`rsplit_once` like `agent_message_display`; no regex.
- [ ] **Step 4:** Run again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(ui): classify chat-to-chat messages`.

### Task 2: Sidebar grouping

**Files:**
- Create: `crates/ui/src/orchestration/sidebar.rs` (pure part only in this task)

**Interfaces:**
- Consumes: `zeron_proto::{Chat, ChatIndicator}`.
- Produces:
  ```rust
  pub(crate) const UNTITLED_ORCHESTRATOR: &str = "Orchestrator";
  pub(crate) fn is_orchestrator(chat: &Chat) -> bool
  pub(crate) struct OrchestratorGroup {
      pub chat_id: String,
      pub name: String,
      pub indicator: ChatIndicator,
      pub needs_you: bool,
      pub active_children: Vec<String>,
      pub done_children: Vec<String>,
  }
  pub(crate) fn orchestrator_groups(chats: &[(Chat, ChatIndicator)]) -> Vec<OrchestratorGroup>
  pub(crate) fn needing_you(groups: &[OrchestratorGroup]) -> usize
  ```

Rules: `needs_you` = orchestrator indicator is `AwaitingInput`. Active child = indicator `Working`, `AwaitingInput` or `Errored`; all others are done. Groups sorted by orchestrator `last_message_at` (else `created_at`) descending; children likewise. Archived chats are skipped entirely. `name` = trimmed title, else `UNTITLED_ORCHESTRATOR`.

- [ ] **Step 1: Write failing tests** in `sidebar.rs` (build `Chat` values with a small test helper):
  - `groups_orchestrators_with_their_children`: one orchestrator, children working / idle / completed → active `[working]`, done `[idle, completed]`.
  - `errored_child_stays_active`.
  - `needs_you_follows_awaiting_input`: orchestrator `AwaitingInput` → `needs_you` true, `needing_you == 1`.
  - `archived_orchestrator_is_hidden` and archived child skipped.
  - `untitled_orchestrator_gets_fallback_name`: `title: None` and `title: Some("  ")` → `Orchestrator`.
  - `plain_parent_is_not_an_orchestrator`: a side chat of a normal chat produces no group.
- [ ] **Step 2:** Run `cargo +1.97.1 test --locked -p zeron-ui --lib orchestration::sidebar`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(ui): group orchestrators and their children`.

### Task 3: New-orchestrator form validation

**Files:**
- Create: `crates/ui/src/orchestration/new_dialog.rs` (pure part only in this task)

**Interfaces:**
- Produces:
  ```rust
  pub(crate) struct NewOrchestratorForm {
      pub name: String,
      pub projects: Vec<String>,
      pub harness: String,
      pub model: Option<String>,
      pub reasoning: Option<String>,
      pub child_harness: Option<String>,
  }
  pub(crate) fn validate(form: &NewOrchestratorForm) -> Result<serde_json::Value, &'static str>
  ```
  The `Ok` value is the `CreateOrchestrator` params: `{ "name": <trimmed>, "projects": [...], "harness": ..., "model"?: ..., "reasoning"?: ..., "childHarness"?: ... }`, omitting `None` fields. `child_harness: None` means "Same provider".

- [ ] **Step 1: Write failing tests**: `blank_name_is_required` (`"  "` → `Err("Name is required.")`), `projects_are_required` (`Err("Select at least one project.")`), `valid_form_builds_params` (exact JSON, name trimmed, `childHarness` absent when `None`, present when `Some("codex")`).
- [ ] **Step 2:** Run `cargo +1.97.1 test --locked -p zeron-ui --lib orchestration::new_dialog`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(ui): validate new orchestrator form`.

**Block A review** after Task 3.

---

## Block B: transcript (one review)

### Task 4: Child event lines

**Files:**
- Modify: `crates/ui/src/transcript.rs`: `RowKind` (`:1040`), `rows_for_entry` (`:1340`, where `agent_message_display` is called at `:1373`), the render match (`:7661`), the marker grouping match (`:1862`)
- Create: `crates/ui/src/orchestration/transcript_rows.rs`

**Interfaces:**
- Consumes: `classify_message`, `child_event_text` (Task 1).
- Produces: `RowKind::ChildEvent { chat_id: SharedString, title: SharedString, kind: ChildEventKind }` and `pub(crate) fn child_event_line(chat_id, title, kind, theme, ...) -> AnyElement` in `transcript_rows.rs`.

Hook: in `rows_for_entry`, for a user entry whose joined text classifies as `ChildEvent`, emit one `ChildEvent` row instead of the `User` row (keep the row id and version scheme the `User` row uses). Treat it like `ForkMarker`/`SwitchMarker` in the grouping match. Render: small dot (`theme` muted for Finished, warning for WaitingForAnswer, error for Failed; use the existing theme color names nearest to these), the `child_event_text`, and an `Open` link that selects `chat_id` through the same path a side-chat row click uses (`AppState::select_chat`). If the chat no longer exists, `Open` is not rendered.

- [ ] **Step 1: Write failing tests** (pure, in `transcript_rows.rs` or the existing transcript test module): `wake_text_becomes_child_event_row` (rows for a user entry with the Finished text → one `ChildEvent` row, no `User` row), `event_for_missing_chat_still_renders_title` (the row data keeps the title from the text regardless of chat lookup).
- [ ] **Step 2:** Run `cargo +1.97.1 test --locked -p zeron-ui --lib`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(ui): render child events as compact lines`.

### Task 5: Attributed bubbles for messages from other chats

**Files:**
- Modify: `crates/ui/src/transcript.rs` (same hook point; replace the `agent_message_display` call path for `FromChat` messages)
- Modify: `crates/ui/src/orchestration/transcript_rows.rs`, `crates/ui/src/orchestration/sidebar.rs`, `crates/ui/src/shell/project_icon.rs:71`

**Interfaces:**
- Consumes: `classify_message` (Task 1), `is_orchestrator` (Task 2).
- Produces: `RowKind::FromChat { chat_id: SharedString, label: SharedString, body: SharedString, contract_demand: Option<SharedString> }`, `pub(crate) fn from_chat_bubble(...) -> AnyElement`, and `pub(crate) fn orchestrator_avatar(name: &str, seed: &str, selected: bool, theme: &Theme) -> AnyElement` in `orchestration/sidebar.rs` (wraps `project_icon::monogram`, made `pub(crate)` at `crates/ui/src/shell/project_icon.rs:71`).

Render: left-aligned bubble (not the right-aligned user bubble), header with avatar and name, then `body` as markdown like the user bubble body. Name = current title of chat `chat_id` if it exists, else `label`. Avatar = `orchestrator_avatar(name, chat_id, false, theme)` when the sender is an orchestrator, else the sender's harness brand icon (`pickers::harness_brand_icon`). `contract_demand: Some(d)` adds the tag `Contract change · {d}` in the header. Delete `agent_message_display` and its call: every message it handled is now `FromChat`.

- [ ] **Step 1: Write failing tests**: `message_from_chat_becomes_from_chat_row`, `contract_change_row_carries_demand`, `plain_prompt_stays_user_row`.
- [ ] **Step 2:** Run `cargo +1.97.1 test --locked -p zeron-ui --lib`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(ui): show messages from other chats with their sender`.

**Block B review** after Task 5.

---

## Block C: sidebar (one review)

### Task 6: Orchestrators section

**Files:**
- Modify: `crates/ui/src/orchestration/sidebar.rs` (render part)
- Modify: `crates/ui/src/shell/spaces.rs` (`render_active_rows` `:5253`: insert the section above the session rows; exclude orchestrator chats from the rows it already builds)

**Interfaces:**
- Consumes: `orchestrator_groups`, `needing_you`, `is_orchestrator` (Task 2); `sidebar_disclosure_header` (`spaces.rs:2244`); `Shell::render_chat_row` (`shell.rs:8608`); `sidebar_collapsed_groups` (`shell.rs:2036`).
- Consumes also: `orchestrator_avatar` (Task 5).
- Produces: a `Shell` method `render_orchestrators_section(...)` called from `render_active_rows`.

Behavior (spec section 3):
- Header `Orchestrators` with a `+` action (Task 7 wires the dialog; here the action may be a no-op stub that Task 7 replaces). Collapse key `orchestrators` in `sidebar_collapsed_groups`. When collapsed and `needing_you > 0`, show `{n} need you` in the header count slot.
- No orchestrators → header and `+` only.
- Orchestrator row: `orchestrator_avatar` in place of the harness icon, name, and `Needs you` in the status corner when `needs_you`; otherwise the normal indicator.
- Active children: rendered with `render_chat_row`, indented one level.
- Done children: a `{n} done` disclosure row (key `orchestrator-done:<chat_id>`, collapsed by default); expanded, it lists them with `render_chat_row`, indented.
- Orchestrator chats never appear in Sessions, pinned or custom sections.

- [ ] **Step 1: Write failing tests**: `orchestrators_are_not_in_sessions` (the rows the sidebar builds for Sessions exclude a chat with `config.orchestrator`), and a gpui test if `shell.rs`'s existing `#[gpui::test]` patterns allow rendering the sidebar: section header present with one orchestrator, `Needs you` label when awaiting input.
- [ ] **Step 2:** Run `cargo +1.97.1 test --locked -p zeron-ui --lib`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again. Expected: PASS. Also `cargo +1.97.1 check --locked --workspace --all-targets`.
- [ ] **Step 5:** Commit `feat(ui): orchestrators section in the sidebar`.

**Block C review** after Task 6.

---

## Block D: create dialog (one review)

### Task 7: New orchestrator dialog

**Files:**
- Modify: `crates/ui/src/orchestration/new_dialog.rs` (render and submit)
- Modify: `crates/ui/src/orchestration/sidebar.rs` (wire `+`)
- Modify: `crates/ui/src/shell.rs` (one field for the open dialog and one render call, next to `SectionDialog`'s)

**Interfaces:**
- Consumes: `NewOrchestratorForm`, `validate` (Task 3); `popover::modal`, `dialog_card`, `dialog_title`, `dialog_field`, `btn_primary`, `btn_ghost` (`popover.rs:855`); `ComposerInput` for Name (as `SectionDialog`, `shell/sidebar_sections.rs:27`); `settings/widgets.rs` `select`/`select_check` for the lists; `methods::LIST_MODELS`, `methods::CREATE_ORCHESTRATOR`.
- Produces: `pub(crate) struct NewOrchestratorDialog` with `open(...)`, `submit(...)`, `render(...)`.

Behavior (spec section 5):
- Projects: this device's spaces, each with a check.
- Provider: connected harnesses among `claude-code` and `codex` only; default the first one.
- Model: models from `LIST_MODELS` for the chosen provider, with reasoning level; default the provider's default model.
- Children: `Same provider` plus the other connected provider.
- Create: `validate`; on `Err(msg)` show `msg` in the dialog. On `Ok(params)` disable Create, call `CREATE_ORCHESTRATOR` via `engine.client().call` in `cx.spawn`. Success → close, `select_chat(chatId)`. Engine error → show its message in the dialog, keep inputs.

- [ ] **Step 1: Write failing tests**: a gpui test if the `SectionDialog` tests show a pattern (open, empty name → `Name is required.` shown, Create calls nothing). Otherwise pure tests on whatever submit-state reducer you extract (`submitting` blocks a second submit; an engine error string is kept and inputs unchanged).
- [ ] **Step 2:** Run `cargo +1.97.1 test --locked -p zeron-ui --lib`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again. Expected: PASS. Also `cargo +1.97.1 check --locked --workspace --all-targets`.
- [ ] **Step 5:** Commit `feat(ui): create orchestrators from a dialog`.

**Block D review** after Task 7, then the final whole-branch review.

---

### Task 8: Manual check in Zeron Dev (controller, with the user)

- [ ] Quit Zeron Dev if running. Rebuild: `cd ~/dev/zeron && RUSTUP_TOOLCHAIN=1.97.1 ZERON_DEV_CODESIGN_IDENTITY=- ZERON_DEV_BUILD_ONLY=1 bash scripts/run-macos-dev.sh`, then launch with `open --env ZERON_AUTO_UPDATE=0 --env ZERON_DATA_DIR=<repo>/target/macos-dev/data --env ZERON_IPC_PORT=49777 "target/macos-dev/Zeron Dev.app"`.
- [ ] The existing `Teste` orchestrator shows under `Orchestrators`, not in Sessions.
- [ ] Create a second orchestrator from `+` with arch-live and chess-arena; it opens.
- [ ] Repeat the Plan 1 investigation: children nest under it while running, then move to `N done`; wake messages show as event lines; each child shows the brief as a bubble from the orchestrator.
- [ ] When the orchestrator asks a question, `Needs you` shows on its row (and in the header when collapsed).
