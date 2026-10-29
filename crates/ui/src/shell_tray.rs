//! Bash mode in the composer: a `!` typed into an empty draft turns Enter into
//! "run this on the chat's host", and each run is staged per chat in a tray
//! above the composer until the next message carries it.
//!
//! The mode belongs to the draft and is keyed like it (`current_key`). The
//! runs belong to the host session and are keyed by [`Composer::shell_tray_key`],
//! which tells a canvas's projects apart.

use std::collections::HashSet;

use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div, prelude::*, px,
};
use zeron_rpc::methods;

use crate::composer::Composer;
use crate::icons;
use crate::output_lines::output_tail;
use crate::shell_blocks::{ShellRun, ShellRunState};
use crate::theme::Theme;

pub(crate) const BASH_PLACEHOLDER: &str = "Run a command";

const ROW_HEIGHT: f32 = 32.0;
const ROW_PAD_X: f32 = 8.0;
/// Rows sit in the queue tray's surface, so they share its concentric radius.
const ROW_RADIUS: f32 = crate::queue::ROW_RADIUS;
const TEXT_SIZE: f32 = 12.5;
const OUTPUT_SIZE: f32 = 11.5;
const OUTPUT_LINE: f32 = 16.0;
const SPINNER_SIZE: f32 = 12.0;
/// An open row previews the tail of its output; the message carries all of it.
const OUTPUT_PREVIEW_LINES: usize = 20;

/// One session's staged runs, keyed in `Composer::shell_trays` by
/// [`Composer::shell_tray_key`]. `open` holds the ids of rows showing output.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ShellTrayState {
    pub runs: Vec<ShellRun>,
    pub open: HashSet<String>,
}

