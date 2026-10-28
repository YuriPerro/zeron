//! How sidebar rows and project groups are ordered and filtered. Pure rules,
//! shared by the drawn list and the keyboard order so the two never drift.
use std::cmp::Ordering;

use zeron_proto::ChatIndicator;

use crate::settings::{SidebarOrganization, SidebarProjectOrder, SidebarSort, SidebarStatusFilter};
use crate::state::AppState;

use super::Shell;

impl Shell {
    /// The sidebar's active chats under the project and status filters, in
    /// the chosen sort. Pinned chats and the open chat pass any status
    /// filter. Both the drawn list and the keyboard order start here.
    pub(super) fn sidebar_sorted_chats(
        &self,
        state: &AppState,
        now: chrono::DateTime<chrono::Utc>,
        pinned: &[String],
    ) -> Vec<(ChatIndicator, zeron_proto::Chat)> {
        let filter = self.settings.sidebar_status_filter;
        let selected = state.selected_chat.as_deref();
        let mut chats: Vec<_> = state
            .sidebar_chats(now, self.settings.space_filter.as_deref())
            .into_iter()
            .filter(|(status, chat)| {
                status_filter_keeps(filter, *status)
                    || pinned.contains(&chat.id)
                    || selected == Some(chat.id.as_str())
            })
            .collect();
        chats.sort_by(|left, right| {
            compare_sidebar_chats(self.settings.sidebar_sort, *left, *right)
        });
        chats
            .into_iter()
            .map(|(status, chat)| (status, chat.clone()))
            .collect()
    }

    /// The project groups' keys in drawn order, while grouping by project.
    pub(super) fn sidebar_project_keys(&self, cx: &gpui::App) -> Vec<String> {
        if self.settings.sidebar_organization != SidebarOrganization::ByProject {
            return Vec::new();
        }
        let pinned = self.active_sidebar_pins(cx);
        let sections = self.active_sidebar_sections(cx);
        let state = self.state.read(cx);
        let mut groups: Vec<(Option<(String, String)>, Vec<()>)> = Vec::new();
        for (_, chat) in self.sidebar_sorted_chats(state, chrono::Utc::now(), &pinned) {
            if pinned.contains(&chat.id)
                || sections
                    .iter()
                    .any(|section| section.session_ids.contains(&chat.id))
            {
                continue;
            }
            let group = Some(super::spaces::sidebar_project_group(state, &chat));
            if !groups.iter().any(|(existing, _)| existing == &group) {
                groups.push((group, Vec::new()));
            }
        }
        self.order_sidebar_groups(state, &mut groups);
        groups
            .into_iter()
            .filter_map(|(group, _)| group.map(|(key, _)| key))
            .collect()
    }

    /// Land a dragged project group: the new order is saved and Projects
    /// switches to Manual so it sticks.
    pub(super) fn move_sidebar_project(
        &mut self,
        visible: &[String],
        from: usize,
        over: usize,
        cx: &mut gpui::Context<Self>,
    ) {
        self.settings.space_order =
            moved_project_order(&self.settings.space_order, visible, from, over);
        self.settings.sidebar_project_order = SidebarProjectOrder::Manual;
        self.schedule_save(cx);
        cx.notify();
    }

    /// Order the device or project groups the way the sidebar draws them.
    pub(super) fn order_sidebar_groups<T>(
        &self,
        state: &AppState,
        groups: &mut Vec<(Option<(String, String)>, Vec<T>)>,
    ) {
        match self.settings.sidebar_organization {
            SidebarOrganization::ByDevice => {
                super::spaces::promote_local_device_group(groups, state.local_device_id.as_deref())
            }
            SidebarOrganization::ByProject => order_project_groups(
                groups,
                self.settings.sidebar_project_order,
                &self.settings.space_order,
            ),
            SidebarOrganization::InOneList => {}
        }
    }
}

