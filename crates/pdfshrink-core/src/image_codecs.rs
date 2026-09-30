//! Decoders/encoders for the image kinds beyond 8-bit gray/RGB: CMYK JPEG
//! (through mozjpeg), JPEG 2000 (pure-Rust `hayro-jpeg2000`), CCITT Group 4
//! (pure-Rust `fax`), palette (`Indexed`) and 16-bit samples. Kept apart from
//! `image_ops.rs`, which decides *what* to do with an image.
//!
//! Every decoder handling untrusted bytes runs under `catch_unwind`, like the
//! mozjpeg encoder: a malformed image must only mean "leave this one alone".

use std::panic::{self, AssertUnwindSafe};

/// Samples decoded from a JPEG 2000 image.
pub struct JpxImage {
    pub width: u32,
    pub height: u32,
    /// Colour channels (1 gray, 3 RGB, 4 CMYK), interleaved, 8 bits each.
    pub channels: u8,
    pub data: Vec<u8>,
    /// Opacity channel, if the codestream has one.
    pub alpha: Option<Vec<u8>>,
}

/// Decodes a JPEG 2000 (JP2 file or raw codestream), scaling samples to
/// 8 bits. Palette-based JPX is resolved to its colours.
pub fn decode_jpx(data: &[u8]) -> Option<JpxImage> {
    panic::catch_unwind(AssertUnwindSafe(|| {
        use hayro_jpeg2000::{DecodeSettings, DecoderContext, Image};
        let image = Image::new(data, &DecodeSettings::default()).ok()?;
        let (width, height) = (image.width(), image.height());
        let channels = image.color_space().num_channels();
        let has_alpha = image.has_alpha();
        if !matches!(channels, 1 | 3 | 4) {
            return None;
        }
        let mut ctx = DecoderContext::default();
        let decoded = image.decode(&mut ctx).ok()?.data_u8();
        let stride = channels as usize + usize::from(has_alpha);
        if decoded.len() != width as usize * height as usize * stride {
            return None;
        }
        let (data, alpha) = if has_alpha {
            let mut color =
                Vec::with_capacity(width as usize * height as usize * channels as usize);
            let mut alpha = Vec::with_capacity(width as usize * height as usize);
            for px in decoded.chunks_exact(stride) {
                color.extend_from_slice(&px[..channels as usize]);
                alpha.push(px[channels as usize]);
            }
            (color, Some(alpha))
        } else {
            (decoded, None)
        };
        Some(JpxImage {
            width,
            height,
            channels,
            data,
            alpha,
        })
    }))
    .ok()
    .flatten()
}

/// Whether a JPEG carries an Adobe APP14 marker. CMYK JPEGs with one are
/// conventionally stored inverted (Photoshop); mozjpeg writes one too.
pub fn jpeg_has_adobe_marker(jpeg: &[u8]) -> bool {
    let mut i = 2;
    while i + 4 <= jpeg.len() && jpeg[i] == 0xFF {
        let marker = jpeg[i + 1];
        if marker == 0xDA || marker == 0xD9 {
            break; // start of scan / end of image
        }
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        if marker == 0xEE && jpeg.get(i + 4..i + 9) == Some(b"Adobe".as_slice()) {
            return true;
        }
        i += 2 + len;
    }
    false
}

/// Decodes a CMYK JPEG to its *stored* 4-channel samples (no inversion).
pub fn decode_cmyk_jpeg(jpeg: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
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

/// Encodes 4-channel samples as a CMYK JPEG. libjpeg writes an Adobe APP14
/// marker for CMYK, and stores the samples as given.
pub fn encode_cmyk_jpeg(data: &[u8], w: u32, h: u32, quality: u8) -> Option<Vec<u8>> {
    panic::catch_unwind(AssertUnwindSafe(|| -> std::io::Result<Vec<u8>> {
        let mut comp = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_CMYK);
        comp.set_size(w as usize, h as usize);
        comp.set_quality(quality as f32);
        let mut started = comp.start_compress(Vec::new())?;
        started.write_scanlines(data)?;
        started.finish()
    }))
    .ok()?
    .ok()
}

/// Reduces 16-bit big-endian samples to 8 bits (high byte).
pub fn samples_16_to_8(raw: &[u8]) -> Vec<u8> {
    raw.as_chunks::<2>().0.iter().map(|s| s[0]).collect()
}

