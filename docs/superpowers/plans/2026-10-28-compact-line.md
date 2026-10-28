# Compaction Line Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show context compaction as a persistent full-width line in the transcript ("Compacting context" with a spinner, then "Compacted · 224K → 31K tokens").

**Architecture:** Harness drivers emit a new `AgentEvent::Compaction`. The doc fold turns it into a new `MessagePart::Compaction` (doc kind `"compaction"`). The engine fills a missing "before" from the chat's last context reading and a missing "after" from the first context reading that follows. The UI renders the part as a divider row, hides the `/compact` bubble it answers, and hides the working word while it runs.

**Tech Stack:** Rust workspace, gpui UI, Loro docs.

**Spec:** `docs/superpowers/specs/2026-10-28-compact-line-design.md`

## Global Constraints

- Toolchain `cargo +1.97.1`, always `--locked`.
- No `//` comments in new code. `///` only on new public items whose purpose the name does not say.
- UI copy, exact: `Compacting context`, `Compacted`, `Compacted · {before} → {after} tokens`, `Compacted · from {before} tokens`, `Compacted · down to {after} tokens`, `Compaction stopped`. The separator is ` · ` (U+00B7) and the arrow is ` → ` (U+2192).
- Short token format: below 1,000 the integer (`850`), below 999,500 thousands rounded half up with `K` (`224K`), otherwise millions with one decimal and `M`, dropping a trailing `.0` (`1.2M`, `1M`).
- Providers in scope: Claude Code and Codex only.
- Commits: English subject, no trailers, no co-author lines.
- The working tree holds the user's uncommitted work in `crates/ui/src/transcript.rs`, `crates/ui/src/shell.rs`, `crates/ui/src/composer.rs` and others listed by `git status`. Never commit, revert, stash or reformat it. Run `rustfmt` on new files only, never `cargo fmt` on the workspace.
- Staging a file `F` that also has the user's changes (`crates/ui/src/transcript.rs` in Tasks 1 and 4): the controller saved the user's diff once, before any task, at `$WS/user-transcript.patch` (`$WS` is the plan workspace). Never overwrite it. To commit, build HEAD plus only your changes and stage that blob without touching the working tree:
  ```bash
  F=crates/ui/src/transcript.rs
  cp $F "$WS/mine.rs"
  patch -R "$WS/mine.rs" < "$WS/user-transcript.patch"
  git update-index --cacheinfo 100644,"$(git hash-object -w "$WS/mine.rs")",$F
  git diff --cached -- $F
  ```
  The cached diff must contain only your hunks. If `patch -R` reports a failed hunk, stop and report it. Never resolve it by hand.
- Do not start, stop or call any running Zeron app or engine, do not touch `~/dev/zeron-fix-claude`, do not switch branches, do not push.
- Known flaky UI test: `pull_requests::tests::pull_request_repository_picker_keeps_selection_and_cancels_detection`.

## Review Focus

1. A manual Claude `/compact` (boundary without `post_tokens`, turn ends) must get its "after" from the next run's first context reading, not stay at "from" forever. Covered by the Task 3 e2e test and `awaits_after_*` tests.
2. A reading written straight to the doc must not be erased by a later segment sync that still holds the part without "after", and a reading that lands before the first segment flush must not be lost. Task 3 writes `folded` while the part is in the open segment and the doc only after the segment closed. Covered by both Task 3 e2e tests.
3. Auto compaction in the middle of a long reply in compact mode: the line must stay visible outside the collapsed tool group. Covered by Task 4 `compaction_row_stays_outside_the_compact_work_group`.
4. The `/compact` bubble must stay when the reply has no compaction line (other providers, or an older engine). Covered by Task 4 `compact_bubble_hides_only_when_the_reply_has_the_line`.
5. A run interrupted mid-compaction must read "Compaction stopped", not spin forever. Covered by Task 4 label tests (`Running` + not streaming).

---

### Task 1: Event, transcript part, fold and doc storage

