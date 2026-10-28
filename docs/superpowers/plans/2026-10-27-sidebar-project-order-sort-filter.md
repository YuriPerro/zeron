# Sidebar project order, sorts and status filter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Project groups keep a user-chosen order (drag a header), chats gain Name and Status sorts, and a status filter hides chats that do not need attention.

**Architecture:** All new ordering rules live in pure functions in a new module `crates/ui/src/shell/sidebar_order.rs`. The two existing order builders, `Shell::render_active_rows` (draw order) and `Shell::sidebar_visible_order` (keyboard order), both call the same two shared methods, `Shell::sidebar_sorted_chats` and `Shell::order_sidebar_groups`. Neither builder orders anything on its own. The project-header drag reuses the custom-section drag state with a `kind` discriminator.

**Tech Stack:** Rust 2024, GPUI, crate `zeron-ui` (`crates/ui`), serde.

**Spec:** `docs/superpowers/specs/2026-10-27-sidebar-project-order-sort-filter-design.md`

## Global Constraints

- Branch `zeron-fork`. Start from a clean tree: the image-tile and file-chip work that is still uncommitted must be committed first, or the work must run in a worktree.
- Device-local settings only. Do not change `crates/proto`, `crates/engine` or sync code.
- Every new settings field is covered by the struct-level `#[serde(default)]`. Default behavior equals today's: `Recent`, `LastUpdated`, `All`.
- Format with `rustfmt --edition 2024 <file>` per touched file. Never run `cargo fmt -p zeron-ui`, because it reformats 33 unrelated files.
- Run targeted tests only: `cargo test -p zeron-ui --lib <filter>`. The full suite SIGABRTs on baseline, which is a preexisting issue.
- Menu copy is English, matching the existing menu: `Projects`, `Manual`, `Recent`, `A–Z`, `Name`, `Status`, `Filter`, `All`, `Active`, `Needs me`.
- Commit messages in English, with no co-author or tool trailer.

## Review Focus

1. The open chat is Idle and the filter is `Active`. It must stay visible, and the keyboard order must include it. Pinned in Task 2 (`filter_keeps_pins_and_the_open_chat`).
2. A drop happens while the filter hides some groups. The hidden groups' keys keep their relative slots in `space_order`. Pinned in Task 1 (`moved_project_order_keeps_hidden_keys_in_place`).
3. A drag starts and then the visible group set changes mid-drag (a new chat arrives in a new project). The drop must go home without writing. Pinned in Task 4 (validity check against `payload.ids`).
4. `space_order` holds stale space ids from older builds. They must be ignored when ordering and preserved on write. Pinned in Task 1 (`manual_order_ignores_unknown_keys`, `moved_project_order_keeps_hidden_keys_in_place`).
5. The organization switches away from By project while a project drag is live. The drag must cancel. Pinned in Task 3: `activate_sidebar_view_row` already cancels transfers, and the cancel must also cover `sidebar_section_drag`.

---

### Task 1: Settings and pure ordering rules

**Files:**
- Modify: `crates/ui/src/settings.rs`
  - enums near `:864-878`
  - `UiSettings` fields near `:1011-1013`
  - `space_order` doc `:1056-1059`
  - `merge_changes` list `:1874-1892`
  - `round_trip` test `:2928`
- Create: `crates/ui/src/shell/sidebar_order.rs` (code and `#[cfg(test)] mod tests`)
- Modify: `crates/ui/src/shell.rs:94-95` (add `mod sidebar_order;`)
- Modify: `crates/ui/src/shell/spaces.rs:2108-2121`. Delete `compare_sidebar_chats` there and update its callers at `:5013`, `:5331` and the test at `:8254`.

**Interfaces:**
- Produces, in `settings.rs`:
  - `pub enum SidebarProjectOrder { #[default] Recent, Manual, Name }`
  - `pub enum SidebarStatusFilter { #[default] All, Active, NeedsMe }`
  - Both enums use the same derives and `#[serde(rename_all = "camelCase")]` as `SidebarSort`.
  - `SidebarSort` gains `Name` and `Status`.
  - Fields: `pub sidebar_project_order: SidebarProjectOrder` and `pub sidebar_status_filter: SidebarStatusFilter`.
  - `space_order` docs now say "Manual project-group order: project keys (`zeron_proto::view::project_key`), topmost first."
- Produces, in `sidebar_order.rs` (all `pub(super)`):
  - `fn compare_sidebar_chats(sort: SidebarSort, left: (ChatIndicator, &zeron_proto::Chat), right: (ChatIndicator, &zeron_proto::Chat)) -> std::cmp::Ordering`
  - `fn status_filter_keeps(filter: SidebarStatusFilter, status: ChatIndicator) -> bool`
  - `fn order_project_groups<T>(groups: &mut [(Option<(String, String)>, Vec<T>)], order: SidebarProjectOrder, manual: &[String])`
  - `fn moved_project_order(saved: &[String], visible: &[String], from: usize, over: usize) -> Vec<String>`

