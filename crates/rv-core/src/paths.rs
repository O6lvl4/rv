//! Standard filesystem layout for rv. Honors XDG; overridable via `RV_HOME`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;

#[derive(Debug, Clone)]
pub struct Paths {
    pub data: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        if let Ok(home) = std::env::var("RV_HOME") {
            let root = PathBuf::from(home);
            return Ok(Self {
                data: root.join("data"),
                config: root.join("config"),
                cache: root.join("cache"),
            });
        }
        let pd = ProjectDirs::from("dev", "O6lvl4", "rv")
            .context("could not resolve XDG directories for rv")?;
        Ok(Self {
            data: pd.data_dir().to_path_buf(),
            config: pd.config_dir().to_path_buf(),
            cache: pd.cache_dir().to_path_buf(),
        })
    }

    pub fn store(&self) -> PathBuf {
        self.data.join("store")
    }
    pub fn versions(&self) -> PathBuf {
        self.data.join("versions")
    }
    pub fn version_dir(&self, v: &str) -> PathBuf {
        self.versions().join(v)
    }
    pub fn global_version_file(&self) -> PathBuf {
        self.config.join("global")
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        for d in [
            &self.data,
            &self.config,
            &self.cache,
            &self.store(),
            &self.versions(),
        ] {
            ensure_dir(d)?;
        }
        Ok(())
    }
}

pub fn ensure_dir(p: &Path) -> Result<()> {
    if !p.exists() {
        std::fs::create_dir_all(p).with_context(|| format!("create dir: {}", p.display()))?;
    }
    Ok(())
}
