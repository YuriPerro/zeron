//! The work running under the open chat, docked above the composer as two
//! pills: its running subagents and its background tasks. A pill opens its
//! tray, which slides out from behind what follows it (the checklist, the
//! queue or the composer itself): the same glass and stack as those trays.
//!
//! A pill's count is its tray's length, so the two never disagree; a pill
//! with nothing to list is absent, and its open tray closes with it.

use std::collections::HashSet;
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div, prelude::*, px,
};
use zeron_proto::BackgroundTask;
use zeron_rpc::methods;

use crate::composer::{Composer, ComposerEvent, QUEUE_COMPOSER_OVERLAP};
use crate::icons::{self, icon};
use crate::motion;
use crate::queue::{PANEL_BORDER, PANEL_PAD_TOP, PANEL_RADIUS, ROW_RADIUS};
use crate::running_work::{SubagentRow, elapsed_label, running_subagent_rows};
use crate::state::clock_now;
use crate::theme::Theme;

/// One of the two pills, and the tray it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunningTray {
    Subagents,
    Background,
}

/// Per-chat presentation state. In memory only, like the checklist's.
#[derive(Debug, Default)]
pub(crate) struct RunningTrayState {
    /// The open tray; opening one closes the other.
    open: Option<RunningTray>,
    /// Background tasks whose stop is in flight. An id leaves once its task
    /// leaves the chat's set, or when the stop fails.
    stopping: HashSet<String>,
}

impl RunningTrayState {
    fn toggle(&mut self, tray: RunningTray) {
        self.open = (self.open != Some(tray)).then_some(tray);
    }
}

const PILL_HEIGHT: f32 = 24.0;
/// The pill row sits a little in from the composer's edge.
const PILL_ROW_INSET: f32 = 6.0;
/// One step narrower than the tray it stacks on; each tray below (the
/// checklist, the queue) steps it in once more.
const SIDE_INSET: f32 = 16.0;
/// Between the pill row and an open tray.
const TRAY_GAP: f32 = 6.0;
const ROW_HEIGHT: f32 = 28.0;
const ROW_PAD_X: f32 = 8.0;
const TEXT_SIZE: f32 = 12.5;
const META_SIZE: f32 = 11.0;
/// Loader dot: a 14px grid, the width of the checklist's glyph slot.
const LOADER_DOT: f32 = 2.0;
/// The stop button sits as far from the row's top, bottom and trailing edge,
/// so its corners are concentric with the row's.
const STOP_SIZE: f32 = 20.0;
const STOP_INSET: f32 = (ROW_HEIGHT - STOP_SIZE) / 2.0;
const STOP_SQUARE: f32 = 7.0;

