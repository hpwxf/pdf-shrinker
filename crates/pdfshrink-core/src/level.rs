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
    /// Experimental: lossless structural wins only on top of `High`'s image
    /// settings, with image quality chosen per image against a strict
    /// visual-similarity floor. See [`Experimental`].
    ExtremeSafe,
    /// Experimental: page-relative resolution cap + SSIM-driven JPEG quality.
    Extreme,
    /// Experimental: most aggressive variant; visibly lossy on close zoom.
    ExtremeMax,
}

impl Level {
    /// Levels offered by the GUI. The experimental `Extreme*` levels are
    /// deliberately left out until they've been validated on real documents;
    /// they're reachable from the CLI (`-l extreme`, …) only.
    pub const ALL: [Level; 4] = [Level::Lossless, Level::Low, Level::Medium, Level::High];

    pub const EXPERIMENTAL: [Level; 3] = [Level::ExtremeSafe, Level::Extreme, Level::ExtremeMax];

    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Lossless => "lossless",
            Level::Low => "low",
            Level::Medium => "medium",
            Level::High => "high",
            Level::ExtremeSafe => "extreme-safe",
            Level::Extreme => "extreme",
            Level::ExtremeMax => "extreme-max",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        match s.to_ascii_lowercase().as_str() {
            "lossless" => Some(Level::Lossless),
            "low" => Some(Level::Low),
            "medium" => Some(Level::Medium),
            "high" => Some(Level::High),
            "extreme-safe" => Some(Level::ExtremeSafe),
            "extreme" => Some(Level::Extreme),
            "extreme-max" => Some(Level::ExtremeMax),
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
                max_dimension: 0,
                experimental: Experimental::OFF,
            },
            Level::Low => Profile {
                level: *self,
                resample_images: true,
                target_dpi: 300.0,
                trigger_ratio: 1.5,
                jpeg_quality: 85,
                max_dimension: 4200,
                experimental: Experimental::OFF,
            },
            Level::Medium => Profile {
                level: *self,
                resample_images: true,
                target_dpi: 150.0,
                trigger_ratio: 1.5,
                jpeg_quality: 75,
                max_dimension: 3000,
                experimental: Experimental::OFF,
            },
            Level::High => Profile {
                level: *self,
                resample_images: true,
                target_dpi: 96.0,
                trigger_ratio: 1.5,
                jpeg_quality: 55,
                max_dimension: 2000,
                experimental: Experimental::OFF,
            },
            Level::ExtremeSafe => Profile {
                level: *self,
                resample_images: true,
                target_dpi: 96.0,
                trigger_ratio: 1.5,
                jpeg_quality: 70,
                max_dimension: 2000,
                experimental: Experimental {
                    page_px: 0,
                    ssim_target: 0.985,
                    min_jpeg_quality: 40,
                    // Pixel-exact on the visible area, but poppler smooths an
                    // image drawn through a Form differently when zoomed far
                    // out: keep "safe" free of any geometry change.
                    crop: false,
                    ..Experimental::LOSSLESS_EXTRAS
                },
            },
            Level::Extreme => Profile {
                level: *self,
                resample_images: true,
                target_dpi: 96.0,
                trigger_ratio: 1.2,
                jpeg_quality: 65,
                max_dimension: 1800,
                experimental: Experimental {
                    page_px: 2200,
                    ssim_target: 0.975,
                    min_jpeg_quality: 35,
                    scan_whiten: true,
                    ..Experimental::LOSSLESS_EXTRAS
                },
            },
            Level::ExtremeMax => Profile {
                level: *self,
                resample_images: true,
                target_dpi: 72.0,
                trigger_ratio: 1.1,
                jpeg_quality: 55,
                max_dimension: 1400,
                experimental: Experimental {
                    page_px: 1600,
                    ssim_target: 0.96,
                    min_jpeg_quality: 30,
                    scan_whiten: true,
                    ..Experimental::LOSSLESS_EXTRAS
                },
            },
        }
    }

    pub fn is_experimental(&self) -> bool {
        Level::EXPERIMENTAL.contains(self)
    }
}

/// Tuning parameters derived from a [`Level`], consumed by the compression engine.
#[derive(Debug, Clone, Copy)]
pub struct Profile {
    pub level: Level,
    /// Whether raster images should be downsampled/recompressed at all.
    pub resample_images: bool,
    /// Target resolution (in effective DPI as placed on the page) images are downsampled to.
    pub target_dpi: f32,
    /// An image is only *downsampled* if its effective DPI exceeds
    /// `target_dpi * trigger_ratio`; it is still re-encoded at `jpeg_quality`
    /// either way (see `image_ops.rs`).
    pub trigger_ratio: f32,
    pub jpeg_quality: u8,
    /// Hard cap, in pixels, on an image's longest side, applied regardless of
    /// its effective DPI. Placement-derived DPI can be fooled by an image
    /// drawn oversized and then clipped to the visible page area (a common
    /// "full-bleed background" technique) — this cap is the backstop for that
    /// case. `0` disables it.
    pub max_dimension: u32,
    /// Extra passes used by the experimental `Extreme*` levels; all off for
    /// the regular levels.
    pub experimental: Experimental,
}

