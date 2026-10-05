//! JPEG encoding, and decoding of the JPEGs we write, behind one interface,
//! with two encoders:
//!
//! - **mozjpeg** (feature `mozjpeg`, part of `native`): trellis quantization,
//!   progressive scans tuned per image. The desktop front ends use it.
//! - **jpeg-encoder** (pure Rust, always compiled): baseline, optimized
//!   Huffman tables, same quantization table and chroma subsampling as
//!   mozjpeg, but no trellis quantization — bigger at equal quality. The
//!   WebAssembly build uses it (mozjpeg is a C library), and
//!   `--tune jpeg=rust` lets the CLI measure the difference.
//!
//! Re-encoded JPEGs are read back (SSIM search, fidelity) with the pure-Rust
//! `jpeg-decoder`, not `image`'s zune-jpeg: zune-jpeg 0.5 decodes
//! jpeg-encoder's baseline + optimized-Huffman output as garbage. CMYK JPEGs
//! decode to their *stored* samples (no Adobe inversion), through mozjpeg
//! when available.

use std::panic::{self, AssertUnwindSafe};

use serde::{Deserialize, Serialize};

/// Which JPEG encoder re-encodes images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JpegEncoder {
    Mozjpeg,
    /// The pure-Rust `jpeg-encoder` crate.
    Rust,
}

impl JpegEncoder {
    /// mozjpeg when this build has it, else the pure-Rust encoder.
    pub const DEFAULT: JpegEncoder = if cfg!(feature = "mozjpeg") {
        JpegEncoder::Mozjpeg
    } else {
        JpegEncoder::Rust
    };

