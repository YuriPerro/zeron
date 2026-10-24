use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use zeron_proto::{HarnessId, ReasoningLevel};

use super::defaults::DEFAULT_ORCHESTRATOR_PROMPT;

const CONFIG_FILE: &str = "orchestrator.toml";
const PROMPT_FILE: &str = "prompt.md";
const MEMORY_FILE: &str = "memoria.md";
const HISTORY_FILE: &str = "historico.md";
const DEMANDS_DIR: &str = "demandas";
const ARCHIVE_DIR: &str = "arquivo";
const REPORTS_DIR: &str = "relatorios";
const SPEC_FILE: &str = "spec.md";
const PLAN_FILE: &str = "plano.md";
const MAX_ID_LEN: usize = 80;
const MAX_SLUG_LEN: usize = 48;
const FALLBACK_SLUG: &str = "demanda";

/// An orchestrator as stored in `orchestrator.toml`. `projects` are space ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub avatar: Option<String>,
    pub harness: HarnessId,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning: Option<ReasoningLevel>,
    pub child_harness: HarnessId,
    #[serde(default)]
    pub projects: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

/// One line of a demand's `plano.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub text: String,
    pub status: PlanStatus,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
}

/// Everything stored for one demand. `reports` are `(child id, text)` sorted by child id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemandView {
    pub demand: String,
    pub spec: String,
    pub plan: String,
    pub reports: Vec<(String, String)>,
}

/// File-backed storage for orchestrators under `{data_dir}/orchestrators`.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn check_id(kind: &str, value: &str) -> io::Result<()> {
    let mut chars = value.chars();
    let head_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if head_ok && rest_ok && value.len() <= MAX_ID_LEN {
        Ok(())
    } else {
        Err(invalid(format!("invalid {kind} id: {value:?}")))
    }
}

fn slugify(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars().map(|c| c.to_ascii_lowercase()) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(MAX_SLUG_LEN);
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        FALLBACK_SLUG.to_string()
    } else {
        slug.to_string()
    }
}

fn render_plan(items: &[PlanItem]) -> String {
    let mut out = String::new();
    for item in items {
        let mark = match item.status {
            PlanStatus::Pending => ' ',
            PlanStatus::InProgress => '~',
            PlanStatus::Completed => 'x',
        };
        out.push_str(&format!("- [{mark}] {}", item.text));
        for extra in [&item.role, &item.project].into_iter().flatten() {
            out.push_str(&format!(" · {extra}"));
        }
        out.push('\n');
    }
    out
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn read_or_empty(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        other => other,
    }
}

fn append_line(path: &Path, line: &str) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?;
    writeln!(file, "{line}")
}

impl Store {
    pub fn new(data_dir: &Path) -> Store {
        Store {
            root: data_dir.join("orchestrators"),
        }
    }

    fn orchestrator_dir(&self, id: &str) -> io::Result<PathBuf> {
        check_id("orchestrator", id)?;
        Ok(self.root.join(id))
    }

