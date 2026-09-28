//! Re-encodes (and, when oversized, downsamples) raster images embedded in a
//! [`lopdf::Document`].
//!
//! Only the image kinds we can safely round-trip are touched: JPEG (`DCTDecode`)
//! and raw 8-bit-per-component DeviceGray/DeviceRGB samples (uncompressed or
//! `FlateDecode`). Anything else (JBIG2, JPX, CCITT, indexed, CMYK, image masks,
//! 16-bit, …) is left completely untouched and counted as skipped.
//!
//! Every eligible image is re-encoded as JPEG at the profile's target quality
//! regardless of its resolution — a raw/Flate-compressed bitmap shrinks a lot
//! just from that, even at unchanged pixel dimensions. Downsampling on top of
//! that only happens once the image's *effective on-page DPI* (see
//! `placement.rs`) exceeds the profile's target. The "only replace if smaller"
//! check in `resample_one` is what keeps this safe for images that were
//! already well-optimized.

use std::collections::{HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};

use image::{GrayImage, RgbImage};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::level::Profile;
use crate::placement;

/// Downsample/re-encode every eligible image in `doc` according to `profile`.
/// Returns `(images_resampled, images_skipped)`.
pub fn resample_images(doc: &mut Document, profile: &Profile) -> (usize, usize) {
    let dpi_map = placement::compute_image_dpi(doc);
    let fallback_pt = fallback_page_size_pt(doc);
    let smask_ids = collect_smask_ids(doc);

    let candidates: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, obj)| match obj {
            Object::Stream(s) if is_candidate_image(s) && !smask_ids.contains(id) => Some(*id),
            _ => None,
        })
        .collect();

    let mut resampled = 0usize;
    let mut skipped = 0usize;

    for id in candidates {
        match resample_one(doc, id, profile, &dpi_map, fallback_pt) {
            Some(()) => resampled += 1,
            None => skipped += 1,
        }
    }

    (resampled, skipped)
}

fn is_candidate_image(stream: &Stream) -> bool {
    let is_image = stream
        .dict
        .get(b"Subtype")
        .and_then(Object::as_name)
        .map(|n| n == b"Image")
        .unwrap_or(false);
    let is_mask = stream.dict.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false);
    is_image && !is_mask
}

fn collect_smask_ids(doc: &Document) -> HashSet<ObjectId> {
    doc.objects
        .values()
        .filter_map(|obj| match obj {
            Object::Stream(s) => s.dict.get(b"SMask").and_then(Object::as_reference).ok(),
            _ => None,
        })
        .collect()
}

/// Largest page size in the document, in points; used as a conservative stand-in
/// for images we couldn't locate a placement for.
fn fallback_page_size_pt(doc: &Document) -> (f32, f32) {
    let mut max_w = 612.0_f32; // US Letter, if nothing better is found
    let mut max_h = 792.0_f32;
    for (_, page_id) in doc.get_pages() {
        if let Ok(page) = doc.get_dictionary(page_id)
            && let Ok(bbox) = page.get(b"MediaBox").and_then(Object::as_array)
            && bbox.len() == 4
            && let (Ok(x0), Ok(y0), Ok(x1), Ok(y1)) = (
                num(&bbox[0]),
                num(&bbox[1]),
                num(&bbox[2]),
                num(&bbox[3]),
            )
        {
            let w = (x1 - x0).abs();
            let h = (y1 - y0).abs();
            if w * h > max_w * max_h {
                max_w = w;
                max_h = h;
            }
        }
    }
    (max_w, max_h)
}

fn num(o: &Object) -> Result<f32, ()> {
    o.as_f32().or_else(|_| o.as_i64().map(|i| i as f32)).map_err(|_| ())
}

enum ColorKind {
    Gray,
    Rgb,
}

enum FilterKind {
    Dct,
    RawFlate,
    RawNone,
}

fn classify_filter(stream: &Stream) -> Option<FilterKind> {
    match stream.filters() {
        Err(_) => Some(FilterKind::RawNone),
        Ok(f) if f.is_empty() => Some(FilterKind::RawNone),
        Ok(f) if f.len() == 1 && f[0] == b"DCTDecode" => Some(FilterKind::Dct),
        Ok(f) if f.len() == 1 && f[0] == b"FlateDecode" => Some(FilterKind::RawFlate),
        _ => None,
    }
}

