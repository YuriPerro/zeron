use super::*;
use crate::orchestration::sidebar::{
    OrchestratorGroup, is_orchestrator, need_you_summary, needing_you, orchestrator_groups,
};
use spaces::{
    ActiveChatRow, SIDEBAR_DISCLOSURE_BODY_INSET, SIDEBAR_DISCLOSURE_HEADER_HEIGHT,
    SIDEBAR_SECTION_GAP,
};
use std::collections::HashMap;
use zeron_proto::ChatIndicator;

const ORCHESTRATORS_KEY: &str = "orchestrators";
const CHEVRON_COLUMN: f32 = 16.0;
const CHILD_INDENT: f32 = 32.0;

type SectionRow = (f32, AnyElement);

fn fold_key(chat_id: &str) -> String {
    format!("orchestrator-fold:{chat_id}")
}

fn done_key(chat_id: &str) -> String {
    format!("orchestrator-done:{chat_id}")
}

fn orchestrator_entries<'a>(
    state: &'a AppState,
    now: chrono::DateTime<Utc>,
) -> Vec<(&'a zeron_proto::Chat, ChatIndicator)> {
    let orchestrators: std::collections::HashSet<&str> = state
        .chats
        .iter()
        .filter(|chat| is_orchestrator(chat))
        .map(|chat| chat.id.as_str())
        .collect();
    if orchestrators.is_empty() {
        return Vec::new();
    }
    state
        .chats
        .iter()
        .filter(|chat| {
            orchestrators.contains(chat.id.as_str())
                || chat
                    .parent_chat_id
                    .as_deref()
                    .is_some_and(|parent| orchestrators.contains(parent))
        })
        .map(|chat| (chat, state.display_status_for(chat, now)))
        .collect()
}

fn row_height(settings: &UiSettings, row: &ActiveChatRow) -> f32 {
    sidebar_row_height(
        settings.sidebar_compact,
        settings.sidebar_show_project_label,
        row.branch.is_some(),
        row.change_request.is_some(),
    )
}

fn stacked_height(heights: impl Iterator<Item = f32>) -> f32 {
    let (total, count) = heights.fold((0.0, 0usize), |(total, count), height| {
        (total + height, count + 1)
    });
    total + SIDEBAR_LIST_GAP * count.saturating_sub(1) as f32
}

impl Shell {
    pub(super) fn orchestrator_row_visible(&self, chat_id: &str, cx: &App) -> bool {
        let state = self.state.read(cx);
        let Some(chat) = state.chats.iter().find(|chat| chat.id == chat_id) else {
            return false;
        };
        if !is_orchestrator(chat) && chat.parent_chat_id.is_none() {
            return false;
        }
        if self.sidebar_collapsed_groups.contains(ORCHESTRATORS_KEY) {
            return false;
        }
        let entries = orchestrator_entries(state, Utc::now());
        orchestrator_groups(&entries).iter().any(|group| {
            let folded = self
                .sidebar_collapsed_groups
                .contains(&fold_key(&group.chat_id));
            if folded && group.chat_id != chat_id {
                return false;
            }
            let done_shown = self.orchestrator_done_open.contains(&group.chat_id);
            group.chat_id == chat_id
                || group.active_children.iter().any(|id| id == chat_id)
                || (done_shown && group.done_children.iter().any(|id| id == chat_id))
        })
    }

    pub(super) fn in_orchestrator_section(&self, chat_id: &str, cx: &App) -> bool {
        let chats = &self.state.read(cx).chats;
        chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .is_some_and(|chat| {
                is_orchestrator(chat)
                    || chat.parent_chat_id.as_deref().is_some_and(|parent| {
                        chats
                            .iter()
                            .any(|candidate| candidate.id == parent && is_orchestrator(candidate))
                    })
            })
    }

    pub(super) fn reveal_orchestrator_row(&mut self, chat_id: &str, cx: &App) -> bool {
        let state = self.state.read(cx);
        let entries = orchestrator_entries(state, Utc::now());
        let groups = orchestrator_groups(&entries);
        let Some(group) = groups.iter().find(|group| {
            group.chat_id == chat_id
                || group.active_children.iter().any(|id| id == chat_id)
                || group.done_children.iter().any(|id| id == chat_id)
        }) else {
            return false;
        };
        let in_done = group.done_children.iter().any(|id| id == chat_id);
        let group_id = group.chat_id.clone();
        let is_child = group_id != chat_id;
        if self.sidebar_collapsed_groups.remove(ORCHESTRATORS_KEY) {
            self.queue_sidebar_reveal(ORCHESTRATORS_KEY);
        }
        if is_child && self.sidebar_collapsed_groups.remove(&fold_key(&group_id)) {
            self.queue_sidebar_reveal(&fold_key(&group_id));
        }
        if in_done && self.orchestrator_done_open.insert(group_id.clone()) {
            self.queue_sidebar_reveal(&done_key(&group_id));
        }
        true
    }