/// Unpacks `bpc`-bit samples (1, 2, 4 or 8; rows byte-aligned) into one byte
/// per sample. `per_row` is the number of samples per row.
pub fn unpack_samples(raw: &[u8], per_row: usize, rows: usize, bpc: u8) -> Option<Vec<u8>> {
    if bpc == 8 {
        return (raw.len() >= per_row * rows).then(|| raw[..per_row * rows].to_vec());
    }
    if !matches!(bpc, 1 | 2 | 4) {
        return None;
    }
    let row_bytes = (per_row * bpc as usize).div_ceil(8);
    if raw.len() < row_bytes * rows {
        return None;
    }
    let mask = (1u8 << bpc) - 1;
    let mut out = Vec::with_capacity(per_row * rows);
    for y in 0..rows {
        let row = &raw[y * row_bytes..(y + 1) * row_bytes];
        for x in 0..per_row {
            let bit = x * bpc as usize;
            out.push((row[bit / 8] >> (8 - bpc as usize - bit % 8)) & mask);
        }
    }
    Some(out)
}

/// Encodes a 1-bit image (rows byte-aligned, as stored in a PDF) as CCITT
/// Group 4. Sample 0 is encoded as black, 1 as white, so decoding with
/// `/K -1 /BlackIs1 false` gives back the exact same samples.
pub fn encode_g4(raw: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    use fax::{Color, VecWriter, encoder::Encoder};
    let row_bytes = (w as usize).div_ceil(8);
    if raw.len() < row_bytes * h as usize {
        return None;
    }
    panic::catch_unwind(AssertUnwindSafe(|| {
        let mut enc = Encoder::new(VecWriter::new());
        for y in 0..h as usize {
            let row = &raw[y * row_bytes..(y + 1) * row_bytes];
            let pels = (0..w as usize).map(|x| {
                if (row[x / 8] >> (7 - x % 8)) & 1 == 0 {
                    Color::Black
                } else {
                    Color::White
                }
            });
            enc.encode_line(pels, w).ok()?;
        }
        Some(enc.finish().ok()?.finish())
    }))
    .ok()
    .flatten()
}

/// Decodes CCITT Group 4 back to packed 1-bit rows (0 = black), for tests.
#[cfg(test)]
pub fn decode_g4(data: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    use fax::Color;
    let row_bytes = (w as usize).div_ceil(8);
    let mut out = Vec::with_capacity(row_bytes * h as usize);
    fax::decoder::decode_g4(data.iter().copied(), w, Some(h), |transitions| {
        let mut row = vec![0u8; row_bytes];
        for (x, c) in fax::decoder::pels(transitions, w).enumerate() {
            if c == Color::White {
                row[x / 8] |= 1 << (7 - x % 8);
            }
        }
        out.extend_from_slice(&row);
    })?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g4_round_trips_exactly() {
        let (w, h) = (37u32, 23u32);
        let row_bytes = (w as usize).div_ceil(8);
        let mut raw = vec![0xFFu8; row_bytes * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                if (x * 7 + y * 3) % 11 < 4 || (x > 10 && x < 20 && y > 5 && y < 15) {
                    raw[y * row_bytes + x / 8] &= !(1 << (7 - x % 8));
                }
            }
        }
        // Padding bits past the width are irrelevant; normalise them.
        let clean = |v: &[u8]| -> Vec<u8> {
            let mut v = v.to_vec();
            for y in 0..h as usize {
                v[y * row_bytes + row_bytes - 1] &= 0xFF << (row_bytes * 8 - w as usize);
            }
            v
        };
        let g4 = encode_g4(&raw, w, h).unwrap();
        assert_eq!(clean(&decode_g4(&g4, w, h).unwrap()), clean(&raw));
    }

    #[test]
    fn unpacks_sub_byte_samples() {
        // 2-bit samples 0,1,2,3,3 on a row of 5 (row padded to 2 bytes).
        assert_eq!(
            unpack_samples(&[0b0001_1011, 0b1100_0000], 5, 1, 2).unwrap(),
            vec![0, 1, 2, 3, 3]
        );
    }

    #[test]
    fn cmyk_jpeg_round_trips_and_carries_an_adobe_marker() {
        let (w, h) = (32u32, 16u32);
        let data: Vec<u8> = (0..w * h * 4).map(|i| (i % 251) as u8).collect();
        let jpeg = encode_cmyk_jpeg(&data, w, h, 95).unwrap();
        assert!(jpeg_has_adobe_marker(&jpeg));
        let (dw, dh, back) = decode_cmyk_jpeg(&jpeg).unwrap();
        assert_eq!((dw, dh), (w, h));
        let err: f64 = data
            .iter()
            .zip(&back)
            .map(|(a, b)| (*a as f64 - *b as f64).abs())
            .sum::<f64>()
            / data.len() as f64;
        assert!(err < 8.0, "mean abs error {err}");
    }
}