/// One chat against another under `sort`, newest first on ties, then by id
/// so equal rows never swap between renders.
pub(super) fn compare_sidebar_chats(
    sort: SidebarSort,
    left: (ChatIndicator, &zeron_proto::Chat),
    right: (ChatIndicator, &zeron_proto::Chat),
) -> Ordering {
    let (left_status, left) = left;
    let (right_status, right) = right;
    let updated = |chat: &zeron_proto::Chat| chat.last_message_at.unwrap_or(chat.created_at);
    let newest = || updated(right).cmp(&updated(left));
    let primary = match sort {
        SidebarSort::Created => right.created_at.cmp(&left.created_at),
        SidebarSort::LastUpdated => newest(),
        SidebarSort::Name => {
            let title = |chat: &zeron_proto::Chat| chat.title.as_deref().map(str::to_lowercase);
            match (title(left), title(right)) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
            .then_with(newest)
        }
        SidebarSort::Status => status_rank(left_status)
            .cmp(&status_rank(right_status))
            .then_with(newest),
    };
    primary.then_with(|| left.id.cmp(&right.id))
}

fn status_rank(status: ChatIndicator) -> u8 {
    match status {
        ChatIndicator::AwaitingInput => 0,
        ChatIndicator::Errored => 1,
        ChatIndicator::Working => 2,
        ChatIndicator::Completed => 3,
        ChatIndicator::Idle => 4,
    }
}

/// Whether `filter` lists a chat in `status`. Pins and the open chat bypass
/// this; the caller keeps them.
pub(super) fn status_filter_keeps(filter: SidebarStatusFilter, status: ChatIndicator) -> bool {
    match filter {
        SidebarStatusFilter::All => true,
        SidebarStatusFilter::Active => status != ChatIndicator::Idle,
        SidebarStatusFilter::NeedsMe => {
            matches!(
                status,
                ChatIndicator::AwaitingInput | ChatIndicator::Errored
            )
        }
    }
}

/// Reorder project groups, which arrive in recent (first-appearance) order.
/// Manual puts groups `manual` doesn't name on top, still in recent order:
/// a new project is usually the one in use.
pub(super) fn order_project_groups<T>(
    groups: &mut [(Option<(String, String)>, Vec<T>)],
    order: SidebarProjectOrder,
    manual: &[String],
) {
    match order {
        SidebarProjectOrder::Recent => {}
        SidebarProjectOrder::Name => groups.sort_by_cached_key(|(group, _)| {
            group
                .as_ref()
                .map(|(key, label)| (key.starts_with("home:"), label.to_lowercase()))
        }),
        SidebarProjectOrder::Manual => groups.sort_by_cached_key(|(group, _)| {
            group
                .as_ref()
                .and_then(|(key, _)| manual.iter().position(|saved| saved == key))
                .map_or((0, 0), |position| (1, position))
        }),
    }
}

