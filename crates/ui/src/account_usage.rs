//! The composer footer's ring cluster. The plan-usage ring shows how much of
//! the active account's rate limit the session's harness has used, beside
//! the context ring. Clicking it opens the harness's accounts — each with its
//! usage meters — and clicking one switches to it, the same `ActivateAgentAccount` Settings →
//! Accounts runs. Both views share [`AccountsSnapshotCache`], so a switch in
//! either shows up in the other.
use std::time::{Duration, Instant};

use gpui::{
    Context, Entity, IntoElement, Render, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use zeron_proto::{AgentAccount, AgentAccountsSnapshot, HarnessId};
use zeron_rpc::methods;

use crate::popover;
use crate::settings::accounts::{
    self, AccountsSnapshotCache, UsageLevel, render_usage_meter, reports_usage, signs_in,
    usage_color, usage_level,
};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// Forced probes hit the provider; the engine throttles them too, but the
/// ring re-probes on every hover, so don't even ask more often than this.
const FORCE_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// Background re-probe while a composer is alive: usage moves as turns run.
const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);

const RESET_WIDTH: f32 = 132.0;

/// When a usage window resets: a countdown within a day ("resets in 2h 14m"),
/// weekday and time within a week ("resets Thu 2:00 PM"), else the date. Pure.
fn reset_label(
    resets_at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let at = resets_at?;
    let left = at.signed_duration_since(now);
    let local = at.with_timezone(&chrono::Local);
    Some(if left.num_minutes() < 1 {
        "resets now".to_owned()
    } else if left.num_hours() < 24 {
        match (left.num_hours(), left.num_minutes() % 60) {
            (0, minutes) => format!("resets in {minutes}m"),
            (hours, minutes) => format!("resets in {hours}h {minutes}m"),
        }
    } else if left.num_days() < 7 {
        format!("resets {}", local.format("%a %-I:%M %p"))
    } else {
        format!("resets {}", local.format("%b %-d"))
    })
}

/// The binding limit: the most-used window of the account. Pure.
pub fn used_fraction(account: &AgentAccount) -> Option<f32> {
    account
        .usage_windows
        .iter()
        .map(|window| window.used_fraction.clamp(0.0, 1.0))
        .reduce(f32::max)
}

/// The ring's reading as `(shown, peak)`: `shown` is the window labeled
/// `pick` when the account has it, else the most-used; `peak` is always the
/// most-used, so the ring's color still warns of a limit it isn't showing.
/// Pure.
pub fn ring_reading(account: &AgentAccount, pick: Option<&str>) -> Option<(f32, f32)> {
    let peak = used_fraction(account)?;
    let shown = pick
        .and_then(|label| {
            account
                .usage_windows
                .iter()
                .find(|window| window.label == label)
        })
        .map_or(peak, |window| window.used_fraction.clamp(0.0, 1.0));
    Some((shown, peak))
}

/// The harness's live account, when it reports usage. Pure.
pub fn active_account(
    snapshot: &AgentAccountsSnapshot,
    harness: HarnessId,
) -> Option<&AgentAccount> {
    snapshot
        .accounts
        .iter()
        .find(|account| account.harness == harness && account.active)
}

/// Loads (and switches) the accounts of the composer's target device.
pub struct AccountUsage {
    state: Entity<AppState>,
    /// `None` = this device, as in [`AccountsSnapshotCache`].
    target: Option<String>,
    harness: Option<HarnessId>,
    /// Once any list has been asked for the current target.
    loaded: bool,
    last_forced: Option<Instant>,
    error: Option<SharedString>,
    load_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    popup: popover::Popup<FooterCard>,
    _poll: Task<()>,
    _cache: Subscription,
    _state: Subscription,
}