- [ ] **Step 1: Write the failing tests** in `sidebar_order.rs`:

```rust
#[test]
fn name_and_status_sorts_break_ties_by_last_updated() {
    // a("beta", t=2, Idle), b("Alpha", t=1, Idle), c("alpha", t=3, Working), d(None, t=4, AwaitingInput), e(t=0, Errored)
    // Name:   c, b, a, then untitled d, e by last updated   → ["c","b","a","d","e"]
    // Status: d (AwaitingInput), e (Errored), c (Working), a, b (Idle by last updated) → ["d","e","c","a","b"]
}
#[test]
fn status_filter_rules() {
    // Active hides only Idle; NeedsMe keeps only AwaitingInput and Errored; All keeps every status.
}
#[test]
fn recent_keeps_first_appearance_order() { /* groups [b, a] stay [b, a] */ }
#[test]
fn name_order_is_case_insensitive_and_puts_no_project_last() {
    // keys/labels: ("home:dev","No project"), ("k-b","beta"), ("k-a","Alpha") → ["k-a","k-b","home:dev"]
}
#[test]
fn manual_order_puts_unknown_groups_on_top_in_recent_order() {
    // groups recent order [c, b, x, a], manual ["a","b","c"] → [x, a, b, c]
}
#[test]
fn manual_order_ignores_unknown_keys() {
    // manual ["space-legacy","b","a"], groups [a, b] → [b, a]
}
#[test]
fn moved_project_order_keeps_hidden_keys_in_place() {
    // see the assertions below
}
```

Assert `moved_project_order(&["a","hidden","b","c"], &["a","b","c"], 0, 2) == ["b","hidden","c","a"]`. The rule: visible keys are rewritten into the slots that visible keys occupied in `saved`, in their new order. Hidden keys keep their own slots. Visible keys missing from `saved` are prepended in visible order. Also assert `moved_project_order(&[], &["x","y"], 1, 0) == ["y","x"]`.

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo test -p zeron-ui --lib shell::sidebar_order`
Expected: compile errors for the missing functions and enum variants.

- [ ] **Step 3: Implement the settings changes and the four functions**

- `compare_sidebar_chats`:
  - `Created` and `LastUpdated` keep today's rule.
  - `Name` compares the lowercase `title`, with `None` last.
  - `Status` uses rank AwaitingInput 0, Errored 1, Working 2, Completed 3, Idle 4, via an exhaustive `match`.
  - Name and Status tie-break on last updated (newest first), then `id`, as today.
- `order_project_groups`:
  - Use a stable `sort_by_key`.
  - `Name` keys on `(key.starts_with("home:"), label.to_lowercase())`.
  - `Manual` keys on `manual.iter().position(..).map_or((0, 0), |p| (1, p))`.
  - `Recent` returns early.
- Add both new fields to the `merge_changes` list.
- Extend `round_trip` with `sidebar_project_order: SidebarProjectOrder::Manual`, `sidebar_status_filter: SidebarStatusFilter::NeedsMe` and `sidebar_sort: SidebarSort::Status`.
- Add a test asserting that `serde_json::from_str::<UiSettings>("{}")` yields `Recent`, `LastUpdated` and `All`.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p zeron-ui --lib shell::sidebar_order && cargo test -p zeron-ui --lib settings::tests::round_trip && cargo test -p zeron-ui --lib sidebar_display_defaults`
Expected: PASS.

- [ ] **Step 5: Commit** with `git commit -m "feat(sidebar): add project order, name/status sorts and status filter rules"`

---

### Task 2: One ordering path for drawing and keyboard

**Files:**
- Modify: `crates/ui/src/shell/spaces.rs`:
  - `sidebar_visible_order` `:4993-5084`
  - `render_active_rows` `:5308-5440`
  - test module near `:896`
- Modify: `crates/ui/src/shell/sidebar_order.rs` (the two `impl Shell` methods below)

**Interfaces:**
- Consumes: everything Task 1 produces.
- Produces (`impl Shell`, `pub(super)`):
  - `fn sidebar_sorted_chats(&self, state: &AppState, now: DateTime<Utc>, pinned: &[String]) -> Vec<(ChatIndicator, zeron_proto::Chat)>`. It calls `state.sidebar_chats(now, space_filter)`, keeps rows where `status_filter_keeps(..)`, or the id is in `pinned`, or the id equals `state.selected_chat`, then sorts with `compare_sidebar_chats(self.settings.sidebar_sort, ..)`.
  - `fn order_sidebar_groups<T>(&self, state: &AppState, groups: &mut Vec<(Option<(String, String)>, Vec<T>)>)`. For `ByDevice` it calls the existing `promote_local_device_group`. For `ByProject` it calls `order_project_groups(groups, self.settings.sidebar_project_order, &self.settings.space_order)`.