    /// Encoders compiled into this build.
    pub fn available() -> &'static [JpegEncoder] {
        if cfg!(feature = "mozjpeg") {
            &[JpegEncoder::Mozjpeg, JpegEncoder::Rust]
        } else {
            &[JpegEncoder::Rust]
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            JpegEncoder::Mozjpeg => "mozjpeg",
            JpegEncoder::Rust => "rust",
        }
    }

    pub fn parse(s: &str) -> Option<JpegEncoder> {
        Self::available().iter().copied().find(|e| e.as_str() == s)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum JpegColor {
    Gray,
    Rgb,
    Cmyk,
}

/// Encodes interleaved 8-bit samples (1, 3 or 4 per pixel per `color`).
/// CMYK samples are stored as given, with an Adobe APP14 marker.
pub(crate) fn encode(
    encoder: JpegEncoder,
    data: &[u8],
    w: u32,
    h: u32,
    color: JpegColor,
    quality: u8,
) -> Option<Vec<u8>> {
    match encoder {
        #[cfg(feature = "mozjpeg")]
        JpegEncoder::Mozjpeg => encode_mozjpeg(data, w, h, color, quality),
        #[cfg(not(feature = "mozjpeg"))]
        JpegEncoder::Mozjpeg => encode_rust(data, w, h, color, quality),
        JpegEncoder::Rust => encode_rust(data, w, h, color, quality),
    }
}

#[cfg(feature = "mozjpeg")]
fn encode_mozjpeg(data: &[u8], w: u32, h: u32, color: JpegColor, quality: u8) -> Option<Vec<u8>> {
    // mozjpeg reports libjpeg errors by unwinding: catch them here.
    panic::catch_unwind(AssertUnwindSafe(|| -> std::io::Result<Vec<u8>> {
        let color_space = match color {
            JpegColor::Gray => mozjpeg::ColorSpace::JCS_GRAYSCALE,
            JpegColor::Rgb => mozjpeg::ColorSpace::JCS_RGB,
            JpegColor::Cmyk => mozjpeg::ColorSpace::JCS_CMYK,
        };
        let mut comp = mozjpeg::Compress::new(color_space);
        comp.set_size(w as usize, h as usize);
        comp.set_quality(quality as f32);
        let mut started = comp.start_compress(Vec::new())?;
        started.write_scanlines(data)?;
        started.finish()
    }))
    .ok()?
    .ok()
}

fn encode_rust(data: &[u8], w: u32, h: u32, color: JpegColor, quality: u8) -> Option<Vec<u8>> {
    use jpeg_encoder::{
        ChromaSubsamplingMethod, ColorType, Encoder, QuantizationTableType, SamplingFactor,
    };
    let (w, h) = (u16::try_from(w).ok()?, u16::try_from(h).ok()?);
    // jpeg-encoder stores CMYK inverted (the Adobe convention): invert first
    // so the stored samples are the ones given, as with mozjpeg.
    let inverted;
    let (data, color_type) = match color {
        JpegColor::Gray => (data, ColorType::Luma),
        JpegColor::Rgb => (data, ColorType::Rgb),
        JpegColor::Cmyk => {
            inverted = data.iter().map(|v| 255 - v).collect::<Vec<u8>>();
            (inverted.as_slice(), ColorType::Cmyk)
        }
    };
    let mut out = Vec::new();
    let mut enc = Encoder::new(&mut out, quality.clamp(1, 100));
    // mozjpeg's defaults: ImageMagick (Robidoux) tables; 4:2:0 for RGB at
    // any quality, none for CMYK (libjpeg's default).
    enc.set_quantization_tables(
        QuantizationTableType::ImageMagick,
        QuantizationTableType::ImageMagick,
    );
    enc.set_sampling_factor(if color == JpegColor::Cmyk {
        SamplingFactor::F_1_1
    } else {
        SamplingFactor::F_2_2
    });
    enc.set_chroma_subsampling_method(ChromaSubsamplingMethod::Average);
    // Baseline: jpeg-encoder's progressive scans come out bigger, not smaller.
    enc.set_progressive(false);
    enc.set_optimized_huffman_tables(true);
    enc.encode(data, w, h, color_type).ok()?;
    Some(out)
}

/// Decodes a gray, RGB or CMYK JPEG to interleaved 8-bit samples; CMYK as
/// *stored* (no Adobe inversion), YCCK converted to CMYK like libjpeg does.
pub(crate) fn decode(jpeg: &[u8]) -> Option<(u32, u32, JpegColor, Vec<u8>)> {
    use jpeg_decoder::{Decoder, PixelFormat};
    panic::catch_unwind(AssertUnwindSafe(|| {
        let mut d = Decoder::new(jpeg);
        let mut data = d.decode().ok()?;
        let info = d.info()?;
        let (w, h) = (info.width as u32, info.height as u32);
        let color = match info.pixel_format {
            PixelFormat::L8 => JpegColor::Gray,
            PixelFormat::RGB24 => JpegColor::Rgb,
            PixelFormat::CMYK32 => {
                // jpeg-decoder hands back 255 − stored (and, from YCCK,
                // 255 − libjpeg's CMYK): undo it.
                data.iter_mut().for_each(|v| *v = 255 - *v);
                JpegColor::Cmyk
            }
            PixelFormat::L16 => return None,
        };
        let channels = match color {
            JpegColor::Gray => 1,
            JpegColor::Rgb => 3,
            JpegColor::Cmyk => 4,
        };
        (data.len() == w as usize * h as usize * channels).then_some((w, h, color, data))
    }))
    .ok()
    .flatten()
}

/// Decodes a CMYK JPEG to its *stored* 4-channel samples (no inversion).
pub(crate) fn decode_cmyk(jpeg: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    #[cfg(feature = "mozjpeg")]
    {
        decode_cmyk_mozjpeg(jpeg)
    }
    #[cfg(not(feature = "mozjpeg"))]
    {
        match decode(jpeg)? {
            (w, h, JpegColor::Cmyk, data) => Some((w, h, data)),
            _ => None,
        }
    }
}

#[cfg(feature = "mozjpeg")]
fn decode_cmyk_mozjpeg(jpeg: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    panic::catch_unwind(AssertUnwindSafe(|| -> Option<(u32, u32, Vec<u8>)> {
        let d = mozjpeg::Decompress::new_mem(jpeg).ok()?;
        let mut started = d.to_colorspace(mozjpeg::ColorSpace::JCS_CMYK).ok()?;
        let (w, h) = (started.width() as u32, started.height() as u32);
        let data: Vec<u8> = started.read_scanlines::<u8>().ok()?;
        started.finish().ok()?;
        (data.len() == w as usize * h as usize * 4).then_some((w, h, data))
    }))
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_codecs::jpeg_has_adobe_marker;

    fn mean_abs_error(a: &[u8], b: &[u8]) -> f64 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (*x as f64 - *y as f64).abs())
            .sum::<f64>()
            / a.len() as f64
    }

    /// A textured image: smooth gradients plus pseudo-random detail, so the
    /// optimized Huffman tables use long codes.
    fn textured(w: u32, h: u32, channels: u32) -> Vec<u8> {
        let mut seed = 0x2545_f491u32;
        (0..w * h * channels)
            .map(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let (x, y, c) = ((i / channels) % w, (i / channels) / w, i % channels);
                ((x * 3 + y * 2 + c * 40) % 200 + seed % 24) as u8
            })
            .collect()
    }

    #[test]
    fn every_encoder_stores_cmyk_as_given_with_an_adobe_marker() {
        let (w, h) = (64u32, 48u32);
        let data = textured(w, h, 4);
        for &encoder in JpegEncoder::available() {
            let jpeg = encode(encoder, &data, w, h, JpegColor::Cmyk, 95).unwrap();
            assert!(jpeg_has_adobe_marker(&jpeg), "{encoder:?}");
            let (dw, dh, color, back) = decode(&jpeg).unwrap();
            assert_eq!((dw, dh, color), (w, h, JpegColor::Cmyk));
            // Same samples, give or take IDCT rounding, through mozjpeg.
            let (.., stored) = decode_cmyk(&jpeg).unwrap();
            assert!(mean_abs_error(&stored, &back) < 1.0, "{encoder:?}");
            let err = mean_abs_error(&data, &back);
            assert!(err < 8.0, "{encoder:?}: mean abs error {err}");
        }
    }

    #[test]
    fn every_encoder_round_trips_gray_and_rgb() {
        let (w, h) = (96u32, 64u32);
        for (color, channels) in [(JpegColor::Gray, 1), (JpegColor::Rgb, 3)] {
            let data = textured(w, h, channels);
            for &encoder in JpegEncoder::available() {
                for quality in [50, 90] {
                    let jpeg = encode(encoder, &data, w, h, color, quality).unwrap();
                    let (dw, dh, dcolor, back) = decode(&jpeg).unwrap();
                    assert_eq!((dw, dh, dcolor), (w, h, color));
                    let err = mean_abs_error(&data, &back);
                    assert!(
                        err < 12.0,
                        "{encoder:?} {color:?} q{quality}: mean abs error {err}"
                    );
                }
            }
        }
    }

    /// The pure-Rust decoder must read every JPEG kind exactly like libjpeg,
    /// including YCCK and jpeg-encoder's baseline output (which zune-jpeg
    /// 0.5 decodes as garbage).
    #[cfg(feature = "mozjpeg")]
    #[test]
    fn decoder_agrees_with_mozjpeg() {
        use jpeg_encoder::{ColorType, Encoder};
        let (w, h) = (96u32, 64u32);
        let rgb = textured(w, h, 3);
        let jpeg = encode(JpegEncoder::Rust, &rgb, w, h, JpegColor::Rgb, 60).unwrap();
        let d = mozjpeg::Decompress::new_mem(&jpeg).unwrap();
        let mut started = d.rgb().unwrap();
        let reference: Vec<u8> = started.read_scanlines().unwrap();
        started.finish().unwrap();
        let (.., ours) = decode(&jpeg).unwrap();
        let err = mean_abs_error(&reference, &ours);
        assert!(err < 1.0, "RGB: mean abs difference {err}");

        let cmyk = textured(w, h, 4);
        let mut ycck = Vec::new();
        Encoder::new(&mut ycck, 90)
            .encode(&cmyk, w as u16, h as u16, ColorType::CmykAsYcck)
            .unwrap();
        let (.., reference) = decode_cmyk_mozjpeg(&ycck).unwrap();
        let (.., color, ours) = decode(&ycck).unwrap();
        assert_eq!(color, JpegColor::Cmyk);
        let err = mean_abs_error(&reference, &ours);
        assert!(err < 1.0, "YCCK: mean abs difference {err}");
    }

    #[test]
    fn encoder_names_parse_back() {
        for &e in JpegEncoder::available() {
            assert_eq!(JpegEncoder::parse(e.as_str()), Some(e));
        }
        assert_eq!(JpegEncoder::parse("libjpeg"), None);
    }
}
