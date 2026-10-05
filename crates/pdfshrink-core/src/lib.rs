//! Core compression logic for PdfShrinker: shared by the CLI, the Tauri app and
//! the Finder service.
//!
//! [`compress_file`] is the single entry point every front end should call.

mod cff_subset;
mod cff_tables;
mod compress;
mod config;
mod deep_dedup;
mod engine;
mod error;
mod font_merge;
mod image_codecs;
mod image_ops;
pub mod integration;
mod level;
mod placement;
mod rust_engine;
mod type1_cff;
mod type1_merge;
mod version;
mod zopfli_pass;

pub use compress::{CompressOptions, Outcome, compress_file, compress_file_with};
pub use config::Config;
pub use engine::{Engine, ImageFidelity, Report};
pub use error::{PdfShrinkError, Result};
pub use level::{Level, Profile};
pub use rust_engine::RustEngine;
pub use version::{BuildInfo, build_info};
