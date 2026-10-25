use super::*;
use crate::orchestration::new_dialog::{
    ChildChoice, DialogState, FolderOutcome, OpenDialog, ProjectChoice, ProjectSource,
    build_projects, display_path, parse_created, parse_models,
};
use crate::settings::widgets::{self, SelectOption};
use zeron_engine::registry::HarnessDescriptor;
use zeron_proto::ReasoningLevel;

const DIALOG_WIDTH: f32 = 560.0;
const LABEL_WIDTH: f32 = 76.0;
const SELECT_WIDTH: f32 = 250.0;
const MODEL_WIDTH: f32 = 250.0;
const REASONING_WIDTH: f32 = 110.0;
const PROJECT_LIST_HEIGHT: f32 = 240.0;

impl Shell {
    pub(super) fn open_new_orchestrator(&mut self, cx: &mut Context<Self>) {
        self.close_sidebar_view_menu(cx);
        self.section_menu = None;
        let state = self.state.read(cx);
        let projects = build_projects(local_project_sources(state), &[], folder_exists);
        let engine = state.engine().cloned();
        let input = cx.new(|cx| ComposerInput::new("Name", cx));
        let events = cx.subscribe(&input, |this: &mut Shell, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.submit_new_orchestrator(cx);
            } else {
                cx.notify();
            }
        });
        self.new_orchestrator.sessions += 1;
        let session = self.new_orchestrator.sessions;
        self.new_orchestrator.selects = Default::default();
        let mut dialog_state = DialogState::new(projects);
        if engine.is_none() {
            dialog_state.set_providers_error("Engine not connected".into());
        }
        let search =
            cx.new(|cx| ComposerInput::with_context("Search projects…", "PaletteSearch", cx));
        let search_events = cx.subscribe(&search, |this: &mut Shell, input, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                let text = input.read(cx).text().to_string();
                this.edit_orchestrator_dialog(cx, |state| state.set_search(&text));
            }
        });
        self.new_orchestrator.open = Some(OpenDialog {
            session,
            state: dialog_state,
            name: input,
            search,
            _search_events: search_events,
            card_focus: cx.focus_handle(),
            focus_pending: true,
            _events: events,
        });
        if let Some(engine) = engine {
            self.load_orchestrator_providers(session, engine, cx);
        }
        cx.notify();
    }

    pub(super) fn close_new_orchestrator(&mut self, cx: &mut Context<Self>) {
        if self.new_orchestrator.open.take().is_some() {
            cx.notify();
        }
    }

    fn orchestrator_dialog_mut(
        &mut self,
        session: u64,
        engine: &crate::state::EngineHandle,
        cx: &App,
    ) -> Option<&mut OpenDialog> {
        let current = self
            .state
            .read(cx)
            .engine()
            .is_some_and(|current| current.same_connection(engine));
        if !current {
            return None;
        }
        self.new_orchestrator
            .open
            .as_mut()
            .filter(|dialog| dialog.session == session)
    }

    fn load_orchestrator_providers(
        &mut self,
        session: u64,
        engine: crate::state::EngineHandle,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let listed = engine
                .client()
                .call(methods::LIST_HARNESSES, serde_json::json!({}))
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<HarnessDescriptor>>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |this, cx| {
                let Some(dialog) = this.orchestrator_dialog_mut(session, &engine, cx) else {
                    return;
                };
                let providers = match listed {
                    Ok(list) => {
                        dialog.state.set_providers(&list);
                        dialog
                            .state
                            .provider_list()
                            .iter()
                            .map(|choice| choice.id)
                            .collect()
                    }
                    Err(error) => {
                        dialog.state.set_providers_error(error);
                        Vec::new()
                    }
                };
                for harness in providers {
                    this.load_orchestrator_models(session, harness, engine.clone(), cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_orchestrator_models(
        &mut self,
        session: u64,
        harness: HarnessId,
        engine: crate::state::EngineHandle,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let loaded = match engine
                .client()
                .call(
                    methods::LIST_MODELS,
                    serde_json::json!({ "harness": harness, "force": false }),
                )
                .await
            {
                Ok(value) => parse_models(harness, value),
                Err(error) => Loadable::Error(error.to_string()),
            };
            this.update(cx, |this, cx| {
                if let Some(dialog) = this.orchestrator_dialog_mut(session, &engine, cx) {
                    dialog.state.set_models(harness, loaded);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn submit_new_orchestrator(&mut self, cx: &mut Context<Self>) {
        let engine = self.state.read(cx).engine().cloned();
        let Some(dialog) = self.new_orchestrator.open.as_mut() else {
            return;
        };
        let name = dialog.name.read(cx).text();
        let session = dialog.session;
        let params = dialog.state.begin_submit(&name);
        let Some(params) = params else {
            cx.notify();
            return;
        };
        let Some(engine) = engine else {
            dialog
                .state
                .finish_submit(Err("Engine not connected".into()));
            cx.notify();
            return;
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let created = engine
                .client()
                .call(methods::CREATE_ORCHESTRATOR, params)
                .await
                .map_err(|error| error.to_string())
                .and_then(parse_created);
            this.update(cx, |this, cx| {
                let current = this
                    .state
                    .read(cx)
                    .engine()
                    .is_some_and(|current| current.same_connection(&engine));
                let created = if current {
                    created
                } else {
                    Err("The connection changed. Try again.".to_owned())
                };
                this.land_new_orchestrator(session, created, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn land_new_orchestrator(
        &mut self,
        session: u64,
        created: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self
            .new_orchestrator
            .open
            .as_mut()
            .filter(|dialog| dialog.session == session)
        else {
            return;
        };
        match created {
            Ok(chat_id) => {
                self.new_orchestrator.open = None;
                self.open_created_orchestrator(chat_id, cx);
            }
            Err(message) => dialog.state.finish_submit(Err(message)),
        }
        cx.notify();
    }

    fn open_created_orchestrator(&mut self, chat_id: String, cx: &mut Context<Self>) {
        let known = self
            .state
            .read(cx)
            .chats
            .iter()
            .any(|chat| chat.id == chat_id);
        if known {
            self.new_orchestrator.pending_chat = None;
            self.reveal_orchestrator_row(&chat_id, cx);
            self.open_chat(chat_id, cx);
            return;
        }
        self.new_orchestrator.pending_chat = Some(chat_id.clone());
        self.new_orchestrator.pending_baseline = self.state.read(cx).selected_chat.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(30))
                .await;
            this.update(cx, |this, _| {
                if this.new_orchestrator.pending_chat.as_ref() == Some(&chat_id) {
                    this.new_orchestrator.pending_chat = None;
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn open_pending_orchestrator(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.new_orchestrator.pending_chat.clone() else {
            return;
        };
        if self.state.read(cx).selected_chat != self.new_orchestrator.pending_baseline {
            self.new_orchestrator.pending_chat = None;
            return;
        }
        if self
            .state
            .read(cx)
            .chats
            .iter()
            .any(|chat| chat.id == chat_id)
        {
            self.open_created_orchestrator(chat_id, cx);
        }
    }

    pub(super) fn sync_new_orchestrator(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.new_orchestrator.open.as_ref() {
            let sources = local_project_sources(self.state.read(cx));
            if !dialog.state.same_sources(&sources) {
                let projects = build_projects(sources, &dialog.state.projects, folder_exists);
                if let Some(dialog) = self.new_orchestrator.open.as_mut() {
                    dialog.state.set_projects(projects);
                }
                cx.notify();
            }
        }
        self.open_pending_orchestrator(cx);
    }

    fn pick_orchestrator_folder(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.new_orchestrator.open.as_ref().map(|d| d.session) else {
            return;
        };
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add Folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.add_orchestrator_folder(session, path, cx)
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn add_orchestrator_folder(
        &mut self,
        session: u64,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let path = path.to_string_lossy().into_owned();
        let state = self.state.read(cx);
        let engine = state.engine().cloned();
        let device = state.local_device_id.clone();
        let Some(dialog) = self
            .new_orchestrator
            .open
            .as_mut()
            .filter(|dialog| dialog.session == session)
        else {
            return;
        };
        if dialog.state.pick_folder(&path) == FolderOutcome::Selected {
            cx.notify();
            return;
        }
        let (Some(engine), Some(device)) = (engine, device) else {
            dialog.state.fail("Engine not connected".into());
            cx.notify();
            return;
        };
        let space_id = uuid::Uuid::new_v4().to_string();
        let git_detected = std::path::Path::new(&path).join(".git").exists();
        dialog.state.expect_project(space_id.clone());
        let space = zeron_proto::Space {
            id: space_id.clone(),
            device_id: device.clone(),
            path: path.clone(),
            name: None,
            git_detected,
            git_checked_at: None,
            checkout_id: None,
            repository_id: None,
            github_repo: None,
            created_at: Utc::now(),
        };
        self.state.update(cx, |state, cx| {
            if !state.spaces.iter().any(|existing| existing.id == space.id) {
                state.spaces.push(space);
            }
            cx.notify();
        });
        let params = serde_json::json!({
            "op": "createSpace",
            "spaceId": space_id,
            "deviceId": device,
            "path": path,
            "gitDetected": git_detected,
        });
        cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::MUTATE, params).await;
            let Err(error) = result else {
                return;
            };
            this.update(cx, |this, cx| {
                this.state.update(cx, |state, cx| {
                    state.spaces.retain(|space| space.id != space_id);
                    cx.notify();
                });
                if let Some(dialog) = this
                    .new_orchestrator
                    .open
                    .as_mut()
                    .filter(|dialog| dialog.session == session)
                {
                    dialog.state.fail(error.to_string());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn edit_orchestrator_dialog(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut DialogState),
    ) {
        if let Some(dialog) = self.new_orchestrator.open.as_mut() {
            edit(&mut dialog.state);
            cx.notify();
        }
    }

    fn orchestrator_label_row(
        theme: &Theme,
        label: &'static str,
        control: impl IntoElement,
    ) -> gpui::Div {
        div()
            .mt(px(12.0))
            .flex()
            .flex_row()
            .items_start()
            .gap(px(8.0))
            .child(
                div()
                    .w(px(LABEL_WIDTH))
                    .flex_none()
                    .h(px(widgets::SELECT_HEIGHT))
                    .flex()
                    .items_center()
                    .text_size(crate::typography::ui_rems(13.0))
                    .text_color(theme.text_muted)
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(control))
    }

    fn orchestrator_project_row(
        theme: &Theme,
        project: &ProjectChoice,
        checked: bool,
        home: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = project.id.clone();
        let missing = project.missing;
        let selector = format!("new-orchestrator-project-{}", project.id);
        let path = if missing {
            "Folder not found".to_owned()
        } else {
            display_path(&project.path, home)
        };
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(6.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .role(gpui::Role::CheckBox)
            .aria_label(SharedString::from(project.label.clone()))
            .aria_toggled(if checked {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .when(missing, |el| el.opacity(0.5))
            .when(!missing, |el| {
                el.cursor_pointer()
                    .hover(|el| el.bg(theme.glass_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit_orchestrator_dialog(cx, |state| state.toggle_project(&id));
                    }))
            })
            .child(
                div()
                    .size(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(theme.text_muted.opacity(0.5))
                    .when(checked, |el| {
                        el.bg(theme.solid)
                            .border_color(theme.solid)
                            .child(icon(icons::CHECK).size(px(12.0)).text_color(theme.on_solid))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(px(200.0))
                    .truncate()
                    .text_size(crate::typography::ui_rems(13.0))
                    .child(SharedString::from(project.label.clone())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(11.5))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(path)),
            )
            .into_any_element()
    }

    pub(super) fn render_new_orchestrator_overlay(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let dialog = self.new_orchestrator.open.as_mut()?;
        if std::mem::take(&mut dialog.focus_pending) {
            window.focus(&dialog.name.focus_handle(cx), cx);
        }
        let dialog = self.new_orchestrator.open.as_ref()?;
        let selects = &self.new_orchestrator.selects;
        let state = &dialog.state;
        let can_create = state.can_submit();

        let home = std::env::var("HOME").ok();
        let visible = state.visible_projects();
        let list = (!state.projects.is_empty()).then(|| {
            div()
                .id("new-orchestrator-projects")
                .max_h(px(PROJECT_LIST_HEIGHT))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .p(px(4.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(crate::theme::hairline(0.08))
                .children(visible.iter().map(|project| {
                    let checked = state.checked.contains(&project.id);
                    Self::orchestrator_project_row(&theme, project, checked, home.as_deref(), cx)
                }))
                .when(visible.is_empty(), |el| {
                    el.child(
                        div()
                            .id("new-orchestrator-no-match")
                            .debug_selector(|| "new-orchestrator-no-match".into())
                            .px(px(6.0))
                            .py(px(5.0))
                            .text_size(crate::typography::ui_rems(12.5))
                            .text_color(theme.text_muted)
                            .child("No projects match."),
                    )
                })
        });
        let selected_note = state.selected_note().map(|text| {
            div()
                .id("new-orchestrator-selected")
                .debug_selector(|| "new-orchestrator-selected".into())
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .child(SharedString::from(text))
        });
        let projects = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        popover::dialog_field(dialog.search.clone().into_any_element())
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        widgets::action_button(&theme, widgets::ActionTone::Filled)
                            .id("new-orchestrator-add-folder")
                            .debug_selector(|| "new-orchestrator-add-folder".into())
                            .flex_none()
                            .child("Add folder…")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.pick_orchestrator_folder(cx)),
                            ),
                    ),
            )
            .children(list)
            .children(selected_note);

        let providers = state.provider_list();
        let provider_loading = state.providers.is_loading().then(|| {
            div().child(Self::orchestrator_label_row(
                &theme,
                "Provider",
                muted_note(&theme, "Loading…"),
            ))
        });
        let host_rows = (!providers.is_empty()).then(|| {
            let ids: Vec<HarnessId> = providers.iter().map(|choice| choice.id).collect();
            let selected = ids
                .iter()
                .position(|id| Some(*id) == state.provider)
                .unwrap_or(0);
            let provider = widgets::select(
                "new-orchestrator-provider",
                "Provider",
                &theme,
                |shell: &mut Shell| &mut shell.new_orchestrator.selects.provider,
            )
            .width(SELECT_WIDTH)
            .open_rightward()
            .options(
                providers
                    .iter()
                    .map(|choice| SelectOption::new(choice.name.clone())),
                selected,
            )
            .on_select(move |shell, ix, _, cx| {
                if let Some(id) = ids.get(ix).copied() {
                    shell.edit_orchestrator_dialog(cx, |state| state.select_provider(id));
                }
            })
            .render(&selects.provider, cx);

            let model_control = match state.current_models() {
                Some(Loadable::Ready(models)) if !models.is_empty() => {
                    let ids: Vec<String> = models.iter().map(|model| model.id.clone()).collect();
                    let selected = ids
                        .iter()
                        .position(|id| Some(id.as_str()) == state.model.as_deref())
                        .unwrap_or(0);
                    let model = widgets::select(
                        "new-orchestrator-model",
                        "Model",
                        &theme,
                        |shell: &mut Shell| &mut shell.new_orchestrator.selects.model,
                    )
                    .width(MODEL_WIDTH)
                    .open_rightward()
                    .options(
                        models
                            .iter()
                            .map(|model| SelectOption::new(model.label.clone())),
                        selected,
                    )
                    .on_select(move |shell, ix, _, cx| {
                        if let Some(id) = ids.get(ix) {
                            shell.edit_orchestrator_dialog(cx, |state| state.select_model(id));
                        }
                    })
                    .render(&selects.model, cx)
                    .into_any_element();
                    let levels: Vec<ReasoningLevel> = state
                        .current_model()
                        .map(|model| model.reasoning_levels.clone())
                        .unwrap_or_default();
                    let reasoning = (!levels.is_empty()).then(|| {
                        let selected = levels
                            .iter()
                            .position(|level| Some(*level) == state.reasoning)
                            .unwrap_or(0);
                        let options = levels
                            .iter()
                            .map(|level| SelectOption::new(crate::pickers::reasoning_label(*level)))
                            .collect::<Vec<_>>();
                        widgets::select(
                            "new-orchestrator-reasoning",
                            "Reasoning",
                            &theme,
                            |shell: &mut Shell| &mut shell.new_orchestrator.selects.reasoning,
                        )
                        .width(REASONING_WIDTH)
                        .options(options, selected)
                        .on_select(move |shell, ix, _, cx| {
                            if let Some(level) = levels.get(ix).copied() {
                                shell.edit_orchestrator_dialog(cx, |state| {
                                    state.select_reasoning(level)
                                });
                            }
                        })
                        .render(&selects.reasoning, cx)
                    });
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(8.0))
                        .child(model)
                        .children(reasoning)
                        .into_any_element()
                }
                _ => muted_note(&theme, state.model_note().unwrap_or("Default")),
            };

            let options = state.child_options();
            let selected = options
                .iter()
                .position(|(choice, _)| *choice == state.child)
                .unwrap_or(0);
            let choices: Vec<ChildChoice> = options.iter().map(|(choice, _)| *choice).collect();
            let children = widgets::select(
                "new-orchestrator-children",
                "Children",
                &theme,
                |shell: &mut Shell| &mut shell.new_orchestrator.selects.children,
            )
            .width(SELECT_WIDTH)
            .open_rightward()
            .options(
                options
                    .iter()
                    .map(|(_, name)| SelectOption::new(name.clone())),
                selected,
            )
            .on_select(move |shell, ix, _, cx| {
                if let Some(choice) = choices.get(ix).copied() {
                    shell.edit_orchestrator_dialog(cx, |state| state.set_child(choice));
                }
            })
            .render(&selects.children, cx);

            div()
                .child(Self::orchestrator_label_row(&theme, "Provider", provider))
                .child(Self::orchestrator_label_row(&theme, "Model", model_control))
                .child(Self::orchestrator_label_row(&theme, "Children", children))
        });

        let notice = state.notice().map(|text| {
            let failed = state.error.is_some();
            div()
                .id("new-orchestrator-notice")
                .debug_selector(|| "new-orchestrator-notice".into())
                .mt(px(12.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(if failed {
                    theme.danger
                } else {
                    theme.text_muted
                })
                .child(SharedString::from(text))
        });

        let card = popover::dialog_card(&theme)
            .w(px(DIALOG_WIDTH))
            .track_focus(&dialog.card_focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close_new_orchestrator(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(popover::dialog_title(&theme, "New orchestrator"))
                    .child(
                        div()
                            .id("new-orchestrator-close")
                            .size(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.close_new_orchestrator(cx)))
                            .child(
                                icon(icons::CLOSE)
                                    .size(px(16.0))
                                    .text_color(theme.text_muted),
                            ),
                    ),
            )
            .child(Self::orchestrator_label_row(
                &theme,
                "Name",
                popover::dialog_field(dialog.name.clone().into_any_element()),
            ))
            .child(Self::orchestrator_label_row(&theme, "Projects", projects))
            .children(provider_loading)
            .children(host_rows)
            .children(notice)
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(&theme, "Cancel", "new-orchestrator-cancel")
                            .id("new-orchestrator-cancel")
                            .debug_selector(|| "new-orchestrator-cancel".into())
                            .on_click(
                                cx.listener(|this, _, _, cx| this.close_new_orchestrator(cx)),
                            ),
                    )
                    .child(
                        popover::btn_primary(&theme, "Create")
                            .id("new-orchestrator-create")
                            .debug_selector(|| "new-orchestrator-create".into())
                            .opacity(if can_create { 1.0 } else { 0.5 })
                            .on_click(
                                cx.listener(|this, _, _, cx| this.submit_new_orchestrator(cx)),
                            ),
                    ),
            );
        Some(popover::modal(
            "new-orchestrator-dialog",
            viewport,
            card.into_any_element(),
        ))
    }
}

fn folder_exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

fn local_project_sources(state: &AppState) -> Vec<ProjectSource> {
    let local = state.local_device_id.as_deref();
    state
        .spaces_sorted()
        .into_iter()
        .filter(|space| local == Some(space.device_id.as_str()))
        .map(|space| ProjectSource {
            id: space.id.clone(),
            label: space.display_name().to_owned(),
            path: space.path.clone(),
        })
        .collect()
}

fn muted_note(theme: &Theme, text: &'static str) -> AnyElement {
    div()
        .h(px(widgets::SELECT_HEIGHT))
        .flex()
        .items_center()
        .text_size(crate::typography::ui_rems(12.5))
        .text_color(theme.text_muted)
        .child(text)
        .into_any_element()
}