fn color_kind(doc: &Document, dict: &Dictionary) -> Option<ColorKind> {
    let cs_obj = dict.get(b"ColorSpace").ok()?;
    let resolved: &Object = match cs_obj {
        Object::Reference(rid) => doc.get_object(*rid).ok()?,
        other => other,
    };
    match resolved {
        Object::Name(name) => match name.as_slice() {
            b"DeviceGray" | b"CalGray" => Some(ColorKind::Gray),
            b"DeviceRGB" | b"CalRGB" => Some(ColorKind::Rgb),
            _ => None,
        },
        Object::Array(arr) => {
            let first = arr.first()?.as_name().ok()?;
            if first != b"ICCBased" {
                return None;
            }
            let icc_id = arr.get(1)?.as_reference().ok()?;
            let icc_stream = doc.get_object(icc_id).ok()?.as_stream().ok()?;
            match icc_stream.dict.get(b"N").and_then(Object::as_i64).ok()? {
                1 => Some(ColorKind::Gray),
                3 => Some(ColorKind::Rgb),
                _ => None,
            }
        }
        _ => None,
    }
}

enum Pixels {
    Gray(GrayImage),
    Rgb(RgbImage),
}

fn decode_pixels(stream: &Stream, color: &ColorKind, filter: &FilterKind, w: u32, h: u32) -> Option<Pixels> {
    match filter {
        FilterKind::Dct => {
            let img = image::load_from_memory_with_format(&stream.content, image::ImageFormat::Jpeg).ok()?;
            Some(match color {
                ColorKind::Gray => Pixels::Gray(img.into_luma8()),
                ColorKind::Rgb => Pixels::Rgb(img.into_rgb8()),
            })
        }
        FilterKind::RawFlate | FilterKind::RawNone => {
            let bpc = stream.dict.get(b"BitsPerComponent").and_then(Object::as_i64).unwrap_or(8);
            if bpc != 8 {
                return None;
            }
            let raw = match filter {
                FilterKind::RawFlate => stream.decompressed_content().ok()?,
                _ => stream.content.clone(),
            };
            match color {
                ColorKind::Gray => {
                    if raw.len() as u64 != w as u64 * h as u64 {
                        return None;
                    }
                    GrayImage::from_raw(w, h, raw).map(Pixels::Gray)
                }
                ColorKind::Rgb => {
                    if raw.len() as u64 != w as u64 * h as u64 * 3 {
                        return None;
                    }
                    RgbImage::from_raw(w, h, raw).map(Pixels::Rgb)
                }
            }
        }
    }
}

/// Target pixel dimensions for an image at `effective_dpi`. Downsampling only
/// kicks in once the image is oversized relative to the profile's target DPI —
/// but the image is re-encoded (recompressed) either way: `trigger_ratio` gates
/// *resizing*, not whether the image gets touched at all. That distinction
/// matters a lot for images whose declared placement makes them look
/// low-resolution (e.g. a background photo drawn oversized and clipped to the
/// page) while still being stored as raw/Flate pixels that JPEG re-encoding
/// alone shrinks dramatically.
///
/// `max_dimension` is a second, independent trigger: a hard cap on the longest
/// side that applies even when the DPI heuristic above didn't fire, since that
/// heuristic trusts the placement matrix — which a clipped, oversized-then-cropped
/// image can make wildly overstate the image's real on-page footprint. Whichever
/// of the two wants a smaller result wins.
fn target_dims(w: u32, h: u32, effective_dpi: f32, profile: &Profile) -> (u32, u32) {
    let dpi_scale = if effective_dpi > 0.0 && effective_dpi > profile.target_dpi * profile.trigger_ratio {
        (profile.target_dpi / effective_dpi).clamp(0.05, 1.0)
    } else {
        1.0
    };

    let longest = w.max(h) as f32;
    let cap_scale = if profile.max_dimension > 0 && longest > profile.max_dimension as f32 {
        profile.max_dimension as f32 / longest
    } else {
        1.0
    };

    let scale = dpi_scale.min(cap_scale);
    if scale >= 1.0 {
        return (w, h);
    }
    let new_w = ((w as f32) * scale).round().max(1.0) as u32;
    let new_h = ((h as f32) * scale).round().max(1.0) as u32;
    (new_w, new_h)
}

fn encode_jpeg(data: &[u8], w: u32, h: u32, gray: bool, quality: u8) -> Option<Vec<u8>> {
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| -> std::io::Result<Vec<u8>> {
        let color_space = if gray {
            mozjpeg::ColorSpace::JCS_GRAYSCALE
        } else {
            mozjpeg::ColorSpace::JCS_RGB
        };
        let mut comp = mozjpeg::Compress::new(color_space);
        comp.set_size(w as usize, h as usize);
        comp.set_quality(quality as f32);
        let mut started = comp.start_compress(Vec::new())?;
        started.write_scanlines(data)?;
        started.finish()
    }));
    outcome.ok()?.ok()
}

/// Re-encode `raw` as a `FlateDecode` stream via lopdf's own compressor, reusing
/// its "keep raw if compression doesn't help" logic. Returns the stored bytes and
/// whether a `Filter` ended up being set.
fn flate_encode(raw: Vec<u8>) -> (Vec<u8>, bool) {
    let mut scratch = Stream::new(Dictionary::new(), raw);
    let _ = scratch.compress();
    let has_filter = scratch.dict.get(b"Filter").is_ok();
    (scratch.content, has_filter)
}

