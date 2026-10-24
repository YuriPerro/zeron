use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeron_proto::{HarnessId, Model, ReasoningLevel};

const LADDER: [ReasoningLevel; 6] = [
    ReasoningLevel::Minimal,
    ReasoningLevel::Low,
    ReasoningLevel::Medium,
    ReasoningLevel::High,
    ReasoningLevel::XHigh,
    ReasoningLevel::Max,
];

/// How hard a child should think relative to the candidate's base level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelativeEffort {
    Low,
    Medium,
    High,
}

/// Model aliases (`@forte`) mapped to ordered `model:level` candidates per harness.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct ModelMap {
    #[serde(default, rename = "alias")]
    pub aliases: BTreeMap<String, BTreeMap<HarnessId, Vec<String>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid model map: {0}")]
pub struct ModelMapError(pub String);

/// The model and reasoning level a role resolved to on one harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModel {
    pub model: String,
    pub reasoning: Option<ReasoningLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResolveError {
    #[error("unknown model alias `{0}`")]
    UnknownAlias(String),
    #[error(
        "no candidate for alias `{alias}` on {} is available in the catalog (tried: {})",
        .harness.as_str(), .tried.join(", ")
    )]
    NoCandidate {
        alias: String,
        harness: HarnessId,
        tried: Vec<String>,
    },
}

/// Parses the `[alias.<name>]` tables of `models.toml`.
pub fn parse_model_map(text: &str) -> Result<ModelMap, ModelMapError> {
    toml::from_str(text).map_err(|err| ModelMapError(err.to_string()))
}

/// Picks the first candidate of `alias` on `harness` whose model is in
/// `catalog`, then fits its level to what the model supports and shifts it by
/// `effort`.
pub fn resolve(
    map: &ModelMap,
    alias: &str,
    harness: HarnessId,
    catalog: &[Model],
    effort: Option<RelativeEffort>,
) -> Result<ResolvedModel, ResolveError> {
    let alias = alias.strip_prefix('@').unwrap_or(alias);
    let by_harness = map
        .aliases
        .get(alias)
        .ok_or_else(|| ResolveError::UnknownAlias(alias.to_string()))?;
    let candidates = by_harness
        .get(&harness)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for candidate in candidates {
        let (id, base) = split_candidate(candidate);
        if let Some(model) = catalog.iter().find(|model| model.id == id) {
            return Ok(ResolvedModel {
                model: model.id.clone(),
                reasoning: fit_level(&model.reasoning_levels, base, effort),
            });
        }
    }
    Err(ResolveError::NoCandidate {
        alias: alias.to_string(),
        harness,
        tried: candidates.to_vec(),
    })
}

fn split_candidate(candidate: &str) -> (&str, Option<ReasoningLevel>) {
    match candidate.rsplit_once(':') {
        Some((id, level)) => match parse_level(level) {
            Some(level) => (id, Some(level)),
            None => (candidate, None),
        },
        None => (candidate, None),
    }
}

fn parse_level(text: &str) -> Option<ReasoningLevel> {
    Some(match text {
        "minimal" => ReasoningLevel::Minimal,
        "low" => ReasoningLevel::Low,
        "medium" => ReasoningLevel::Medium,
        "high" => ReasoningLevel::High,
        "xhigh" => ReasoningLevel::XHigh,
        "max" => ReasoningLevel::Max,
        "ultra" => ReasoningLevel::Ultra,
        "ultracode" => ReasoningLevel::Ultracode,
        "ultrathink" => ReasoningLevel::Ultrathink,
        _ => return None,
    })
}

