use serde::{Deserialize, Serialize};

/// Compression level requested by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// No image resampling; only structural cleanup (dedup, stream recompression).
    Lossless,
    Low,
    #[default]
    Medium,
    High,
}

impl Level {
    pub const ALL: [Level; 4] = [Level::Lossless, Level::Low, Level::Medium, Level::High];

    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Lossless => "lossless",
            Level::Low => "low",
            Level::Medium => "medium",
            Level::High => "high",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        match s.to_ascii_lowercase().as_str() {
            "lossless" => Some(Level::Lossless),
            "low" => Some(Level::Low),
            "medium" => Some(Level::Medium),
            "high" => Some(Level::High),
            _ => None,
        }
    }

    /// Resolve the tuning profile for this level.
    pub fn profile(&self) -> Profile {
        match self {
            Level::Lossless => Profile {
                level: *self,
                resample_images: false,
                target_dpi: 0.0,
                trigger_ratio: 0.0,
                jpeg_quality: 0,
            },
            Level::Low => Profile {
                level: *self,
                resample_images: true,
                target_dpi: 300.0,
                trigger_ratio: 1.5,
                jpeg_quality: 85,
            },
            Level::Medium => Profile {
                level: *self,
                resample_images: true,
                target_dpi: 150.0,
                trigger_ratio: 1.5,
                jpeg_quality: 75,
            },
            Level::High => Profile {
                level: *self,
                resample_images: true,
                target_dpi: 96.0,
                trigger_ratio: 1.5,
                jpeg_quality: 55,
            },
        }
    }
}

/// Tuning parameters derived from a [`Level`], consumed by the compression engines.
#[derive(Debug, Clone, Copy)]
pub struct Profile {
    pub level: Level,
    /// Whether raster images should be downsampled/recompressed at all.
    pub resample_images: bool,
    /// Target resolution (in effective DPI as placed on the page) images are downsampled to.
    pub target_dpi: f32,
    /// An image is only touched if its effective DPI exceeds `target_dpi * trigger_ratio`.
    pub trigger_ratio: f32,
    pub jpeg_quality: u8,
}
