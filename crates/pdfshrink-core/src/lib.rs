//! Core compression logic for PdfShrinker: shared by the CLI, the Tauri app and
//! the Finder service.
//!
//! [`compress_file`] is the single entry point every front end should call;
//! [`compress_bytes`] is the in-memory core of it, and all the WebAssembly
//! build (`default-features = false`) has.

mod cff_subset;
mod cff_tables;
#[cfg(feature = "native")]
mod compress;
#[cfg(feature = "native")]
mod config;
mod deep_dedup;
mod engine;
mod error;
mod font_merge;
mod image_codecs;
mod image_ops;
#[cfg(feature = "native")]
pub mod integration;
mod jpeg;
mod level;
mod par;
mod placement;
mod rust_engine;
mod type1_cff;
mod type1_merge;
mod version;
mod zopfli_pass;

#[cfg(feature = "native")]
pub use compress::{CompressOptions, Outcome, compress_file, compress_file_with};
#[cfg(feature = "native")]
pub use config::Config;
pub use engine::{Engine, ImageFidelity, Report};
pub use error::{PdfShrinkError, Result};
pub use jpeg::JpegEncoder;
pub use level::{Level, Profile};
pub use rust_engine::{RustEngine, compress_bytes};
pub use version::{BuildInfo, build_info};
