//! Core compression logic for PdfShrinker: shared by the CLI, the Tauri app and
//! the Finder Quick Action.
//!
//! [`compress_file`] is the single entry point every front end should call.

mod compress;
mod config;
mod engine;
mod error;
mod ghostscript_engine;
pub mod integration;
mod image_ops;
mod level;
mod placement;
mod rust_engine;

pub use compress::{compress_file, CompressOptions, Outcome};
pub use config::Config;
pub use engine::{Engine, EngineChoice, Report};
pub use error::{PdfShrinkError, Result};
pub use ghostscript_engine::GhostscriptEngine;
pub use level::{Level, Profile};
pub use rust_engine::RustEngine;