impl Profile {
    /// Keys accepted by [`Profile::tune`].
    pub const TUNABLE: &'static str = "dpi, trigger, quality, max_dim, page_px, ssim, min_quality \
        (numbers); dedup, fonts, cff, zopfli, gray, palette, opaque_smask, crop, scan_whiten (0/1)";

    /// Override one parameter from a `key=value` string, for experimenting
    /// with variants from the CLI (`--tune page_px=1800`).
    pub fn tune(&mut self, kv: &str) -> std::result::Result<(), String> {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("'{kv}': expected key=value"))?;
        let bad = || format!("'{kv}': invalid value");
        let f = || v.parse::<f32>().map_err(|_| bad());
        let u = || v.parse::<u32>().map_err(|_| bad());
        let b = || match v {
            "1" | "true" | "on" => Ok(true),
            "0" | "false" | "off" => Ok(false),
            _ => Err(bad()),
        };
        let x = &mut self.experimental;
        match k {
            "dpi" => self.target_dpi = f()?,
            "trigger" => self.trigger_ratio = f()?,
            "quality" => self.jpeg_quality = u()?.clamp(1, 100) as u8,
            "max_dim" => self.max_dimension = u()?,
            "page_px" => x.page_px = u()?,
            "ssim" => x.ssim_target = f()?,
            "min_quality" => x.min_jpeg_quality = u()?.clamp(1, 100) as u8,
            "dedup" => x.deep_dedup = b()?,
            "fonts" => x.merge_fonts = b()?,
            "cff" => x.cff = b()?,
            "zopfli" => x.zopfli = b()?,
            "gray" => x.detect_gray = b()?,
            "palette" => x.palette = b()?,
            "opaque_smask" => x.drop_opaque_smask = b()?,
            "crop" => x.crop = b()?,
            "scan_whiten" => x.scan_whiten = b()?,
            _ => return Err(format!("unknown key '{k}' (expected: {})", Self::TUNABLE)),
        }
        Ok(())
    }
}

/// Knobs for the experimental `Extreme*` levels. Kept separate from the main
/// [`Profile`] fields so the regular levels are guaranteed unaffected
/// ([`Experimental::OFF`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Experimental {
    /// Merge streams whose *decoded* content (and dictionary, minus encoding
    /// keys) is identical, iterated to a fixpoint so e.g. two images become
    /// mergeable once their soft masks have been merged. Lossless.
    pub deep_dedup: bool,
    /// Merge the many per-page subsets of one TrueType CID font (as emitted by
    /// Keynote/PowerPoint/Quartz exports) into a single font program. Lossless.
    pub merge_fonts: bool,
    /// Convert Type 1 font programs to CFF (`/FontFile3`, `Type1C`): same
    /// outlines, 3–5× smaller. Hint replacement is dropped (stems merged into
    /// one non-overlapping set), which only affects hinted rendering at small
    /// sizes.
    pub cff: bool,
    /// Re-deflate every non-image Flate stream with Zopfli. Lossless, slow.
    pub zopfli: bool,
    /// Store RGB images whose pixels are all (near-)gray as DeviceGray.
    pub detect_gray: bool,
    /// Keep images with few distinct colors (screenshots, diagrams, logos)
    /// lossless as an `Indexed` palette + Flate/PNG predictor instead of JPEG,
    /// whenever that is smaller than the JPEG (it usually is, and it avoids
    /// ringing around text).
    pub palette: bool,
    /// Drop soft masks that are fully opaque.
    pub drop_opaque_smask: bool,
    /// Crop images to the part that can ever be visible (the page edge cuts
    /// off an oversized "full-bleed" background, …). Only done when every use
    /// of the image was seen; the cropped image is drawn through a small Form
    /// XObject so no content stream needs rewriting.
    pub crop: bool,
    /// Scanned pages (an image spanning the page width, mostly light paper
    /// that isn't pure white): stretch levels so the paper becomes white.
    /// Lossy by design — it changes the page's look slightly (brighter paper).
    pub scan_whiten: bool,
    /// Page-relative resolution cap: an image is downsampled so that it has at
    /// most this many pixels across the page's (displayed) width at its
    /// placement.
    /// Complements `target_dpi`, which is meaningless for oversized pages such
    /// as slide exports (1920×1080 pt). `0` disables.
    pub page_px: u32,
    /// Per-image adaptive JPEG quality: the lowest quality in
    /// `[min_jpeg_quality, jpeg_quality]` whose SSIM against the
    /// (resized) source stays at or above this value. `0.0` disables (fixed
    /// `jpeg_quality`).
    pub ssim_target: f32,
    pub min_jpeg_quality: u8,
}

impl Experimental {
    pub const OFF: Experimental = Experimental {
        deep_dedup: false,
        merge_fonts: false,
        cff: false,
        zopfli: false,
        detect_gray: false,
        palette: false,
        drop_opaque_smask: false,
        crop: false,
        scan_whiten: false,
        page_px: 0,
        ssim_target: 0.0,
        min_jpeg_quality: 0,
    };

    /// Every lossless extra switched on; lossy knobs left off.
    pub const LOSSLESS_EXTRAS: Experimental = Experimental {
        deep_dedup: true,
        merge_fonts: true,
        cff: true,
        zopfli: true,
        detect_gray: true,
        palette: true,
        drop_opaque_smask: true,
        crop: true,
        ..Experimental::OFF
    };
}