fn resample_one(
    doc: &mut Document,
    id: ObjectId,
    profile: &Profile,
    dpi_map: &HashMap<ObjectId, f32>,
    fallback_pt: (f32, f32),
) -> Option<()> {
    let (width, height, color, filter, effective_dpi, smask_id, original_len) = {
        let stream = doc.objects.get(&id)?.as_stream().ok()?;
        let width = stream.dict.get(b"Width").and_then(Object::as_i64).ok()? as u32;
        let height = stream.dict.get(b"Height").and_then(Object::as_i64).ok()? as u32;
        if width == 0 || height == 0 {
            return None;
        }
        let color = color_kind(doc, &stream.dict)?;
        let filter = classify_filter(stream)?;
        let effective_dpi = dpi_map.get(&id).copied().unwrap_or_else(|| {
            let (pw, ph) = fallback_pt;
            let dpi_x = width as f32 / (pw / 72.0);
            let dpi_y = height as f32 / (ph / 72.0);
            (dpi_x + dpi_y) / 2.0
        });
        let smask_id = stream.dict.get(b"SMask").and_then(Object::as_reference).ok();
        (width, height, color, filter, effective_dpi, smask_id, stream.content.len())
    };

    let (new_w, new_h) = target_dims(width, height, effective_dpi, profile);
    let needs_resize = (new_w, new_h) != (width, height);

    let pixels = {
        let stream = doc.objects.get(&id)?.as_stream().ok()?;
        decode_pixels(stream, &color, &filter, width, height)?
    };

    let (jpeg_bytes, gray) = match pixels {
        Pixels::Gray(img) => {
            let img = if needs_resize {
                image::imageops::resize(&img, new_w, new_h, image::imageops::FilterType::Lanczos3)
            } else {
                img
            };
            (encode_jpeg(img.as_raw(), new_w, new_h, true, profile.jpeg_quality)?, true)
        }
        Pixels::Rgb(img) => {
            let img = if needs_resize {
                image::imageops::resize(&img, new_w, new_h, image::imageops::FilterType::Lanczos3)
            } else {
                img
            };
            (encode_jpeg(img.as_raw(), new_w, new_h, false, profile.jpeg_quality)?, false)
        }
    };

    if jpeg_bytes.len() >= original_len {
        return None;
    }

    {
        let stream = doc.objects.get_mut(&id)?.as_stream_mut().ok()?;
        stream.dict.set("Width", new_w as i64);
        stream.dict.set("Height", new_h as i64);
        stream.dict.set("BitsPerComponent", 8i64);
        stream.dict.set(
            "ColorSpace",
            Object::Name(if gray { b"DeviceGray".to_vec() } else { b"DeviceRGB".to_vec() }),
        );
        stream.dict.remove(b"DecodeParms");
        stream.dict.remove(b"Decode");
        stream.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        stream.set_content(jpeg_bytes);
    }

    if let Some(smask_id) = smask_id {
        resample_smask(doc, smask_id, new_w, new_h);
    }

    Some(())
}

fn resample_smask(doc: &mut Document, id: ObjectId, new_w: u32, new_h: u32) {
    let decoded: Option<GrayImage> = (|| {
        let stream = doc.objects.get(&id)?.as_stream().ok()?;
        let w = stream.dict.get(b"Width").and_then(Object::as_i64).ok()? as u32;
        let h = stream.dict.get(b"Height").and_then(Object::as_i64).ok()? as u32;
        if w == 0 || h == 0 || (new_w >= w && new_h >= h) {
            return None;
        }
        let bpc = stream.dict.get(b"BitsPerComponent").and_then(Object::as_i64).unwrap_or(8);
        if bpc != 8 {
            return None;
        }
        let filter = classify_filter(stream)?;
        let raw = match filter {
            FilterKind::RawFlate => stream.decompressed_content().ok()?,
            FilterKind::RawNone => stream.content.clone(),
            FilterKind::Dct => return None,
        };
        if raw.len() as u64 != w as u64 * h as u64 {
            return None;
        }
        GrayImage::from_raw(w, h, raw)
    })();

    let Some(img) = decoded else { return };
    let resized = image::imageops::resize(&img, new_w, new_h, image::imageops::FilterType::Lanczos3);
    let (content, has_filter) = flate_encode(resized.into_raw());

    if let Some(stream) = doc.objects.get_mut(&id).and_then(|o| o.as_stream_mut().ok()) {
        stream.dict.set("Width", new_w as i64);
        stream.dict.set("Height", new_h as i64);
        stream.dict.set("BitsPerComponent", 8i64);
        stream.dict.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
        stream.dict.remove(b"DecodeParms");
        if has_filter {
            stream.dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        } else {
            stream.dict.remove(b"Filter");
        }
        stream.set_content(content);
    }
}
