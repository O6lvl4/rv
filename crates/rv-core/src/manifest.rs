//! Read project-level Ruby pin sources: `Gemfile`'s `ruby "..."` directive
//! and `.ruby-version` (rbenv-style).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionHit {
    pub version: String, // e.g. "3.3.5"
    pub source: VersionSource,
    pub origin: PathBuf, // file the value came from
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSource {
    EnvVar,
    Gemfile,
    RubyVersionFile,
    Global,
    LatestInstalled,
}

/// Walk up from `start` looking for the most authoritative Ruby pin.
///
/// Order:
/// 1. `Gemfile` `ruby "..."` directive
/// 2. `.ruby-version`
pub fn find_project_version(start: &Path) -> Result<Option<VersionHit>> {
    let mut dir: Option<&Path> = Some(start);
    while let Some(d) = dir {
        let gemfile = d.join("Gemfile");
        if gemfile.is_file() {
            if let Some(v) = read_gemfile_ruby(&gemfile)? {
                return Ok(Some(VersionHit {
                    version: v,
                    source: VersionSource::Gemfile,
                    origin: gemfile,
                }));
            }
        }
        let ruby_version = d.join(".ruby-version");
        if ruby_version.is_file() {
            let raw = std::fs::read_to_string(&ruby_version)
                .with_context(|| format!("read {}", ruby_version.display()))?;
            let v = clean_version(raw.trim());
            if !v.is_empty() {
                return Ok(Some(VersionHit {
                    version: v,
                    source: VersionSource::RubyVersionFile,
                    origin: ruby_version,
                }));
            }
        }
        dir = d.parent();
    }
    Ok(None)
}

/// Parse a `Gemfile` for the `ruby "X.Y.Z"` directive. Tolerant of single
/// quotes, surrounding whitespace, and hashes used for engine/version blocks
/// (those are skipped — we only want the bare interpreter version).
pub fn read_gemfile_ruby(gemfile: &Path) -> Result<Option<String>> {
    let content =
        std::fs::read_to_string(gemfile).with_context(|| format!("read {}", gemfile.display()))?;
    for raw in content.lines() {
        let line = raw.trim();
        // skip comments
        let line = line.split('#').next().unwrap_or("").trim();
        if !line.starts_with("ruby ") && !line.starts_with("ruby\t") {
            continue;
        }
        // `ruby "3.3.5"` or `ruby '3.3.5'`
        let after = line.trim_start_matches("ruby").trim_start();
        let q = after.chars().next();
        if q != Some('"') && q != Some('\'') {
            continue; // could be `ruby File.read('.ruby-version')` etc — skip
        }
        let quote = q.unwrap();
        let rest = &after[1..];
        if let Some(end) = rest.find(quote) {
            return Ok(Some(clean_version(&rest[..end])));
        }
    }
    Ok(None)
}

/// Strip leading "ruby-" if present (chruby/asdf-style); keep the rest.
fn clean_version(v: &str) -> String {
    let v = v.trim();
    let v = v.strip_prefix("ruby-").unwrap_or(v);
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_double_quoted() {
        let dir = std::env::temp_dir().join("rv-test-gemfile");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Gemfile");
        std::fs::write(
            &p,
            "source 'https://rubygems.org'\nruby \"3.3.5\"\ngem 'rails'\n",
        )
        .unwrap();
        assert_eq!(read_gemfile_ruby(&p).unwrap().as_deref(), Some("3.3.5"));
    }
    #[test]
    fn parse_single_quoted() {
        let dir = std::env::temp_dir().join("rv-test-gemfile2");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Gemfile");
        std::fs::write(&p, "ruby '3.2.4'").unwrap();
        assert_eq!(read_gemfile_ruby(&p).unwrap().as_deref(), Some("3.2.4"));
    }
    #[test]
    fn ignore_dynamic_ruby() {
        let dir = std::env::temp_dir().join("rv-test-gemfile3");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Gemfile");
        std::fs::write(&p, "ruby File.read('.ruby-version').strip").unwrap();
        assert_eq!(read_gemfile_ruby(&p).unwrap(), None);
    }
}
