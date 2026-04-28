use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Darwin,
    Linux,
}

#[derive(Debug, Clone, Copy)]
pub struct Platform {
    pub os: Os,
}

impl Platform {
    pub fn detect() -> Result<Self> {
        let os = match std::env::consts::OS {
            "macos" => Os::Darwin,
            "linux" => Os::Linux,
            other => bail!(
                "rv does not support {other} (Ruby on Windows is its own build saga; \
                 rv targets unix for now)"
            ),
        };
        Ok(Self { os })
    }
}
