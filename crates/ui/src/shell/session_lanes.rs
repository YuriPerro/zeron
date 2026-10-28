//! The lanes card, stacked under the session card and shown with it: what
//! runs alongside the open chat — its **side chats** (forks, and chats an
//! agent spawned through the Zeron MCP server) and its **subagents** (the
//! spawn chips of its transcript, with their live status), then its
//! **background tasks**, each with a stop button. Each lane is a section of
//! session-card rows; clicking a row opens it in the right pane. Settled
//! subagents fold into one "finished" row, closed until opened.

use zeron_proto::{BackgroundTask, Chat, ChatIndicator};

use super::session_info::SESSION_CARD_WIDTH;
use crate::running_work::{
    STOP_SIZE, SubagentRow, background_stop_button, background_task_title, elapsed_label,
    stop_background_task, subagent_rows,
};
use super::*;

/// Rows a lane shows before "Show more" pages it, and the page size.
const INITIAL_ROWS: usize = 8;
const PAGE_ROWS: usize = 10;
/// A background row's stop button sits as far from the row's top, bottom
/// and trailing edge, so its corners are concentric with the row's.
const STOP_INSET: f32 = (popover::PICKER_ROW_HEIGHT - STOP_SIZE) / 2.0;

/// What the lanes card has unfolded or paged, and where it has scrolled; it
/// follows the open chat.
#[derive(Default)]
pub(super) struct LaneState {
    chat_id: String,
    finished_open: bool,
    shown_chats: usize,
    shown_finished: usize,
    scroll: gpui::ScrollHandle,
    /// The card's floating scrollbar, the file tree's treatment.
    bar: popover::MenuScrollbarState,
}

impl LaneState {
    /// Folds and pages are per chat: another chat starts from rest.
    fn follow(&mut self, chat_id: &str) {
        if self.chat_id != chat_id {
            *self = Self {
                chat_id: chat_id.to_owned(),
                ..Self::default()
            };
        }
    }

    fn shown(count: usize) -> usize {
        count.max(INITIAL_ROWS)
    }

    /// Page the side-chat lane until its `index`th branch shows.
    pub(super) fn reveal_chat(&mut self, chat_id: &str, index: usize) {
        self.follow(chat_id);
        if index >= Self::shown(self.shown_chats) {
            self.shown_chats =
                INITIAL_ROWS + (index + 1 - INITIAL_ROWS).div_ceil(PAGE_ROWS) * PAGE_ROWS;
        }
    }
}

/// The subagent lane: what has not settled rides it, the rest folds away.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SubagentLane {
    /// Running first, then any row whose status is not stamped yet.
    pub active: Vec<SubagentRow>,
    /// Done and failed, most recently updated first.
    pub finished: Vec<SubagentRow>,
}

impl SubagentLane {
    pub fn new(rows: Vec<SubagentRow>) -> Self {
        let (finished, active) = rows.into_iter().partition(SubagentRow::frozen);
        Self { active, finished }
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_empty() && self.finished.is_empty()
    }
}

/// A side chat of the open chat, as its lane lists it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ChildChatRow {
    pub chat_id: String,
    pub device_id: String,
    pub title: SharedString,
    pub status: ChatIndicator,
    pub time_ago: SharedString,
    /// The chat's linked pull request, drawn as the sidebar's badge.
    pub change_request: Option<zeron_proto::ChangeRequestSummary>,
    /// The harness it runs on, drawn as the sidebar's brand mark.
    pub harness: Option<zeron_proto::HarnessId>,
    activity: DateTime<Utc>,
}

/// The live (unarchived) children of `chat_id`, most recent activity first —
/// the same order the sidebar's Sessions list keeps.
pub(super) fn child_chat_rows(
    state: &AppState,
    chat_id: &str,
    now: DateTime<Utc>,
) -> Vec<ChildChatRow> {
    let mut rows: Vec<ChildChatRow> = state
        .chats
        .iter()
        .filter(|chat| !chat.archived && chat.parent_chat_id.as_deref() == Some(chat_id))
        .map(|chat| {
            let activity = chat.last_message_at.unwrap_or(chat.created_at);
            ChildChatRow {
                chat_id: chat.id.clone(),
                device_id: chat.device_id.clone(),
                title: child_chat_title(chat).into(),
                status: state.display_status_for(chat, now),
                time_ago: zeron_proto::view::format_time_ago(activity, now).into(),
                change_request: state.change_request_for_chat(chat).cloned(),
                harness: chat.config.as_ref().map(|config| config.harness),
                activity,
            }
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.activity));
    rows
}