fn fit_level(
    supported: &[ReasoningLevel],
    base: Option<ReasoningLevel>,
    effort: Option<RelativeEffort>,
) -> Option<ReasoningLevel> {
    if supported.is_empty() {
        return None;
    }
    let ladder: Vec<ReasoningLevel> = LADDER
        .into_iter()
        .filter(|level| supported.contains(level))
        .collect();
    let base = match (base, effort) {
        (Some(base), _) => base,
        (None, Some(_)) => ReasoningLevel::Medium,
        (None, None) => return None,
    };
    if !LADDER.contains(&base) && supported.contains(&base) {
        return Some(base);
    }
    let rank = |level: ReasoningLevel| LADDER.iter().position(|step| *step == level);
    let wanted = rank(base).unwrap_or(LADDER.len() - 1);
    let below = ladder
        .iter()
        .rposition(|level| rank(*level) <= Some(wanted));
    let index = below.or_else(|| (!ladder.is_empty()).then_some(0))?;
    let shifted = match effort {
        Some(RelativeEffort::Low) => index.saturating_sub(1),
        Some(RelativeEffort::High) => (index + 1).min(ladder.len() - 1),
        Some(RelativeEffort::Medium) | None => index,
    };
    Some(ladder[shifted])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::defaults::DEFAULT_MODELS_TOML;

    fn model(id: &str, levels: &[ReasoningLevel]) -> Model {
        Model {
            id: id.to_string(),
            label: id.to_string(),
            description: None,
            reasoning_levels: levels.to_vec(),
            options: Vec::new(),
        }
    }

    fn default_map() -> ModelMap {
        parse_model_map(DEFAULT_MODELS_TOML).unwrap()
    }

    fn single(candidate: &str) -> ModelMap {
        parse_model_map(&format!("[alias.x]\nclaude-code = [\"{candidate}\"]\n")).unwrap()
    }

    #[test]
    fn resolves_first_available_candidate() {
        let catalog = [model(
            "claude-fable-5-1",
            &[
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
            ],
        )];
        let resolved = resolve(
            &default_map(),
            "@forte",
            HarnessId::ClaudeCode,
            &catalog,
            None,
        )
        .unwrap();
        assert_eq!(resolved.model, "claude-fable-5-1");
        assert_eq!(resolved.reasoning, Some(ReasoningLevel::High));
    }

    #[test]
    fn no_candidate_names_alias_harness_and_tried() {
        let err = resolve(&default_map(), "@forte", HarnessId::ClaudeCode, &[], None).unwrap_err();
        let text = err.to_string();
        for needle in [
            "forte",
            "claude-code",
            "claude-opus-5-5:high",
            "claude-fable-5-1:high",
        ] {
            assert!(text.contains(needle), "{text} missing {needle}");
        }
    }

    #[test]
    fn unknown_alias_is_an_error() {
        let err = resolve(&default_map(), "@nada", HarnessId::ClaudeCode, &[], None).unwrap_err();
        assert!(matches!(err, ResolveError::UnknownAlias(name) if name == "nada"));
    }

    #[test]
    fn relative_effort_moves_one_step() {
        use ReasoningLevel::*;
        let catalog = [model("m", &[Low, Medium, High, Max])];
        let at = |base: &str, effort| {
            resolve(
                &single(base),
                "x",
                HarnessId::ClaudeCode,
                &catalog,
                Some(effort),
            )
            .unwrap()
            .reasoning
        };
        assert_eq!(at("m:medium", RelativeEffort::Low), Some(Low));
        assert_eq!(at("m:medium", RelativeEffort::High), Some(High));
        assert_eq!(at("m:medium", RelativeEffort::Medium), Some(Medium));
        assert_eq!(at("m:max", RelativeEffort::High), Some(Max));
        assert_eq!(at("m:low", RelativeEffort::Low), Some(Low));
    }

    #[test]
    fn unsupported_level_clamps_down() {
        use ReasoningLevel::*;
        let catalog = [model("m", &[Low, High])];
        let resolved = resolve(
            &single("m:medium"),
            "x",
            HarnessId::ClaudeCode,
            &catalog,
            None,
        )
        .unwrap();
        assert_eq!(resolved.reasoning, Some(Low));
    }

    #[test]
    fn unsupported_level_clamps_up_when_nothing_below() {
        use ReasoningLevel::*;
        let catalog = [model("m", &[High, Max])];
        let resolved =
            resolve(&single("m:low"), "x", HarnessId::ClaudeCode, &catalog, None).unwrap();
        assert_eq!(resolved.reasoning, Some(High));
    }

    #[test]
    fn model_without_levels_resolves_without_reasoning() {
        let catalog = [model("m", &[])];
        let resolved = resolve(
            &single("m:high"),
            "x",
            HarnessId::ClaudeCode,
            &catalog,
            Some(RelativeEffort::High),
        )
        .unwrap();
        assert_eq!(resolved.model, "m");
        assert_eq!(resolved.reasoning, None);
    }

    #[test]
    fn level_outside_the_ladder_needs_exact_support() {
        use ReasoningLevel::*;
        let with = [model("m", &[High, Max, Ultra])];
        let without = [model("m", &[High, Max])];
        let exact = resolve(
            &single("m:ultra"),
            "x",
            HarnessId::ClaudeCode,
            &with,
            Some(RelativeEffort::Low),
        )
        .unwrap();
        assert_eq!(exact.reasoning, Some(Ultra));
        let clamped = resolve(
            &single("m:ultra"),
            "x",
            HarnessId::ClaudeCode,
            &without,
            None,
        )
        .unwrap();
        assert_eq!(clamped.reasoning, Some(Max));
    }

    #[test]
    fn candidate_without_level_has_no_reasoning_unless_effort_asked() {
        use ReasoningLevel::*;
        let catalog = [model("m", &[Low, Medium, High])];
        let plain = resolve(&single("m"), "x", HarnessId::ClaudeCode, &catalog, None).unwrap();
        assert_eq!(plain.reasoning, None);
        let asked = resolve(
            &single("m"),
            "x",
            HarnessId::ClaudeCode,
            &catalog,
            Some(RelativeEffort::High),
        )
        .unwrap();
        assert_eq!(asked.reasoning, Some(High));
    }

    #[test]
    fn missing_harness_reports_no_candidate() {
        let err = resolve(&default_map(), "forte", HarnessId::Cursor, &[], None).unwrap_err();
        assert!(matches!(err, ResolveError::NoCandidate { tried, .. } if tried.is_empty()));
    }
}
