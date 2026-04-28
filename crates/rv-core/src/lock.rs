use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::project::LOCK_FILE;

pub const LOCK_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Lock {
    pub version: u32,
    #[serde(default)]
    pub ruby: Option<LockedRuby>,
    #[serde(default, rename = "tool")]
    pub tools: Vec<LockedTool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedRuby {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedTool {
    pub name: String,
    pub gem: String,
    pub version: String,
    pub bin: String,
    pub gem_sha256: String, // checksum from rubygems.org
    pub built_with: String,
}

impl Lock {
    pub fn empty() -> Self {
        Self {
            version: LOCK_VERSION,
            ..Default::default()
        }
    }

    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(LOCK_FILE);
        if !path.is_file() {
            return Ok(Self::empty());
        }
        let raw =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let path = root.join(LOCK_FILE);
        let text = toml::to_string_pretty(self)
            .with_context(|| format!("serialize {}", path.display()))?;
        std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    pub fn upsert_tool(&mut self, t: LockedTool) {
        if let Some(slot) = self.tools.iter_mut().find(|x| x.name == t.name) {
            *slot = t;
        } else {
            self.tools.push(t);
        }
        self.tools.sort_by(|a, b| a.name.cmp(&b.name));
    }

    pub fn find_tool(&self, name: &str) -> Option<&LockedTool> {
        self.tools.iter().find(|t| t.name == name)
    }
}