- [ ] **Step 1: Write the failing gpui tests** next to `sidebar_project_groups_preserve_pin_order_and_keyboard_order`, using its fixture (projects a and b; chats pin, b-new, a-new, b-old):

```rust
#[gpui::test]
fn manual_project_order_holds_when_a_lower_project_gets_newer_chat(cx) {
    // ByProject, sidebar_project_order = Manual, space_order = ["a","b"]
    // → visible order ["pin","a-new","b-new","b-old"] even though b-new is newest
}
#[gpui::test]
fn keyboard_order_matches_drawn_order_for_every_view_combination(cx) {
    // for org in [ByProject, ByDevice, InOneList], projects in all 3, sort in all 4, filter in all 3:
    // assert_eq!(shell.sidebar_visible_order(cx), drawn_ids(shell, cx))
}
#[gpui::test]
fn filter_keeps_pins_and_the_open_chat(cx) {
    // every chat Idle, filter = Active, selected_chat = "b-old"
    // → visible order ["pin","b-old"]
}
```

`drawn_ids` renders the window, then reads `debug_bounds` of each row's existing debug selector, ordered by `y`. If rows lack a selector, call `render_active_rows` and flatten its row ids in order. Pick whichever the current test helpers already support.

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo test -p zeron-ui --lib sidebar_project && cargo test -p zeron-ui --lib keyboard_order_matches`
Expected: the manual-order and filter tests FAIL on order.

- [ ] **Step 3: Route both builders through the shared methods**

- Replace each builder's `sidebar_chats` + `sort_by` block with `sidebar_sorted_chats`.
- `render_active_rows` must compute `pinned_order` before that call.
- In `sidebar_visible_order`, group with the full `sidebar_project_group(state, &chat)` tuple, not the key plus `String::new()`, so `Name` order has labels.
- Replace each builder's `promote_local_device_group` call with `order_sidebar_groups`.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p zeron-ui --lib sidebar_ && cargo test -p zeron-ui --lib shell::sidebar_order`
Expected: PASS, including the existing sidebar tests.

- [ ] **Step 5: Commit** with `git commit -m "feat(sidebar): share one ordering path between drawing and keyboard jumps"`

---

### Task 3: View menu rows

**Files:**
- Modify: `crates/ui/src/shell/spaces.rs`:
  - `SidebarViewRow` `:2172-2197`
  - `SIDEBAR_VIEW_GROUPS` / `SIDEBAR_VIEW_ROWS` `:2199-2215`
  - `activate_sidebar_view_row` `:4013`
  - menu render `:4215-4300`
  - keyboard nav uses of `SIDEBAR_VIEW_GROUPS` `:4081-4192`, `:4453-4518`

**Interfaces:**
- Produces:
  - `SidebarViewRow` variants `ProjectsManual`, `ProjectsRecent`, `ProjectsName`, `SortName`, `SortStatus`, `FilterAll`, `FilterActive`, `FilterNeedsMe`. All of them are `closes_submenu`.
  - `fn sidebar_view_groups(organization: SidebarOrganization) -> &'static [(&'static str, std::ops::Range<usize>)]`. It replaces every `SIDEBAR_VIEW_GROUPS` use.

Row table (index → row, label, icon):

| Index | Row | Label | Icon |
|---|---|---|---|
| 0-2 | ByDevice, ByProject, InOneList | as today | as today |
| 3 | ProjectsManual | `Manual` | `DRAG_HANDLE` |
| 4 | ProjectsRecent | `Recent` | `CLOCK_CIRCLE` |
| 5 | ProjectsName | `A–Z` | `SORT_VERTICAL` |
| 6-7 | LastUpdated, Created | as today | as today |
| 8 | SortName | `Name` | `SORT` |
| 9 | SortStatus | `Status` | `CHECKLIST` |
| 10 | FilterAll | `All` | `LIST` |
| 11 | FilterActive | `Active` | `ACTION_PLAY` |
| 12 | FilterNeedsMe | `Needs me` | `BELL` |
| 13-19 | the seven Show rows | as today | as today |