**Files:**
- Modify: `crates/proto/src/agent.rs` (new event variant and phase enum)
- Modify: `crates/doc/src/parts.rs` (new part, status enum, `id`, `byte_len`, fold arms)
- Modify: `crates/doc/src/schema.rs` (`DocPartJson` fields, `to_doc_part`, `from_doc_part` arm, `fill_compaction_after`)
- Modify, compile-driven only: every other exhaustive `match` on `AgentEvent` or `MessagePart` the compiler flags (`crates/mcp/src/transcript.rs`, `crates/mobile/src/layout/rows.rs`, `crates/engine/src/doc_host.rs`, `crates/engine/src/sessions.rs`, `crates/client`, `crates/ui`). In each, the new variant does what `Switch` does there, or is skipped where `Switch` produces output (mobile and mcp render nothing for it). In `crates/ui` add only a placeholder arm that emits no row. Task 4 replaces it.

**Interfaces:**
- Produces, in `zeron_proto`:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub enum CompactionPhase { Started, Finished }

  AgentEvent::Compaction { phase: CompactionPhase, tokens_before: Option<u64>, tokens_after: Option<u64> }
  ```
  with `#[serde(rename_all = "camelCase")]` on the variant, like its neighbours.
- Produces, in `zeron_doc`:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub enum CompactionStatus { Running, Done }

  MessagePart::Compaction { id: String, status: CompactionStatus, tokens_before: Option<u64>, tokens_after: Option<u64> }

  impl SessionDoc { pub fn fill_compaction_after(&self, tokens: u64) -> Result<bool, DocError> }
  ```
  Doc encoding: `kind: "compaction"`, keys `compactionStatus` (`"running"`/`"done"`), `tokensBefore`, `tokensAfter`. An unknown `compactionStatus` reads as `Done`.

Fold rules (`fold_event_into_parts`):
- `Started`: if `out` already has a `Running` compaction, no change. Otherwise push `Compaction { id: format!("c{}", out.len()), status: Running, tokens_before, tokens_after: None }`.
- `Finished`: the last `Running` compaction becomes `Done`. `tokens_before` becomes the event's value when it is `Some`, otherwise it keeps its own. `tokens_after` becomes the event's value. With no `Running` compaction, push a `Done` one with `id: format!("c{}", out.len())` and the event's numbers.
- `byte_len` for the part is 0.

`fill_compaction_after`: walk entries newest first, then each entry's parts last to first. Stop at the first `"compaction"` part. If its status is `"done"` and it has no `tokensAfter`, insert `tokensAfter`, commit and return `Ok(true)`. Otherwise return `Ok(false)`. With no compaction part, return `Ok(false)`. Store the number the same way `DocPartJson` serializes `u64` fields, so `read_entries` reads it back.

- [ ] **Step 1: Write the failing tests**

In `crates/doc/src/parts.rs` tests:
```rust
fn compaction(phase: CompactionPhase, before: Option<u64>, after: Option<u64>) -> AgentEvent {
    AgentEvent::Compaction { phase, tokens_before: before, tokens_after: after }
}

#[test]
fn compaction_started_then_finished_folds_into_one_done_part() {
    let mut out = vec![MessagePart::Text { id: "t0".into(), text: "hi".into() }];
    fold_event_into_parts(&mut out, &compaction(CompactionPhase::Started, Some(224_000), None));
    fold_event_into_parts(&mut out, &compaction(CompactionPhase::Started, None, None));
    fold_event_into_parts(&mut out, &compaction(CompactionPhase::Finished, None, Some(31_000)));
    assert_eq!(out[1], MessagePart::Compaction {
        id: "c1".into(),
        status: CompactionStatus::Done,
        tokens_before: Some(224_000),
        tokens_after: Some(31_000),
    });
    assert_eq!(out.len(), 2);
}