fn ready(run: &ShellRun) -> bool {
    run.state != ShellRunState::Running
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunReply {
    exit_code: Option<i32>,
    output: String,
    truncated: bool,
    timed_out: bool,
}

impl Composer {
    /// The selected chat id, or `space-canvas:{space}` on the new-chat
    /// canvas. It is also the run's `chatId`, so the host resolves the same
    /// working directory its terminal would.
    pub(crate) fn shell_tray_key(&self, cx: &App) -> String {
        self.state.read(cx).panel_session_key()
    }

    /// The device that runs commands for `key`, this one included.
    fn shell_host(&self, key: &str, cx: &App) -> Option<String> {
        let state = self.state.read(cx);
        state
            .terminal_target_device(key)
            .or_else(|| state.local_device_id.clone())
    }

    pub(crate) fn bash_mode(&self) -> bool {
        self.bash_allowed() && self.bash_drafts.contains(&self.current_key)
    }

    pub(crate) fn enter_bash_mode(&mut self, cx: &mut Context<Self>) {
        if !self.bash_allowed() {
            return;
        }
        self.bash_drafts.insert(self.current_key.clone());
        self.sync_bash_input(cx);
        cx.notify();
    }

    pub(crate) fn leave_bash_mode(&mut self, cx: &mut Context<Self>) {
        let key = self.current_key.clone();
        self.leave_bash_mode_for(&key, cx);
    }

    /// Leave bash mode for the draft under `key`, which need not be the one
    /// showing.
    pub(crate) fn leave_bash_mode_for(&mut self, key: &str, cx: &mut Context<Self>) {
        self.bash_drafts.remove(key);
        self.sync_bash_input(cx);
        cx.notify();
    }

    /// Point the input at the selected chat's mode: what `!`, Backspace and
    /// Escape do, the text face, and the placeholder. Leaving restores the
    /// default placeholder only over bash mode's own, so a question's prompt
    /// is left alone.
    pub(crate) fn sync_bash_input(&mut self, cx: &mut Context<Self>) {
        let allowed = self.bash_allowed();
        let active = self.bash_mode();
        self.input.update(cx, |input, cx| {
            input.bash_trigger = allowed && !active;
            if input.bash_mode == active {
                return;
            }
            input.set_bash_mode(active, cx);
            if active {
                input.set_placeholder(BASH_PLACEHOLDER, cx);
            } else if input.placeholder() == BASH_PLACEHOLDER {
                input.set_placeholder("Do anything…", cx);
            }
        });
    }

    /// Stage `command` as a running block of the selected chat and run it on
    /// that chat's host. The reply lands in the chat the run started in, even
    /// after the user moves on.
    pub(crate) fn run_shell_command(&mut self, command: String, cx: &mut Context<Self>) {
        if command.trim().is_empty() {
            return;
        }
        let chat_id = self.shell_tray_key(cx);
        let run_id = uuid::Uuid::new_v4().to_string();
        self.shell_trays
            .entry(chat_id.clone())
            .or_default()
            .runs
            .push(ShellRun {
                id: run_id.clone(),
                chat_id: chat_id.clone(),
                command: command.clone(),
                state: ShellRunState::Running,
            });
        self.input.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.finish_shell_run(&chat_id, &run_id, Err("Engine not connected".into()), cx);
            return;
        };
        // A chat whose row hasn't arrived has no known host. Falling back to
        // this device would run the command on the wrong machine.
        let state = self.state.read(cx);
        if !chat_id.starts_with(crate::state::CANVAS_PANEL_PREFIX)
            && !state.chats.iter().any(|chat| chat.id == chat_id)
        {
            self.finish_shell_run(&chat_id, &run_id, Err("Chat not loaded yet".into()), cx);
            return;
        }
        let mut params = serde_json::json!({
            "runId": run_id,
            "chatId": chat_id,
            "command": command,
        });
        if let Some(host) = self.shell_host(&chat_id, cx) {
            params["targetDeviceId"] = serde_json::Value::String(host);
        }
        cx.spawn(async move |this, cx| {
            let reply = engine
                .client()
                .call_as::<RunReply>(methods::RUN_SHELL_COMMAND, params)
                .await
                .map_err(|err| err.to_string());
            this.update(cx, |composer, cx| {
                composer.finish_shell_run(&chat_id, &run_id, reply, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_shell_run(
        &mut self,
        chat_id: &str,
        run_id: &str,
        reply: Result<RunReply, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = self
            .shell_trays
            .get_mut(chat_id)
            .and_then(|tray| tray.runs.iter_mut().find(|run| run.id == run_id))
        else {
            return;
        };
        run.state = match reply {
            Ok(reply) => ShellRunState::Done {
                exit_code: reply.exit_code,
                output: reply.output,
                truncated: reply.truncated,
                timed_out: reply.timed_out,
            },
            Err(err) => {
                crate::toast::error(cx, format!("Couldn’t run the command: {err}"));
                ShellRunState::Failed(err)
            }
        };
        cx.notify();
    }

    /// Drop a staged run. One still running is cancelled on its host too.
    pub(crate) fn cancel_shell_run(&mut self, chat_id: &str, run_id: &str, cx: &mut Context<Self>) {
        let Some(tray) = self.shell_trays.get_mut(chat_id) else {
            return;
        };
        let Some(index) = tray.runs.iter().position(|run| run.id == run_id) else {
            return;
        };
        let run = tray.runs.remove(index);
        tray.open.remove(run_id);
        if tray.runs.is_empty() {
            self.shell_trays.remove(chat_id);
        }
        cx.notify();
        if run.state != ShellRunState::Running {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let mut params = serde_json::json!({ "runId": run_id });
        if let Some(host) = self.shell_host(chat_id, cx) {
            params["targetDeviceId"] = serde_json::Value::String(host);
        }
        cx.spawn(async move |_, _| {
            if let Err(err) = engine
                .client()
                .call(methods::CANCEL_SHELL_COMMAND, params)
                .await
            {
                tracing::warn!(error = %err, "cancelling a shell run failed");
            }
        })
        .detach();
    }

    /// Whether `text` leads with a native command for the harness it goes to.
    /// The host routes one only when it opens the prompt
    /// ([`zeron_proto::invocation::native_command`], the engine's own test),
    /// so staged runs never go in front of it. With no harness resolved yet,
    /// any harness's command counts.
    pub(crate) fn sends_native_command(&self, text: &str, cx: &App) -> bool {
        use zeron_proto::{HarnessId, invocation::native_command};
        match self.pickers().read(cx).resolved(cx).harness {
            Some(harness) => native_command(text, harness),
            None => HarnessId::ALL
                .iter()
                .any(|harness| native_command(text, *harness)),
        }
    }

    /// How many staged runs the next message would carry.
    pub(crate) fn shell_outputs_ready(&self, cx: &App) -> usize {
        self.shell_trays
            .get(&self.shell_tray_key(cx))
            .map_or(0, |tray| tray.runs.iter().filter(|run| ready(run)).count())
    }

    /// Unstage the finished runs under `key` for a send. Running ones stay.
    pub(crate) fn take_shell_outputs(&mut self, key: &str) -> Vec<ShellRun> {
        let Some(tray) = self.shell_trays.get_mut(key) else {
            return Vec::new();
        };
        let (taken, kept) = std::mem::take(&mut tray.runs).into_iter().partition(ready);
        tray.runs = kept;
        let ShellTrayState { runs, open, .. } = tray;
        open.retain(|id| runs.iter().any(|run| &run.id == id));
        if tray.runs.is_empty() {
            self.shell_trays.remove(key);
        }
        taken
    }

    /// Hand a failed send's runs back to the tray they left, ahead of any
    /// staged since.
    pub(crate) fn restore_shell_outputs(&mut self, key: &str, mut runs: Vec<ShellRun>) {
        if runs.is_empty() {
            return;
        }
        let tray = self.shell_trays.entry(key.to_owned()).or_default();
        runs.append(&mut tray.runs);
        tray.runs = runs;
    }

    /// The selected chat's staged runs, or `None` when it has none. It shares
    /// the queue's glass surface and sits between the queue and the composer.
    pub(crate) fn render_shell_tray(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let key = self.shell_tray_key(cx);
        let tray = self.shell_trays.get(&key)?;
        if tray.runs.is_empty() {
            return None;
        }
        let (runs, open) = (tray.runs.clone(), tray.open.clone());
        let theme = Theme::of(cx).clone();
        let rows: Vec<AnyElement> = runs
            .iter()
            .map(|run| self.shell_row(&key, run, open.contains(&run.id), &theme, cx))
            .collect();
        let list = crate::edge_fade::edge_faded(
            Theme::TRANSCRIPT_FADE_BAND,
            true,
            true,
            div()
                .id("shell-tray-rows")
                .max_h(window.viewport_size().height * 0.3)
                .overflow_y_scroll()
                .track_scroll(&self.shell_tray_scroll)
                .flex()
                .flex_col()
                .children(rows),
        )
        .fade_overflow_y(&self.shell_tray_scroll);
        let surface = crate::queue::queue_panel_surface(&theme, cx.has_active_drag())
            .debug_selector(|| "shell-tray".into())
            .child(list);
        Some(
            crate::frost::frosted(crate::queue::PANEL_RADIUS, crate::frost::MENU_BLUR, surface)
                .into_any_element(),
        )
    }

    /// `$ command`, then a spinner or how it ended, then ✕. Clicking the row
    /// toggles the tail of its output.
    fn shell_row(
        &self,
        key: &str,
        run: &ShellRun,
        open: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = SharedString::from(format!("shell-run-{}", run.id));
        let status: Option<AnyElement> = match &run.state {
            ShellRunState::Running => Some(
                crate::loaders::ring_spinner(
                    format!("{id}-spinner"),
                    SPINNER_SIZE,
                    theme.text_muted,
                )
                .into_any_element(),
            ),
            ShellRunState::Done {
                exit_code,
                timed_out,
                ..
            } => crate::shell_blocks::status_label(*exit_code, *timed_out, theme),
            ShellRunState::Failed(_) => None,
        };
        let (chat_id, run_id) = (run.chat_id.clone(), run.id.clone());
        let remove = self.queue_action(
            &id,
            "remove",
            "Remove",
            icons::QUEUE_CLOSE,
            true,
            theme,
            cx.listener(move |this, _, _, cx| this.cancel_shell_run(&chat_id, &run_id, cx)),
        );
        let (toggle_key, toggle_id) = (key.to_owned(), run.id.clone());
        let accent = theme.accent;
        let header = div()
            .id(id.clone())
            .role(gpui::Role::Button)
            .aria_label(if open { "Hide output" } else { "Show output" })
            .h(px(ROW_HEIGHT))
            .flex_none()
            .px(px(ROW_PAD_X))
            .pr(px(ROW_PAD_X - 4.0))
            .rounded(px(ROW_RADIUS))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(|s| s.bg(crate::theme::ink(0.04)))
            .tab_index(0)
            .focus_visible(move |s| s.bg(accent.opacity(0.14)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(tray) = this.shell_trays.get_mut(&toggle_key)
                    && !tray.open.remove(&toggle_id)
                {
                    tray.open.insert(toggle_id.clone());
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme.font_mono.clone())
                    .text_size(px(TEXT_SIZE))
                    .text_color(theme.text.opacity(0.9))
                    .child(format!("$ {}", run.command)),
            )
            .children(status)
            .child(remove);
        let output = match &run.state {
            ShellRunState::Running => None,
            ShellRunState::Done { output, .. } => Some((output.as_str(), theme.text_muted)),
            ShellRunState::Failed(error) => Some((error.as_str(), theme.danger)),
        }
        .filter(|_| open)
        .map(|(output, color)| (output_tail(output, OUTPUT_PREVIEW_LINES).0, color));
        div()
            .flex()
            .flex_col()
            .child(header)
            .when_some(output, |el, (lines, color)| {
                el.child(
                    div()
                        .px(px(ROW_PAD_X))
                        .pb(px(6.0))
                        .font_family(theme.font_mono.clone())
                        .text_size(px(OUTPUT_SIZE))
                        .line_height(px(OUTPUT_LINE))
                        .text_color(color)
                        .children(lines.into_iter().map(|line| div().truncate().child(line))),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request_test_support::{self as fixture, ScriptedRpc};
    use gpui::{TestAppContext, WindowHandle};
    use std::sync::{Arc, Mutex};
    use zeron_rpc::{RpcError, RpcReply};

    type Calls = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

    #[derive(Clone, Copy)]
    enum Host {
        Replies,
        Fails,
        Hangs,
        /// Replies once the rig opens the gate.
        Gated,
        /// Runs reply; sends fail once the rig opens the gate.
        SendFails,
        /// Queue-edit commits come back as a conflict.
        EditConflicts,
    }

    struct Rig {
        _dir: tempfile::TempDir,
        rpc: Arc<ScriptedRpc>,
        calls: Calls,
        gate: Arc<tokio::sync::Notify>,
        handle: WindowHandle<Composer>,
    }

    fn window(cx: &mut TestAppContext, host: Host) -> Rig {
        let dir = fixture::settings(cx, Default::default());
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(crate::theme::Theme::dark());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            crate::composer::init(cx, Default::default());
        });
        let calls: Calls = Default::default();
        let gate = Arc::new(tokio::sync::Notify::new());
        let rpc = ScriptedRpc::new({
            let calls = calls.clone();
            let gate = gate.clone();
            move |method, params| {
                calls.lock().unwrap().push((method.clone(), params));
                let gate = gate.clone();
                async move {
                    let reply = serde_json::json!({
                        "exitCode": 0, "output": "a\n",
                        "truncated": false, "timedOut": false,
                    });
                    match (method.as_str(), host) {
                        (methods::RUN_SHELL_COMMAND, Host::Replies | Host::SendFails) => {
                            RpcReply::value(&reply)
                        }
                        (methods::QUEUE_COMMAND, Host::SendFails) => {
                            gate.notified().await;
                            Err(RpcError::Failed("disk full".into()))
                        }
                        (methods::BEGIN_QUEUED_MESSAGE_EDIT, _) => {
                            RpcReply::value(&serde_json::json!({
                                "outcome": "acquired", "leaseId": "lease",
                                "baseTextHash": "hash", "attachments": [],
                                "text": crate::shell_blocks::format_prompt(&[done("q")], "why?"),
                            }))
                        }
                        (methods::FINISH_QUEUED_MESSAGE_EDIT, Host::EditConflicts) => {
                            RpcReply::value(&serde_json::json!({ "outcome": "conflict" }))
                        }
                        (methods::FINISH_QUEUED_MESSAGE_EDIT, _) => {
                            RpcReply::value(&serde_json::json!({ "outcome": "committed" }))
                        }
                        (methods::RUN_SHELL_COMMAND, Host::Gated) => {
                            gate.notified().await;
                            RpcReply::value(&reply)
                        }
                        (methods::RUN_SHELL_COMMAND, Host::Fails) => {
                            Err(RpcError::Failed("host offline".into()))
                        }
                        (methods::RUN_SHELL_COMMAND, Host::Hangs) => {
                            futures::future::pending().await
                        }
                        (
                            methods::CANCEL_SHELL_COMMAND
                            | methods::QUEUE_COMMAND
                            | methods::MUTATE,
                            _,
                        ) => RpcReply::value(&serde_json::json!({})),
                        _ => Err(RpcError::UnknownMethod(method)),
                    }
                }
            }
        });
        let state = cx.update(|cx| fixture::state(cx, Some(rpc.client())));
        state.update(cx, |state, _| {
            state.chats = ["chat-a", "chat-b"]
                .into_iter()
                .map(|id| {
                    serde_json::from_value(serde_json::json!({
                        "id": id, "deviceId": "box",
                        "archived": false, "createdAt": chrono::Utc::now(),
                    }))
                    .unwrap()
                })
                .collect();
            state.selected_chat = Some("chat-a".into());
            state.devices = serde_json::from_value(serde_json::json!([{
                "id": "box", "name": "Box", "platform": "macos", "lastSeenAt": null,
                "capabilities": zeron_proto::capabilities::CURRENT,
            }]))
            .unwrap();
        });
        let handle = cx.add_window(|_, cx| Composer::new(state, cx));
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        Rig {
            _dir: dir,
            rpc,
            calls,
            gate,
            handle,
        }
    }

    impl Rig {
        fn type_text(&self, text: &str, cx: &mut TestAppContext) {
            cx.simulate_input(self.handle.into(), text);
        }

        fn press(&self, keys: &str, cx: &mut TestAppContext) {
            cx.simulate_keystrokes(self.handle.into(), keys);
        }

        fn text(&self, cx: &mut TestAppContext) -> String {
            self.handle
                .read_with(cx, |composer, cx| composer.input.read(cx).text().to_owned())
                .unwrap()
        }

        fn placeholder(&self, cx: &mut TestAppContext) -> String {
            self.handle
                .read_with(cx, |composer, cx| {
                    composer.input.read(cx).placeholder().to_owned()
                })
                .unwrap()
        }

        fn tray(&self, cx: &mut TestAppContext) -> ShellTrayState {
            self.tray_of("chat-a", cx)
        }

        fn tray_of(&self, key: &str, cx: &mut TestAppContext) -> ShellTrayState {
            self.handle
                .read_with(cx, |composer, _| {
                    composer.shell_trays.get(key).cloned().unwrap_or_default()
                })
                .unwrap()
        }

        fn stage(&self, runs: Vec<ShellRun>, cx: &mut TestAppContext) {
            self.handle
                .update(cx, |composer, _, cx| {
                    composer
                        .shell_trays
                        .entry("chat-a".into())
                        .or_default()
                        .runs = runs;
                    cx.notify();
                })
                .unwrap();
        }

        fn select(&self, chat_id: &str, cx: &mut TestAppContext) {
            let chat_id = chat_id.to_owned();
            self.handle
                .update(cx, |composer, _, cx| {
                    composer.state.update(cx, |state, cx| {
                        state.selected_chat = Some(chat_id);
                        cx.notify();
                    })
                })
                .unwrap();
            cx.run_until_parked();
        }

        fn bash(&self, cx: &mut TestAppContext) -> bool {
            self.handle
                .read_with(cx, |composer, _| composer.bash_mode())
                .unwrap()
        }

        /// The new-chat canvas of project `space`, `a` and `b` on `box`.
        fn canvas(&self, space: &str, cx: &mut TestAppContext) {
            let space = space.to_owned();
            self.handle
                .update(cx, |composer, _, cx| {
                    composer.state.update(cx, |state, cx| {
                        state.spaces = ["a", "b"]
                            .into_iter()
                            .map(|id| {
                                serde_json::from_value(serde_json::json!({
                                    "id": id, "deviceId": "box", "path": format!("/src/{id}"),
                                    "createdAt": chrono::Utc::now(),
                                }))
                                .unwrap()
                            })
                            .collect();
                        state.selected_chat = None;
                        state.selected_space = Some(space);
                        cx.notify();
                    })
                })
                .unwrap();
            cx.run_until_parked();
        }

        fn click_send(&self, cx: &mut TestAppContext) {
            cx.update_window(self.handle.into(), |_, window, cx| window.draw(cx).clear())
                .unwrap();
            let mut visual = gpui::VisualTestContext::from_window(self.handle.into(), cx);
            let button = visual.debug_bounds("composer-send").unwrap();
            visual.simulate_click(button.center(), gpui::Modifiers::default());
        }

        /// Queue `q1` (a `q` run ahead of "why?") and open it for editing.
        fn begin_edit(&self, cx: &mut TestAppContext, runtime: &tokio::runtime::Runtime) {
            self.handle
                .update(cx, |composer, _, cx| {
                    composer.state.update(cx, |state, _| {
                        state.queue = vec![
                            serde_json::from_value(serde_json::json!({
                                "id": "q1",
                                "text": crate::shell_blocks::format_prompt(&[done("q")], "why?"),
                                "issuedBy": "local", "issuedAt": 0,
                            }))
                            .unwrap(),
                        ];
                    });
                    composer.begin_queue_edit("q1".into(), cx);
                })
                .unwrap();
            self.settle(cx, runtime, |rig, cx| {
                rig.handle
                    .read_with(cx, |composer, _| composer.editing_queued.is_some())
                    .unwrap()
            });
        }

        /// A leading skill chip that carries the chat's harness's command.
        fn skill_command(&self, cx: &mut TestAppContext) -> String {
            let harness = self
                .handle
                .read_with(cx, |composer, cx| {
                    composer.pickers().read(cx).resolved(cx).harness
                })
                .unwrap()
                .unwrap_or(zeron_proto::HarnessId::ClaudeCode);
            zeron_proto::invocation::Invocation::Skill {
                name: "review".into(),
                path: "/skills/review/SKILL.md".into(),
                command: Some(zeron_proto::invocation::SkillCommand {
                    name: "review".into(),
                    harness,
                }),
            }
            .link()
        }

        fn settle_failure(&self, cx: &mut TestAppContext, runtime: &tokio::runtime::Runtime) {
            self.settle(cx, runtime, |rig, cx| {
                rig.handle
                    .read_with(cx, |composer, _| composer.failure().is_some())
                    .unwrap()
            });
        }

        /// The prompt of the one message sent so far.
        fn sent_prompt(
            &self,
            cx: &mut TestAppContext,
            runtime: &tokio::runtime::Runtime,
        ) -> String {
            self.settle(cx, runtime, |rig, _| {
                !rig.calls(methods::QUEUE_COMMAND).is_empty()
            });
            let sent = self.calls(methods::QUEUE_COMMAND);
            let [params] = sent.as_slice() else {
                panic!("expected one send, got {sent:?}");
            };
            params["command"]["request"]["prompt"]
                .as_str()
                .unwrap()
                .to_owned()
        }

        fn calls(&self, method: &str) -> Vec<serde_json::Value> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(called, _)| called == method)
                .map(|(_, params)| params.clone())
                .collect()
        }

        fn settle(
            &self,
            cx: &mut TestAppContext,
            runtime: &tokio::runtime::Runtime,
            done: impl Fn(&Rig, &mut TestAppContext) -> bool,
        ) {
            self.rpc.settle(cx, runtime, |cx| done(self, cx));
        }

        fn run_ls(&self, cx: &mut TestAppContext) {
            self.type_text("!", cx);
            self.type_text("ls", cx);
            self.press("enter", cx);
        }
    }

    #[gpui::test]
    fn bang_on_empty_draft_enters_bash_mode(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.type_text("!", cx);
        assert!(rig.bash(cx));
        assert_eq!(rig.text(cx), "");
        assert_eq!(rig.placeholder(cx), "Run a command");
    }

    #[gpui::test]
    fn bang_mid_text_is_text(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.type_text("a!", cx);
        assert!(!rig.bash(cx));
        assert_eq!(rig.text(cx), "a!");
    }

    #[gpui::test]
    fn pasted_bang_does_not_enter(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        cx.update(|cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("!ls".into())));
        cx.dispatch_action(rig.handle.into(), crate::composer::Paste);
        assert!(!rig.bash(cx));
        assert_eq!(rig.text(cx), "!ls");
    }

    #[gpui::test]
    fn backspace_on_empty_and_escape_leave(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        for key in ["backspace", "escape"] {
            rig.type_text("!", cx);
            assert!(rig.bash(cx), "{key}");
            rig.press(key, cx);
            assert!(!rig.bash(cx), "{key}");
            assert_eq!(rig.placeholder(cx), "Do anything…", "{key}");
        }
    }

    #[gpui::test]
    fn enter_runs_on_the_host(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Hangs);
        rig.run_ls(cx);
        rig.settle(cx, &runtime, |rig, _| {
            !rig.calls(methods::RUN_SHELL_COMMAND).is_empty()
        });
        let calls = rig.calls(methods::RUN_SHELL_COMMAND);
        let [params] = calls.as_slice() else {
            panic!("expected one run, got {calls:?}");
        };
        assert_eq!(params["chatId"], "chat-a");
        assert_eq!(params["command"], "ls");
        assert_eq!(params["targetDeviceId"], "box");
        assert_eq!(rig.text(cx), "");
        assert!(rig.bash(cx));
        let tray = rig.tray(cx);
        let [run] = tray.runs.as_slice() else {
            panic!("expected one staged run, got {:?}", tray.runs);
        };
        assert_eq!(params["runId"], run.id.as_str());
        assert_eq!(run.chat_id, "chat-a");
        assert_eq!(run.command, "ls");
        assert_eq!(run.state, ShellRunState::Running);
    }

    #[gpui::test]
    fn enter_on_empty_bash_input_does_nothing(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.type_text("!", cx);
        rig.press("enter", cx);
        runtime.block_on(tokio::time::sleep(std::time::Duration::from_millis(20)));
        cx.run_until_parked();
        assert!(rig.calls(methods::RUN_SHELL_COMMAND).is_empty());
        assert!(rig.tray(cx).runs.is_empty());
        assert!(rig.bash(cx));
    }

    #[gpui::test]
    fn reply_marks_the_run_done(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.run_ls(cx);
        rig.settle(cx, &runtime, |rig, cx| {
            rig.tray(cx)
                .runs
                .first()
                .is_some_and(|run| run.state != ShellRunState::Running)
        });
        assert_eq!(
            rig.tray(cx).runs[0].state,
            ShellRunState::Done {
                exit_code: Some(0),
                output: "a\n".into(),
                truncated: false,
                timed_out: false,
            }
        );
    }

    #[gpui::test]
    fn rpc_error_marks_failed_and_toasts(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Fails);
        rig.run_ls(cx);
        rig.settle(cx, &runtime, |rig, cx| {
            rig.tray(cx)
                .runs
                .first()
                .is_some_and(|run| run.state != ShellRunState::Running)
        });
        let ShellRunState::Failed(err) = rig.tray(cx).runs[0].state.clone() else {
            panic!("expected a failed run, got {:?}", rig.tray(cx).runs);
        };
        assert!(err.contains("host offline"), "{err}");
        let toasts = cx.update(|cx| crate::toast::messages(cx));
        assert!(
            toasts
                .iter()
                .any(|toast| toast.as_ref() == format!("Couldn’t run the command: {err}")),
            "{toasts:?}"
        );
    }

    #[gpui::test]
    fn cancel_sends_cancel_and_removes(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Hangs);
        rig.run_ls(cx);
        rig.settle(cx, &runtime, |rig, _| {
            !rig.calls(methods::RUN_SHELL_COMMAND).is_empty()
        });
        let id = rig.tray(cx).runs[0].id.clone();
        rig.handle
            .update(cx, |composer, _, cx| {
                composer.cancel_shell_run("chat-a", &id, cx)
            })
            .unwrap();
        assert!(rig.tray(cx).runs.is_empty());
        rig.settle(cx, &runtime, |rig, _| {
            !rig.calls(methods::CANCEL_SHELL_COMMAND).is_empty()
        });
        let cancels = rig.calls(methods::CANCEL_SHELL_COMMAND);
        let [params] = cancels.as_slice() else {
            panic!("expected one cancel, got {cancels:?}");
        };
        assert_eq!(params["runId"], id.as_str());
        assert_eq!(params["targetDeviceId"], "box");
    }

    #[gpui::test]
    fn side_chat_has_no_bash_mode(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.handle
            .update(cx, |composer, _, cx| composer.set_side_chat(cx))
            .unwrap();
        rig.type_text("!", cx);
        assert!(!rig.bash(cx));
        assert_eq!(rig.text(cx), "!");
    }

    fn run(id: &str, state: ShellRunState) -> ShellRun {
        ShellRun {
            id: id.into(),
            chat_id: "chat-a".into(),
            command: format!("cmd-{id}"),
            state,
        }
    }

    fn done(id: &str) -> ShellRun {
        run(
            id,
            ShellRunState::Done {
                exit_code: Some(0),
                output: "a\n".into(),
                truncated: false,
                timed_out: false,
            },
        )
    }

    #[gpui::test]
    fn send_prefixes_done_runs_and_clears_them(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        let running = run("running", ShellRunState::Running);
        rig.stage(vec![done("done"), running.clone()], cx);
        rig.type_text("why?", cx);
        rig.press("enter", cx);
        assert_eq!(
            rig.sent_prompt(cx, &runtime),
            crate::shell_blocks::format_prompt(&[done("done")], "why?")
        );
        assert_eq!(rig.tray(cx).runs, vec![running]);
    }

    #[gpui::test]
    fn send_with_only_runs_is_allowed(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.stage(vec![done("done")], cx);
        rig.press("enter", cx);
        assert_eq!(
            rig.sent_prompt(cx, &runtime),
            crate::shell_blocks::format_prompt(&[done("done")], "")
        );
        assert!(rig.tray(cx).runs.is_empty());
    }

    #[gpui::test]
    fn result_lands_in_the_starting_chat(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Gated);
        rig.run_ls(cx);
        rig.settle(cx, &runtime, |rig, _| {
            !rig.calls(methods::RUN_SHELL_COMMAND).is_empty()
        });
        rig.select("chat-b", cx);
        rig.gate.notify_one();
        rig.settle(cx, &runtime, |rig, cx| {
            rig.tray(cx)
                .runs
                .first()
                .is_some_and(|run| run.state != ShellRunState::Running)
        });
        assert!(matches!(
            rig.tray(cx).runs[0].state,
            ShellRunState::Done { .. }
        ));
        assert!(rig.tray_of("chat-b", cx).runs.is_empty());
    }

    #[gpui::test]
    fn tray_hidden_without_runs(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        let shown = |cx: &mut TestAppContext| {
            rig.handle
                .update(cx, |composer, window, cx| {
                    composer.render_shell_tray(window, cx).is_some()
                })
                .unwrap()
        };
        assert!(!shown(cx));
        rig.type_text("!", cx);
        assert!(!shown(cx));
        rig.stage(vec![done("done")], cx);
        assert!(shown(cx));
    }

    #[gpui::test]
    fn failed_send_hands_the_runs_back(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::SendFails);
        let running = run("running", ShellRunState::Running);
        rig.stage(vec![done("done"), running.clone()], cx);
        rig.type_text("why?", cx);
        rig.press("enter", cx);
        rig.gate.notify_one();
        rig.settle_failure(cx, &runtime);
        assert_eq!(rig.tray(cx).runs, vec![done("done"), running]);
        assert_eq!(rig.text(cx), "why?");
    }

    #[gpui::test]
    fn failed_send_restores_prose_outside_bash_mode(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::SendFails);
        rig.type_text("why?", cx);
        rig.press("enter", cx);
        rig.type_text("!", cx);
        assert!(rig.bash(cx));
        rig.gate.notify_one();
        rig.settle_failure(cx, &runtime);
        assert!(!rig.bash(cx));
        assert_eq!(rig.text(cx), "why?");
        assert_eq!(rig.placeholder(cx), "Do anything…");
    }

    #[gpui::test]
    fn send_button_sends_staged_outputs_in_bash_mode(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.stage(vec![done("done")], cx);
        rig.type_text("!", cx);
        rig.press("enter", cx);
        runtime.block_on(tokio::time::sleep(std::time::Duration::from_millis(20)));
        cx.run_until_parked();
        assert!(
            rig.calls(methods::QUEUE_COMMAND).is_empty(),
            "Enter sends nothing"
        );
        assert!(rig.calls(methods::RUN_SHELL_COMMAND).is_empty());
        rig.click_send(cx);
        assert_eq!(
            rig.sent_prompt(cx, &runtime),
            crate::shell_blocks::format_prompt(&[done("done")], "")
        );
        assert!(rig.tray(cx).runs.is_empty());
        assert!(rig.bash(cx));
    }

    #[gpui::test]
    fn queued_edit_keeps_shell_blocks(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.begin_edit(cx, &runtime);
        assert_eq!(rig.text(cx), "why?");
        rig.type_text(" now", cx);
        rig.press("enter", cx);
        rig.settle(cx, &runtime, |rig, _| {
            !rig.calls(methods::FINISH_QUEUED_MESSAGE_EDIT).is_empty()
        });
        let finishes = rig.calls(methods::FINISH_QUEUED_MESSAGE_EDIT);
        assert_eq!(
            finishes[0]["text"],
            crate::shell_blocks::format_prompt(&[done("q")], "why? now")
        );
    }

    #[gpui::test]
    fn canvas_project_switch_keeps_bash_mode_with_its_draft(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.canvas("a", cx);
        rig.type_text("!", cx);
        assert!(rig.bash(cx));
        rig.canvas("b", cx);
        assert!(rig.bash(cx), "the draft kept its mode");
        rig.press("escape", cx);
        rig.type_text("hello", cx);
        rig.canvas("a", cx);
        assert!(!rig.bash(cx));
        assert_eq!(rig.text(cx), "hello");
        assert_eq!(rig.placeholder(cx), "Do anything…");
    }

    #[gpui::test]
    fn failed_send_after_a_chat_switch_restores_into_its_own_chat(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::SendFails);
        rig.type_text("why?", cx);
        rig.press("enter", cx);
        rig.select("chat-b", cx);
        rig.type_text("!", cx);
        assert!(rig.bash(cx));
        rig.gate.notify_one();
        rig.settle_failure(cx, &runtime);
        assert!(rig.bash(cx), "chat-b keeps its bash input");
        assert_eq!(rig.text(cx), "");
        rig.select("chat-a", cx);
        assert!(!rig.bash(cx));
        assert_eq!(rig.text(cx), "why?");
    }

    #[gpui::test]
    fn unknown_chat_fails_instead_of_running_locally(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.select("chat-unlisted", cx);
        rig.run_ls(cx);
        runtime.block_on(tokio::time::sleep(std::time::Duration::from_millis(20)));
        cx.run_until_parked();
        assert!(rig.calls(methods::RUN_SHELL_COMMAND).is_empty());
        let runs = rig.tray_of("chat-unlisted", cx).runs;
        let [run] = runs.as_slice() else {
            panic!("expected one staged run, got {runs:?}");
        };
        let ShellRunState::Failed(err) = &run.state else {
            panic!("expected a failed run, got {run:?}");
        };
        let toasts = cx.update(|cx| crate::toast::messages(cx));
        assert!(
            toasts
                .iter()
                .any(|toast| toast.as_ref() == format!("Couldn’t run the command: {err}")),
            "{toasts:?}"
        );
    }

    #[gpui::test]
    fn native_command_leaves_runs_staged(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.stage(vec![done("done")], cx);
        rig.type_text("/compact", cx);
        rig.press("enter", cx);
        assert_eq!(rig.sent_prompt(cx, &runtime), "/compact");
        assert_eq!(rig.tray(cx).runs, vec![done("done")]);
    }

    #[gpui::test]
    fn canvas_runs_ride_the_first_message(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.canvas("a", cx);
        rig.type_text("!", cx);
        rig.type_text("pwd", cx);
        rig.press("enter", cx);
        rig.settle(cx, &runtime, |rig, cx| {
            rig.tray_of("space-canvas:a", cx)
                .runs
                .first()
                .is_some_and(|run| run.state != ShellRunState::Running)
        });
        let ran = rig.tray_of("space-canvas:a", cx).runs;
        assert_eq!(
            rig.calls(methods::RUN_SHELL_COMMAND)[0]["chatId"],
            "space-canvas:a"
        );
        rig.press("escape", cx);
        rig.type_text("why?", cx);
        rig.press("enter", cx);
        assert_eq!(
            rig.sent_prompt(cx, &runtime),
            crate::shell_blocks::format_prompt(&ran, "why?")
        );
        assert!(rig.tray_of("space-canvas:a", cx).runs.is_empty());
    }

    #[gpui::test]
    fn queue_edit_recovery_never_returns_to_bash_mode(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.type_text("!", cx);
        assert!(rig.bash(cx));
        rig.begin_edit(cx, &runtime);
        rig.handle
            .update(cx, |composer, _, cx| {
                composer.state.update(cx, |state, cx| {
                    state.queue.clear();
                    cx.notify();
                })
            })
            .unwrap();
        cx.run_until_parked();
        assert!(
            rig.handle
                .read_with(cx, |composer, _| composer.editing_queued.is_none())
                .unwrap()
        );
        assert!(!rig.bash(cx));
        assert_eq!(rig.text(cx), "why?");
    }

    #[gpui::test]
    fn skill_command_leaves_runs_staged(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.stage(vec![done("done")], cx);
        let command = rig.skill_command(cx);
        rig.type_text(&command, cx);
        rig.press("enter", cx);
        assert_eq!(rig.sent_prompt(cx, &runtime), command);
        assert_eq!(rig.tray(cx).runs, vec![done("done")]);
    }

    #[gpui::test]
    fn queued_edit_to_a_native_command_stages_its_blocks(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.begin_edit(cx, &runtime);
        rig.handle
            .update(cx, |composer, _, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("/compact", cx))
            })
            .unwrap();
        rig.press("enter", cx);
        rig.settle(cx, &runtime, |rig, _| {
            !rig.calls(methods::FINISH_QUEUED_MESSAGE_EDIT).is_empty()
        });
        let finishes = rig.calls(methods::FINISH_QUEUED_MESSAGE_EDIT);
        assert_eq!(finishes[0]["text"], "/compact");
        let runs = rig.tray(cx).runs;
        let [run] = runs.as_slice() else {
            panic!("expected the row's run back in the tray, got {runs:?}");
        };
        assert_eq!(
            (&run.command, &run.state),
            (&done("q").command, &done("q").state)
        );
    }

    #[gpui::test]
    fn canvas_send_leaves_the_next_canvas_out_of_bash_mode(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.canvas("a", cx);
        rig.type_text("!", cx);
        rig.type_text("pwd", cx);
        rig.press("enter", cx);
        rig.settle(cx, &runtime, |rig, cx| {
            rig.tray_of("space-canvas:a", cx)
                .runs
                .first()
                .is_some_and(|run| run.state != ShellRunState::Running)
        });
        rig.click_send(cx);
        rig.sent_prompt(cx, &runtime);
        rig.canvas("a", cx);
        assert!(!rig.bash(cx));
    }

    #[gpui::test]
    fn conflicted_native_command_edit_keeps_its_blocks(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::EditConflicts);
        rig.begin_edit(cx, &runtime);
        rig.handle
            .update(cx, |composer, _, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("/compact", cx))
            })
            .unwrap();
        rig.press("enter", cx);
        rig.settle_failure(cx, &runtime);
        assert!(rig.tray(cx).runs.is_empty(), "nothing restaged");
        let blocks = crate::shell_blocks::format_prompt(&[done("q")], "");
        rig.handle
            .read_with(cx, |composer, _| {
                assert!(composer.editing_queued.is_some());
                assert_eq!(composer.queue_edit_shell_blocks, blocks);
            })
            .unwrap();
    }
}
