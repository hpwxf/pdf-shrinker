use std::path::Path;

use crate::Result;
use crate::level::Profile;

/// Outcome of compressing a single file.
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

    fn compress(&self, input: &Path, output: &Path, profile: &Profile) -> Result<Report>;
}