#[test]
fn compaction_finished_alone_folds_a_done_part() {
    let mut out = Vec::new();
    fold_event_into_parts(&mut out, &compaction(CompactionPhase::Finished, Some(224_463), None));
    assert_eq!(out, vec![MessagePart::Compaction {
        id: "c0".into(),
        status: CompactionStatus::Done,
        tokens_before: Some(224_463),
        tokens_after: None,
    }]);
}
```

In `crates/doc/src/schema.rs` tests, next to the fork/switch seam round trips:
- `compaction_part_round_trips`: push an entry with `Compaction { Running, Some(1), None }` and one with `Compaction { Done, Some(224_463), Some(31_000) }`. `read_entries` returns both unchanged.
- `fill_compaction_after_stamps_only_the_newest_pending_part`: entries in order: `Done{before 10, after None}`, a text entry, `Done{before 224_463, after None}`. `fill_compaction_after(31_000)` returns `true` and sets only the newest part's `tokens_after`. A second call returns `false`. With the newest part `Running`, the call returns `false`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo +1.97.1 test --locked -p zeron-doc compaction`
Expected: compile errors for the missing types.

- [ ] **Step 3: Implement the types, fold arms, schema mapping and `fill_compaction_after`, then fix the compiler-flagged matches across the workspace**

- [ ] **Step 4: Run the tests and the workspace check**

Run: `cargo +1.97.1 test --locked -p zeron-doc && cargo +1.97.1 test --locked -p zeron-proto && cargo +1.97.1 check --locked --workspace --all-targets`
Expected: all pass, no errors.

- [ ] **Step 5: Commit**

Stage only the files this task changed, by path. A file that also carries the user's changes (the `crates/ui` placeholder lands in `transcript.rs`) goes through the staging procedure in Global Constraints.
```bash
git commit -m "feat(doc): compaction event and transcript part"
```

---

### Task 2: Claude Code and Codex emit compaction events

**Files:**
- Modify: `crates/harness/src/claude/normalize.rs:588-608` and its test `compaction_is_liveness_then_new_context_usage` (around line 1833)
- Modify: `crates/harness/src/codex/mod.rs:1773-1818`
- Modify: `crates/harness/tests/fixtures/fake-codex.sh:122`
- Modify: `crates/harness/tests/codex.rs` (`native_commands_use_rpc_operations_and_render_results`, around line 1764)

**Interfaces:**
- Consumes: `AgentEvent::Compaction`, `CompactionPhase` (Task 1).

Claude Code:
- `system` / `status` with `status == "compacting"` returns `[ReasoningDelta { text: "" }, Compaction { Started, None, None }]`.
- `system` / `compact_boundary` returns `Compaction { Finished, tokens_before: compact_metadata.pre_tokens, tokens_after: compact_metadata.post_tokens }`, followed by the existing `ContextUsage { tokens: post_tokens, window: None }` only when `post_tokens` is present. A boundary without metadata returns `[Compaction { Finished, None, None }]`.

Codex:
- `item/started` with item type `contextCompaction` sends `Compaction { Started, None, None }`.
- `item/completed` with that type sends `Compaction { Finished, None, None }` instead of the `"Context compacted."` text delta. Remove the `"contextCompaction"` arm from the completed-output match.
- In the fixture, emit `{"method":"item/started","params":{"item":{"id":"compact-1","type":"contextCompaction"}}}` right before the existing `item/completed` line.

- [ ] **Step 1: Update the tests first**

Rename the Claude test to `compaction_frames_become_compaction_events` and assert:
```rust
assert_eq!(
    normalize_one(r#"{"type":"system","subtype":"status","status":"compacting"}"#),
    vec![
        AgentEvent::ReasoningDelta { text: String::new() },
        AgentEvent::Compaction { phase: CompactionPhase::Started, tokens_before: None, tokens_after: None },
    ]
);
assert_eq!(
    normalize_one(r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"auto","pre_tokens":180000,"post_tokens":9000}}"#),
    vec![
        AgentEvent::Compaction { phase: CompactionPhase::Finished, tokens_before: Some(180000), tokens_after: Some(9000) },
        AgentEvent::ContextUsage { tokens: Some(9000), window: None },
    ]
);
assert_eq!(
    normalize_one(r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"manual","pre_tokens":224463}}"#),
    vec![AgentEvent::Compaction { phase: CompactionPhase::Finished, tokens_before: Some(224463), tokens_after: None }]
);
assert_eq!(
    normalize_one(r#"{"type":"system","subtype":"compact_boundary"}"#),
    vec![AgentEvent::Compaction { phase: CompactionPhase::Finished, tokens_before: None, tokens_after: None }]
);
```
Keep the test's remaining assertions (the `informational` frame).

