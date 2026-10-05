use serde::{Deserialize, Serialize};

use crate::jpeg::JpegEncoder;

/// Compression level requested by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    /// No image re-encoding; only structural cleanup (dedup, font merging,
    /// stream recompression).
    Lossless,
    Low,
    #[default]
    Medium,
    High,
    /// Page-relative resolution cap, cropping to the visible area, scanned
    /// paper whitened, Zopfli.
    Extreme,
    /// Most aggressive variant; visibly lossy on close zoom.
    ExtremeMax,
}

impl Level {
    pub const ALL: [Level; 6] = [
        Level::Lossless,
        Level::Low,
        Level::Medium,
        Level::High,
        Level::Extreme,
        Level::ExtremeMax,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Lossless => "lossless",
            Level::Low => "low",
            Level::Medium => "medium",
            Level::High => "high",
            Level::Extreme => "extreme",
            Level::ExtremeMax => "extreme-max",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        let s = s.to_ascii_lowercase();
        Level::ALL.into_iter().find(|l| l.as_str() == s)
    }

    /// One-line summary of what the level does, for `--help` (the app has
    /// its own translated copy in `app/ui/i18n.js`).
    pub fn summary(&self) -> &'static str {
        match self {
            Level::Lossless => {
                "images untouched; duplicate objects and font subsets merged, Type 1 fonts → CFF"
            }
            Level::Low => {
                "+ images re-encoded (JPEG ≤ q85 or lossless palette), 200 dpi when above 250 dpi"
            }
            Level::Medium => {
                "+ JPEG quality per image (q50–75, SSIM ≥ 0.99), 150 dpi when above 180 dpi"
            }
            Level::High => "+ 96 dpi when above 144 dpi, JPEG quality per image (SSIM ≥ 0.985)",
            Level::Extreme => {
                "+ slides capped at 2200 px across, images cropped to the page, scanned paper \
                 whitened, Zopfli (slower)"
            }
            Level::ExtremeMax => {
                "+ 72 dpi, 1600 px across, SSIM ≥ 0.96: visibly lossy when zoomed in"
            }
        }
    }

    /// Whether this build may produce a different file than the desktop
    /// build at this level: true when the level re-encodes images and the
    /// JPEG encoder isn't mozjpeg (the WebAssembly build).
    pub fn differs_from_desktop(&self) -> bool {
        let p = self.profile();
        p.resample_images && p.jpeg_encoder != JpegEncoder::Mozjpeg
    }

    /// Resolve the tuning profile for this level.
    pub fn profile(&self) -> Profile {
        match self {
            Level::Lossless => Profile {
                level: *self,
                resample_images: false,
                ..Profile::BASE
            },
            Level::Low => Profile {
                level: *self,
                target_dpi: 200.0,
                trigger_ratio: 1.25,
                jpeg_quality: 85,
                ssim_target: 0.995,
                min_jpeg_quality: 65,
                max_dimension: 4200,
                ..Profile::BASE
            },
            Level::Medium => Profile {
                level: *self,
                target_dpi: 150.0,
                trigger_ratio: 1.2,
                jpeg_quality: 75,
                ssim_target: 0.99,
                min_jpeg_quality: 50,
                max_dimension: 3000,
                ..Profile::BASE
            },
            Level::High => Profile {
                level: *self,
                target_dpi: 96.0,
                jpeg_quality: 70,
                max_dimension: 2000,
                ssim_target: 0.985,
                min_jpeg_quality: 40,
                ..Profile::BASE
            },
            Level::Extreme => Profile {
                level: *self,
                target_dpi: 96.0,
                trigger_ratio: 1.2,
                jpeg_quality: 65,
                max_dimension: 1800,
                page_px: 2200,
                ssim_target: 0.975,
                min_jpeg_quality: 35,
                crop: true,
                scan_whiten: true,
                zopfli: true,
                ..Profile::BASE
            },
            Level::ExtremeMax => Profile {
                level: *self,
                target_dpi: 72.0,
                trigger_ratio: 1.1,
                jpeg_quality: 55,
                max_dimension: 1400,
                page_px: 1600,
                ssim_target: 0.96,
                min_jpeg_quality: 30,
                crop: true,
                scan_whiten: true,
                zopfli: true,
                ..Profile::BASE
            },
        }
    }
}