    fn existing_orchestrator_dir(&self, id: &str) -> io::Result<PathBuf> {
        let dir = self.orchestrator_dir(id)?;
        if dir.is_dir() {
            Ok(dir)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("orchestrator {id} not found"),
            ))
        }
    }

    fn demand_dir(&self, id: &str, demand: &str) -> io::Result<PathBuf> {
        check_id("demand", demand)?;
        if demand == ARCHIVE_DIR {
            return Err(invalid(format!("invalid demand id: {demand:?}")));
        }
        let dir = self
            .existing_orchestrator_dir(id)?
            .join(DEMANDS_DIR)
            .join(demand);
        if dir.is_dir() {
            Ok(dir)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("demand {demand} not found"),
            ))
        }
    }

    pub fn create(&self, cfg: &OrchestratorConfig) -> io::Result<()> {
        let dir = self.orchestrator_dir(&cfg.id)?;
        let toml = toml::to_string_pretty(cfg).map_err(io::Error::other)?;
        fs::create_dir_all(&self.root)?;
        fs::create_dir(&dir)?;
        fs::create_dir(dir.join(DEMANDS_DIR))?;
        fs::write(dir.join(CONFIG_FILE), toml)?;
        fs::write(dir.join(PROMPT_FILE), DEFAULT_ORCHESTRATOR_PROMPT)?;
        fs::write(dir.join(MEMORY_FILE), "")?;
        fs::write(dir.join(HISTORY_FILE), "")
    }

    pub fn get(&self, id: &str) -> io::Result<OrchestratorConfig> {
        let text = fs::read_to_string(self.orchestrator_dir(id)?.join(CONFIG_FILE))?;
        toml::from_str(&text).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    /// Every orchestrator with a config file, sorted by id.
    pub fn list(&self) -> io::Result<Vec<OrchestratorConfig>> {
        let entries = match fs::read_dir(&self.root) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            other => other?,
        };
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.path().join(CONFIG_FILE).is_file() {
                if let Some(id) = entry.file_name().to_str() {
                    ids.push(id.to_string());
                }
            }
        }
        ids.sort();
        ids.iter().map(|id| self.get(id)).collect()
    }

    pub fn prompt(&self, id: &str) -> io::Result<String> {
        fs::read_to_string(self.orchestrator_dir(id)?.join(PROMPT_FILE))
    }

    pub fn memory(&self, id: &str) -> io::Result<String> {
        fs::read_to_string(self.orchestrator_dir(id)?.join(MEMORY_FILE))
    }

    /// Creates the demand folder and returns its id, `<YYYY-MM-DD>-<slug>`,
    /// suffixed `-2`, `-3`... when the name is taken.
    pub fn create_demand(&self, id: &str, title: &str, today: NaiveDate) -> io::Result<String> {
        let demands = self.existing_orchestrator_dir(id)?.join(DEMANDS_DIR);
        fs::create_dir_all(&demands)?;
        let base = format!("{}-{}", today.format("%Y-%m-%d"), slugify(title));
        let mut attempt = 1u32;
        loop {
            let demand = if attempt == 1 {
                base.clone()
            } else {
                format!("{base}-{attempt}")
            };
            match fs::create_dir(demands.join(&demand)) {
                Ok(()) => return Ok(demand),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => attempt += 1,
                Err(err) => return Err(err),
            }
        }
    }

    pub fn write_spec(&self, id: &str, demand: &str, text: &str) -> io::Result<()> {
        fs::write(self.demand_dir(id, demand)?.join(SPEC_FILE), text)
    }

    pub fn write_plan(&self, id: &str, demand: &str, items: &[PlanItem]) -> io::Result<()> {
        fs::write(
            self.demand_dir(id, demand)?.join(PLAN_FILE),
            render_plan(items),
        )
    }

    pub fn submit_report(
        &self,
        id: &str,
        demand: &str,
        child_id: &str,
        text: &str,
    ) -> io::Result<()> {
        check_id("child", child_id)?;
        let reports = self.demand_dir(id, demand)?.join(REPORTS_DIR);
        fs::create_dir_all(&reports)?;
        fs::write(reports.join(format!("{child_id}.md")), text)
    }

    pub fn read_demand(&self, id: &str, demand: &str) -> io::Result<DemandView> {
        let dir = self.demand_dir(id, demand)?;
        let mut reports = Vec::new();
        match fs::read_dir(dir.join(REPORTS_DIR)) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
            Ok(entries) => {
                for entry in entries {
                    let path = entry?.path();
                    let child = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .filter(|_| path.extension().is_some_and(|ext| ext == "md"));
                    if let Some(child) = child {
                        reports.push((child.to_string(), fs::read_to_string(&path)?));
                    }
                }
            }
        }
        reports.sort();
        Ok(DemandView {
            demand: demand.to_string(),
            spec: read_or_empty(&dir.join(SPEC_FILE))?,
            plan: read_or_empty(&dir.join(PLAN_FILE))?,
            reports,
        })
    }

    /// Moves the demand to `demandas/arquivo/` and logs it in `historico.md`.
    pub fn close_demand(
        &self,
        id: &str,
        demand: &str,
        summary: &str,
        today: NaiveDate,
    ) -> io::Result<()> {
        let source = self.demand_dir(id, demand)?;
        let orchestrator = self.existing_orchestrator_dir(id)?;
        let archive = orchestrator.join(DEMANDS_DIR).join(ARCHIVE_DIR);
        fs::create_dir_all(&archive)?;
        let target = archive.join(demand);
        if target.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("demand {demand} is already archived"),
            ));
        }
        fs::rename(source, target)?;
        append_line(
            &orchestrator.join(HISTORY_FILE),
            &format!(
                "- {} · {demand} · {}",
                today.format("%Y-%m-%d"),
                single_line(summary)
            ),
        )
    }

    pub fn remember(&self, id: &str, note: &str) -> io::Result<()> {
        append_line(
            &self.existing_orchestrator_dir(id)?.join(MEMORY_FILE),
            &format!("- {}", single_line(note)),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use chrono::NaiveDate;
    use zeron_proto::HarnessId;

    use super::*;
    use crate::orchestration::defaults::DEFAULT_ORCHESTRATOR_PROMPT;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 24).unwrap()
    }

    fn config(id: &str) -> OrchestratorConfig {
        OrchestratorConfig {
            id: id.into(),
            name: "Loja".into(),
            avatar: None,
            harness: HarnessId::ClaudeCode,
            model: Some("claude-opus-5-5".into()),
            reasoning: None,
            child_harness: HarnessId::Codex,
            projects: vec!["space-1".into(), "space-2".into()],
        }
    }

    fn store_with(id: &str) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.create(&config(id)).unwrap();
        (dir, store)
    }

    fn read(dir: &tempfile::TempDir, rel: &str) -> String {
        std::fs::read_to_string(dir.path().join("orchestrators").join(rel)).unwrap()
    }

    #[test]
    fn create_writes_config_prompt_and_empty_notes() {
        let (dir, store) = store_with("loja");
        assert_eq!(store.get("loja").unwrap(), config("loja"));
        assert_eq!(store.list().unwrap(), vec![config("loja")]);
        assert_eq!(store.prompt("loja").unwrap(), DEFAULT_ORCHESTRATOR_PROMPT);
        assert_eq!(store.memory("loja").unwrap(), "");
        assert_eq!(read(&dir, "loja/historico.md"), "");
        assert!(
            dir.path()
                .join("orchestrators/loja/orchestrator.toml")
                .is_file()
        );
    }

    #[test]
    fn demand_slug_is_dated_and_unique() {
        let (_dir, store) = store_with("loja");
        let first = store
            .create_demand("loja", "Cupom expirado no checkout", day())
            .unwrap();
        let second = store
            .create_demand("loja", "Cupom expirado no checkout", day())
            .unwrap();
        let third = store
            .create_demand("loja", "Cupom expirado no checkout", day())
            .unwrap();
        assert_eq!(first, "2026-10-24-cupom-expirado-no-checkout");
        assert_eq!(second, "2026-10-24-cupom-expirado-no-checkout-2");
        assert_eq!(third, "2026-10-24-cupom-expirado-no-checkout-3");
    }

    #[test]
    fn demand_slug_is_capped_and_ascii() {
        let (_dir, store) = store_with("loja");
        let long = store
            .create_demand("loja", &"Abc ".repeat(40), day())
            .unwrap();
        let slug = long.strip_prefix("2026-10-24-").unwrap();
        assert!(slug.len() <= 48);
        assert!(!slug.ends_with('-'));
        let symbols = store.create_demand("loja", "!!! ???", day()).unwrap();
        assert!(symbols.starts_with("2026-10-24-"));
        assert!(!symbols.ends_with('-'));
    }

    #[test]
    fn plan_renders_status_role_and_project() {
        let (dir, store) = store_with("loja");
        let demand = store.create_demand("loja", "Plano", day()).unwrap();
        let items = [
            PlanItem {
                text: "Explorar".into(),
                status: PlanStatus::Completed,
                role: Some("explorador".into()),
                project: Some("api".into()),
            },
            PlanItem {
                text: "Implementar".into(),
                status: PlanStatus::InProgress,
                role: Some("executor".into()),
                project: None,
            },
            PlanItem {
                text: "Revisar".into(),
                status: PlanStatus::Pending,
                role: None,
                project: None,
            },
        ];
        store.write_plan("loja", &demand, &items).unwrap();
        assert_eq!(
            read(&dir, &format!("loja/demandas/{demand}/plano.md")),
            "- [x] Explorar · explorador · api\n- [~] Implementar · executor\n- [ ] Revisar\n"
        );
    }

    #[test]
    fn read_demand_returns_spec_plan_and_reports() {
        let (_dir, store) = store_with("loja");
        let demand = store.create_demand("loja", "Leitura", day()).unwrap();
        let empty = store.read_demand("loja", &demand).unwrap();
        assert_eq!(empty.spec, "");
        assert_eq!(empty.plan, "");
        assert!(empty.reports.is_empty());

        store.write_spec("loja", &demand, "# Spec").unwrap();
        store
            .write_plan(
                "loja",
                &demand,
                &[PlanItem {
                    text: "Um".into(),
                    status: PlanStatus::Pending,
                    role: None,
                    project: None,
                }],
            )
            .unwrap();
        store
            .submit_report("loja", &demand, "zeta", "feito Z")
            .unwrap();
        store
            .submit_report("loja", &demand, "alfa", "feito A")
            .unwrap();

        let view = store.read_demand("loja", &demand).unwrap();
        assert_eq!(view.demand, demand);
        assert_eq!(view.spec, "# Spec");
        assert_eq!(view.plan, "- [ ] Um\n");
        assert_eq!(
            view.reports,
            vec![
                ("alfa".to_string(), "feito A".to_string()),
                ("zeta".to_string(), "feito Z".to_string())
            ]
        );
    }

    #[test]
    fn close_moves_to_archive_and_appends_history() {
        let (dir, store) = store_with("loja");
        let demand = store.create_demand("loja", "Fechar", day()).unwrap();
        store.write_spec("loja", &demand, "spec").unwrap();
        store
            .close_demand("loja", &demand, "Cupom corrigido", day())
            .unwrap();
        assert!(
            !dir.path()
                .join(format!("orchestrators/loja/demandas/{demand}"))
                .exists()
        );
        assert_eq!(
            read(&dir, &format!("loja/demandas/arquivo/{demand}/spec.md")),
            "spec"
        );
        assert_eq!(
            read(&dir, "loja/historico.md"),
            format!("- 2026-10-24 · {demand} · Cupom corrigido\n")
        );
    }

    #[test]
    fn remember_appends_notes() {
        let (_dir, store) = store_with("loja");
        store.remember("loja", "usa bun").unwrap();
        store.remember("loja", "prefere PRs curtos").unwrap();
        assert_eq!(
            store.memory("loja").unwrap(),
            "- usa bun\n- prefere PRs curtos\n"
        );
    }

    #[test]
    fn rejects_traversal_ids() {
        let (dir, store) = store_with("loja");
        let demand = store.create_demand("loja", "Seguro", day()).unwrap();
        for bad in ["../x", "a/b", "Upper", "", "-lead"] {
            let kind = |r: std::io::Result<()>| r.unwrap_err().kind();
            assert_eq!(
                kind(store.create(&config(bad))),
                ErrorKind::InvalidInput,
                "{bad}"
            );
            assert_eq!(
                store.get(bad).unwrap_err().kind(),
                ErrorKind::InvalidInput,
                "{bad}"
            );
            assert_eq!(
                store.create_demand(bad, "t", day()).unwrap_err().kind(),
                ErrorKind::InvalidInput,
                "{bad}"
            );
            assert_eq!(
                kind(store.remember(bad, "n")),
                ErrorKind::InvalidInput,
                "{bad}"
            );
            assert_eq!(
                kind(store.write_spec("loja", bad, "x")),
                ErrorKind::InvalidInput,
                "{bad}"
            );
            assert_eq!(
                kind(store.submit_report("loja", &demand, bad, "x")),
                ErrorKind::InvalidInput,
                "{bad}"
            );
            assert_eq!(
                store.read_demand("loja", bad).unwrap_err().kind(),
                ErrorKind::InvalidInput,
                "{bad}"
            );
            assert_eq!(
                kind(store.close_demand("loja", bad, "s", day())),
                ErrorKind::InvalidInput,
                "{bad}"
            );
        }
        assert_eq!(
            store.write_spec("loja", "arquivo", "x").unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
        let long = "a".repeat(81);
        assert_eq!(
            store.get(&long).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
        let mut entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        entries.sort();
        assert_eq!(entries, vec![std::ffi::OsString::from("orchestrators")]);
        let roots: Vec<_> = std::fs::read_dir(dir.path().join("orchestrators"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(roots, vec![std::ffi::OsString::from("loja")]);
    }
}