In the Codex test, the `/compact` case expects empty text, and the loop also collects compaction phases. For `/compact` they must equal `[Started, Finished]`. For the `/review` cases they must be empty.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo +1.97.1 test --locked -p zeron-harness compaction_frames_become_compaction_events && cargo +1.97.1 test --locked -p zeron-harness --test codex native_commands_use_rpc_operations_and_render_results`
Expected: FAIL on the new assertions.

- [ ] **Step 3: Implement the Claude and Codex changes and the fixture line**

- [ ] **Step 4: Run the harness tests**

Run: `cargo +1.97.1 test --locked -p zeron-harness`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/harness
git commit -m "feat(harness): claude and codex report compaction start and end"
```

---

### Task 3: Engine fills the numbers the provider did not send

**Files:**
- Create: `crates/engine/src/compaction.rs` (pure helpers and unit tests). Register it with `mod compaction;` in `crates/engine/src/lib.rs`.
- Modify: `crates/engine/src/sessions.rs`, inside `drive_run` (fn at line ~3370): initial entries read (~3455), the `ContextUsage` branch (~4361), the fold (~4695).
- Test: `crates/engine/tests/e2e.rs`

**Interfaces:**
- Consumes: Task 1 types and `SessionDoc::fill_compaction_after`, `SessionDoc::context_usage`.
- Produces, in `crates/engine/src/compaction.rs`:
  ```rust
  pub(crate) fn with_before(event: AgentEvent, usage: Option<zeron_proto::ContextUsage>) -> AgentEvent
  pub(crate) fn awaits_after(entries: &[SessionMessageEntry]) -> bool
  pub(crate) fn fill_after(parts: &mut [MessagePart], tokens: u64) -> bool
  pub(crate) fn has_pending(parts: &[MessagePart]) -> bool
  ```
  - `with_before`: a `Compaction` with `tokens_before: None` gets `usage.and_then(|u| u.tokens)`. Any other event is returned unchanged.
  - `awaits_after`: true when the last `Assistant` entry holds a `Done` compaction with `tokens_after: None`.
  - `fill_after`: sets `tokens_after` on the last `Done` compaction lacking it. Returns whether it set one.
  - `has_pending`: whether `parts` holds a `Done` compaction lacking `tokens_after`.

Wiring in `drive_run`:
1. Bind the initial `read_entries` result once. Compute `let mut compaction_pending = compaction::awaits_after(&entries);` and keep the existing loop over the same entries.
2. Before `inner.publish(&chat_id, &event)`, replace an untagged `Compaction` event with `compaction::with_before(event, doc_ref.context_usage())`. `Subagent`-wrapped events are left alone.
3. After the fold, when the event was a `Compaction` with phase `Finished`, set `compaction_pending = compaction::has_pending(&folded)`.
4. In the `ContextUsage` branch, before its `continue`: when `compaction_pending` and `tokens` is `Some(t)`, then
   - if `compaction::fill_after(&mut folded, t)` returns true, the part lives in the open segment. Mark it dirty with the same pattern the loop uses after a fold (`if !dirty { dirty = true; flush_at = now + STREAM_COMMIT_MS }`, see ~4835). `folded` is cleared when a turn parks (~4802), so a hit here always means an open segment;
   - otherwise call `doc_ref.fill_compaction_after(t)` and log a warning on error, like the context usage write does;
   - set `compaction_pending = false`.

- [ ] **Step 1: Write the failing tests**