/// Tuning parameters derived from a [`Level`], consumed by the compression engine.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// Merge streams whose *decoded* content (and dictionary, minus encoding
    /// keys) is identical, iterated to a fixpoint so e.g. two images become
    /// mergeable once their soft masks have been merged. Lossless.
    pub deep_dedup: bool,
    /// Merge the many per-page subsets of one TrueType CID font (as emitted by
    /// Keynote/PowerPoint/Quartz exports) or Type 1 font (LaTeX figures) into
    /// a single font program. Lossless.
    pub merge_fonts: bool,
    /// Convert Type 1 font programs to CFF (`/FontFile3`, `Type1C`): same
    /// outlines, 3–5× smaller. Hint replacement is dropped (stems merged into
    /// one non-overlapping set), which only affects hinted rendering at small
    /// sizes.
    pub cff: bool,
    /// Finish incomplete CFF subsets (`/FontFile3`): drop unreachable
    /// subroutines, unused strings and, in CID-keyed fonts, empty glyphs.
    /// Lossless — same outlines.
    pub cff_subset: bool,
    /// Re-deflate every non-image Flate stream with Zopfli. Lossless, slow
    /// (~3× the run time for a few % smaller).
    pub zopfli: bool,
    /// Store RGB images whose pixels are all (near-)gray as DeviceGray.
    pub detect_gray: bool,
    /// Keep images with few distinct colors (screenshots, diagrams, logos)
    /// lossless as an `Indexed` palette + Flate/PNG predictor instead of JPEG,
    /// whenever that is smaller than the JPEG (it usually is, and it avoids
    /// ringing around text).
    pub palette: bool,
    /// Drop soft masks that are fully opaque; store the others with a PNG
    /// predictor when that is smaller.
    pub drop_opaque_smask: bool,
    /// Crop images to the part that can ever be visible (the page edge cuts
    /// off an oversized "full-bleed" background, …). Only done when every use
    /// of the image was seen; the cropped image is drawn through a small Form
    /// XObject so no content stream needs rewriting. Off below `Extreme`:
    /// poppler smooths an image drawn through a Form differently when zoomed
    /// far out.
    pub crop: bool,
    /// Scanned pages (an image spanning the page width, mostly light paper
    /// that isn't pure white): stretch levels so the paper becomes white.
    /// Lossy by design — it changes the page's look slightly (brighter paper).
    pub scan_whiten: bool,
    /// Measure how faithful re-encoded images are (`Report::image_fidelity`).
    /// Doesn't change the output; costs a decode and an SSIM per image.
    pub measure_fidelity: bool,
    /// Which encoder writes JPEGs (see `jpeg.rs`): mozjpeg when the build
    /// has it, else the pure-Rust one — the WebAssembly build's output can
    /// differ from the desktop's at every level that re-encodes images.
    pub jpeg_encoder: JpegEncoder,
}

impl Profile {
    /// What every level starts from: all lossless passes on, image settings
    /// neutral, nothing that changes a page's geometry or look beyond
    /// compression artifacts.
    const BASE: Profile = Profile {
        level: Level::Medium,
        resample_images: true,
        target_dpi: 0.0,
        trigger_ratio: 1.5,
        jpeg_quality: 0,
        max_dimension: 0,
        page_px: 0,
        ssim_target: 0.0,
        min_jpeg_quality: 0,
        deep_dedup: true,
        merge_fonts: true,
        cff: true,
        cff_subset: true,
        zopfli: false,
        detect_gray: true,
        palette: true,
        drop_opaque_smask: true,
        crop: false,
        scan_whiten: false,
        measure_fidelity: false,
        jpeg_encoder: JpegEncoder::DEFAULT,
    };

    /// Keys accepted by [`Profile::tune`].
    pub const TUNABLE: &'static str = "dpi, trigger, quality, max_dim, page_px, ssim, min_quality \
        (numbers); dedup, fonts, cff, cff_subset, zopfli, gray, palette, opaque_smask, crop, scan_whiten, fidelity (0/1); \
        jpeg (mozjpeg or rust)";

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
        match k {
            "dpi" => self.target_dpi = f()?,
            "trigger" => self.trigger_ratio = f()?,
            "quality" => self.jpeg_quality = u()?.clamp(1, 100) as u8,
            "max_dim" => self.max_dimension = u()?,
            "page_px" => self.page_px = u()?,
            "ssim" => self.ssim_target = f()?,
            "min_quality" => self.min_jpeg_quality = u()?.clamp(1, 100) as u8,
            "dedup" => self.deep_dedup = b()?,
            "fonts" => self.merge_fonts = b()?,
            "cff" => self.cff = b()?,
            "cff_subset" => self.cff_subset = b()?,
            "zopfli" => self.zopfli = b()?,
            "gray" => self.detect_gray = b()?,
            "palette" => self.palette = b()?,
            "opaque_smask" => self.drop_opaque_smask = b()?,
            "crop" => self.crop = b()?,
            "scan_whiten" => self.scan_whiten = b()?,
            "fidelity" => self.measure_fidelity = b()?,
            "jpeg" => self.jpeg_encoder = JpegEncoder::parse(v).ok_or_else(bad)?,
            _ => return Err(format!("unknown key '{k}' (expected: {})", Self::TUNABLE)),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_names_round_trip_through_parse_and_serde() {
        for level in Level::ALL {
            assert_eq!(Level::parse(level.as_str()), Some(level));
            #[cfg(feature = "native")]
            let toml = toml::to_string(&crate::Config {
                default_level: level,
            })
            .unwrap();
            #[cfg(feature = "native")]
            assert!(toml.contains(&format!("\"{}\"", level.as_str())), "{toml}");
        }
        assert_eq!(Level::parse("extreme-safe"), None);
    }
}
