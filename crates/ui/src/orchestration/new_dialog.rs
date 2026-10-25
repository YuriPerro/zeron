use crate::composer::ComposerInput;
use crate::pickers::{
    clamp_reasoning, default_model, default_reasoning, normalize_model_rows, offered_harnesses,
};
use crate::popover::Loadable;
use crate::settings::widgets::SelectState;
use gpui::{Entity, FocusHandle, Subscription};
use serde_json::{Map, Value};
use std::collections::HashMap;
use zeron_engine::registry::HarnessDescriptor;
use zeron_proto::{HarnessId, Model, ReasoningLevel};

pub(crate) const HOSTS: [HarnessId; 2] = [HarnessId::ClaudeCode, HarnessId::Codex];
pub(crate) const CONNECT_FIRST: &str = "Connect Claude Code or Codex first.";
const PROJECTS_REQUIRED: &str = "Select at least one project.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewOrchestratorForm {
    pub name: String,
    pub projects: Vec<String>,
    pub harness: String,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub child_harness: Option<String>,
}

/// Builds the `CreateOrchestrator` params, or the message to show under the form.
pub(crate) fn validate(form: &NewOrchestratorForm) -> Result<Value, &'static str> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err("Name is required.");
    }
    if form.projects.is_empty() {
        return Err("Select at least one project.");
    }
    let mut params = Map::new();
    params.insert("name".into(), name.into());
    params.insert("projects".into(), form.projects.clone().into());
    params.insert("harness".into(), form.harness.clone().into());
    let optional = [
        ("model", &form.model),
        ("reasoning", &form.reasoning),
        ("childHarness", &form.child_harness),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            params.insert(key.into(), value.clone().into());
        }
    }
    Ok(Value::Object(params))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderChoice {
    pub id: HarnessId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectChoice {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildChoice {
    Same,
    Other(HarnessId),
}

/// The connected providers that can host an orchestrator, Claude Code first.
pub(crate) fn provider_choices(list: &[HarnessDescriptor]) -> Vec<ProviderChoice> {
    let offered = offered_harnesses(list);
    HOSTS
        .iter()
        .filter_map(|id| {
            offered
                .iter()
                .find(|descriptor| descriptor.id == *id)
                .map(|descriptor| ProviderChoice {
                    id: *id,
                    name: descriptor.name.clone(),
                })
        })
        .collect()
}

/// Models as the composer picker lists them for `harness`.
pub(crate) fn parse_models(harness: HarnessId, value: Value) -> Loadable<Vec<Model>> {
    match serde_json::from_value::<Vec<Model>>(value) {
        Ok(models) => Loadable::Ready(normalize_model_rows(harness, models)),
        Err(error) => Loadable::Error(error.to_string()),
    }
}

/// The new orchestrator's chat id from a `CreateOrchestrator` reply.
pub(crate) fn parse_created(value: Value) -> Result<String, String> {
    value
        .get("chatId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "The engine did not return the new chat.".to_owned())
}

#[derive(Debug)]
pub(crate) struct DialogState {
    pub projects: Vec<ProjectChoice>,
    pub checked: Vec<String>,
    pub providers: Loadable<Vec<ProviderChoice>>,
    pub provider: Option<HarnessId>,
    pub models: HashMap<HarnessId, Loadable<Vec<Model>>>,
    pub model: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    pub child: ChildChoice,
    pub error: Option<String>,
    pub submitting: bool,
}

impl DialogState {
    pub(crate) fn new(projects: Vec<ProjectChoice>) -> Self {
        Self {
            projects,
            checked: Vec::new(),
            providers: Loadable::Loading,
            provider: None,
            models: HashMap::new(),
            model: None,
            reasoning: None,
            child: ChildChoice::Same,
            error: None,
            submitting: false,
        }
    }

    pub(crate) fn provider_list(&self) -> &[ProviderChoice] {
        self.providers.ready().map_or(&[], Vec::as_slice)
    }

    pub(crate) fn set_providers(&mut self, list: &[HarnessDescriptor]) {
        let choices = provider_choices(list);
        for choice in &choices {
            self.models.entry(choice.id).or_insert(Loadable::Loading);
        }
        self.provider = self
            .provider
            .filter(|id| choices.iter().any(|choice| choice.id == *id))
            .or_else(|| choices.first().map(|choice| choice.id));
        self.providers = Loadable::Ready(choices);
        self.fix_child();
        self.apply_model_defaults();
    }

    pub(crate) fn set_providers_error(&mut self, message: String) {
        self.providers = Loadable::Error(message);
    }

    pub(crate) fn set_models(&mut self, harness: HarnessId, models: Loadable<Vec<Model>>) {
        self.models.insert(harness, models);
        if self.provider == Some(harness) {
            self.apply_model_defaults();
        }
    }

    pub(crate) fn current_models(&self) -> Option<&Loadable<Vec<Model>>> {
        self.models.get(&self.provider?)
    }

    pub(crate) fn current_model(&self) -> Option<&Model> {
        let id = self.model.as_deref()?;
        self.current_models()?
            .ready()?
            .iter()
            .find(|model| model.id == id)
    }

    fn apply_model_defaults(&mut self) {
        let chosen = self
            .current_models()
            .and_then(Loadable::ready)
            .and_then(|models| default_model(models))
            .map(|model| (model.id.clone(), default_reasoning(&model.reasoning_levels)));
        (self.model, self.reasoning) = match chosen {
            Some((id, reasoning)) => (Some(id), reasoning),
            None => (None, None),
        };
    }

    fn fix_child(&mut self) {
        if let ChildChoice::Other(id) = self.child
            && (Some(id) == self.provider || !self.provider_list().iter().any(|p| p.id == id))
        {
            self.child = ChildChoice::Same;
        }
    }

    pub(crate) fn select_provider(&mut self, id: HarnessId) {
        if self.provider == Some(id) || !self.provider_list().iter().any(|p| p.id == id) {
            return;
        }
        self.provider = Some(id);
        self.fix_child();
        self.apply_model_defaults();
    }

    pub(crate) fn select_model(&mut self, id: &str) {
        let Some(model) = self
            .current_models()
            .and_then(Loadable::ready)
            .and_then(|models| models.iter().find(|model| model.id == id))
        else {
            return;
        };
        let reasoning = clamp_reasoning(self.reasoning, &model.reasoning_levels);
        self.model = Some(model.id.clone());
        self.reasoning = reasoning;
    }

    pub(crate) fn select_reasoning(&mut self, level: ReasoningLevel) {
        if self
            .current_model()
            .is_some_and(|model| model.reasoning_levels.contains(&level))
        {
            self.reasoning = Some(level);
        }
    }

    pub(crate) fn set_child(&mut self, child: ChildChoice) {
        self.child = child;
        self.fix_child();
    }

    pub(crate) fn toggle_project(&mut self, id: &str) {
        if let Some(position) = self.checked.iter().position(|checked| checked == id) {
            self.checked.remove(position);
        } else if self.projects.iter().any(|project| project.id == id) {
            self.checked.push(id.to_owned());
        }
    }

    /// "Same provider" first, then every other connected provider.
    pub(crate) fn child_options(&self) -> Vec<(ChildChoice, String)> {
        let others = self
            .provider_list()
            .iter()
            .filter(|choice| Some(choice.id) != self.provider)
            .map(|choice| (ChildChoice::Other(choice.id), choice.name.clone()));
        std::iter::once((ChildChoice::Same, "Same provider".to_owned()))
            .chain(others)
            .collect()
    }

    fn disabled_reason(&self) -> Option<&'static str> {
        if self.providers.ready().is_some_and(Vec::is_empty) {
            Some(CONNECT_FIRST)
        } else if self.projects.is_empty() {
            Some(PROJECTS_REQUIRED)
        } else {
            None
        }
    }

    /// What the dialog shows under the form: the last failure, else why Create is off.
    pub(crate) fn notice(&self) -> Option<String> {
        self.error
            .clone()
            .or_else(|| match &self.providers {
                Loadable::Error(message) => Some(message.clone()),
                _ => None,
            })
            .or_else(|| self.disabled_reason().map(str::to_owned))
    }

    /// What the Model row says when it has no list to pick from.
    pub(crate) fn model_note(&self) -> Option<&'static str> {
        match self.current_models() {
            Some(Loadable::Ready(models)) if !models.is_empty() => None,
            Some(Loadable::Ready(_) | Loadable::Error(_)) => Some("Default"),
            _ => Some("Loading…"),
        }
    }

    pub(crate) fn can_submit(&self) -> bool {
        !self.submitting
            && self.disabled_reason().is_none()
            && self.provider.is_some()
            && !matches!(self.current_models(), Some(Loadable::Loading))
    }

    /// Validates and marks the form as sending. `None` when nothing may be sent.
    pub(crate) fn begin_submit(&mut self, name: &str) -> Option<Value> {
        if self.submitting {
            return None;
        }
        if let Some(reason) = self.disabled_reason() {
            self.error = Some(reason.to_owned());
            return None;
        }
        if !self.can_submit() {
            return None;
        }
        match validate(&self.form(name)?) {
            Ok(params) => {
                self.error = None;
                self.submitting = true;
                Some(params)
            }
            Err(message) => {
                self.error = Some(message.to_owned());
                None
            }
        }
    }

    pub(crate) fn finish_submit(&mut self, outcome: Result<(), String>) {
        self.submitting = false;
        self.error = outcome.err();
    }

    fn form(&self, name: &str) -> Option<NewOrchestratorForm> {
        let reasoning = self
            .reasoning
            .and_then(|level| serde_json::to_value(level).ok())
            .and_then(|value| value.as_str().map(str::to_owned));
        Some(NewOrchestratorForm {
            name: name.to_owned(),
            projects: self.checked.clone(),
            harness: self.provider?.as_str().to_owned(),
            model: self.model.clone(),
            reasoning,
            child_harness: match self.child {
                ChildChoice::Same => None,
                ChildChoice::Other(id) => Some(id.as_str().to_owned()),
            },
        })
    }
}

#[derive(Default)]
pub(crate) struct DialogSelects {
    pub provider: SelectState,
    pub model: SelectState,
    pub reasoning: SelectState,
    pub children: SelectState,
}

pub(crate) struct OpenDialog {
    pub session: u64,
    pub state: DialogState,
    pub name: Entity<ComposerInput>,
    pub card_focus: FocusHandle,
    pub focus_pending: bool,
    pub _events: Subscription,
}

/// The open "New orchestrator" window, if any. The select states outlive the
/// window because a closing menu still reaches them for its exit animation.
#[derive(Default)]
pub(crate) struct NewOrchestratorDialog {
    pub open: Option<OpenDialog>,
    pub selects: DialogSelects,
    pub sessions: u64,
    pub pending_chat: Option<String>,
    pub pending_baseline: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::popover::Loadable;
    use serde_json::json;
    use zeron_engine::registry::HarnessDescriptor;
    use zeron_proto::{HarnessId, Model, ReasoningLevel, SteeringMode};

    fn form() -> NewOrchestratorForm {
        NewOrchestratorForm {
            name: "  Plataforma ".into(),
            projects: vec!["api".into(), "web".into()],
            harness: "claude-code".into(),
            model: None,
            reasoning: None,
            child_harness: None,
        }
    }

    #[test]
    fn blank_name_is_required() {
        let form = NewOrchestratorForm {
            name: "  ".into(),
            ..form()
        };
        assert_eq!(validate(&form), Err("Name is required."));
    }

    #[test]
    fn projects_are_required() {
        let form = NewOrchestratorForm {
            projects: Vec::new(),
            ..form()
        };
        assert_eq!(validate(&form), Err("Select at least one project."));
    }

    #[test]
    fn valid_form_builds_params() {
        assert_eq!(
            validate(&form()),
            Ok(json!({
                "name": "Plataforma",
                "projects": ["api", "web"],
                "harness": "claude-code",
            }))
        );
        let full = NewOrchestratorForm {
            model: Some("opus".into()),
            reasoning: Some("high".into()),
            child_harness: Some("codex".into()),
            ..form()
        };
        assert_eq!(
            validate(&full),
            Ok(json!({
                "name": "Plataforma",
                "projects": ["api", "web"],
                "harness": "claude-code",
                "model": "opus",
                "reasoning": "high",
                "childHarness": "codex",
            }))
        );
    }

    fn descriptor(id: HarnessId, name: &str, installed: bool) -> HarnessDescriptor {
        HarnessDescriptor {
            id,
            name: name.into(),
            supports_steering: true,
            steering_mode: SteeringMode::StepBoundary,
            reasoning_levels: Vec::new(),
            installed,
            can_install: false,
            enabled: Some(true),
        }
    }

    fn model(id: &str, levels: &[ReasoningLevel]) -> Model {
        Model {
            id: id.into(),
            label: id.to_uppercase(),
            description: None,
            reasoning_levels: levels.to_vec(),
            options: Vec::new(),
        }
    }

    fn both() -> Vec<HarnessDescriptor> {
        vec![
            descriptor(HarnessId::Codex, "Codex", true),
            descriptor(HarnessId::Cursor, "Cursor", true),
            descriptor(HarnessId::ClaudeCode, "Claude Code", true),
        ]
    }

    fn catalog() -> Vec<Model> {
        vec![
            model("opus", &[ReasoningLevel::Low, ReasoningLevel::High]),
            model("sonnet", &[ReasoningLevel::Low, ReasoningLevel::Medium]),
        ]
    }

    fn projects() -> Vec<ProjectChoice> {
        vec![
            ProjectChoice {
                id: "api".into(),
                label: "api".into(),
            },
            ProjectChoice {
                id: "web".into(),
                label: "web".into(),
            },
        ]
    }

    fn ready_state() -> DialogState {
        let mut state = DialogState::new(projects());
        state.set_providers(&both());
        state.set_models(HarnessId::ClaudeCode, Loadable::Ready(catalog()));
        state.toggle_project("api");
        state
    }

    #[test]
    fn providers_are_connected_claude_code_and_codex_only() {
        let mut list = both();
        list.push(descriptor(HarnessId::Grok, "Grok", true));
        let ids: Vec<_> = provider_choices(&list).iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![HarnessId::ClaudeCode, HarnessId::Codex]);

        let uninstalled = vec![
            descriptor(HarnessId::ClaudeCode, "Claude Code", false),
            descriptor(HarnessId::Codex, "Codex", true),
        ];
        let ids: Vec<_> = provider_choices(&uninstalled)
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec![HarnessId::Codex]);
    }

    #[test]
    fn no_connected_provider_blocks_with_the_connect_message() {
        let mut state = DialogState::new(projects());
        state.set_providers(&[descriptor(HarnessId::Grok, "Grok", true)]);
        assert_eq!(state.provider, None);
        assert_eq!(
            state.notice().as_deref(),
            Some("Connect Claude Code or Codex first.")
        );
        assert!(!state.can_submit());
        assert!(state.begin_submit("Plataforma").is_none());
        assert!(!state.submitting);
    }

    #[test]
    fn no_projects_blocks_with_the_validation_message() {
        let mut state = DialogState::new(Vec::new());
        state.set_providers(&both());
        state.set_models(HarnessId::ClaudeCode, Loadable::Ready(catalog()));
        assert_eq!(
            state.notice().as_deref(),
            Some("Select at least one project.")
        );
        assert!(!state.can_submit());
        assert!(state.begin_submit("Plataforma").is_none());
    }

    #[test]
    fn first_connected_provider_and_its_default_model_are_selected() {
        let state = ready_state();
        assert_eq!(state.provider, Some(HarnessId::ClaudeCode));
        assert_eq!(state.model.as_deref(), Some("opus"));
        assert_eq!(state.reasoning, Some(ReasoningLevel::High));
        assert_eq!(state.child, ChildChoice::Same);
    }

    #[test]
    fn create_waits_for_the_model_catalog() {
        let mut state = DialogState::new(projects());
        state.set_providers(&both());
        state.toggle_project("api");
        assert!(!state.can_submit());
        state.set_models(HarnessId::ClaudeCode, Loadable::Error("offline".into()));
        assert!(state.can_submit());
        let params = state.begin_submit("Plataforma").unwrap();
        assert_eq!(
            params,
            json!({"name": "Plataforma", "projects": ["api"], "harness": "claude-code"})
        );
    }

    #[test]
    fn picking_a_model_clamps_reasoning_to_its_levels() {
        let mut state = ready_state();
        state.select_reasoning(ReasoningLevel::Low);
        state.select_model("sonnet");
        assert_eq!(state.model.as_deref(), Some("sonnet"));
        assert_eq!(state.reasoning, Some(ReasoningLevel::Low));

        state.select_reasoning(ReasoningLevel::Medium);
        state.select_model("opus");
        assert_eq!(state.reasoning, Some(ReasoningLevel::High));

        state.select_reasoning(ReasoningLevel::Max);
        assert_eq!(state.reasoning, Some(ReasoningLevel::High));
    }

    #[test]
    fn switching_provider_resets_model_and_drops_a_matching_child() {
        let mut state = ready_state();
        state.set_models(
            HarnessId::Codex,
            Loadable::Ready(vec![model("gpt", &[ReasoningLevel::Medium])]),
        );
        state.set_child(ChildChoice::Other(HarnessId::Codex));
        state.select_provider(HarnessId::Codex);
        assert_eq!(state.provider, Some(HarnessId::Codex));
        assert_eq!(state.model.as_deref(), Some("gpt"));
        assert_eq!(state.reasoning, Some(ReasoningLevel::Medium));
        assert_eq!(state.child, ChildChoice::Same);

        let options: Vec<_> = state
            .child_options()
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        assert_eq!(options, vec!["Same provider", "Claude Code"]);
    }

    #[test]
    fn selection_builds_the_create_params() {
        let mut state = ready_state();
        state.toggle_project("web");
        state.set_child(ChildChoice::Other(HarnessId::Codex));
        assert_eq!(
            state.begin_submit(" Plataforma ").unwrap(),
            json!({
                "name": "Plataforma",
                "projects": ["api", "web"],
                "harness": "claude-code",
                "model": "opus",
                "reasoning": "high",
                "childHarness": "codex",
            })
        );
    }

    #[test]
    fn toggling_a_project_twice_unchecks_it() {
        let mut state = ready_state();
        state.toggle_project("api");
        assert!(state.checked.is_empty());
        assert_eq!(state.begin_submit("Plataforma"), None,);
        assert_eq!(state.error.as_deref(), Some("Select at least one project."));
    }

    #[test]
    fn validation_message_is_kept_until_the_next_attempt() {
        let mut state = ready_state();
        assert!(state.begin_submit("  ").is_none());
        assert_eq!(state.error.as_deref(), Some("Name is required."));
        assert!(!state.submitting);
        assert!(state.begin_submit("Plataforma").is_some());
        assert_eq!(state.error, None);
    }

    #[test]
    fn submitting_blocks_a_second_submit() {
        let mut state = ready_state();
        assert!(state.begin_submit("Plataforma").is_some());
        assert!(state.submitting);
        assert!(!state.can_submit());
        assert!(state.begin_submit("Plataforma").is_none());
    }

    #[test]
    fn engine_error_is_kept_and_inputs_stay() {
        let mut state = ready_state();
        state.toggle_project("web");
        state.select_model("sonnet");
        let before = (state.checked.clone(), state.model.clone(), state.reasoning);
        assert!(state.begin_submit("Plataforma").is_some());
        state.finish_submit(Err("unknown project id `web`".into()));
        assert!(!state.submitting);
        assert_eq!(state.error.as_deref(), Some("unknown project id `web`"));
        assert_eq!(
            before,
            (state.checked.clone(), state.model.clone(), state.reasoning)
        );
        assert!(state.can_submit());
    }

    #[test]
    fn created_reply_yields_the_chat_id() {
        assert_eq!(
            parse_created(json!({"orchestratorId": "plataforma", "chatId": "c1"})),
            Ok("c1".to_owned())
        );
        assert!(parse_created(json!({"orchestratorId": "plataforma"})).is_err());
    }

    #[test]
    fn provider_load_failure_becomes_the_notice_and_blocks_create() {
        let mut state = DialogState::new(projects());
        assert!(matches!(state.providers, Loadable::Loading));
        assert_eq!(state.notice(), None);
        assert!(!state.can_submit());
        state.set_providers_error("engine offline".into());
        assert_eq!(state.notice().as_deref(), Some("engine offline"));
        assert!(!state.can_submit());
        assert!(state.begin_submit("Plataforma").is_none());
    }

    #[test]
    fn model_note_follows_the_catalog_state() {
        let mut state = DialogState::new(projects());
        state.set_providers(&both());
        assert_eq!(state.model_note(), Some("Loading…"));
        state.set_models(HarnessId::ClaudeCode, Loadable::Ready(Vec::new()));
        assert_eq!(state.model_note(), Some("Default"));
        state.set_models(HarnessId::ClaudeCode, Loadable::Error("x".into()));
        assert_eq!(state.model_note(), Some("Default"));
        state.set_models(HarnessId::ClaudeCode, Loadable::Ready(catalog()));
        assert_eq!(state.model_note(), None);
    }
}