impl Composer {
    /// The pills that render for the selected chat, left to right: one per
    /// non-empty list.
    pub(crate) fn running_pills(&self, cx: &App) -> Vec<RunningTray> {
        let (subagents, tasks) = self.running_work(cx);
        [
            (!subagents.is_empty()).then_some(RunningTray::Subagents),
            (!tasks.is_empty()).then_some(RunningTray::Background),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn running_work(&self, cx: &App) -> (Vec<SubagentRow>, Vec<BackgroundTask>) {
        let state = self.state.read(cx);
        let Some(chat_id) = state.selected_chat.as_deref() else {
            return Default::default();
        };
        (
            running_subagent_rows(state, chat_id),
            state.background_tasks_for(chat_id, clock_now()).to_vec(),
        )
    }

    /// A pill was clicked: toggle its tray on the selected chat.
    pub(crate) fn toggle_running_tray(&mut self, tray: RunningTray, cx: &mut Context<Self>) {
        let Some(chat_id) = self.state.read(cx).selected_chat.clone() else {
            return;
        };
        self.running_trays.entry(chat_id).or_default().toggle(tray);
        cx.notify();
    }

    /// The pills and the open tray, or `None` when nothing runs.
    /// `trays_below`: how many trays (checklist, queue) follow it.
    pub(crate) fn render_running_trays(
        &mut self,
        trays_below: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let chat_id = self.state.read(cx).selected_chat.clone()?;
        let (subagents, tasks) = self.running_work(cx);
        let entry = self.running_trays.entry(chat_id.clone()).or_default();
        entry
            .stopping
            .retain(|id| tasks.iter().any(|task| task.id == *id));
        let len = |tray: RunningTray| match tray {
            RunningTray::Subagents => subagents.len(),
            RunningTray::Background => tasks.len(),
        };
        if entry.open.is_some_and(|tray| len(tray) == 0) {
            entry.open = None;
        }
        let pills = self.running_pills(cx);
        if pills.is_empty() {
            return None;
        }
        let entry = &self.running_trays[&chat_id];
        let open = entry.open;
        let stopping = entry.stopping.clone();

        let theme = Theme::of(cx).clone();
        let reduced = motion::reduced_motion(cx);
        let view = cx.entity_id();
        let now = clock_now();
        let pill_row = div()
            .mx(px(PILL_ROW_INSET))
            .flex()
            .items_center()
            .gap(px(6.0))
            .children(pills.iter().map(|&tray| {
                self.running_pill(
                    &chat_id,
                    tray,
                    len(tray),
                    open == Some(tray),
                    reduced,
                    &theme,
                    cx,
                )
            }));

        // Each tray rides its own morph, so switching pills closes one while
        // the other opens.
        let inset = SIDE_INSET * (1 + trays_below) as f32;
        let max_rows = f32::from(window.viewport_size().height) * 0.3;
        let mut any_revealed = false;
        let trays = pills.iter().filter_map(|&tray| {
            let key = format!("running-tray-{view}-{chat_id}-{tray:?}");
            let reveal = motion::state_t(&key, open == Some(tray), motion::COLLAPSE, reduced);
            if reveal <= 0.0 {
                return None;
            }
            any_revealed = true;
            let rows: Vec<AnyElement> = match tray {
                RunningTray::Subagents => subagents
                    .iter()
                    .map(|row| self.subagent_tray_row(&chat_id, row, now, &theme, cx))
                    .collect(),
                RunningTray::Background => tasks
                    .iter()
                    .map(|task| {
                        let stopping = stopping.contains(&task.id);
                        self.background_tray_row(&chat_id, task, stopping, now, &theme, cx)
                    })
                    .collect(),
            };
            let rows_height = (rows.len() as f32 * ROW_HEIGHT).min(max_rows);
            let full =
                2.0 * PANEL_BORDER + 2.0 * PANEL_PAD_TOP + QUEUE_COMPOSER_OVERLAP + rows_height;
            let surface = crate::queue::queue_panel_surface(&theme, cx.has_active_drag())
                .h(px(full * reveal))
                .overflow_hidden()
                .child(
                    div()
                        .id(SharedString::from(format!("running-tray-rows-{tray:?}")))
                        .flex_none()
                        .h(px(rows_height))
                        .overflow_y_scroll()
                        .children(rows),
                );
            Some(
                div()
                    .mx(px(inset - PILL_ROW_INSET))
                    .mt(px(TRAY_GAP * reveal))
                    // Cancel the column gap and tuck the tray behind what
                    // follows, in step with its height.
                    .mb(px(-(Theme::SPACE_SM + QUEUE_COMPOSER_OVERLAP) * reveal))
                    .opacity(reveal)
                    .child(crate::frost::frosted(
                        PANEL_RADIUS,
                        crate::frost::MENU_BLUR,
                        surface,
                    )),
            )
        });
        let trays: Vec<_> = trays.collect();
        if any_revealed {
            self.tick_running_trays(cx);
        }
        Some(
            div()
                .flex()
                .flex_col()
                .child(pill_row)
                .children(trays)
                .into_any_element(),
        )
    }

    /// Elapsed times only move with the clock: while a tray shows them,
    /// re-render once a second.
    fn tick_running_trays(&mut self, cx: &mut Context<Self>) {
        if self.running_tick.is_some() {
            return;
        }
        self.running_tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            this.update(cx, |composer, cx| {
                composer.running_tick = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// `● 2 subagents ⌃`: the loader, the count, the noun and a chevron that
    /// turns over while the tray is open.
    #[allow(clippy::too_many_arguments)]
    fn running_pill(
        &self,
        chat_id: &str,
        tray: RunningTray,
        count: usize,
        open: bool,
        reduced: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = cx.entity_id();
        let (key, noun) = match tray {
            RunningTray::Subagents => ("running-pill-subagents", "subagents"),
            RunningTray::Background => ("running-pill-background", "background"),
        };
        let loader = match tray {
            RunningTray::Subagents => {
                crate::loaders::echo_loader(format!("{key}-{view}"), LOADER_DOT, theme.busy)
                    .into_any_element()
            }
            RunningTray::Background => {
                crate::loaders::halo_loader(format!("{key}-{view}"), LOADER_DOT, theme.busy)
                    .into_any_element()
            }
        };
        let turn = motion::state_t(
            &format!("{key}-chevron-{view}-{chat_id}"),
            open,
            motion::CHEVRON,
            reduced,
        );
        let accent = theme.accent;
        let hover = theme.element_hover;
        let radius = PILL_HEIGHT / 2.0;
        // Inside the glass's border, concentric with it.
        let inner_radius = radius - PANEL_BORDER;
        let pill = div()
            .id(key)
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(format!("{count} {noun}")))
            .aria_expanded(open)
            .h(px(PILL_HEIGHT - 2.0 * PANEL_BORDER))
            .pl(px(8.0))
            .pr(px(10.0))
            .rounded(px(inner_radius))
            .flex()
            .items_center()
            .gap(px(7.0))
            .cursor_pointer()
            .when(open, |el| el.bg(hover))
            .hover(move |s| s.bg(hover))
            .tab_index(0)
            .focus_visible(move |s| s.bg(accent.opacity(0.14)))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_running_tray(tray, cx)))
            .child(loader)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(5.0))
                    .text_size(px(12.0))
                    .child(
                        div()
                            .font_family(theme.font_mono.clone())
                            .text_size(px(11.5))
                            .text_color(theme.text)
                            .child(SharedString::from(count.to_string())),
                    )
                    .child(div().text_color(theme.text_muted).child(noun)),
            )
            .child(
                icon(icons::ALT_ARROW_UP)
                    .size(px(11.0))
                    .text_color(theme.text_faint)
                    .with_transformation(gpui::Transformation::rotate(gpui::percentage(
                        0.5 * turn,
                    ))),
            );
        crate::frost::frosted(
            radius,
            crate::frost::MENU_BLUR,
            div()
                .occlude()
                .rounded(px(radius))
                .bg(crate::queue::tray_glass(theme))
                .border_1()
                .border_color(theme.border)
                .when(!theme.is_frost(), |el| el.shadow_sm())
                .child(pill),
        )
        .into_any_element()
    }