/// A side chat titles itself on its first turn; until then the preview or a
/// placeholder stands in.
pub(super) fn child_chat_title(chat: &Chat) -> String {
    chat.title
        .clone()
        .or_else(|| chat.last_message_preview.clone())
        .unwrap_or_else(|| "New side chat".into())
}

/// A status mark in a row's icon slot: Working spins, Completed wears the
/// check, the rest a dot in the status color.
fn status_mark(
    key: String,
    status: ChatIndicator,
    view: gpui::EntityId,
    theme: &Theme,
    cx: &mut App,
) -> AnyElement {
    let color = spaces::status_dot_color(status, theme);
    match status {
        ChatIndicator::Completed => icon(icons::CHECK)
            .size(px(12.0))
            .text_color(color)
            .into_any_element(),
        ChatIndicator::Working => {
            loaders::mini_glyph_spinner(format!("{key}-working"), 2.0, theme.glyph, view, cx)
                .into_any_element()
        }
        _ => div()
            .size(px(6.0))
            .rounded_full()
            .bg(color)
            .into_any_element(),
    }
}

fn glyph(path: &'static str, theme: &Theme) -> AnyElement {
    icon(path)
        .size(px(15.0))
        .text_color(theme.text_muted)
        .into_any_element()
}

/// The harness a row runs on, as the sidebar's brand mark.
fn harness_mark(harness: zeron_proto::HarnessId, theme: &Theme) -> gpui::Svg {
    let (path, tint) = crate::pickers::harness_brand_icon(harness);
    icon(path)
        .size(px(13.0))
        .flex_none()
        .text_color(tint.unwrap_or(theme.text_muted).opacity(0.8))
}

/// A row's trailing time, in the muted subline size.
fn time_label(text: SharedString, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .text_size(crate::typography::ui_rems(11.0))
        .text_color(theme.text_muted)
        .child(text)
}

/// A row's title, dissolving at its right edge like the sidebar's.
fn row_title(id: String, title: SharedString) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .child(sidebar_faded_label(
            id.into(),
            true,
            div().line_height(px(17.0)).child(title),
        ))
        .into_any_element()
}

impl Shell {
    /// Show the card with `chat_id`'s side-chat row paged in, so an inline
    /// rename has its row on screen; false when it is not one of the open
    /// chat's live side chats.
    pub(super) fn reveal_session_lane_chat(
        &mut self,
        chat_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let parent = self.active_chat.clone();
        let rows = child_chat_rows(self.state.read(cx), &parent, Utc::now());
        let Some(index) = rows.iter().position(|row| row.chat_id == chat_id) else {
            return false;
        };
        self.session_lanes.reveal_chat(&parent, index);
        // Takeover hides the transcript column the card lives in; a rename
        // field there would take keystrokes unseen.
        if self.right_pane_expanded {
            self.toggle_right_pane_expand(cx);
        }
        self.show_session_info(cx);
        true
    }

    /// Reveal the editor using the laid-out row, including while the card
    /// opens. Paging alone can leave the focused field below the viewport.
    fn keep_session_lane_rename_in_view(
        &mut self,
        row: gpui::Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !self.chat_rename_revealing() {
            return;
        }
        let scroll = &self.session_lanes.scroll;
        let viewport = scroll.bounds();
        let inset = px(popover::CARD_INSET);
        let delta = if row.top() < viewport.top() + inset {
            row.top() - (viewport.top() + inset)
        } else if row.bottom() > viewport.bottom() - inset {
            row.bottom() - (viewport.bottom() - inset)
        } else {
            return;
        };
        let offset = scroll.offset();
        let y = (offset.y - delta).clamp(-scroll.max_offset().y, px(0.0));
        if y != offset.y {
            scroll.set_offset(gpui::point(offset.x, y));
            cx.notify();
        }
    }