Groups:
- ByProject: `[("Organize",0..3),("Projects",3..6),("Sort",6..10),("Filter",10..13),("Show",13..20)]`.
- Other organizations: the same groups without `Projects`.
- The separator goes before the group labeled `Show`, not at `group == 2`.
- `values` (the trigger's current choice) comes from each radio group's range, not from hardcoded indices.

- [ ] **Step 1: Write the failing test** `sidebar_view_menu_offers_projects_only_by_project`:
  - It asserts that `sidebar_view_groups(ByProject)` has 5 labels and `sidebar_view_groups(ByDevice)` has 4 with no "Projects".
  - It asserts every range lies inside `SIDEBAR_VIEW_ROWS.len() == 20`.
  - It activates `FilterNeedsMe`, `SortStatus` and `ProjectsManual` on a test shell, then asserts the three settings fields.
- [ ] **Step 2: Run it and see it fail.** Command: `cargo test -p zeron-ui --lib sidebar_view_menu`. Expected: FAIL.
- [ ] **Step 3: Implement the rows, groups and activation.** `activate_sidebar_view_row` also calls `self.cancel_sidebar_section_drag(cx)`.
- [ ] **Step 4: Run the tests and see them pass.** Command: `cargo test -p zeron-ui --lib sidebar_view`. Expected: PASS, including the existing view-menu keyboard tests.
- [ ] **Step 5: Commit** with `git commit -m "feat(sidebar): add Projects, Sort and Filter choices to the view menu"`

---

### Task 4: Drag a project header to reorder

**Files:**
- Modify: `crates/ui/src/shell.rs`:
  - `SidebarSectionDrag` `:1278`
  - new field next to `sidebar_section_bounds` `:2211` and its init `:2734`
  - regular-groups container near `:9915`
- Modify: `crates/ui/src/shell/sidebar_sections.rs`:
  - `begin_sidebar_section_drag` `:460`
  - `sidebar_section_drag_is_valid` `:590`
  - `finish_sidebar_section_drag` `:602`
  - `end_sidebar_section_drag` `:639`
- Modify: `crates/ui/src/shell/spaces.rs`: the group header around `:5755-5830`, which attaches `on_drag` when `ByProject`.
- Test: `crates/ui/src/shell/sidebar_sections.rs` tests.

**Interfaces:**
- Consumes: `moved_project_order` (Task 1) and `order_sidebar_groups` (Task 2).
- Produces:
  - `enum SidebarSectionDragKind { Custom, Project }`.
  - `SidebarSectionDrag.kind: SidebarSectionDragKind`. For projects, `section_id` is the project key and `ids` are the visible project keys in drawn order.
  - Field `sidebar_project_bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>`, filled by `on_children_prepainted` on the regular-groups container exactly as `sidebar_section_bounds` is.
  - `fn move_sidebar_project(&mut self, ids: &[String], from: usize, over: usize, cx: &mut Context<Self>)`. It sets `space_order = moved_project_order(..)`, sets `sidebar_project_order = Manual` and calls `schedule_save(cx)`.

Behavior:
- `begin` reads bounds by kind.
- `is_valid` for `Project` requires `ByProject` and an unchanged set of visible project keys equal to `payload.ids`.
- `finish` for `Project` calls `move_sidebar_project` when `from != over`. Otherwise it goes home without writing.
- `end` builds settle keys by kind: for projects, the key the group slot uses in `sidebar_resort`.
- The header drags only in `ByProject`, and only when there are 2 or more project groups.

- [ ] **Step 1: Write the failing test** `project_groups_reorder_by_dragging_their_header`. Model it on `sections_reorder_by_dragging_their_header` (`sidebar_sections.rs:1210`):
  - Set up ByProject with projects a, b, c, each with one chat, in Recent order `[c, b, a]`.
  - Drag the header of `c` below `a`. Assert `space_order == ["b","a","c"]`, `sidebar_project_order == Manual`, and the visible order follows.
  - Release a second drag outside the sidebar. Assert `space_order` is unchanged.
  - A no-op drop (drag 6px and release) leaves `sidebar_project_order` unchanged when it started as `Recent`.
- [ ] **Step 2: Run it and see it fail.** Command: `cargo test -p zeron-ui --lib project_groups_reorder`. Expected: FAIL (the header has no drag).
- [ ] **Step 3: Implement the kind, bounds, header drag and move.**
- [ ] **Step 4: Run the tests and see them pass.** Command: `cargo test -p zeron-ui --lib sections_ && cargo test -p zeron-ui --lib project_groups_reorder && cargo test -p zeron-ui --lib sidebar_`. Expected: PASS. The custom-section drag tests must still pass.
- [ ] **Step 5: Commit** with `git commit -m "feat(sidebar): reorder project groups by dragging their header"`

---

### Task 5: Visual check in the demo

- [ ] Run `scripts/dev-demo.sh` (isolated profile under `/tmp/zeron-demo-*`, mock harness).
- [ ] Switch to By project, then Projects › Recent. Send a message in the bottom project's chat; the group jumps to the top.
- [ ] Drag that group back down. The menu now reads Manual. Send another message: the group stays put. Restart the demo: the order persists.
- [ ] Check Sort › Status, Filter › Needs me and Filter › Active, with the open chat staying visible.
- [ ] Check Cmd+1…9 opens the rows in drawn order.
- [ ] Report the result, with a screenshot, to the user before any push.
