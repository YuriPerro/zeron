# Sidebar: manual project order, more sorts, status filter

Fork-only (`zeron-fork`). Device-local settings; no engine or proto changes.

## Problem

With **Organize: By project**, project groups are ordered by first appearance in
the recency-sorted chat list (`sidebar_visible_order`, `shell/spaces.rs`). The
project holding the newest chat jumps to the top, so projects move every time a
chat gets a message. There is no way to fix the order, and the sidebar offers
only two sorts and no status filter.

## Goals

1. Project groups keep an order the user chooses by dragging a group header.
2. Chats inside each project still sort by the selected sort.
3. Two more chat sorts: **Name** and **Status**.
4. A status filter: **All**, **Active**, **Needs me**.

Non-goals: syncing the project order across devices; filters by PR or device;
changing pins, custom sections or the By device / In one list layouts.

## Menu

"Sidebar view options" gains one group and two choices:

```
Organize  By project | By device | In one list      (existing)
Projects  Manual | Recent | A–Z                      new, only when By project
Sort      Last updated | Created | Name | Status     Name, Status new
Filter    All | Active | Needs me                    new
Show      ...                                        (existing)
```

All new choices are radios that close their submenu, like Organize and Sort.

## Settings (`crates/ui/src/settings.rs`)

- `SidebarProjectOrder { Recent (default), Manual, Name }` in
  `sidebar_project_order`. Default `Recent` keeps today's behavior for existing
  settings files.
- `space_order: Vec<String>` is read again. It holds project keys
  (`zeron_proto::view::project_key`, the same key `sidebar_project_group`
  returns), not space ids. Its doc comment drops the "legacy" note. Keys that
  match no visible group, including space ids saved by older builds, are ignored
  when ordering and kept on write, so a project that comes back finds its slot.
- `SidebarSort` gains `Name` and `Status`.
- `SidebarStatusFilter { All (default), Active, NeedsMe }` in
  `sidebar_status_filter`.

All fields use `#[serde(default)]`, so older settings files load unchanged.

## Ordering

One function computes the ordered, filtered sidebar projection, and both
`render_active_rows` and `sidebar_visible_order` use it. Today they compute the
order separately. Keyboard jumps (Cmd+1…9) and session cycling must match what
is drawn.

**Chat sort** (`compare_sidebar_chats`):
- `Name`: case-insensitive title, ties by last updated.
- `Status`: `AwaitingInput`, `Errored`, `Working`, `Completed`, `Idle`, ties by
  last updated. The comparator takes each chat's `ChatIndicator`, computed once
  per projection from the same source the row status word uses, not per
  comparison.

**Project groups** (By project only):
- `Recent`: today's first-appearance order.
- `Name`: case-insensitive project display name; "No project" groups last.
- `Manual`: groups in `space_order`. Groups missing from it go **on top** in
  recent order, because a new project is usually the one in use. A group stays in
  `space_order` while it has no visible chats, so its slot is kept when it
  reappears.

**Filter** runs before grouping:
- `Active` hides `Idle`.
- `NeedsMe` keeps `AwaitingInput` and `Errored`.
- Pinned chats and the open chat are always shown.
- A project group with no remaining chats is hidden.

Pins and custom sections keep their current positions above the groups.

## Drag to reorder projects

In By project mode a group header can be dragged vertically, following the
custom-section drag (`shell/sidebar_sections.rs`: lifted header, siblings slide,
landing slot from the header's leading edge).

On drop:
- The new order of the visible groups is written to `space_order`. Keys of
  hidden groups keep their relative positions.
- If the mode was `Recent` or `Name`, it switches to `Manual`, which makes the
  drop stick.
- A no-op drop animates home and writes nothing.
- Dragging is unavailable in By device and In one list.

Settings save through the existing debounced settings write.

## Testing

Unit tests in `shell/spaces.rs` (pure ordering) and gpui tests, following
`sidebar_project_groups_preserve_pin_order_and_keyboard_order`:

- Manual order holds when a chat in a lower project gets a newer message.
- An unknown project lands on top in Manual mode, and stale keys are ignored.
- Name and Status chat sorts, including the tie-breaks.
- Name project order puts "No project" last.
- `Active` and `NeedsMe` filters keep pins and the open chat.
- `sidebar_visible_order` equals the drawn order for every Projects × Sort ×
  Filter combination over a fixture.
- A header drop writes `space_order`, switches to Manual, and a no-op drop
  writes nothing.
- Settings round-trip, and an older settings file loads with the defaults.

## Risks

- `shell/spaces.rs` changes often upstream. Keeping the ordering in one new
  function, with small call-site edits, limits merge conflicts.
- With `Status` sort, rows move when a chat's status changes. That is the
  purpose of the sort. The existing resort glide animates the move.
