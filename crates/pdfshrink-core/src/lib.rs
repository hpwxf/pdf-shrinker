//! Core compression logic for PdfShrinker: shared by the CLI, the Tauri app and
//! the Finder Quick Action.
//!
//! [`compress_file`] is the single entry point every front end should call.

mod compress;
mod config;
mod engine;
mod error;
mod ghostscript_engine;
mod image_ops;
pub mod integration;
mod level;
mod placement;
mod rust_engine;
mod version;

pub use compress::{CompressOptions, Outcome, compress_file};
pub use config::Config;
pub use engine::{Engine, EngineChoice, Report};
pub use error::{PdfShrinkError, Result};
pub use ghostscript_engine::GhostscriptEngine;
pub use level::{Level, Profile};
pub use rust_engine::RustEngine;
pub use version::{BuildInfo, build_info};