    fn orchestrator_row_data(
        &self,
        now: chrono::DateTime<Utc>,
        cx: &App,
    ) -> (Vec<OrchestratorGroup>, HashMap<String, ActiveChatRow>) {
        let state = self.state.read(cx);
        let entries = orchestrator_entries(state, now);
        let groups = orchestrator_groups(&entries);
        let rows = entries
            .into_iter()
            .map(|(chat, status)| {
                (
                    chat.id.clone(),
                    self.sidebar_chat_data(status, chat.clone(), state),
                )
            })
            .collect();
        (groups, rows)
    }

    fn render_orchestrator_chat_row(
        &mut self,
        row: ActiveChatRow,
        title: String,
        indent: bool,
        now: chrono::DateTime<Utc>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> SectionRow {
        let height = row_height(&self.settings, &row);
        let ActiveChatRow {
            status,
            chat,
            folder,
            branch,
            change_request,
            ..
        } = row;
        let selected = self.state.read(cx).selected_chat.as_deref() == Some(chat.id.as_str());
        let harness = self
            .settings
            .sidebar_show_harness
            .then(|| chat.config.as_ref().map(|config| config.harness))
            .flatten();
        let element = self.render_chat_row(
            chat.id.clone(),
            transcript::single_line(&title).into(),
            format_time_ago(chat.last_message_at.unwrap_or(chat.created_at), now).into(),
            folder.into(),
            branch.map(SharedString::from),
            change_request,
            harness,
            status,
            selected,
            false,
            false,
            None,
            None,
            self.settings.sidebar_show_project_icon,
            None,
            theme,
            cx,
        );
        let element = div()
            .w_full()
            .when(indent, |el| el.pl(px(CHILD_INDENT)))
            .child(element)
            .into_any_element();
        (height, element)
    }

    fn render_orchestrator_done(
        &mut self,
        group: &OrchestratorGroup,
        data: &mut HashMap<String, ActiveChatRow>,
        now: chrono::DateTime<Utc>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> SectionRow {
        let key = done_key(&group.chat_id);
        let open = self.orchestrator_done_open.contains(&group.chat_id);
        let rows: Vec<ActiveChatRow> = group
            .done_children
            .iter()
            .filter_map(|id| data.remove(id))
            .collect();
        let list_height = SIDEBAR_LIST_GAP
            + stacked_height(rows.iter().map(|row| row_height(&self.settings, row)));
        self.begin_queued_sidebar_reveal(&key, open, list_height);
        let motion = self
            .sidebar_disclosure_motion
            .get(&key)
            .copied()
            .filter(|motion| motion.animating());
        let animating = motion.is_some();
        let list_now = motion.map_or(if open { list_height } else { 0.0 }, |m| m.current());

        let body = (open || animating).then(|| {
            let items: Vec<AnyElement> = rows
                .into_iter()
                .map(|row| {
                    let title = row
                        .chat
                        .title
                        .clone()
                        .unwrap_or_else(|| "New session".into());
                    self.render_orchestrator_chat_row(row, title, true, now, theme, cx)
                        .1
                })
                .collect();
            let list = div()
                .w_full()
                .flex()
                .flex_col()
                .pt(px(SIDEBAR_LIST_GAP))
                .gap(px(SIDEBAR_LIST_GAP))
                .children(items);
            self.render_sidebar_disclosure_body(&key, open, list_height, list.into_any_element())
        });

        let chevron = self.sidebar_disclosure_chevron(&key, open, theme);
        let toggled = group.chat_id.clone();
        let motion_key = key.clone();
        let selector = format!("orchestrator-done-{}", group.chat_id);
        let header = spaces::sidebar_disclosure_header(
            theme,
            None,
            format!("{} done", group.done_children.len()).into(),
            None,
            None,
            chevron,
        )
        .ml(px(CHILD_INDENT))
        .id(SharedString::from(key))
        .debug_selector(move || selector.clone())
        .on_click(cx.listener(move |this, _, _, cx| {
            let was_open = this.orchestrator_done_open.remove(&toggled);
            if !was_open {
                this.orchestrator_done_open.insert(toggled.clone());
            }
            this.begin_sidebar_disclosure_motion(
                &motion_key,
                if was_open { list_height } else { 0.0 },
                if was_open { 0.0 } else { list_height },
            );
            cx.notify();
        }));
        let height = SIDEBAR_DISCLOSURE_HEADER_HEIGHT + list_now;
        let element = div()
            .w_full()
            .flex()
            .flex_col()
            .child(header)
            .children(body)
            .into_any_element();
        (height, element)
    }

    fn render_orchestrator_group(
        &mut self,
        group: &OrchestratorGroup,
        data: &mut HashMap<String, ActiveChatRow>,
        now: chrono::DateTime<Utc>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<SectionRow> {
        let mut head_row = data.remove(&group.chat_id)?;
        head_row.status = group.indicator;
        let (head_height, head) =
            self.render_orchestrator_chat_row(head_row, group.name.clone(), false, now, theme, cx);
        let mut children: Vec<SectionRow> = Vec::new();
        for child_id in &group.active_children {
            if let Some(row) = data.remove(child_id) {
                let title = row
                    .chat
                    .title
                    .clone()
                    .unwrap_or_else(|| "New session".into());
                children.push(self.render_orchestrator_chat_row(row, title, true, now, theme, cx));
            }
        }
        if !group.done_children.is_empty() {
            children.push(self.render_orchestrator_done(group, data, now, theme, cx));
        }

        let key = fold_key(&group.chat_id);
        let open = !self.sidebar_collapsed_groups.contains(&key);
        let has_children = !children.is_empty();
        let list_height =
            SIDEBAR_LIST_GAP + stacked_height(children.iter().map(|(height, _)| *height));
        self.begin_queued_sidebar_reveal(&key, open, list_height);
        let motion = self
            .sidebar_disclosure_motion
            .get(&key)
            .copied()
            .filter(|motion| motion.animating());
        let animating = motion.is_some();
        let list_now = motion.map_or(if open { list_height } else { 0.0 }, |m| m.current());
        let body = (has_children && (open || animating)).then(|| {
            let list = div()
                .w_full()
                .flex()
                .flex_col()
                .pt(px(SIDEBAR_LIST_GAP))
                .gap(px(SIDEBAR_LIST_GAP))
                .children(children.into_iter().map(|(_, element)| element));
            self.render_sidebar_disclosure_body(&key, open, list_height, list.into_any_element())
        });

        let chevron_cell = div()
            .w(px(CHEVRON_COLUMN))
            .flex_none()
            .flex()
            .justify_center();
        let chevron_cell = if has_children {
            let toggled = key.clone();
            let selector = format!("orchestrator-fold-{}", group.chat_id);
            chevron_cell
                .id(SharedString::from(key.clone()))
                .debug_selector(move || selector.clone())
                .cursor_pointer()
                .role(gpui::Role::Button)
                .aria_label("Fold orchestrator")
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let was_open = !this.sidebar_collapsed_groups.contains(&toggled);
                    this.begin_sidebar_disclosure_motion(
                        &toggled,
                        if was_open { list_height } else { 0.0 },
                        if was_open { 0.0 } else { list_height },
                    );
                    if was_open {
                        this.sidebar_collapsed_groups.insert(toggled.clone());
                    } else {
                        this.sidebar_collapsed_groups.remove(&toggled);
                    }
                    cx.notify();
                }))
                .child(self.sidebar_disclosure_chevron(&key, open, theme))
                .into_any_element()
        } else {
            chevron_cell.into_any_element()
        };
        let head = div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .child(chevron_cell)
            .child(div().flex_1().min_w_0().child(head));
        let element = div()
            .w_full()
            .flex()
            .flex_col()
            .child(head)
            .children(body)
            .into_any_element();
        let height = head_height + if has_children { list_now } else { 0.0 };
        Some((height, element))
    }

