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
    /// How faithful the re-encoded images are, when the profile asked
    /// (`Profile::measure_fidelity`) and at least one image was re-encoded.
    pub image_fidelity: Option<ImageFidelity>,
}

/// Luma SSIM of each re-encoded image against its source, at the source's
/// size (so downsampling counts as well as JPEG loss): 1.0 is identical.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageFidelity {
    /// Mean weighted by each image's area on the page.
    pub mean: f32,
    /// The least faithful image.
    pub min: f32,
    /// Number of images measured.
    pub images: usize,
}

impl ImageFidelity {
    /// Aggregates `(ssim, area)` pairs; `None` when there are none.
    pub(crate) fn from_scores(scores: impl Iterator<Item = (f32, f64)>) -> Option<Self> {
        let (mut sum, mut weight, mut min, mut images) = (0.0f64, 0.0f64, f32::MAX, 0);
        for (s, w) in scores {
            let w = w.max(1e-6);
            sum += s as f64 * w;
            weight += w;
            min = min.min(s);
            images += 1;
        }
        (images > 0).then(|| ImageFidelity {
            mean: (sum / weight) as f32,
            min,
            images,
        })
    }
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
