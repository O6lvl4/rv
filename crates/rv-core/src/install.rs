//! Install a Ruby version. We shell out to ruby-build (the de-facto Ruby
//! source-compile tool used by rbenv) and place the result in our own
//! content-addressed-ish layout.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::paths::Paths;

const RUBY_BUILD: &str = "ruby-build";

pub struct InstallReport {
    pub version: String,
    pub install_dir: PathBuf,
    pub already_present: bool,
}

pub fn list_remote() -> Result<Vec<String>> {
    let output = Command::new(RUBY_BUILD)
        .arg("--definitions")
        .output()
        .with_context(|| {
            "ruby-build not on $PATH — install with `brew install ruby-build` (or your distro's package manager)"
        })?;
    if !output.status.success() {
        bail!(
            "ruby-build --definitions failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut versions: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    versions.sort();
    Ok(versions)
}

/// Install the given Ruby version. Returns the install dir.
///
/// Layout: `<paths.versions>/<version>/` — for now we don't bother with a
/// content-addressed prefix because Ruby builds are already long-lived and
/// per-version. We can revisit if same-version-different-bytes ever matters.
pub fn install(paths: &Paths, version: &str) -> Result<InstallReport> {
    paths.ensure_dirs()?;
    let dest = paths.version_dir(version);
    let bin = dest.join("bin").join("ruby");
    if bin.exists() {
        return Ok(InstallReport {
            version: version.to_string(),
            install_dir: dest,
            already_present: true,
        });
    }
    if dest.exists() {
        // Half-baked previous install — wipe and start over.
        std::fs::remove_dir_all(&dest).ok();
    }
    let status = Command::new(RUBY_BUILD)
        .arg(version)
        .arg(&dest)
        .env("RUBY_BUILD_BUILD_PATH", paths.cache.join("ruby-build-tmp"))
        .env(
            "RUBY_BUILD_CACHE_PATH",
            paths.cache.join("ruby-build-cache"),
        )
        .status()
        .with_context(|| format!("spawn {RUBY_BUILD} {version}"))?;
    if !status.success() {
        bail!("ruby-build {} failed (exit {:?})", version, status.code());
    }
    if !bin.exists() {
        return Err(anyhow!(
            "ruby-build claimed success but {} was not produced",
            bin.display()
        ));
    }
    Ok(InstallReport {
        version: version.to_string(),
        install_dir: dest,
        already_present: false,
    })
}

pub fn uninstall(paths: &Paths, version: &str) -> Result<()> {
    let dir = paths.version_dir(version);
    if !dir.exists() {
        bail!("ruby {version} is not installed");
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
    Ok(())
}

#[allow(dead_code)]
fn _path_anchor() -> &'static Path {
    Path::new("/")
}