    /// The lanes card for the open chat, at most `max_height` tall and
    /// scrolling past it; `floating` over the transcript, it casts the
    /// popover's shadow.
    pub(super) fn render_session_lanes(
        &mut self,
        chat: &Chat,
        floating: bool,
        max_height: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Utc::now();
        self.session_lanes.follow(&chat.id);
        let (side_chats, subagents, tasks) = {
            let state = self.state.read(cx);
            (
                child_chat_rows(state, &chat.id, now),
                SubagentLane::new(subagent_rows(state, &chat.id)),
                state.background_tasks_for(&chat.id, now).to_vec(),
            )
        };
        let view = cx.entity_id();
        // Subagents run on the session's own harness.
        let harness = chat.config.as_ref().map(|config| config.harness);
        // ── The two ways to start a side chat: rows while the lane is empty,
        // then icon buttons on the heading so the branches lead. A fork in
        // flight shows in its own tab and never blocks another.
        let compact = !side_chats.is_empty();
        let mut heading = div()
            .flex()
            .items_center()
            .child(div().flex_1().child(popover::menu_heading(theme, "Side chats")));
        let mut starters: Vec<AnyElement> = Vec::new();
        for (id, path, label, fork) in [
            (
                "session-info-side-chat",
                icons::PLUS,
                "New side chat",
                false,
            ),
            ("session-info-fork", icons::FORK, "Fork chat", true),
        ] {
            let start = cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.close_session_info_overlay(cx);
                if fork {
                    this.create_side_chat(cx);
                } else {
                    this.create_child_chat(None, cx);
                }
            });
            if compact {
                heading = heading.child(
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .size(px(22.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .hover(|style| style.bg(crate::theme::ink(0.08)))
                        .tooltip(crate::settings::widgets::text_tooltip(label))
                        .on_click(start)
                        .child(icon(path).size(px(14.0)).text_color(theme.text_muted)),
                );
            } else {
                starters.push(
                    popover::picker_row(theme, false, false, id)
                        .id(id)
                        .debug_selector(move || id.into())
                        .on_click(start)
                        .child(popover::picker_icon_slot(glyph(path, theme)))
                        .child(div().flex_1().child(label))
                        .into_any_element(),
                );
            }
        }
        let mut rows: Vec<AnyElement> = vec![heading.pr(px(2.0)).into_any_element()];
        rows.extend(starters);

        // ── Side chats.
        let shown = LaneState::shown(self.session_lanes.shown_chats);
        let hidden = side_chats.len().saturating_sub(shown);
        for row in side_chats.into_iter().take(shown) {
            rows.push(
                self.render_side_chat_row(row, view, theme, cx)
                    .into_any_element(),
            );
        }
        if hidden > 0 {
            rows.push(self.render_show_more(
                "session-lanes-chats-more",
                hidden,
                theme,
                cx,
                |lanes| {
                    lanes.shown_chats = LaneState::shown(lanes.shown_chats) + PAGE_ROWS;
                },
            ));
        }
        // ── Subagents: what runs rides the lane, the settled fold away.
        if !subagents.is_empty() {
            rows.push(popover::picker_divider().into_any_element());
            rows.push(popover::menu_heading(theme, "Subagents").into_any_element());
            let SubagentLane { active, finished } = subagents;
            let open = self.session_lanes.finished_open;
            let shown = LaneState::shown(self.session_lanes.shown_finished);
            let hidden = if open {
                finished.len().saturating_sub(shown)
            } else {
                0
            };
            let folded = finished.len();
            let finished: Vec<_> = if open {
                finished.into_iter().take(shown).collect()
            } else {
                Vec::new()
            };
            for row in active {
                rows.push(
                    self.render_subagent_row(row, harness, view, theme, cx)
                        .into_any_element(),
                );
            }
            if folded > 0 {
                rows.push(
                    self.render_finished_fold(folded, open, theme, cx)
                        .into_any_element(),
                );
            }
            for row in finished {
                rows.push(
                    self.render_subagent_row(row, harness, view, theme, cx)
                        .into_any_element(),
                );
            }
            if hidden > 0 {
                rows.push(self.render_show_more(
                    "session-lanes-finished-more",
                    hidden,
                    theme,
                    cx,
                    |lanes| {
                        lanes.shown_finished = LaneState::shown(lanes.shown_finished) + PAGE_ROWS;
                    },
                ));
            }
        }
        // ── Background tasks, each stoppable from its row.
        if !tasks.is_empty() {
            rows.push(popover::picker_divider().into_any_element());
            rows.push(
                popover::menu_heading(theme, "Background")
                    .debug_selector(|| "session-lanes-background".into())
                    .into_any_element(),
            );
            for task in &tasks {
                rows.push(
                    self.render_background_row(&chat.id, task, now, theme, cx)
                        .into_any_element(),
                );
            }
            // Elapsed times only move with the clock: while the lane shows
            // them, the card re-renders once a second.
            self.clock.arm(
                now + chrono::TimeDelta::seconds(1),
                now,
                |shell: &mut Shell| &mut shell.clock,
                cx,
            );
        }

        let body = div()
            .id("session-lanes-body")
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.session_lanes.scroll)
            // The inset lives inside the scroller, so dividers bleed edge to
            // edge as they do in the session card.
            .p(px(popover::CARD_INSET))
            .flex()
            .flex_col()
            .gap(px(popover::MENU_GAP))
            .children(rows);
        // The rail shows only once the lanes outgrow the card and scroll.
        let scrollbar = popover::rail_for(
            self,
            popover::RailAccess {
                bar: |this: &mut Self| &mut this.session_lanes.bar,
                scroll: |this: &Self| this.session_lanes.scroll.clone(),
            },
            "session-lanes-scrollbar",
            theme,
            cx,
        );
        div()
            .id("session-lanes")
            .debug_selector(|| "session-lanes".into())
            .relative()
            .w(px(SESSION_CARD_WIDTH))
            .max_h(px(max_height))
            .min_h_0()
            .flex()
            .flex_col()
            .rounded(px(popover::CARD_RADIUS))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(popover::surface_bg(theme))
            .when(floating && !theme.is_frost(), |card| card.shadow_lg())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.session_lanes.bar.set_list_hovered(*hovered) {
                    cx.notify();
                }
            }))
            .child(body)
            .children(scrollbar)
            .into_any_element()
    }

    fn render_side_chat_row(
        &mut self,
        row: ChildChatRow,
        view: gpui::EntityId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let key = format!("session-lanes-chat-{}", row.chat_id);
        let mark = status_mark(key.clone(), row.status, view, theme, cx);
        let rename_input = self.rename_input_for(&row.chat_id, ChatRenameSurface::SessionCard);
        let revealing = rename_input.is_some() && self.chat_rename_revealing();
        let title = match rename_input {
            Some(input) => chat_title_editor(format!("{key}-title-editor").into(), input, theme),
            None => row_title(format!("{key}-title"), row.title.clone()),
        };
        let open_id = row.chat_id.clone();
        let menu_id = row.chat_id.clone();
        popover::picker_row(theme, false, false, key.clone())
            .id(SharedString::from(key.clone()))
            .aria_label(SharedString::from(format!("Open side chat {}", row.title)))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.close_session_info_overlay(cx);
                this.open_child_chat_tab(&open_id, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.chat_menu.open(ChatMenuState {
                        chat_id: menu_id.clone(),
                        tab: None,
                        position: event.position,
                        page: ChatMenuPage::Root,
                    });
                    cx.notify();
                }),
            )
            .child(popover::picker_icon_slot(mark))
            // The harness mark leads the title, as on the sidebar's rows.
            .children(row.harness.map(|harness| harness_mark(harness, theme)))
            .child(title)
            .when(revealing, |row| {
                let shell = cx.weak_entity();
                row.child(
                    gpui::canvas(
                        |_, _, _| {},
                        move |bounds, _, window, cx| {
                            window.defer(cx, move |_, cx| {
                                shell
                                    .update(cx, |this, cx| {
                                        this.keep_session_lane_rename_in_view(bounds, cx)
                                    })
                                    .ok();
                            });
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
            })
            .children(row.change_request.clone().map(|summary| {
                crate::change_requests::pull_request_badge(
                    format!("{key}-pr").into(),
                    summary,
                    crate::change_requests::ChangeRequestBadgeSurface::Sidebar,
                    Some(row.device_id.clone()),
                    theme,
                )
            }))
            .child(time_label(row.time_ago.clone(), theme))
    }

    fn render_subagent_row(
        &mut self,
        row: SubagentRow,
        harness: Option<zeron_proto::HarnessId>,
        view: gpui::EntityId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let key = format!("session-lanes-subagent-{}", row.doc_id);
        let mark = status_mark(key.clone(), row.indicator(), view, theme, cx);
        let time = zeron_proto::view::format_time_ago(row.spawned_at, Utc::now());
        let title = row_title(format!("{key}-title"), row.title.clone());
        popover::picker_row(theme, false, false, key.clone())
            .id(SharedString::from(key))
            .aria_label(SharedString::from(format!("Open subagent {}", row.title)))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.close_session_info_overlay(cx);
                this.add_subagent_surface(
                    this.active_chat.clone(),
                    row.doc_id.clone(),
                    row.title.to_string(),
                    row.frozen(),
                    cx,
                );
            }))
            .child(popover::picker_icon_slot(mark))
            .children(harness.map(|harness| harness_mark(harness, theme)))
            .child(title)
            .child(time_label(time.into(), theme))
    }

    /// A background task: the loader, what it runs, its elapsed time and the
    /// stop button. While its stop is in flight the row reads `Stopping…` and
    /// the button is inert.
    fn render_background_row(
        &mut self,
        chat_id: &str,
        task: &BackgroundTask,
        now: DateTime<Utc>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let key = format!("session-lanes-task-{}", task.id);
        let stopping = self
            .state
            .read(cx)
            .background_task_stopping(chat_id, &task.id);
        let loader = loaders::halo_loader(format!("{key}-loader"), 2.0, theme.busy);
        let title = row_title(format!("{key}-title"), background_task_title(task));
        let time = if stopping {
            time_label("Stopping…".into(), theme).text_color(theme.danger)
        } else {
            time_label(elapsed_label(task.started_at, now), theme)
        };
        let state = self.state.downgrade();
        let (chat_id, task_id) = (chat_id.to_owned(), task.id.clone());
        let stop = background_stop_button(
            &key,
            stopping,
            popover::MENU_ITEM_RADIUS - STOP_INSET,
            theme,
            move |cx| {
                if let Some(state) = state.upgrade() {
                    stop_background_task(&state, chat_id.clone(), task_id.clone(), cx)
                }
            },
        );
        popover::picker_row(theme, false, false, key.clone())
            .id(SharedString::from(key.clone()))
            .debug_selector({
                let key = key.clone();
                move || key
            })
            .cursor(gpui::CursorStyle::Arrow)
            .pr(px(STOP_INSET))
            .child(popover::picker_icon_slot(loader.into_any_element()))
            .child(title)
            .child(time)
            .child(stop)
    }

    /// The settled subagents' fold, with their count.
    fn render_finished_fold(
        &mut self,
        count: usize,
        open: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let chevron = icon(icons::ALT_ARROW_RIGHT)
            .size(px(12.0))
            .text_color(theme.text_muted)
            .with_transformation(gpui::Transformation::rotate(gpui::percentage(if open {
                0.25
            } else {
                0.0
            })));
        popover::picker_row(theme, false, false, "session-lanes-finished")
            .id("session-lanes-finished")
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(format!(
                "{} {count} finished subagents",
                if open { "Collapse" } else { "Expand" }
            )))
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.session_lanes.finished_open = !this.session_lanes.finished_open;
                cx.notify();
            }))
            .child(popover::picker_icon_slot(chevron.into_any_element()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(format!("{count} finished"))),
            )
    }

    /// "Show N more" under a paged lane, its label lined up with the rows'.
    fn render_show_more(
        &mut self,
        id: &'static str,
        remaining: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
        page: fn(&mut LaneState),
    ) -> AnyElement {
        popover::picker_row(theme, false, false, id)
            .id(id)
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                page(&mut this.session_lanes);
                cx.notify();
            }))
            .child(popover::picker_icon_slot(div().into_any_element()))
            .child(
                div()
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child(format!("Show {} more", remaining.min(PAGE_ROWS))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::panel_interaction_tests::{settled, setup};

    fn chat(id: &str, parent: Option<&str>, minutes_ago: i64) -> Chat {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "deviceId": "dev",
            "archived": false,
            "createdAt": Utc::now() - chrono::Duration::minutes(minutes_ago),
            "parentChatId": parent,
        }))
        .unwrap()
    }

    #[test]
    fn child_chat_rows_are_live_children_newest_first() {
        let mut state = AppState::new();
        let mut archived = chat("old", Some("main"), 1);
        archived.archived = true;
        let mut titled = chat("b", Some("main"), 30);
        titled.title = Some("Investigate caching".into());
        state.apply_chats(vec![
            chat("main", None, 60),
            chat("a", Some("main"), 5),
            titled,
            chat("unrelated", Some("elsewhere"), 2),
            archived,
        ]);
        let rows = child_chat_rows(&state, "main", Utc::now());
        assert_eq!(
            rows.iter().map(|r| r.chat_id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(rows[0].title.as_ref(), "New side chat");
        assert_eq!(rows[1].title.as_ref(), "Investigate caching");
        assert_eq!(rows[0].status, ChatIndicator::Idle);
    }

    #[test]
    fn revealing_a_side_chat_pages_its_lane_in_steps() {
        let mut lanes = LaneState::default();
        lanes.reveal_chat("main", 3);
        assert_eq!(LaneState::shown(lanes.shown_chats), INITIAL_ROWS);
        lanes.reveal_chat("main", INITIAL_ROWS);
        assert_eq!(
            LaneState::shown(lanes.shown_chats),
            INITIAL_ROWS + PAGE_ROWS
        );
        // Another chat's lanes start from rest.
        lanes.finished_open = true;
        lanes.reveal_chat("other", 0);
        assert!(!lanes.finished_open);
        assert_eq!(LaneState::shown(lanes.shown_chats), INITIAL_ROWS);
    }

    #[gpui::test]
    fn background_lane_shows_only_with_tasks(cx: &mut gpui::TestAppContext) {
        let (_dir, shell, cx) = setup(cx);
        cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show_session_info(cx)));
        settled(&shell, cx);
        assert!(cx.debug_bounds("session-lanes").is_some());
        assert!(cx.debug_bounds("session-lanes-background").is_none());

        cx.update(|_, cx| {
            shell.update(cx, |shell, cx| {
                shell.state.update(cx, |state, cx| {
                    state.sessions = vec![zeron_proto::Session {
                        last_turn_outcome: None,
                        terminal_activity: None,
                        last_completed_turn: None,
                        chat_id: "a".into(),
                        device_id: "local".into(),
                        status: zeron_proto::SessionStatus::Working,
                        started_at: Some(Utc::now()),
                        updated_at: Utc::now(),
                        running_subagents: 0,
                        background_tasks: vec![zeron_proto::BackgroundTask {
                            id: "bg-1".into(),
                            kind: zeron_proto::BackgroundTaskKind::Shell,
                            description: "watch the build".into(),
                            started_at: Utc::now() - chrono::Duration::seconds(41),
                        }],
                    }];
                    cx.notify();
                })
            })
        });
        settled(&shell, cx);
        assert!(cx.debug_bounds("session-lanes-background").is_some());
        assert!(cx.debug_bounds("session-lanes-task-bg-1").is_some());
        let title = shell.read_with(cx, |shell, cx| {
            let state = shell.state.read(cx);
            background_task_title(&state.background_tasks_for("a", Utc::now())[0])
        });
        assert_eq!(title.as_ref(), "watch the build");
    }
}
