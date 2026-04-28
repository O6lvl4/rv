//! rv's filesystem paths come from anyv-core, parameterized with the app
//! name `"rv"`. This is a thin re-export so the rest of rv-core uses
//! `crate::paths::Paths` without caring where the implementation lives.

pub use anyv_core::paths::{ensure_dir, Paths as AnyvPaths};

use anyhow::Result;

/// rv's flavor of `Paths`. We keep the same public name so call sites
/// (`paths.versions()`, `paths.global_version_file()`, …) don't need to
/// change.
pub type Paths = AnyvPaths;

/// Discover paths for the rv app. Honors `RV_HOME`, then XDG.
pub fn discover() -> Result<Paths> {
    AnyvPaths::discover("rv")
}
