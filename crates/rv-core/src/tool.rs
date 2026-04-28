//! Install and pin Ruby gems as project tools — replacing ad-hoc
//! `gem install rubocop` with sha-pinned, lockfile-tracked installs.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::lock::LockedTool;
use crate::paths::Paths;
use crate::project::ToolSpec;
use crate::{registry, rubygems};

#[derive(Debug, Clone)]
pub struct ResolvedTool {
    pub name: String,       // user-facing name in rv.toml
    pub gem: String,        // canonical gem name on rubygems.org
    pub version: String,    // concrete (never "latest")
    pub bin: String,        // binary stub produced by `gem install`
    pub gem_sha256: String, // gem-file sha256 from rubygems.org
}

pub async fn resolve(
    client: &reqwest::Client,
    name: &str,
    spec: &ToolSpec,
) -> Result<ResolvedTool> {
    let gem = spec
        .gem_override()
        .map(|s| s.to_string())
        .or_else(|| registry::lookup(name).map(|e| e.gem.to_string()))
        .ok_or_else(|| {
            anyhow!(
                "unknown tool '{name}' — pick from the registry or set `gem = \"...\"` in rv.toml"
            )
        })?;

    let raw = spec.version().trim();
    let info = match raw {
        "latest" | "*" => rubygems::latest(client, &gem).await?,
        v => rubygems::version_info(client, &gem, v).await?,
    };

    let bin = spec
        .bin_override()
        .map(|s| s.to_string())
        .or_else(|| registry::lookup(name).map(|e| e.bin.to_string()))
        .unwrap_or_else(|| name.to_string());

    Ok(ResolvedTool {
        name: name.to_string(),
        gem,
        version: info.number,
        bin,
        gem_sha256: info.sha,
    })
}

/// Install a gem into a per-Ruby + per-tool isolated GEM_HOME so different
/// projects' rubocop versions don't fight.
///
/// Layout: `<data>/tools/<ruby-version>/<gem>/<gem-version>/` containing the
/// usual `bin/`, `gems/`, `specifications/` produced by `gem install -i`.
pub fn install(paths: &Paths, ruby_version: &str, resolved: &ResolvedTool) -> Result<LockedTool> {
    let ruby_bin = paths.version_dir(ruby_version).join("bin").join("ruby");
    let gem_bin = paths.version_dir(ruby_version).join("bin").join("gem");
    if !ruby_bin.exists() || !gem_bin.exists() {
        bail!(
            "ruby {ruby_version} not installed (looked at {})",
            ruby_bin.display()
        );
    }
    let dest_gem_home = tool_gem_home(paths, ruby_version, &resolved.gem, &resolved.version);
    crate::paths::ensure_dir(&dest_gem_home)?;

    let bin = dest_gem_home.join("bin").join(&resolved.bin);
    if bin.exists() {
        return Ok(make_locked(resolved, ruby_version));
    }

    // gem install <gem> -v <version> -i <gem-home> --no-document --no-update-sources
    // Add the per-Ruby bin dir to PATH so `gem` finds its companions.
    let bin_dir = paths.version_dir(ruby_version).join("bin");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut new_path = std::ffi::OsString::from(bin_dir.as_os_str());
    new_path.push(":");
    new_path.push(&path);

    let status = Command::new(&gem_bin)
        .args([
            "install",
            &resolved.gem,
            "-v",
            &resolved.version,
            "-i",
            &dest_gem_home.to_string_lossy(),
            "--no-document",
            "--no-update-sources",
        ])
        .env("PATH", new_path)
        .env("GEM_HOME", &dest_gem_home)
        .env("GEM_PATH", &dest_gem_home)
        .status()
        .with_context(|| format!("spawn gem install {}@{}", resolved.gem, resolved.version))?;
    if !status.success() {
        bail!(
            "gem install {}@{} failed (exit {:?})",
            resolved.gem,
            resolved.version,
            status.code()
        );
    }
    if !bin.exists() {
        bail!(
            "gem install produced no binary {} in {}",
            resolved.bin,
            dest_gem_home.join("bin").display()
        );
    }
    Ok(make_locked(resolved, ruby_version))
}

fn make_locked(r: &ResolvedTool, ruby_version: &str) -> LockedTool {
    LockedTool {
        name: r.name.clone(),
        gem: r.gem.clone(),
        version: r.version.clone(),
        bin: r.bin.clone(),
        gem_sha256: r.gem_sha256.clone(),
        built_with: ruby_version.to_string(),
    }
}

pub fn tool_gem_home(paths: &Paths, ruby_version: &str, gem: &str, gem_version: &str) -> PathBuf {
    paths
        .data
        .join("tools")
        .join(ruby_version)
        .join(gem)
        .join(gem_version)
}

pub fn tool_bin_path(paths: &Paths, locked: &LockedTool) -> PathBuf {
    tool_gem_home(paths, &locked.built_with, &locked.gem, &locked.version)
        .join("bin")
        .join(&locked.bin)
}
