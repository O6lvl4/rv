//! Project-level configuration: `rv.toml` parsing and project-root discovery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const PROJECT_FILE: &str = "rv.toml";
pub const LOCK_FILE: &str = "rv.lock";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Project {
    #[serde(default)]
    pub ruby: Option<RubySection>,
    /// gem-name → spec. Short form: `rubocop = "latest"`. Long form supports
    /// an explicit `gem` (when the user-facing name differs from the gem
    /// name) and a `bin` override.
    #[serde(default)]
    pub tools: BTreeMap<String, ToolSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RubySection {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolSpec {
    Short(String),
    Long {
        #[serde(default)]
        gem: Option<String>,
        version: String,
        #[serde(default)]
        bin: Option<String>,
    },
}

impl ToolSpec {
    pub fn version(&self) -> &str {
        match self {
            ToolSpec::Short(v) => v,
            ToolSpec::Long { version, .. } => version,
        }
    }
    pub fn gem_override(&self) -> Option<&str> {
        match self {
            ToolSpec::Short(_) => None,
            ToolSpec::Long { gem, .. } => gem.as_deref(),
        }
    }
    pub fn bin_override(&self) -> Option<&str> {
        match self {
            ToolSpec::Short(_) => None,
            ToolSpec::Long { bin, .. } => bin.as_deref(),
        }
    }
}

/// Walk up looking for the project root. `rv.toml`, `Gemfile`, or
/// `.ruby-version` (whichever appears first) marks it.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let mut dir: Option<&Path> = Some(start);
    while let Some(d) = dir {
        if d.join(PROJECT_FILE).is_file()
            || d.join("Gemfile").is_file()
            || d.join(".ruby-version").is_file()
        {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

pub fn load(root: &Path) -> Result<Project> {
    let path = root.join(PROJECT_FILE);
    if !path.is_file() {
        return Ok(Project::default());
    }
    let raw = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

pub fn save(root: &Path, project: &Project) -> Result<()> {
    let path = root.join(PROJECT_FILE);
    let text =
        toml::to_string_pretty(project).with_context(|| format!("serialize {}", path.display()))?;
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
