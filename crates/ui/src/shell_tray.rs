//! Bash mode in the composer: a `!` typed into an empty draft turns Enter into
//! "run this on the chat's host", and each run is staged per chat until the
//! next message carries it. Rendering the staged runs lands with the tray.

use gpui::{App, Context};
use zeron_rpc::methods;

use crate::composer::Composer;
use crate::shell_blocks::{ShellRun, ShellRunState};

pub(crate) const BASH_PLACEHOLDER: &str = "Run a command";

/// One chat's bash mode and staged runs, keyed in `Composer::shell_trays` by
/// [`Composer::shell_tray_key`].
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ShellTrayState {
    pub bash_mode: bool,
    pub runs: Vec<ShellRun>,
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

    pub(crate) fn bash_mode(&self, cx: &App) -> bool {
        self.bash_allowed()
            && self
                .shell_trays
                .get(&self.shell_tray_key(cx))
                .is_some_and(|tray| tray.bash_mode)
    }

    pub(crate) fn enter_bash_mode(&mut self, cx: &mut Context<Self>) {
        if !self.bash_allowed() {
            return;
        }
        let key = self.shell_tray_key(cx);
        self.shell_trays.entry(key).or_default().bash_mode = true;
        self.sync_bash_input(cx);
        cx.notify();
    }

    pub(crate) fn leave_bash_mode(&mut self, cx: &mut Context<Self>) {
        let key = self.shell_tray_key(cx);
        if let Some(tray) = self.shell_trays.get_mut(&key) {
            tray.bash_mode = false;
            if tray.runs.is_empty() {
                self.shell_trays.remove(&key);
            }
        }
        self.sync_bash_input(cx);
        cx.notify();
    }

    /// Point the input at the selected chat's mode: what `!`, Backspace and
    /// Escape do, the text face, and the placeholder. Leaving restores the
    /// default placeholder only over bash mode's own, so a question's prompt
    /// is left alone.
    pub(crate) fn sync_bash_input(&mut self, cx: &mut Context<Self>) {
        let allowed = self.bash_allowed();
        let active = self.bash_mode(cx);
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
                crate::toast::error(cx, format!("Couldn't run the command: {err}"));
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
        if !tray.bash_mode && tray.runs.is_empty() {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request_test_support::{self as fixture, ScriptedRpc};
    use gpui::{AppContext as _, TestAppContext, WindowHandle};
    use std::sync::{Arc, Mutex};
    use zeron_rpc::{RpcError, RpcReply};

    type Calls = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

    #[derive(Clone, Copy)]
    enum Host {
        Replies,
        Fails,
        Hangs,
    }

    struct Rig {
        _dir: tempfile::TempDir,
        rpc: Arc<ScriptedRpc>,
        calls: Calls,
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
        let rpc = ScriptedRpc::new({
            let calls = calls.clone();
            move |method, params| {
                calls.lock().unwrap().push((method.clone(), params));
                async move {
                    match (method.as_str(), host) {
                        (methods::RUN_SHELL_COMMAND, Host::Replies) => {
                            RpcReply::value(&serde_json::json!({
                                "exitCode": 0, "output": "a\n",
                                "truncated": false, "timedOut": false,
                            }))
                        }
                        (methods::RUN_SHELL_COMMAND, Host::Fails) => {
                            Err(RpcError::Failed("host offline".into()))
                        }
                        (methods::RUN_SHELL_COMMAND, Host::Hangs) => {
                            futures::future::pending().await
                        }
                        (methods::CANCEL_SHELL_COMMAND, _) => {
                            RpcReply::value(&serde_json::json!({}))
                        }
                        _ => Err(RpcError::UnknownMethod(method)),
                    }
                }
            }
        });
        let state = cx.update(|cx| fixture::state(cx, Some(rpc.client())));
        state.update(cx, |state, _| {
            state.chats = vec![
                serde_json::from_value(serde_json::json!({
                    "id": "chat-a", "deviceId": "box",
                    "archived": false, "createdAt": chrono::Utc::now(),
                }))
                .unwrap(),
            ];
            state.selected_chat = Some("chat-a".into());
        });
        let handle = cx.add_window(|_, cx| Composer::new(state, cx));
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        Rig {
            _dir: dir,
            rpc,
            calls,
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
            self.handle
                .read_with(cx, |composer, _| {
                    composer
                        .shell_trays
                        .get("chat-a")
                        .cloned()
                        .unwrap_or_default()
                })
                .unwrap()
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
        assert!(rig.tray(cx).bash_mode);
        assert_eq!(rig.text(cx), "");
        assert_eq!(rig.placeholder(cx), "Run a command");
    }

    #[gpui::test]
    fn bang_mid_text_is_text(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        rig.type_text("a!", cx);
        assert!(!rig.tray(cx).bash_mode);
        assert_eq!(rig.text(cx), "a!");
    }

    #[gpui::test]
    fn pasted_bang_does_not_enter(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        cx.update(|cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("!ls".into())));
        cx.dispatch_action(rig.handle.into(), crate::composer::Paste);
        assert!(!rig.tray(cx).bash_mode);
        assert_eq!(rig.text(cx), "!ls");
    }

    #[gpui::test]
    fn backspace_on_empty_and_escape_leave(cx: &mut TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rig = window(cx, Host::Replies);
        for key in ["backspace", "escape"] {
            rig.type_text("!", cx);
            assert!(rig.tray(cx).bash_mode, "{key}");
            rig.press(key, cx);
            assert!(!rig.tray(cx).bash_mode, "{key}");
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
        let tray = rig.tray(cx);
        assert!(tray.bash_mode);
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
        assert!(rig.tray(cx).bash_mode);
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
                .any(|toast| toast.as_ref() == format!("Couldn't run the command: {err}")),
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
        assert!(!rig.tray(cx).bash_mode);
        assert_eq!(rig.text(cx), "!");
    }
}
