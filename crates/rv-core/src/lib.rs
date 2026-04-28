//! rv-core: shared library for the rv CLI.

pub mod install;
pub mod lock;
pub mod manifest;
pub mod paths;
pub mod platform;
pub mod project;
pub mod registry;
pub mod resolve;
pub mod rubygems;
pub mod tool;

pub use paths::Paths;
pub use platform::Platform;