Unit tests in `compaction.rs`:
- `with_before_fills_only_a_missing_before`: `Finished{None, None}` with usage tokens `Some(224_000)` → `Finished{Some(224_000), None}`. `Finished{Some(5), None}` stays `Some(5)`. A `TextDelta` comes back equal.
- `awaits_after_reads_only_the_last_reply`: true for `[user, assistant{Done before 1, after None}]`. False when a later assistant entry exists without compaction. False when the part has `tokens_after`. False for a `Running` part.
- `fill_after_sets_the_last_pending_part`: two `Done` parts lacking after, `fill_after(.., 31_000)` sets only the last one and returns `true`. A second call on the result sets the first one. With none pending, it returns `false`.

E2E tests in `crates/engine/tests/e2e.rs`, modeled on `context_usage_settles_after_done_without_reopening_the_turn`, using a helper `fn compaction_part(core: &EngineCore) -> MessagePart` that returns the single compaction part across `entries(&core)`:
- `compaction_after_comes_from_the_reading_inside_the_turn`: script `[ContextUsage{Some(224_000), Some(1_000_000)}, Compaction{Started,None,None}, Compaction{Finished,None,None}, ContextUsage{Some(31_000), None}, TextDelta{"ok"}, done(Completed)]`. Wait until idle. The part equals `Compaction { id: "c0", status: Done, tokens_before: Some(224_000), tokens_after: Some(31_000) }`.
- `compaction_after_comes_from_a_reading_after_the_turn`: script `[ContextUsage{Some(224_000), Some(1_000_000)}, Compaction{Finished,None,None}, done(Completed), ContextUsage{Some(31_000), None}]`. Wait until `context_usage().tokens == Some(31_000)`. The part has `tokens_before: Some(224_000)` and `tokens_after: Some(31_000)`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo +1.97.1 test --locked -p zeron-engine compaction`
Expected: FAIL (module missing, then e2e asserts).

- [ ] **Step 3: Implement the module and the wiring**

- [ ] **Step 4: Run the engine tests**

Run: `cargo +1.97.1 test --locked -p zeron-engine compaction && cargo +1.97.1 test --locked -p zeron-engine --test e2e context_usage`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/compaction.rs crates/engine/src/lib.rs crates/engine/src/sessions.rs crates/engine/tests/e2e.rs
git commit -m "feat(engine): fill compaction token counts from context readings"
```

---

### Task 4: The line in the transcript

**Files:**
- Create: `crates/ui/src/compaction_line.rs` (pure label and visibility rules, the row element, unit tests). Register it in `crates/ui/src/lib.rs`.
- Modify, as short hooks only: `crates/ui/src/transcript.rs`
  - `RowKind` (~1040): new variant `CompactionMarker { label: SharedString, running: bool }`.
  - `rows_for_entry` part match (~1767): replace Task 1's placeholder with a row built like the `Switch` arm. `id: "{entry.id}#{part_id}"`, `version: fnv1a(label) ^ running as u64`, no timestamp, no copy text.
  - The timestamp exclusion (~1881): add `RowKind::CompactionMarker { .. }` next to fork and switch.
  - Row render (~7710): `RowKind::CompactionMarker { label, running } => crate::compaction_line::marker(label.clone(), *running, cx.entity_id(), &theme, cx)`.
  - Working indicator (~7259): return `None` while the last assistant entry is streaming with a `Running` compaction.
  - `sync` (~5199), before `diff_rows`: drop the rows of the user entries `compaction_line::hidden_bubbles` returns. Use the same entries the rows were built from.