/// `saved` after the visible group at `from` lands at `over`. Visible keys
/// refill the slots visible keys held, in their new order; hidden keys keep
/// theirs. Visible keys `saved` lacks join at the top, as they show.
pub(super) fn moved_project_order(
    saved: &[String],
    visible: &[String],
    from: usize,
    over: usize,
) -> Vec<String> {
    let mut order: Vec<String> = visible
        .iter()
        .filter(|key| !saved.contains(key))
        .chain(saved)
        .cloned()
        .collect();
    let mut moved = visible.to_vec();
    if from < moved.len() {
        let key = moved.remove(from);
        moved.insert(over.min(moved.len()), key);
    }
    let mut next = moved.into_iter();
    for slot in order.iter_mut() {
        if visible.contains(slot) {
            *slot = next.next().unwrap_or_default();
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{SidebarProjectOrder, SidebarSort, SidebarStatusFilter};
    use chrono::{TimeZone as _, Utc};
    use zeron_proto::ChatIndicator;

    fn chat(id: &str, title: Option<&str>, updated: i64) -> zeron_proto::Chat {
        zeron_proto::Chat {
            id: id.into(),
            device_id: "device".into(),
            title: title.map(Into::into),
            archived: false,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at: Some(Utc.timestamp_opt(updated, 0).unwrap()),
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
            harness_session_id: None,
            harness_session_cwd: None,
            harness_session_harness: None,
            parent_chat_id: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
        }
    }

    fn sorted(sort: SidebarSort) -> Vec<String> {
        let rows = [
            (ChatIndicator::Idle, chat("a", Some("beta"), 2)),
            (ChatIndicator::Idle, chat("b", Some("Alpha"), 1)),
            (ChatIndicator::Working, chat("c", Some("alpha"), 3)),
            (ChatIndicator::AwaitingInput, chat("d", None, 4)),
            (ChatIndicator::Errored, chat("e", None, 0)),
        ];
        let mut rows: Vec<_> = rows.iter().map(|(status, chat)| (*status, chat)).collect();
        rows.sort_by(|left, right| compare_sidebar_chats(sort, *left, *right));
        rows.into_iter().map(|(_, chat)| chat.id.clone()).collect()
    }

    fn groups(keys: &[(&str, &str)]) -> Vec<(Option<(String, String)>, Vec<()>)> {
        keys.iter()
            .map(|(key, label)| (Some((key.to_string(), label.to_string())), vec![()]))
            .collect()
    }

    fn keys(groups: &[(Option<(String, String)>, Vec<()>)]) -> Vec<String> {
        groups
            .iter()
            .map(|(group, _)| group.as_ref().unwrap().0.clone())
            .collect()
    }

    fn manual(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|key| key.to_string()).collect()
    }

    #[test]
    fn name_and_status_sorts_break_ties_by_last_updated() {
        assert_eq!(sorted(SidebarSort::Name), ["c", "b", "a", "d", "e"]);
        assert_eq!(sorted(SidebarSort::Status), ["d", "e", "c", "a", "b"]);
        assert_eq!(sorted(SidebarSort::LastUpdated), ["d", "c", "a", "b", "e"]);
    }

    #[test]
    fn status_filter_rules() {
        use ChatIndicator::*;
        let all = [Working, AwaitingInput, Errored, Completed, Idle];
        let kept = |filter| {
            all.into_iter()
                .filter(|status| status_filter_keeps(filter, *status))
                .collect::<Vec<_>>()
        };
        assert_eq!(kept(SidebarStatusFilter::All), all);
        assert_eq!(
            kept(SidebarStatusFilter::Active),
            [Working, AwaitingInput, Errored, Completed]
        );
        assert_eq!(kept(SidebarStatusFilter::NeedsMe), [AwaitingInput, Errored]);
    }

    #[test]
    fn recent_keeps_first_appearance_order() {
        let mut list = groups(&[("b", "beta"), ("a", "alpha")]);
        order_project_groups(&mut list, SidebarProjectOrder::Recent, &manual(&["a", "b"]));
        assert_eq!(keys(&list), ["b", "a"]);
    }

    #[test]
    fn name_order_is_case_insensitive_and_puts_no_project_last() {
        let mut list = groups(&[
            ("home:dev", "No project"),
            ("k-b", "beta"),
            ("k-a", "Alpha"),
        ]);
        order_project_groups(&mut list, SidebarProjectOrder::Name, &[]);
        assert_eq!(keys(&list), ["k-a", "k-b", "home:dev"]);
    }

    #[test]
    fn manual_order_puts_unknown_groups_on_top_in_recent_order() {
        let mut list = groups(&[("c", "c"), ("b", "b"), ("x", "x"), ("a", "a")]);
        order_project_groups(
            &mut list,
            SidebarProjectOrder::Manual,
            &manual(&["a", "b", "c"]),
        );
        assert_eq!(keys(&list), ["x", "a", "b", "c"]);
    }

    #[test]
    fn manual_order_ignores_unknown_keys() {
        let mut list = groups(&[("a", "a"), ("b", "b")]);
        order_project_groups(
            &mut list,
            SidebarProjectOrder::Manual,
            &manual(&["space-legacy", "b", "a"]),
        );
        assert_eq!(keys(&list), ["b", "a"]);
    }

    #[test]
    fn moved_project_order_keeps_hidden_keys_in_place() {
        assert_eq!(
            moved_project_order(
                &manual(&["a", "hidden", "b", "c"]),
                &manual(&["a", "b", "c"]),
                0,
                2
            ),
            ["b", "hidden", "c", "a"]
        );
        assert_eq!(
            moved_project_order(&[], &manual(&["x", "y"]), 1, 0),
            ["y", "x"]
        );
        assert_eq!(
            moved_project_order(&manual(&["a", "b"]), &manual(&["new", "a", "b"]), 0, 2),
            ["a", "b", "new"]
        );
    }
}
