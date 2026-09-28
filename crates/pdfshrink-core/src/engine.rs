use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::level::Profile;
use crate::Result;

/// Which compression backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EngineChoice {
    /// Pure-Rust engine (lopdf + image re-encoding). Always available.
    #[default]
    Rust,
    /// Shell out to a locally installed Ghostscript (`gs`).
    Ghostscript,
    /// Run both engines and keep whichever output is smaller.
    Best,
}

impl EngineChoice {
    pub fn as_str(&self) -> &'static str {
        match self {
            EngineChoice::Rust => "rust",
            EngineChoice::Ghostscript => "gs",
            EngineChoice::Best => "best",
        }
    }

    pub fn parse(s: &str) -> Option<EngineChoice> {
        match s.to_ascii_lowercase().as_str() {
            "rust" => Some(EngineChoice::Rust),
            "gs" | "ghostscript" => Some(EngineChoice::Ghostscript),
            "best" => Some(EngineChoice::Best),
            _ => None,
        }
    }
}

/// Outcome of running a single engine over a single file.
#[derive(Debug, Clone)]
pub struct Report {
    pub input_size: u64,
    pub output_size: u64,
    pub images_resampled: usize,
    pub images_skipped: usize,
}

impl Report {
    pub fn ratio(&self) -> f32 {
        if self.input_size == 0 {
            return 0.0;
        }
        1.0 - (self.output_size as f32 / self.input_size as f32)
    }
}

/// A backend able to turn one PDF into a smaller one at a given [`Profile`].
pub trait Engine {
    fn name(&self) -> &'static str;

    /// Whether this engine is usable on this machine right now.
    fn is_available(&self) -> bool {
        true
    }

    fn compress(&self, input: &Path, output: &Path, profile: &Profile) -> Result<Report>;
}