impl AccountUsage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this
                    .update(cx, |usage, cx| {
                        if usage.harness.is_some() {
                            usage.load(true, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            _state: cx.observe(&state, |_, _, cx| cx.notify()),
            state,
            target: None,
            harness: None,
            loaded: false,
            last_forced: None,
            error: None,
            load_task: None,
            action_task: None,
            popup: popover::Popup::default(),
            _poll: poll,
            // Settings → Accounts writes the same cache.
            _cache: cx.observe_global::<AccountsSnapshotCache>(|_, cx| cx.notify()),
        }
    }

    /// Point at the session's harness and device; loads on first sight of a
    /// device. Cheap enough to call every render.
    pub fn track(
        &mut self,
        harness: Option<HarnessId>,
        target: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let harness = harness.filter(|h| signs_in(*h) && reports_usage(*h));
        self.harness = harness;
        if self.target != target {
            self.target = target;
            self.loaded = false;
            self.last_forced = None;
            self.error = None;
        }
        if harness.is_some() && !self.loaded {
            self.loaded = true;
            self.load(true, cx);
        }
    }

    fn snapshot<'a>(&self, cx: &'a gpui::App) -> Option<&'a AgentAccountsSnapshot> {
        cx.try_global::<AccountsSnapshotCache>()?
            .0
            .get(&self.target)
    }

    fn params(&self, mut value: serde_json::Value) -> serde_json::Value {
        if let (Some(target), Some(object)) = (&self.target, value.as_object_mut()) {
            object.insert("targetDeviceId".into(), serde_json::json!(target));
        }
        value
    }

    /// Plain list first when nothing is cached (the engine's persisted usage
    /// paints at once), then the forced probe replaces it.
    fn load(&mut self, force_usage: bool, cx: &mut Context<Self>) {
        if force_usage {
            if self
                .last_forced
                .is_some_and(|at| at.elapsed() < FORCE_MIN_INTERVAL)
            {
                return;
            }
            self.last_forced = Some(Instant::now());
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let key = self.target.clone();
        let paint_first = force_usage && self.snapshot(cx).is_none();
        let plain = self.params(serde_json::json!({ "forceUsage": false }));
        let params = self.params(serde_json::json!({ "forceUsage": force_usage }));
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let fetch = |params: serde_json::Value| {
                let engine = engine.clone();
                async move {
                    engine
                        .client()
                        .call(methods::LIST_AGENT_ACCOUNTS, params)
                        .await
                        .ok()
                        .and_then(|value| {
                            serde_json::from_value::<AgentAccountsSnapshot>(value).ok()
                        })
                }
            };
            let store = |snapshot: AgentAccountsSnapshot, cx: &mut gpui::AsyncApp| {
                let key = key.clone();
                this.update(cx, |_, cx| {
                    cx.default_global::<AccountsSnapshotCache>()
                        .0
                        .insert(key, snapshot);
                })
                .ok();
            };
            if paint_first && let Some(snapshot) = fetch(plain).await {
                store(snapshot, cx);
            }
            if let Some(snapshot) = fetch(params).await {
                store(snapshot, cx);
            }
        }));
    }

    /// Switch optimistically, like Settings → Accounts: the rows flip at once
    /// and the engine's reply replaces them; a refusal restores the list.
    fn switch(&mut self, account: &AgentAccount, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let key = self.target.clone();
        let cache = &mut cx.default_global::<AccountsSnapshotCache>().0;
        let previous = cache.get(&key).cloned();
        if let Some(snapshot) = cache.get_mut(&key) {
            // Per-provider agents keep one live login per provider: only the
            // switched-to row's group changes.
            accounts::mark_switched(snapshot, account);
        }
        self.error = None;
        let params = self.params(serde_json::json!({
            "id": account.id,
            "accountId": account.id,
            "harness": account.harness,
        }));
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::ACTIVATE_AGENT_ACCOUNT, params)
                .await;
            this.update(cx, |usage, cx| {
                match result.map(serde_json::from_value::<AgentAccountsSnapshot>) {
                    Ok(Ok(snapshot)) => {
                        cx.default_global::<AccountsSnapshotCache>()
                            .0
                            .insert(key, snapshot);
                    }
                    Ok(Err(_)) => usage.load(false, cx),
                    Err(err) => {
                        if let Some(previous) = previous {
                            cx.default_global::<AccountsSnapshotCache>()
                                .0
                                .insert(key, previous);
                        }
                        usage.error = Some(err.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The ring's [`ring_reading`] of the live account, when the harness has
    /// a live account with usage to show.
    fn fraction(&self, cx: &gpui::App) -> Option<(f32, f32)> {
        let pick = crate::settings::account_usage_window(cx);
        ring_reading(
            active_account(self.snapshot(cx)?, self.harness?)?,
            pick.as_deref(),
        )
    }

    /// A trigger click: open this ring's popover, or close it when the
    /// press found it open (the card's mouse-down-out already began that
    /// close — see `Popup::note_trigger_press`).
    fn toggle(&mut self, card: FooterCard, cx: &mut Context<Self>) {
        if self.popup.take_press_was_open() || self.popup.as_open() == Some(&card) {
            self.dismiss(cx);
            return;
        }
        if card == FooterCard::Accounts {
            // Opening the card is the moment someone cares: re-probe.
            self.load(true, cx);
        }
        self.popup.open(card);
        cx.notify();
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.popup.begin_close() {
            popover::reap_popup(cx, |usage: &mut Self| &mut usage.popup);
        }
        cx.notify();
    }

    /// A footer ring that opens `card` above itself, right-aligned like the
    /// model picker.
    fn trigger(
        &self,
        chip: gpui::Stateful<gpui::Div>,
        card: FooterCard,
        content: impl FnOnce(&Self, &mut Context<Self>) -> gpui::Div,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let chip = chip
            .relative()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |usage, _, _, _| {
                    usage
                        .popup
                        .note_trigger_press_matching(|open| *open == card);
                }),
            )
            .on_click(cx.listener(move |usage, _, _, cx| usage.toggle(card, cx)));
        if self.popup.get() != Some(&card) {
            return chip.into_any_element();
        }
        let content = content(self, cx)
            .on_mouse_down_out(cx.listener(|usage, _, _, cx| usage.dismiss(cx)))
            .into_any_element();
        chip.child(popover::anchored_menu_above_end(
            card.id(),
            content,
            self.popup.closing_since(),
        ))
        .into_any_element()
    }

    fn accounts_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = &Theme::of(cx).for_popup();
        let harness = self.harness;
        let now = chrono::Utc::now();
        let rows: Vec<AgentAccount> = match (harness, self.snapshot(cx)) {
            (Some(harness), Some(snapshot)) => accounts::provider_accounts(snapshot, harness)
                .into_iter()
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        let title = format!(
            "{} accounts",
            harness.map_or("Agent", accounts::provider_name)
        );
        popover::popover_card(theme)
            .w(px(400.0 + RESET_WIDTH + 8.0))
            .flex()
            .flex_col()
            .child(popover::menu_heading(theme, &title))
            .children(rows.iter().enumerate().map(|(ix, account)| {
                let email: SharedString = account
                    .email
                    .clone()
                    .or_else(|| account.display_name.clone())
                    .unwrap_or_else(|| "Unknown account".into())
                    .into();
                let can_switch = !account.active && account.switchable;
                let mut meta: Vec<gpui::AnyElement> = account
                    .plan_label
                    .iter()
                    .map(|plan| {
                        div()
                            .child(SharedString::from(plan.clone()))
                            .into_any_element()
                    })
                    .collect();
                if account.active {
                    meta.push(
                        div()
                            .text_color(theme.accent)
                            .child(SharedString::from("In use"))
                            .into_any_element(),
                    );
                } else if let Some(reason) = account
                    .usage_error
                    .clone()
                    .filter(|_| account.usage_windows.is_empty())
                {
                    meta.push(div().child(SharedString::from(reason)).into_any_element());
                }
                let switch_to = account.clone();
                popover::menu_row(theme, account.active, format!("account-usage-row-{ix}"))
                    .id(("account-usage-row", ix))
                    .when(!can_switch, |row| row.cursor_default())
                    .when(can_switch, |row| {
                        row.on_click(cx.listener(move |usage, _, _, cx| {
                            usage.switch(&switch_to, cx);
                        }))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(email),
                            )
                            .when(!meta.is_empty(), |el| {
                                el.child(crate::settings::widgets::meta_line(theme, meta))
                            }),
                    )
                    .child(
                        div()
                            .id(("account-usage-meters", ix))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .when(!account.usage_windows.is_empty(), |meters| {
                                meters.tooltip(widgets::text_tooltip(accounts::usage_resets(
                                    &account.usage_windows,
                                    now,
                                )))
                            })
                            .children(account.usage_windows.iter().take(2).map(|window| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(render_usage_meter(window, theme))
                                    .child(
                                        div()
                                            .min_w(px(RESET_WIDTH))
                                            .flex_none()
                                            .whitespace_nowrap()
                                            .text_size(crate::typography::ui_rems(11.5))
                                            .text_color(theme.text_muted)
                                            .children(
                                                reset_label(window.resets_at, now)
                                                    .map(SharedString::from),
                                            ),
                                    )
                            })),
                    )
            }))
            .children(self.window_picker(&rows, theme, cx))
            .children(self.error.clone().map(|error| {
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .text_size(px(12.0))
                    .text_color(theme.danger)
                    .child(error)
            }))
    }

    /// Which window the ring shows, offered once the live account has more
    /// than one. A pick the account lacks reads as "Most used", as it shows.
    fn window_picker(
        &self,
        rows: &[AgentAccount],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let labels: Vec<String> = rows
            .iter()
            .find(|account| account.active)?
            .usage_windows
            .iter()
            .map(|window| window.label.clone())
            .collect();
        if labels.len() < 2 {
            return None;
        }
        let pick = crate::settings::account_usage_window(cx)
            .map(String::from)
            .filter(|pick| labels.contains(pick));
        let choices = std::iter::once((None, "Most used".to_string()))
            .chain(labels.into_iter().map(|label| (Some(label.clone()), label)));
        Some(
            div()
                .flex()
                .flex_col()
                .child(popover::menu_separator())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .px(px(8.0))
                        .py(px(4.0))
                        .text_size(crate::typography::ui_rems(11.5))
                        .child(
                            div()
                                .flex_1()
                                .text_color(theme.text_muted)
                                .child("Footer shows"),
                        )
                        .children(choices.enumerate().map(|(ix, (window, label))| {
                            let selected = window == pick;
                            div()
                                .id(("account-usage-window", ix))
                                .px(px(8.0))
                                .py(px(2.0))
                                .rounded(px(5.0))
                                .cursor_pointer()
                                .when(selected, |chip| {
                                    chip.bg(crate::theme::ink(0.08)).text_color(theme.accent)
                                })
                                .when(!selected, |chip| {
                                    chip.text_color(theme.text_muted)
                                        .hover(|style| style.bg(crate::theme::ink(0.05)))
                                })
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    crate::settings::set_account_usage_window(window.clone(), cx);
                                    cx.notify();
                                }))
                                .child(SharedString::from(label))
                        })),
                ),
        )
    }
}

