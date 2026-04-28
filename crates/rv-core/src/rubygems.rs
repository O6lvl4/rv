//! Talk to rubygems.org's API for gem metadata + checksums.

use anyhow::{Context, Result};
use serde::Deserialize;

const RUBYGEMS_BASE: &str = "https://rubygems.org/api/v1";

#[derive(Debug, Deserialize)]
pub struct GemVersion {
    pub number: String,
    #[serde(default)]
    pub sha: String, // gem file sha256
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default, rename = "ruby_version")]
    pub ruby_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GemInfo {
    pub version: String,
    #[serde(default)]
    pub sha: String,
}

/// Fetch the latest stable version of a gem, with sha256.
pub async fn latest(client: &reqwest::Client, gem: &str) -> Result<GemVersion> {
    // /gems/<name>.json gives the latest stable.
    let url = format!("{RUBYGEMS_BASE}/gems/{gem}.json");
    let info: GemInfo = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()?
        .json()
        .await?;
    Ok(GemVersion {
        number: info.version,
        sha: info.sha,
        prerelease: false,
        ruby_version: None,
    })
}

/// Fetch every published version of a gem (stable + prereleases).
pub async fn list_versions(client: &reqwest::Client, gem: &str) -> Result<Vec<GemVersion>> {
    let url = format!("{RUBYGEMS_BASE}/versions/{gem}.json");
    let versions: Vec<GemVersion> = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()?
        .json()
        .await?;
    Ok(versions)
}

/// Fetch a specific version's metadata (mostly for the `sha` field).
pub async fn version_info(
    client: &reqwest::Client,
    gem: &str,
    version: &str,
) -> Result<GemVersion> {
    let all = list_versions(client, gem).await?;
    all.into_iter()
        .find(|v| v.number == version)
        .ok_or_else(|| anyhow::anyhow!("version {version} of {gem} not found on rubygems.org"))
}