**Interfaces:**
- Consumes: `MessagePart::Compaction`, `CompactionStatus` (Task 1). `crate::loaders::gradient_spinner(id, &theme, 2.5, view, cx)`.
- Produces, in `crates/ui/src/compaction_line.rs`:
  ```rust
  pub(crate) fn short_tokens(tokens: u64) -> String
  pub(crate) fn label(status: CompactionStatus, streaming: bool, before: Option<u64>, after: Option<u64>) -> String
  pub(crate) fn running(status: CompactionStatus, streaming: bool) -> bool
  pub(crate) fn is_compact_command(raw: &str) -> bool
  pub(crate) fn hidden_bubbles(entries: &[SessionMessageEntry]) -> HashSet<String>
  pub(crate) fn compacting_now(entry: &SessionMessageEntry) -> bool
  pub(crate) fn marker(label: SharedString, running: bool, view: EntityId, theme: &Theme, cx: &mut App) -> AnyElement
  ```
  - `is_compact_command`: `leading_command(&invocation_prompt(raw))` names `compact` (both from `zeron_proto::invocation`). This covers typed `/compact` and the picker's command link.
  - `hidden_bubbles`: ids of `User` entries whose joined text parts satisfy `is_compact_command` and whose next entry holds a `MessagePart::Compaction`. Check the next entry's parts first and read the text only when it has one.
  - `compacting_now`: the entry is `Streaming` and holds a `Running` compaction.
  - `marker`: the `fork_marker` layout (`py 14`, two 1px `border_strong` rules with flex 1 and a centered middle). The middle is a row with gap 6: the spinner only when `running`, then the label at `ui_rems(12.0)` in `text_muted.opacity(0.7)`, `flex_none`, no wrap.

- [ ] **Step 1: Write the failing tests** (in `compaction_line.rs`)

```rust
#[test]
fn short_tokens_rounds_like_the_spec() {
    for (n, s) in [(850, "850"), (31_499, "31K"), (224_463, "224K"), (999_499, "999K"),
                   (999_500, "1M"), (1_000_000, "1M"), (1_234_567, "1.2M")] {
        assert_eq!(short_tokens(n), s, "{n}");
    }
}

#[test]
fn labels_cover_every_state() {
    use CompactionStatus::*;
    assert_eq!(label(Running, true, Some(224_463), None), "Compacting context");
    assert_eq!(label(Running, false, Some(224_463), None), "Compaction stopped");
    assert_eq!(label(Done, false, Some(224_463), Some(31_000)), "Compacted · 224K → 31K tokens");
    assert_eq!(label(Done, true, Some(224_463), None), "Compacted · from 224K tokens");
    assert_eq!(label(Done, false, None, Some(31_000)), "Compacted · down to 31K tokens");
    assert_eq!(label(Done, false, None, None), "Compacted");
    assert!(running(Running, true));
    assert!(!running(Running, false));
    assert!(!running(Done, true));
}

#[test]
fn compact_command_matches_typed_and_picked_forms() {
    let picked = zeron_proto::invocation::Invocation::Command { name: "compact".into() }.link();
    assert!(is_compact_command("/compact"));
    assert!(is_compact_command("  /compact keep the plan"));
    assert!(is_compact_command(&picked));
    assert!(!is_compact_command("/review"));
    assert!(!is_compact_command("please compact"));
}
```
- `compact_bubble_hides_only_when_the_reply_has_the_line`: `[user "/compact", assistant with a Compaction part]` → `{user id}`. `[user "/compact", assistant with only text]` → empty. `[user "hello", assistant with a Compaction part]` → empty.
- `compaction_row_stays_outside_the_compact_work_group` (in `transcript.rs` tests, next to the switch row tests ~12150): an assistant entry `[Tool, Compaction{Done, Some(224_463), Some(31_000)}, Tool, Text "done"]` built with `rows_for_entry(.., compact = true, ..)` holds exactly one `CompactionMarker` row with `compact_fold: None` and label `Compacted · 224K → 31K tokens`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo +1.97.1 test --locked -p zeron-ui compaction -- --test-threads=1`
Expected: compile errors for the missing module.

- [ ] **Step 3: Implement the module and the hooks**

- [ ] **Step 4: Run the UI tests and the workspace check**

Run: `cargo +1.97.1 test --locked -p zeron-ui -- --test-threads=1 && cargo +1.97.1 check --locked --workspace --all-targets`
Expected: PASS, apart from the known flaky test.

- [ ] **Step 5: Commit with the staging procedure**

Stage `compaction_line.rs` and `lib.rs` normally. Stage `transcript.rs` through the procedure in Global Constraints, then check `git diff --cached --stat`.
```bash
git commit -m "feat(ui): compaction line in the transcript"
```
