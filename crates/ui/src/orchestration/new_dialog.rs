use serde_json::{Map, Value};

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