/// Which footer ring's popover is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FooterCard {
    Accounts,
    Context,
}

impl FooterCard {
    fn id(self) -> &'static str {
        match self {
            FooterCard::Accounts => "account-usage-menu",
            FooterCard::Context => "context-usage-menu",
        }
    }
}

/// The footer's ring cluster: account usage (accent arc, so the two read
/// apart), then context occupancy. Each opens its popover on click.
impl Render for AccountUsage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let context = self.state.read(cx).context_usage;
        let account = self.fraction(cx).map(|(fraction, peak)| {
            let level = usage_level(peak);
            let chip = crate::context_usage::ring_chip(
                "account-usage",
                fraction,
                usage_color(level, &theme),
                match level {
                    UsageLevel::Normal => theme.text_muted,
                    _ => usage_color(level, &theme),
                },
                format!("{}%", (fraction * 100.0).round() as u32),
                self.popup.get() == Some(&FooterCard::Accounts),
                &theme,
            );
            self.trigger(
                chip,
                FooterCard::Accounts,
                |usage, cx| usage.accounts_card(cx),
                cx,
            )
        });
        let context = crate::context_usage::has_window(context).then(|| {
            let chip = crate::context_usage::chip(
                context,
                self.popup.get() == Some(&FooterCard::Context),
                &theme,
            );
            self.trigger(
                chip,
                FooterCard::Context,
                move |_, cx| crate::context_usage::card(context, &Theme::of(cx).for_popup()),
                cx,
            )
        });
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .children(account)
            .children(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::AgentUsageWindow;

    fn account(harness: HarnessId, active: bool, used: &[f32]) -> AgentAccount {
        serde_json::from_value(serde_json::json!({
            "id": format!("{harness:?}-{active}"),
            "harness": harness,
            "email": null,
            "planLabel": null,
            "active": active,
            "switchable": true,
            "usageWindows": used
                .iter()
                .map(|fraction| AgentUsageWindow {
                    label: "5h".into(),
                    used_fraction: *fraction,
                    resets_at: None,
                })
                .collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[test]
    fn reset_label_counts_down_within_a_day() {
        let now = chrono::Utc::now();
        let at = |minutes: i64| Some(now + chrono::Duration::minutes(minutes));
        assert_eq!(reset_label(None, now), None);
        assert_eq!(reset_label(at(0), now).as_deref(), Some("resets now"));
        assert_eq!(reset_label(at(42), now).as_deref(), Some("resets in 42m"));
        assert_eq!(
            reset_label(at(2 * 60 + 14), now).as_deref(),
            Some("resets in 2h 14m")
        );
        let in_three_days = reset_label(at(3 * 24 * 60), now).unwrap();
        assert!(in_three_days.starts_with("resets ") && in_three_days.contains(':'));
        let in_a_month = reset_label(at(30 * 24 * 60), now).unwrap();
        assert!(!in_a_month.contains(':'));
    }

    #[test]
    fn ring_shows_the_most_used_window() {
        assert_eq!(used_fraction(&account(HarnessId::Codex, true, &[])), None);
        assert_eq!(
            used_fraction(&account(HarnessId::Codex, true, &[0.12, 0.64])),
            Some(0.64)
        );
        assert_eq!(
            used_fraction(&account(HarnessId::Codex, true, &[1.4])),
            Some(1.0)
        );
    }

    #[test]
    fn ring_shows_the_picked_window_and_colors_by_the_most_used() {
        let mut both = account(HarnessId::ClaudeCode, true, &[0.04, 0.49]);
        both.usage_windows[0].label = "Session".into();
        both.usage_windows[1].label = "Week".into();
        assert_eq!(ring_reading(&both, None), Some((0.49, 0.49)));
        assert_eq!(ring_reading(&both, Some("Session")), Some((0.04, 0.49)));
        assert_eq!(ring_reading(&both, Some("Week")), Some((0.49, 0.49)));
        assert_eq!(ring_reading(&both, Some("Month")), Some((0.49, 0.49)));
        assert_eq!(
            ring_reading(&account(HarnessId::ClaudeCode, true, &[]), Some("Week")),
            None
        );
    }

    #[test]
    fn active_account_is_scoped_to_the_harness() {
        let snapshot = AgentAccountsSnapshot {
            accounts: vec![
                account(HarnessId::ClaudeCode, true, &[0.3]),
                account(HarnessId::Codex, false, &[0.1]),
                account(HarnessId::Codex, true, &[0.2]),
            ],
            warnings: Vec::new(),
        };
        assert_eq!(
            active_account(&snapshot, HarnessId::Codex).map(|a| a.id.as_str()),
            Some("Codex-true")
        );
        assert!(active_account(&snapshot, HarnessId::Cursor).is_none());
    }
}