    fn render_orchestrators_add(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("orchestrators-add")
            .debug_selector(|| "orchestrators-add".into())
            .size(px(20.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .cursor_pointer()
            .role(gpui::Role::Button)
            .aria_label("New orchestrator")
            .hover(|el| el.bg(theme.glass_hover()))
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.open_new_orchestrator(cx);
            }))
            .tooltip(crate::settings::widgets::text_tooltip_above(
                "New orchestrator",
            ))
            .child(
                icon(icons::PLUS)
                    .size(px(14.0))
                    .text_color(spaces::sidebar_header_glyph(theme)),
            )
            .into_any_element()
    }

    pub(super) fn render_orchestrators_section(
        &mut self,
        gap_below: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Utc::now();
        let (groups, mut data) = self.orchestrator_row_data(now, cx);
        let mut items: Vec<SectionRow> = Vec::new();
        for group in &groups {
            if let Some(item) = self.render_orchestrator_group(group, &mut data, now, theme, cx) {
                items.push(item);
            }
        }

        let collapsed = self.sidebar_collapsed_groups.contains(ORCHESTRATORS_KEY);
        let has_rows = !items.is_empty();
        let body_height =
            SIDEBAR_DISCLOSURE_BODY_INSET + stacked_height(items.iter().map(|(height, _)| *height));
        self.begin_queued_sidebar_reveal(ORCHESTRATORS_KEY, !collapsed, body_height);
        let summary = collapsed
            .then(|| need_you_summary(needing_you(&groups)))
            .flatten();
        let count = (collapsed && summary.is_none() && !groups.is_empty()).then_some(groups.len());
        let action = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .children(summary.map(|text| {
                div()
                    .text_size(crate::typography::ui_rems(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(spaces::status_dot_color(
                        ChatIndicator::AwaitingInput,
                        theme,
                    ))
                    .child(SharedString::from(text))
            }))
            .child(self.render_orchestrators_add(theme, cx))
            .into_any_element();
        let chevron = if has_rows {
            self.sidebar_disclosure_chevron(ORCHESTRATORS_KEY, !collapsed, theme)
        } else {
            div().flex_none().size(px(12.0)).into_any_element()
        };
        let header = spaces::sidebar_disclosure_header(
            theme,
            None,
            "Orchestrators".into(),
            count,
            Some(action),
            chevron,
        )
        .id("orchestrators-toggle")
        .debug_selector(|| "orchestrators-toggle".into())
        .when(has_rows, |header| {
            header.on_click(cx.listener(move |this, _, _, cx| {
                let was_open = !this.sidebar_collapsed_groups.contains(ORCHESTRATORS_KEY);
                this.begin_sidebar_disclosure_motion(
                    ORCHESTRATORS_KEY,
                    if was_open { body_height } else { 0.0 },
                    if was_open { 0.0 } else { body_height },
                );
                if was_open {
                    this.sidebar_collapsed_groups
                        .insert(ORCHESTRATORS_KEY.to_owned());
                } else {
                    this.sidebar_collapsed_groups.remove(ORCHESTRATORS_KEY);
                }
                cx.notify();
            }))
        });
        let body = has_rows.then(|| {
            let list = div()
                .w_full()
                .flex()
                .flex_col()
                .pt(px(SIDEBAR_DISCLOSURE_BODY_INSET))
                .gap(px(SIDEBAR_LIST_GAP))
                .children(items.into_iter().map(|(_, element)| element));
            self.render_sidebar_disclosure_body(
                ORCHESTRATORS_KEY,
                !collapsed,
                body_height,
                list.into_any_element(),
            )
        });
        let painted_height = self.orchestrators_height.clone();
        div()
            .id("sidebar-orchestrators-section")
            .relative()
            .flex()
            .flex_col()
            .when(gap_below, |el| el.pb(px(SIDEBAR_SECTION_GAP)))
            .child(header)
            .children(body)
            .child(
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, _, _| painted_height.set(f32::from(bounds.size.height)),
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::sidebar::test_support::chat;
    use gpui::MouseButton;

    struct SidebarHost(Entity<Shell>);

    impl Render for SidebarHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |shell, cx| {
                let viewport = window.viewport_size();
                let overlay = shell.render_new_orchestrator_overlay(viewport, window, cx);
                div()
                    .w(px(280.0))
                    .h(px(800.0))
                    .child(shell.render_chat_sidebar(&Theme::default(), cx))
                    .children(overlay)
            })
        }
    }

    fn session(chat_id: &str, status: zeron_proto::SessionStatus) -> zeron_proto::Session {
        zeron_proto::Session {
            last_completed_turn: None,
            last_turn_outcome: None,
            terminal_activity: None,
            running_subagents: 0,
            chat_id: chat_id.into(),
            device_id: "dev".into(),
            status,
            started_at: None,
            updated_at: Utc::now(),
        }
    }

    fn sidebar(
        cx: &mut gpui::TestAppContext,
        organization: SidebarOrganization,
        chats: Vec<zeron_proto::Chat>,
        sessions: Vec<zeron_proto::Session>,
    ) -> (Entity<Shell>, &mut gpui::VisualTestContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            SidebarHost(cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.keep(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zeron_proto::HarnessId::Mock,
                    },
                    cx,
                );
                shell.settings.sidebar_organization = organization;
                shell.state.update(cx, |state, _| {
                    state.chats = chats;
                    state.sessions = sessions;
                });
                shell
            }))
        });
        let shell = host.read_with(cx, |host, _| host.0.clone());
        (shell, cx)
    }

    fn redraw(cx: &mut gpui::VisualTestContext) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
    }

    fn click(cx: &mut gpui::VisualTestContext, selector: &'static str) {
        let point = cx.debug_bounds(selector).unwrap().center();
        cx.simulate_mouse_down(point, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(point, MouseButton::Left, gpui::Modifiers::default());
    }

    fn fixture() -> Vec<zeron_proto::Chat> {
        vec![
            chat("orch", None, true, 1),
            chat("work", Some("orch"), false, 2),
            chat("idle", Some("orch"), false, 3),
            chat("plain", None, false, 4),
        ]
    }

    fn working_fixture() -> Vec<zeron_proto::Session> {
        vec![
            session("orch", zeron_proto::SessionStatus::AwaitingInput),
            session("work", zeron_proto::SessionStatus::Working),
        ]
    }

    #[gpui::test]
    fn section_nests_active_children_and_folds_done(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            fixture(),
            working_fixture(),
        );

        assert!(cx.debug_bounds("orchestrators-toggle").is_some());
        assert!(cx.debug_bounds("orchestrators-add").is_some());
        let orchestrator = cx.debug_bounds("chat-orch").unwrap();
        let child = cx.debug_bounds("chat-work").unwrap();
        assert!(child.left() > orchestrator.left());
        assert!(child.top() > orchestrator.top());
        assert!(cx.debug_bounds("chat-idle").is_none());
        assert!(cx.debug_bounds("orchestrator-done-orch").is_some());

        assert!(cx.debug_bounds("session-slot-orch").is_none());
        assert!(cx.debug_bounds("session-slot-work").is_none());
        assert!(cx.debug_bounds("session-slot-plain").is_some());

        click(cx, "orchestrator-done-orch");
        redraw(cx);
        assert!(cx.debug_bounds("chat-idle").is_some());
        click(cx, "orchestrator-done-orch");
        redraw(cx);
        shell.read_with(cx, |shell, _| {
            assert!(!shell.orchestrator_done_open.contains("orch"));
        });
    }

    #[gpui::test]
    fn section_shows_in_every_organization_and_filter(cx: &mut gpui::TestAppContext) {
        for organization in [
            SidebarOrganization::InOneList,
            SidebarOrganization::ByDevice,
            SidebarOrganization::ByProject,
        ] {
            let (shell, cx) = sidebar(cx, organization, fixture(), working_fixture());
            shell.update(cx, |shell, _| {
                shell.settings.space_filter = Some("elsewhere".into());
            });
            redraw(cx);
            assert!(cx.debug_bounds("chat-orch").is_some(), "{organization:?}");
            assert!(cx.debug_bounds("chat-work").is_some(), "{organization:?}");
        }
    }

    #[gpui::test]
    fn empty_section_is_header_and_add_only(cx: &mut gpui::TestAppContext) {
        let (_, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            vec![chat("plain", None, false, 1)],
            Vec::new(),
        );
        assert!(cx.debug_bounds("orchestrators-toggle").is_some());
        assert!(cx.debug_bounds("orchestrators-add").is_some());
        assert!(cx.debug_bounds("session-slot-plain").is_some());
    }

    #[gpui::test]
    fn archived_orchestrator_leaves_the_section(cx: &mut gpui::TestAppContext) {
        let mut archived = chat("orch", None, true, 1);
        archived.archived = true;
        let (_, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            vec![archived, chat("work", Some("orch"), false, 2)],
            Vec::new(),
        );
        assert!(cx.debug_bounds("orchestrators-toggle").is_some());
        assert!(cx.debug_bounds("session-slot-orch").is_none());
        assert!(cx.debug_bounds("chat-work").is_none());
    }

    #[gpui::test]
    fn hover_card_follows_the_rows_on_screen(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            fixture(),
            working_fixture(),
        );
        shell.update(cx, |shell, cx| {
            assert!(shell.sidebar_hover_card_row_visible("orch", cx));
            assert!(shell.sidebar_hover_card_row_visible("work", cx));
            assert!(!shell.sidebar_hover_card_row_visible("idle", cx));
            shell.orchestrator_done_open.insert("orch".into());
            assert!(shell.sidebar_hover_card_row_visible("idle", cx));
            shell
                .sidebar_collapsed_groups
                .insert("orchestrators".into());
            assert!(!shell.sidebar_hover_card_row_visible("orch", cx));
        });
    }

    #[gpui::test]
    fn reveal_finds_orchestrator_and_done_rows(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            fixture(),
            working_fixture(),
        );
        shell.update(cx, |shell, cx| {
            shell
                .sidebar_collapsed_groups
                .insert("orchestrators".into());
            assert!(shell.reveal_sidebar_chat("orch", cx));
            assert!(!shell.sidebar_collapsed_groups.contains("orchestrators"));
            assert!(!shell.orchestrator_done_open.contains("orch"));

            assert!(shell.reveal_sidebar_chat("work", cx));
            assert!(!shell.orchestrator_done_open.contains("orch"));

            shell
                .sidebar_collapsed_groups
                .insert("orchestrators".into());
            assert!(shell.reveal_sidebar_chat("idle", cx));
            assert!(!shell.sidebar_collapsed_groups.contains("orchestrators"));
            assert!(shell.orchestrator_done_open.contains("orch"));

            assert!(shell.reveal_sidebar_chat("plain", cx));
            assert!(!shell.reveal_sidebar_chat("missing", cx));
        });
    }

    #[gpui::test]
    fn rename_of_a_done_child_uses_the_sidebar_row(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            fixture(),
            working_fixture(),
        );
        shell.update(cx, |shell, cx| {
            shell
                .sidebar_collapsed_groups
                .insert("orchestrators".into());
            shell.open_rename_chat("idle".into(), cx);
            let rename = shell.chat_rename.as_ref().expect("rename opened");
            assert_eq!(rename.chat_id, "idle");
            assert_eq!(rename.surface, ChatRenameSurface::Sidebar);
            assert!(!shell.sidebar_collapsed_groups.contains("orchestrators"));
            assert!(shell.orchestrator_done_open.contains("orch"));
        });
    }

    #[gpui::test]
    fn ordinary_side_chats_are_not_in_the_orchestrator_section(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            vec![
                chat("plain", None, false, 1),
                chat("side", Some("plain"), false, 2),
            ],
            Vec::new(),
        );
        shell.update(cx, |shell, cx| {
            assert!(!shell.in_orchestrator_section("side", cx));
            assert!(!shell.in_orchestrator_section("plain", cx));
        });
    }

    #[gpui::test]
    fn painted_height_is_measured_for_pinned_hit_testing(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            vec![chat("plain", None, false, 1)],
            Vec::new(),
        );
        redraw(cx);
        let header_only = shell.read_with(cx, |shell, _| shell.orchestrators_height.get());
        assert!(header_only >= SIDEBAR_DISCLOSURE_HEADER_HEIGHT);

        shell.update(cx, |shell, cx| {
            shell.state.update(cx, |state, _| state.chats = fixture());
        });
        redraw(cx);
        let with_rows = shell.read_with(cx, |shell, _| shell.orchestrators_height.get());
        assert!(with_rows > header_only);
    }

    #[gpui::test]
    fn done_toggle_starts_a_disclosure_motion(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            fixture(),
            working_fixture(),
        );
        click(cx, "orchestrator-done-orch");
        shell.read_with(cx, |shell, _| {
            assert!(shell.orchestrator_done_open.contains("orch"));
            assert!(
                shell
                    .sidebar_disclosure_motion
                    .contains_key("orchestrator-done:orch")
            );
        });
    }

    #[gpui::test]
    fn header_toggle_collapses_the_section(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            fixture(),
            working_fixture(),
        );
        click(cx, "orchestrators-toggle");
        shell.read_with(cx, |shell, _| {
            assert!(shell.sidebar_collapsed_groups.contains("orchestrators"));
        });
        click(cx, "orchestrators-toggle");
        shell.read_with(cx, |shell, _| {
            assert!(!shell.sidebar_collapsed_groups.contains("orchestrators"));
        });
    }

    fn open_dialog(shell: &Entity<Shell>, cx: &mut gpui::VisualTestContext) {
        shell.update(cx, |shell, cx| {
            shell.state.update(cx, |state, _| {
                state.local_device_id = Some("dev".into());
                state.spaces = vec![
                    space("api", "dev", "/work/api"),
                    space("web", "dev", "/work/web"),
                    space("far", "other", "/work/far"),
                ];
            });
            shell.open_new_orchestrator(cx);
            let dialog = shell.new_orchestrator.open.as_mut().unwrap();
            dialog
                .state
                .set_providers(&[provider(HarnessId::ClaudeCode, "Claude Code")]);
            dialog.state.set_models(
                HarnessId::ClaudeCode,
                crate::popover::Loadable::Ready(vec![zeron_proto::Model {
                    id: "opus".into(),
                    label: "Opus".into(),
                    description: None,
                    reasoning_levels: vec![zeron_proto::ReasoningLevel::High],
                    options: Vec::new(),
                }]),
            );
        });
        redraw(cx);
    }

    fn space(id: &str, device: &str, path: &str) -> zeron_proto::Space {
        serde_json::from_value(serde_json::json!({
            "id": id, "deviceId": device, "path": path,
            "createdAt": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    fn provider(id: HarnessId, name: &str) -> zeron_engine::registry::HarnessDescriptor {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "supportsSteering": true,
            "steeringMode": "step-boundary", "reasoningLevels": []
        }))
        .unwrap()
    }

    #[gpui::test]
    fn add_action_opens_the_dialog_with_this_devices_projects(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            fixture(),
            working_fixture(),
        );
        shell.update(cx, |shell, cx| {
            shell.state.update(cx, |state, _| {
                state.local_device_id = Some("dev".into());
                state.spaces = vec![
                    space("web", "dev", "/work/web"),
                    space("api", "dev", "/work/api"),
                    space("far", "other", "/work/far"),
                ];
            });
        });
        click(cx, "orchestrators-add");
        redraw(cx);
        shell.read_with(cx, |shell, _| {
            let dialog = shell.new_orchestrator.open.as_ref().expect("dialog opened");
            let labels: Vec<_> = dialog
                .state
                .projects
                .iter()
                .map(|project| project.label.as_str())
                .collect();
            assert_eq!(labels, vec!["api", "web"]);
        });
        assert!(cx.debug_bounds("new-orchestrator-create").is_some());
        assert!(cx.debug_bounds("new-orchestrator-project-api").is_some());
        assert!(cx.debug_bounds("new-orchestrator-project-far").is_none());
    }

    #[gpui::test]
    fn empty_name_shows_the_message_and_sends_nothing(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(cx, SidebarOrganization::InOneList, fixture(), Vec::new());
        open_dialog(&shell, cx);
        click(cx, "new-orchestrator-project-api");
        click(cx, "new-orchestrator-create");
        redraw(cx);
        shell.read_with(cx, |shell, _| {
            let state = &shell.new_orchestrator.open.as_ref().unwrap().state;
            assert_eq!(state.checked, vec!["api".to_owned()]);
            assert_eq!(state.error.as_deref(), Some("Name is required."));
            assert!(!state.submitting);
        });
        assert!(cx.debug_bounds("new-orchestrator-notice").is_some());
    }

    #[gpui::test]
    fn escape_and_cancel_close_the_dialog(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(cx, SidebarOrganization::InOneList, fixture(), Vec::new());
        open_dialog(&shell, cx);
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| {
            assert!(shell.new_orchestrator.open.is_none())
        });

        open_dialog(&shell, cx);
        click(cx, "new-orchestrator-project-api");
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| {
            assert!(shell.new_orchestrator.open.is_none())
        });

        open_dialog(&shell, cx);
        click(cx, "new-orchestrator-cancel");
        shell.read_with(cx, |shell, _| {
            assert!(shell.new_orchestrator.open.is_none())
        });
    }

    #[gpui::test]
    fn created_orchestrator_closes_the_dialog_and_opens_its_chat(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(cx, SidebarOrganization::InOneList, fixture(), Vec::new());
        open_dialog(&shell, cx);
        shell.update(cx, |shell, cx| {
            let session = shell.new_orchestrator.open.as_ref().unwrap().session;
            shell
                .sidebar_collapsed_groups
                .insert("orchestrators".into());
            shell.land_new_orchestrator(session, Ok("orch".into()), cx);
            assert!(!shell.sidebar_collapsed_groups.contains("orchestrators"));
            assert!(shell.new_orchestrator.open.is_none());
            assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("orch"));
            assert!(matches!(shell.route, Route::Chat));
        });
    }

    #[gpui::test]
    fn engine_refusal_keeps_the_dialog_and_its_inputs(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(cx, SidebarOrganization::InOneList, fixture(), Vec::new());
        open_dialog(&shell, cx);
        shell.update(cx, |shell, cx| {
            let dialog = shell.new_orchestrator.open.as_mut().unwrap();
            dialog
                .name
                .update(cx, |input, cx| input.set_text("Plataforma", cx));
            dialog.state.toggle_project("web");
            let session = dialog.session;
            assert!(dialog.state.begin_submit("Plataforma").is_some());
            shell.land_new_orchestrator(session, Err("unknown project id `web`".into()), cx);
            let dialog = shell.new_orchestrator.open.as_ref().unwrap();
            assert_eq!(
                dialog.state.error.as_deref(),
                Some("unknown project id `web`")
            );
            assert_eq!(dialog.state.checked, vec!["web".to_owned()]);
            assert!(!dialog.state.submitting);
            assert_eq!(dialog.name.read(cx).text(), "Plataforma");
            assert_eq!(shell.state.read(cx).selected_chat, None);
        });
    }

    #[gpui::test]
    fn a_result_for_a_closed_dialog_opens_nothing(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(cx, SidebarOrganization::InOneList, fixture(), Vec::new());
        open_dialog(&shell, cx);
        shell.update(cx, |shell, cx| {
            let session = shell.new_orchestrator.open.as_ref().unwrap().session;
            shell.close_new_orchestrator(cx);
            shell.land_new_orchestrator(session, Ok("orch".into()), cx);
            assert_eq!(shell.state.read(cx).selected_chat, None);
        });
    }

    #[gpui::test]
    fn enter_in_the_name_field_submits(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(cx, SidebarOrganization::InOneList, fixture(), Vec::new());
        cx.update(|_, cx| crate::composer::init(cx, Default::default()));
        open_dialog(&shell, cx);
        shell.update(cx, |shell, cx| {
            let dialog = shell.new_orchestrator.open.as_mut().unwrap();
            dialog.state.toggle_project("api");
            dialog
                .name
                .update(cx, |input, cx| input.set_text("Plataforma", cx));
        });
        redraw(cx);
        cx.simulate_keystrokes("enter");
        shell.read_with(cx, |shell, _| {
            let state = &shell.new_orchestrator.open.as_ref().unwrap().state;
            assert_eq!(state.error.as_deref(), Some("Engine not connected"));
            assert!(!state.submitting);
        });
    }

    #[gpui::test]
    fn dialog_shows_provider_model_reasoning_and_children(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(cx, SidebarOrganization::InOneList, fixture(), Vec::new());
        open_dialog(&shell, cx);
        for selector in [
            "new-orchestrator-provider",
            "new-orchestrator-model",
            "new-orchestrator-reasoning",
            "new-orchestrator-children",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
    }

    #[gpui::test]
    fn without_a_connected_provider_create_is_off_and_says_why(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(cx, SidebarOrganization::InOneList, fixture(), Vec::new());
        open_dialog(&shell, cx);
        shell.update(cx, |shell, _| {
            let dialog = shell.new_orchestrator.open.as_mut().unwrap();
            dialog.state.set_providers(&[]);
        });
        redraw(cx);
        shell.read_with(cx, |shell, _| {
            let state = &shell.new_orchestrator.open.as_ref().unwrap().state;
            assert_eq!(
                state.notice().as_deref(),
                Some("Connect Claude Code or Codex first.")
            );
            assert!(!state.can_submit());
        });
        assert!(cx.debug_bounds("new-orchestrator-notice").is_some());
        assert!(cx.debug_bounds("new-orchestrator-provider").is_none());
    }

    #[gpui::test]
    fn a_chat_not_synced_yet_is_selected_when_it_appears(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(cx, SidebarOrganization::InOneList, fixture(), Vec::new());
        open_dialog(&shell, cx);
        shell.update(cx, |shell, cx| {
            let session = shell.new_orchestrator.open.as_ref().unwrap().session;
            shell.land_new_orchestrator(session, Ok("fresh".into()), cx);
            assert!(shell.new_orchestrator.open.is_none());
            assert_eq!(shell.state.read(cx).selected_chat, None);
            assert_eq!(
                shell.new_orchestrator.pending_chat.as_deref(),
                Some("fresh")
            );
            shell.state.update(cx, |state, cx| {
                state.chats.push(chat("fresh", None, true, 9));
                cx.notify();
            });
        });
        shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("fresh"));
            assert_eq!(shell.new_orchestrator.pending_chat, None);
        });
    }

    #[gpui::test]
    fn a_connection_change_mid_submit_does_not_leave_the_dialog_disabled(
        cx: &mut gpui::TestAppContext,
    ) {
        let (shell, cx) = sidebar(cx, SidebarOrganization::InOneList, fixture(), Vec::new());
        open_dialog(&shell, cx);
        shell.update(cx, |shell, cx| {
            let dialog = shell.new_orchestrator.open.as_mut().unwrap();
            dialog.state.toggle_project("api");
            let session = dialog.session;
            assert!(dialog.state.begin_submit("Plataforma").is_some());
            shell.land_new_orchestrator(
                session,
                Err("The connection changed. Try again.".into()),
                cx,
            );
            let state = &shell.new_orchestrator.open.as_ref().unwrap().state;
            assert!(!state.submitting);
            assert!(state.can_submit());
        });
    }

    #[gpui::test]
    fn folding_an_orchestrator_hides_its_children_and_keeps_its_row(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            fixture(),
            working_fixture(),
        );
        assert!(cx.debug_bounds("chat-work").is_some());
        assert!(cx.debug_bounds("orchestrator-done-orch").is_some());
        let unfolded = shell.read_with(cx, |shell, _| shell.orchestrators_height.get());
        click(cx, "orchestrator-fold-orch");
        redraw(cx);
        redraw(cx);
        shell.read_with(cx, |shell, _| {
            assert!(
                shell
                    .sidebar_collapsed_groups
                    .contains("orchestrator-fold:orch")
            );
            assert!(!shell.sidebar_collapsed_groups.contains("orchestrators"));
        });
        shell.update(cx, |shell, _| shell.sidebar_disclosure_motion.clear());
        redraw(cx);
        assert!(cx.debug_bounds("chat-orch").is_some());
        let folded = shell.read_with(cx, |shell, _| shell.orchestrators_height.get());
        assert!(folded < unfolded);
        shell.update(cx, |shell, cx| {
            assert!(shell.orchestrator_row_visible("orch", cx));
            assert!(!shell.orchestrator_row_visible("work", cx));
        });
        click(cx, "orchestrator-fold-orch");
        shell.read_with(cx, |shell, _| {
            assert!(
                !shell
                    .sidebar_collapsed_groups
                    .contains("orchestrator-fold:orch")
            );
        });
    }

    #[gpui::test]
    fn reveal_expands_a_folded_parent(cx: &mut gpui::TestAppContext) {
        let (shell, cx) = sidebar(
            cx,
            SidebarOrganization::InOneList,
            fixture(),
            working_fixture(),
        );
        shell.update(cx, |shell, cx| {
            shell
                .sidebar_collapsed_groups
                .insert("orchestrator-fold:orch".into());
            assert!(shell.reveal_sidebar_chat("orch", cx));
            assert!(
                shell
                    .sidebar_collapsed_groups
                    .contains("orchestrator-fold:orch")
            );
            assert!(shell.reveal_sidebar_chat("work", cx));
            assert!(
                !shell
                    .sidebar_collapsed_groups
                    .contains("orchestrator-fold:orch")
            );
        });
    }
}
