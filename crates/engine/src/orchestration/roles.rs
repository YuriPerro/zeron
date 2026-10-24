use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::defaults::{DEFAULT_MODELS_TOML, DEFAULT_ROLES};

const FENCE: &str = "+++";
const MODELS_FILE: &str = "models.toml";

/// What a child running a role may do to the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoleAccess {
    ReadOnly,
    #[default]
    Write,
}

/// A global role: a name, a model alias and the prompt the child starts with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    pub description: String,
    pub model: String,
    #[serde(default)]
    pub access: RoleAccess,
    #[serde(skip)]
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RoleError {
    #[error("role file must start with a +++ frontmatter block")]
    MissingFrontmatter,
    #[error("role frontmatter is not closed by a +++ line")]
    UnterminatedFrontmatter,
    #[error("invalid role frontmatter: {0}")]
    InvalidFrontmatter(String),
    #[error("{}: {reason}", .path.display())]
    File { path: PathBuf, reason: String },
}

/// Parses a role file: TOML frontmatter between `+++` lines, then the prompt.
pub fn parse_role(text: &str) -> Result<Role, RoleError> {
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some(FENCE) {
        return Err(RoleError::MissingFrontmatter);
    }
    let mut frontmatter = Vec::new();
    let mut closed = false;
    let mut body = Vec::new();
    for line in lines {
        if closed {
            body.push(line);
        } else if line.trim_end() == FENCE {
            closed = true;
        } else {
            frontmatter.push(line);
        }
    }
    if !closed {
        return Err(RoleError::UnterminatedFrontmatter);
    }
    let mut role: Role = toml::from_str(&frontmatter.join("\n"))
        .map_err(|err| RoleError::InvalidFrontmatter(err.to_string()))?;
    role.prompt = body.join("\n").trim().to_string();
    Ok(role)
}

/// Loads every `.md` role in `dir`, sorted by name. Invalid files are reported
/// in the second list and never abort the load.
pub fn load_roles(dir: &Path) -> (Vec<Role>, Vec<RoleError>) {
    let mut roles = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (roles, errors);
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    paths.sort();
    for path in paths {
        let parsed = std::fs::read_to_string(&path)
            .map_err(|err| err.to_string())
            .and_then(|text| parse_role(&text).map_err(|err| err.to_string()));
        match parsed {
            Ok(role) => roles.push(role),
            Err(reason) => errors.push(RoleError::File { path, reason }),
        }
    }
    roles.sort_by(|a, b| a.name.cmp(&b.name));
    (roles, errors)
}

/// Writes the default roles and model map, only when `dir` holds no `.md` file.
pub fn seed_defaults(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let has_roles = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .any(|entry| entry.path().extension().is_some_and(|ext| ext == "md"));
    if has_roles {
        return Ok(());
    }
    for (file, content) in DEFAULT_ROLES {
        std::fs::write(dir.join(file), content)?;
    }
    std::fs::write(dir.join(MODELS_FILE), DEFAULT_MODELS_TOML)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const EXPLORER: &str = "+++\nname = \"explorador\"\ndescription = \"Reads code.\"\nmodel = \"@rapido\"\naccess = \"read-only\"\n+++\n\n  Read the code.\n\n";

    #[test]
    fn parses_role_frontmatter_and_body() {
        let role = parse_role(EXPLORER).unwrap();
        assert_eq!(role.name, "explorador");
        assert_eq!(role.description, "Reads code.");
        assert_eq!(role.model, "@rapido");
        assert_eq!(role.access, RoleAccess::ReadOnly);
        assert_eq!(role.prompt, "Read the code.");
    }

    #[test]
    fn access_defaults_to_write() {
        let role =
            parse_role("+++\nname = \"a\"\ndescription = \"d\"\nmodel = \"@medio\"\n+++\nbody")
                .unwrap();
        assert_eq!(role.access, RoleAccess::Write);
    }

    #[test]
    fn role_without_frontmatter_is_an_error() {
        assert!(parse_role("just a prompt").is_err());
    }

    #[test]
    fn load_roles_skips_invalid_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("explorador.md"), EXPLORER).unwrap();
        std::fs::write(dir.path().join("broken.md"), "no frontmatter").unwrap();
        std::fs::write(dir.path().join("models.toml"), "").unwrap();
        let (roles, errors) = load_roles(dir.path());
        assert_eq!(roles.len(), 1);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn seed_writes_defaults_once() {
        let dir = tempdir().unwrap();
        seed_defaults(dir.path()).unwrap();
        let (roles, errors) = load_roles(dir.path());
        assert_eq!(roles.len(), 4);
        assert!(errors.is_empty());
        assert!(dir.path().join("models.toml").is_file());
        std::fs::write(dir.path().join("executor.md"), "edited").unwrap();
        seed_defaults(dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("executor.md")).unwrap(),
            "edited"
        );
    }
}