    /// A running subagent: the loader, its task and `model · elapsed`. A click
    /// opens its transcript.
    fn subagent_tray_row(
        &self,
        chat_id: &str,
        row: &SubagentRow,
        now: chrono::DateTime<chrono::Utc>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let elapsed = elapsed_label(row.spawned_at, now);
        let meta: SharedString = match &row.model {
            Some(model) => format!("{model} · {elapsed}").into(),
            None => elapsed,
        };
        let accent = theme.accent;
        let event = ComposerEvent::OpenSubagent {
            chat_id: chat_id.to_owned(),
            doc_id: row.doc_id.clone(),
            title: row.title.to_string(),
            frozen: false,
        };
        tray_row(format!("running-subagent-{}", row.doc_id))
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(format!("Open subagent {}", row.title)))
            .cursor_pointer()
            .hover(|s| s.bg(theme.element_hover))
            .tab_index(0)
            .focus_visible(move |s| s.bg(accent.opacity(0.14)))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(event.clone())))
            .child(crate::loaders::echo_loader(
                format!("running-subagent-{}-loader", row.doc_id),
                LOADER_DOT,
                theme.busy,
            ))
            .child(row_title(row.title.clone(), theme))
            .child(row_meta(meta, theme.text_faint, theme))
            .into_any_element()
    }

    /// A background task: the loader, what it runs, its elapsed time and the
    /// stop button. While its stop is in flight the row reads `Stopping…` and
    /// the button is inert.
    fn background_tray_row(
        &self,
        chat_id: &str,
        task: &BackgroundTask,
        stopping: bool,
        now: chrono::DateTime<chrono::Utc>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title: SharedString = if task.description.is_empty() {
            task.id.clone().into()
        } else {
            task.description.clone().into()
        };
        let meta = if stopping {
            row_meta("Stopping…".into(), theme.danger, theme)
        } else {
            row_meta(elapsed_label(task.started_at, now), theme.text_faint, theme)
        };
        let key = format!("running-task-{}", task.id);
        let danger = theme.danger;
        let accent = theme.accent;
        let (chat_id, task_id) = (chat_id.to_owned(), task.id.clone());
        let stop = div()
            .id(SharedString::from(format!("{key}-stop")))
            .role(gpui::Role::Button)
            .aria_label("Stop task")
            .size(px(STOP_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(ROW_RADIUS - STOP_INSET))
            .bg(danger.opacity(0.14))
            .when(stopping, |el| {
                el.cursor(gpui::CursorStyle::Arrow).opacity(0.45)
            })
            .when(!stopping, |el| {
                el.cursor_pointer()
                    .hover(move |s| s.bg(danger.opacity(0.30)))
                    .tab_index(0)
                    .focus_visible(move |s| s.bg(accent.opacity(0.18)))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.stop_running_task(chat_id.clone(), task_id.clone(), cx);
                    }))
            })
            .tooltip(crate::settings::widgets::text_tooltip("Stop task"))
            .tooltip_show_delay(Duration::from_millis(350))
            .child(div().size(px(STOP_SQUARE)).rounded(px(1.5)).bg(danger));
        tray_row(key.clone())
            .pr(px(STOP_INSET))
            .child(crate::loaders::halo_loader(
                format!("{key}-loader"),
                LOADER_DOT,
                theme.busy,
            ))
            .child(row_title(title, theme))
            .child(meta)
            .child(stop)
            .into_any_element()
    }

    /// Ask the chat's host to stop one of its background tasks. The row is
    /// marked until the task leaves the set; a refusal clears the mark, and
    /// says so only while the task still runs — one that ended on its own in
    /// the meantime needs no toast.
    pub(crate) fn stop_running_task(
        &mut self,
        chat_id: String,
        task_id: String,
        cx: &mut Context<Self>,
    ) {
        let (engine, host) = {
            let state = self.state.read(cx);
            let Some(engine) = state.engine().cloned() else {
                return;
            };
            let host = state
                .chats
                .iter()
                .find(|chat| chat.id == chat_id)
                .map(|chat| chat.device_id.clone());
            (engine, host)
        };
        let tray = self.running_trays.entry(chat_id.clone()).or_default();
        if !tray.stopping.insert(task_id.clone()) {
            return;
        }
        cx.notify();
        let mut params = serde_json::json!({ "chatId": chat_id, "taskId": task_id });
        if let Some(host) = host {
            params["targetDeviceId"] = serde_json::Value::String(host);
        }
        cx.spawn(async move |this, cx| {
            let Err(err) = engine
                .client()
                .call(methods::STOP_BACKGROUND_TASK, params)
                .await
            else {
                return;
            };
            tracing::warn!(error = %err, "stopping a background task failed");
            this.update(cx, |composer, cx| {
                if let Some(tray) = composer.running_trays.get_mut(&chat_id) {
                    tray.stopping.remove(&task_id);
                }
                let running = composer
                    .state
                    .read(cx)
                    .background_tasks_for(&chat_id, clock_now())
                    .iter()
                    .any(|task| task.id == task_id);
                if running {
                    crate::toast::error(cx, format!("Couldn't stop the task: {err}"));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn tray_row(key: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(key))
        .flex_none()
        .h(px(ROW_HEIGHT))
        .px(px(ROW_PAD_X))
        .rounded(px(ROW_RADIUS))
        .flex()
        .items_center()
        .gap(px(8.0))
}

fn row_title(title: SharedString, theme: &Theme) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_size(px(TEXT_SIZE))
        .text_color(theme.text)
        .child(title)
}

fn row_meta(text: SharedString, color: gpui::Hsla, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .font_family(theme.font_mono.clone())
        .text_size(px(META_SIZE))
        .text_color(color)
        .child(text)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use serde_json::{Value, json};
    use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry, SubagentStatus};
    use zeron_proto::{BackgroundTask, BackgroundTaskKind, Session, SessionStatus, ToolCall};
    use zeron_rpc::{RpcError, RpcReply, methods};

    use super::*;
    use crate::pull_request_test_support::{self as fixture, ScriptedRpc};
    use crate::state::{AppState, EngineHandle};
    use crate::theme::Theme;

    fn chat(id: &str) -> zeron_proto::Chat {
        serde_json::from_value(json!({
            "id": id, "title": id, "deviceId": format!("{id}-host"), "archived": false,
            "createdAt": Utc::now(),
        }))
        .unwrap()
    }

    fn task(id: &str) -> BackgroundTask {
        BackgroundTask {
            id: id.into(),
            kind: BackgroundTaskKind::Shell,
            description: format!("watch {id}"),
            started_at: Utc::now() - chrono::Duration::seconds(41),
        }
    }

    fn session(chat_id: &str, tasks: Vec<BackgroundTask>) -> Session {
        Session {
            last_turn_outcome: None,
            terminal_activity: None,
            last_completed_turn: None,
            chat_id: chat_id.into(),
            device_id: format!("{chat_id}-host"),
            status: SessionStatus::Working,
            started_at: Some(Utc::now()),
            updated_at: Utc::now(),
            running_subagents: 0,
            background_tasks: tasks,
        }
    }

    fn running_spawn(id: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: "m".into(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Tool {
                id: id.into(),
                call: ToolCall::Unknown {
                    name: format!("Agent: {id}"),
                    input: Some(json!({ "description": "map the flaky tests" })),
                },
                is_error: false,
                resolved: false,
                output: None,
                diff: None,
                output_ref: None,
                output_bytes: None,
                diff_ref: None,
                diff_stats: None,
                subagent_ref: Some(format!("main--sub--{id}")),
                subagent_status: Some(SubagentStatus::Running),
                subagent_tail: None,
            }],
            created_at: Utc::now().timestamp_millis(),
            device_id: "d".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn window(
        cx: &mut gpui::TestAppContext,
    ) -> (
        tempfile::TempDir,
        gpui::Entity<AppState>,
        gpui::WindowHandle<Composer>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
        });
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.chats = vec![chat("a"), chat("b")];
            state
        });
        let handle = {
            let state = state.clone();
            cx.add_window(move |_, cx| Composer::new(state, cx))
        };
        (dir, state, handle)
    }

    fn select(
        state: &gpui::Entity<AppState>,
        handle: &gpui::WindowHandle<Composer>,
        chat: &str,
        cx: &mut gpui::TestAppContext,
    ) {
        state.update(cx, |state, _| state.selected_chat = Some(chat.into()));
        handle
            .update(cx, |composer, _, _| composer.current_key = chat.into())
            .unwrap();
    }

    /// Renders the pills for the selected chat; `false` when there are none.
    fn rendered(handle: &gpui::WindowHandle<Composer>, cx: &mut gpui::TestAppContext) -> bool {
        handle
            .update(cx, |composer, window, cx| {
                composer.render_running_trays(0, window, cx).is_some()
            })
            .unwrap()
    }

    fn pills(
        handle: &gpui::WindowHandle<Composer>,
        cx: &mut gpui::TestAppContext,
    ) -> Vec<RunningTray> {
        handle
            .update(cx, |composer, _, cx| composer.running_pills(cx))
            .unwrap()
    }

    fn open(
        handle: &gpui::WindowHandle<Composer>,
        chat: &str,
        cx: &mut gpui::TestAppContext,
    ) -> Option<RunningTray> {
        handle
            .read_with(cx, |composer, _| {
                composer.running_trays.get(chat).and_then(|tray| tray.open)
            })
            .unwrap()
    }

    fn toggle(
        handle: &gpui::WindowHandle<Composer>,
        tray: RunningTray,
        cx: &mut gpui::TestAppContext,
    ) {
        handle
            .update(cx, |composer, _, cx| composer.toggle_running_tray(tray, cx))
            .unwrap();
    }

    #[gpui::test]
    fn pills_show_only_for_nonempty_lists(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        select(&state, &handle, "a", cx);
        assert!(!rendered(&handle, cx));
        state.update(cx, |state, _| {
            state.sessions = vec![session("a", vec![task("bg-1")])]
        });
        assert!(rendered(&handle, cx));
        assert_eq!(pills(&handle, cx), [RunningTray::Background]);
        state.update(cx, |state, _| state.transcript = vec![running_spawn("t1")]);
        assert_eq!(
            pills(&handle, cx),
            [RunningTray::Subagents, RunningTray::Background]
        );
    }

    #[gpui::test]
    fn one_tray_open_at_a_time(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        select(&state, &handle, "a", cx);
        state.update(cx, |state, _| {
            state.sessions = vec![session("a", vec![task("bg-1")])];
            state.transcript = vec![running_spawn("t1")];
        });
        assert!(rendered(&handle, cx));
        toggle(&handle, RunningTray::Subagents, cx);
        assert_eq!(open(&handle, "a", cx), Some(RunningTray::Subagents));
        toggle(&handle, RunningTray::Background, cx);
        assert_eq!(open(&handle, "a", cx), Some(RunningTray::Background));
        // The open pill closes its own tray.
        toggle(&handle, RunningTray::Background, cx);
        assert_eq!(open(&handle, "a", cx), None);
        // A tray whose list empties closes with its pill.
        toggle(&handle, RunningTray::Background, cx);
        state.update(cx, |state, _| {
            state.sessions = vec![session("a", Vec::new())]
        });
        assert!(rendered(&handle, cx));
        assert_eq!(open(&handle, "a", cx), None);
    }

    #[gpui::test]
    fn tray_state_follows_the_selected_chat(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        state.update(cx, |state, _| {
            state.sessions = vec![
                session("a", vec![task("bg-a")]),
                session("b", vec![task("bg-b")]),
            ];
        });
        select(&state, &handle, "a", cx);
        assert!(rendered(&handle, cx));
        toggle(&handle, RunningTray::Background, cx);
        select(&state, &handle, "b", cx);
        assert!(rendered(&handle, cx));
        assert_eq!(open(&handle, "b", cx), None);
        select(&state, &handle, "a", cx);
        assert!(rendered(&handle, cx));
        assert_eq!(open(&handle, "a", cx), Some(RunningTray::Background));
    }

    type Calls = Arc<Mutex<Vec<(String, Value)>>>;

    /// An engine whose `StopBackgroundTask` answers with `reply`, recording
    /// every stop it gets.
    fn engine(
        state: &gpui::Entity<AppState>,
        reply: Result<Value, String>,
        cx: &mut gpui::TestAppContext,
    ) -> (Arc<ScriptedRpc>, Calls) {
        let calls: Calls = Default::default();
        let rpc = ScriptedRpc::new({
            let calls = calls.clone();
            move |method, params| {
                let reply = if method == methods::STOP_BACKGROUND_TASK {
                    calls.lock().unwrap().push((method.clone(), params));
                    reply.clone().map_err(RpcError::Failed)
                } else {
                    Err(RpcError::UnknownMethod(method))
                };
                async move { reply.and_then(|value| RpcReply::value(&value)) }
            }
        });
        let client = rpc.client();
        state.update(cx, |state, _| {
            state.set_test_engine(EngineHandle::from_test_client(client))
        });
        (rpc, calls)
    }

    fn stop(handle: &gpui::WindowHandle<Composer>, task_id: &str, cx: &mut gpui::TestAppContext) {
        handle
            .update(cx, |composer, _, cx| {
                composer.stop_running_task("a".into(), task_id.into(), cx)
            })
            .unwrap();
    }

    fn stopping(
        handle: &gpui::WindowHandle<Composer>,
        cx: &mut gpui::TestAppContext,
    ) -> Vec<String> {
        handle
            .read_with(cx, |composer, _| {
                composer
                    .running_trays
                    .get("a")
                    .map(|tray| tray.stopping.iter().cloned().collect())
                    .unwrap_or_default()
            })
            .unwrap()
    }

    #[gpui::test]
    fn stop_marks_the_row_and_calls_the_rpc(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let (_dir, state, handle) = window(cx);
        let (rpc, calls) = engine(&state, Ok(json!({})), cx);
        select(&state, &handle, "a", cx);
        state.update(cx, |state, _| {
            state.sessions = vec![session("a", vec![task("bg-1")])]
        });
        assert!(rendered(&handle, cx));
        stop(&handle, "bg-1", cx);
        assert_eq!(stopping(&handle, cx), ["bg-1"]);
        rpc.settle(cx, &runtime, |_| !calls.lock().unwrap().is_empty());
        rpc.settle(cx, &runtime, |_| rpc.completed() >= 1);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [(
                methods::STOP_BACKGROUND_TASK.to_owned(),
                json!({ "chatId": "a", "taskId": "bg-1", "targetDeviceId": "a-host" })
            )]
        );
        // Acknowledged: the row stays marked until the task leaves the set.
        assert!(rendered(&handle, cx));
        assert_eq!(stopping(&handle, cx), ["bg-1"]);
        state.update(cx, |state, _| {
            state.sessions = vec![session("a", Vec::new())]
        });
        assert!(!rendered(&handle, cx));
        assert!(stopping(&handle, cx).is_empty());
    }

    #[gpui::test]
    fn failed_stop_on_a_gone_task_shows_no_toast(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let (_dir, state, handle) = window(cx);
        let (rpc, _calls) = engine(&state, Err("not running".into()), cx);
        select(&state, &handle, "a", cx);
        state.update(cx, |state, _| {
            state.sessions = vec![session("a", vec![task("bg-1"), task("bg-2")])]
        });
        assert!(rendered(&handle, cx));
        stop(&handle, "bg-1", cx);
        // The task ends on its own before the refusal comes back.
        state.update(cx, |state, _| {
            state.sessions = vec![session("a", vec![task("bg-2")])]
        });
        cx.run_until_parked();
        assert_eq!(stopping(&handle, cx), ["bg-1"]);
        // Only the refusal clears the mark here: nothing renders the tray.
        rpc.settle(cx, &runtime, |cx| stopping(&handle, cx).is_empty());
        cx.run_until_parked();
        cx.read(|cx| assert!(crate::toast::messages(cx).is_empty()));
    }

    #[gpui::test]
    fn failed_stop_on_a_live_task_toasts_and_clears_the_mark(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let (_dir, state, handle) = window(cx);
        let (rpc, _calls) = engine(&state, Err("stop timed out".into()), cx);
        select(&state, &handle, "a", cx);
        state.update(cx, |state, _| {
            state.sessions = vec![session("a", vec![task("bg-1")])]
        });
        assert!(rendered(&handle, cx));
        stop(&handle, "bg-1", cx);
        rpc.settle(cx, &runtime, |cx| {
            cx.read(|cx| !crate::toast::messages(cx).is_empty())
        });
        assert!(stopping(&handle, cx).is_empty());
        cx.read(|cx| {
            assert_eq!(
                crate::toast::messages(cx),
                [gpui::SharedString::from(
                    "Couldn't stop the task: stop timed out"
                )]
            )
        });
    }
}
