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
//! check in `plan_one` is what keeps this safe for images that were
//! already well-optimized.
//!
//! The experimental levels (see [`crate::level::Experimental`]) add, per image:
//! a page-relative resolution cap, JPEG quality chosen by SSIM against the
//! source, near-gray RGB stored as gray, few-colour images kept lossless as an
//! `Indexed` palette, fully opaque soft masks dropped, and soft masks stored
//! with a PNG predictor.
//!
//! Images are processed in parallel: each is *planned* (decoded, resized,
//! re-encoded) from a read-only snapshot, and the results are then applied to
//! the document sequentially.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};

use image::{GrayImage, RgbImage};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};
use rayon::prelude::*;

use crate::level::{Experimental, Profile};
use crate::placement::{self, Placement};

/// Downsample/re-encode every eligible image in `doc` according to `profile`.
/// Returns `(images_resampled, images_skipped)`.
pub fn resample_images(doc: &mut Document, profile: &Profile) -> (usize, usize) {
    let placements = placement::compute_image_placement(doc);
    let fallback_pt = fallback_page_size_pt(doc);
    let smask_ids = collect_smask_ids(doc);
    let refcounts = if profile.experimental.crop && placements.complete {
        count_references(doc)
    } else {
        HashMap::new()
    };

    let jobs: Vec<Job> = doc
        .objects
        .iter()
        .filter_map(|(id, obj)| match obj {
            Object::Stream(s) if is_candidate_image(s) && !smask_ids.contains(id) => {
                let smask = s
                    .dict
                    .get(b"SMask")
                    .and_then(Object::as_reference)
                    .ok()
                    .and_then(|sid| {
                        let s = doc.get_object(sid).ok()?.as_stream().ok()?;
                        Some((sid, with_direct_decode_parms(doc, s)))
                    });
                let placement = placements.map.get(id).copied();
                let crop_ok = placement.is_some_and(|p| {
                    refcounts.get(id) == Some(&p.via_dicts)
                        && smask
                            .as_ref()
                            .is_none_or(|(sid, _)| refcounts.get(sid) == Some(&1))
                        && [
                            &b"Mask"[..],
                            b"OC",
                            b"StructParent",
                            b"StructParents",
                            b"Alternates",
                        ]
                        .iter()
                        .all(|k| !s.dict.has(k))
                });
                Some(Job {
                    id: *id,
                    stream: with_direct_decode_parms(doc, s),
                    color: color_kind(doc, &s.dict),
                    placement,
                    crop_ok,
                    smask,
                })
            }
            _ => None,
        })
        .collect();
    let candidates = jobs.len();
    if std::env::var_os("PDFSHRINK_DEBUG").is_some() {
        let (mut n, mut bytes) = (0usize, 0f64);
        for j in &jobs {
            if let Some(p) = j.placement {
                let [u0, v0, u1, v1] = p.visible;
                let frac = ((u1 - u0).max(0.0) * (v1 - v0).max(0.0)).min(1.0);
                if frac < 0.9 {
                    n += 1;
                    bytes += j.stream.content.len() as f64 * (1.0 - frac as f64);
                }
            }
        }
        eprintln!(
            "  crop potential: {n} images <90% visible, ~{:.0} KB of stored bytes off-page",
            bytes / 1e3
        );
    }

    let plans: Vec<Plan> = jobs
        .into_par_iter()
        .filter_map(|job| plan_one(job, profile, fallback_pt))
        .collect();

    let resampled = plans.len();
    // Soft masks can be shared by several images (after dedup); resize each
    // once, to the largest size any of its parents asked for.
    let mut smask_updates: HashMap<ObjectId, SmaskUpdate> = HashMap::new();
    for plan in plans {
        let target = match plan.crop {
            None => plan.id,
            Some(rect) => {
                // The cropped image becomes a new object, drawn by a Form
                // XObject that takes over the original id: every existing
                // `Do` keeps working, and still paints into the same unit
                // square, just with only the visible part of the pixels.
                let new_id = doc.add_object(Stream::new(Dictionary::new(), Vec::new()));
                doc.objects
                    .insert(plan.id, Object::Stream(crop_wrapper(new_id, rect)));
                new_id
            }
        };
        let Some(Object::Stream(stream)) = doc.objects.get_mut(&target) else {
            continue;
        };
        stream.dict = plan.dict;
        stream.set_content(plan.content);
        match plan.smask {
            SmaskAction::Keep => {}
            SmaskAction::Drop => {
                stream.dict.remove(b"SMask");
            }
            SmaskAction::Replace(u) => {
                let e = smask_updates.entry(u.id).or_insert_with(|| u.clone());
                if u.w * u.h > e.w * e.h {
                    *e = u;
                }
            }
        }
    }
    for (id, u) in smask_updates {
        if let Some(Object::Stream(s)) = doc.objects.get_mut(&id) {
            s.dict.set("Width", u.w as i64);
            s.dict.set("Height", u.h as i64);
            s.dict.set("BitsPerComponent", 8i64);
            s.dict
                .set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
            s.dict.remove(b"Decode");
            match u.decode_parms {
                Some(p) => s.dict.set("DecodeParms", p),
                None => {
                    s.dict.remove(b"DecodeParms");
                }
            }
            if u.flate {
                s.dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
            } else {
                s.dict.remove(b"Filter");
            }
            s.set_content(u.content);
        }
    }

    (resampled, candidates - resampled)
}

/// Clone of `stream` with an indirect (or single-element array) `/DecodeParms`
/// inlined: lopdf only honours a direct dictionary, and would otherwise hand
/// back still-predicted bytes (one extra filter byte per row) that then fail
/// the size check — silently skipping e.g. every PNG-predicted image
/// matplotlib/LaTeX produce.
fn with_direct_decode_parms(doc: &Document, stream: &Stream) -> Stream {
    let mut s = stream.clone();
    let resolve = |o: &Object| -> Option<Object> {
        match o {
            Object::Reference(r) => doc.get_object(*r).ok().cloned(),
            other => Some(other.clone()),
        }
    };
    if let Ok(parms) = stream.dict.get(b"DecodeParms") {
        let direct = match resolve(parms) {
            Some(Object::Array(a)) if a.len() == 1 => resolve(&a[0]),
            other => other,
        };
        if let Some(d @ Object::Dictionary(_)) = direct {
            s.dict.set("DecodeParms", d);
        }
    }
    s
}

struct Job {
    id: ObjectId,
    stream: Stream,
    color: Option<ColorKind>,
    placement: Option<Placement>,
    /// Every use of this image (and of its soft mask) was seen by the
    /// placement walk, so cropping to its visible part is safe.
    crop_ok: bool,
    smask: Option<(ObjectId, Stream)>,
}

struct Plan {
    id: ObjectId,
    dict: Dictionary,
    content: Vec<u8>,
    smask: SmaskAction,
    /// Kept part of the image, `[u0, v0, u1, v1]` in image space, when it was
    /// cropped.
    crop: Option<[f32; 4]>,
}

/// Number of times each object is referenced anywhere in the document.
fn count_references(doc: &Document) -> HashMap<ObjectId, usize> {
    fn walk(o: &Object, counts: &mut HashMap<ObjectId, usize>) {
        match o {
            Object::Reference(r) => *counts.entry(*r).or_default() += 1,
            Object::Array(a) => a.iter().for_each(|x| walk(x, counts)),
            Object::Dictionary(d) => d.iter().for_each(|(_, x)| walk(x, counts)),
            Object::Stream(s) => s.dict.iter().for_each(|(_, x)| walk(x, counts)),
            _ => {}
        }
    }
    let mut counts = HashMap::new();
    for o in doc.objects.values() {
        walk(o, &mut counts);
    }
    for (_, o) in doc.trailer.iter() {
        walk(o, &mut counts);
    }
    counts
}

/// Form XObject standing in for a cropped image: draws `image` into the
/// `rect` part of the unit square the original image used to cover.
fn crop_wrapper(image: ObjectId, rect: [f32; 4]) -> Stream {
    let [u0, v0, u1, v1] = rect;
    let content = format!(
        "q {:.6} 0 0 {:.6} {:.6} {:.6} cm /I Do Q",
        u1 - u0,
        v1 - v0,
        u0,
        v0
    );
    let mut xobjects = Dictionary::new();
    xobjects.set("I", Object::Reference(image));
    let mut resources = Dictionary::new();
    resources.set("XObject", Object::Dictionary(xobjects));
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"XObject".to_vec()));
    dict.set("Subtype", Object::Name(b"Form".to_vec()));
    dict.set("FormType", 1i64);
    dict.set(
        "BBox",
        Object::Array(vec![0.into(), 0.into(), 1.into(), 1.into()]),
    );
    dict.set("Resources", Object::Dictionary(resources));
    Stream::new(dict, content.into_bytes())
}

/// Pixel rectangle `(x0, y0, x1, y1)` (rows counted from the top) covering the
/// visible `[u0, v0, u1, v1]` part of a `w`×`h` image plus a small margin
/// against resampling edge effects; `None` unless it saves at least 10 % of
/// the pixels.
fn crop_rect(visible: [f32; 4], w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    const MARGIN: f32 = 2.0;
    let [u0, v0, u1, v1] = visible;
    if !(u1 > u0 && v1 > v0) {
        return None;
    }
    let (wf, hf) = (w as f32, h as f32);
    let x0 = (u0 * wf - MARGIN).floor().clamp(0.0, wf) as u32;
    let x1 = (u1 * wf + MARGIN).ceil().clamp(0.0, wf) as u32;
    let y0 = ((1.0 - v1) * hf - MARGIN).floor().clamp(0.0, hf) as u32;
    let y1 = ((1.0 - v0) * hf + MARGIN).ceil().clamp(0.0, hf) as u32;
    let kept = (x1 - x0) as u64 * (y1 - y0) as u64;
    (x1 > x0 && y1 > y0 && kept * 10 < w as u64 * h as u64 * 9).then_some((x0, y0, x1, y1))
}

enum SmaskAction {
    Keep,
    Drop,
    Replace(SmaskUpdate),
}

#[derive(Clone)]
struct SmaskUpdate {
    id: ObjectId,
    w: u32,
    h: u32,
    content: Vec<u8>,
    flate: bool,
    decode_parms: Option<Object>,
}

fn is_candidate_image(stream: &Stream) -> bool {
    let is_image = stream
        .dict
        .get(b"Subtype")
        .and_then(Object::as_name)
        .map(|n| n == b"Image")
        .unwrap_or(false);
    let is_mask = stream
        .dict
        .get(b"ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false);
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
            && let (Ok(x0), Ok(y0), Ok(x1), Ok(y1)) =
                (num(&bbox[0]), num(&bbox[1]), num(&bbox[2]), num(&bbox[3]))
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
    o.as_f32()
        .or_else(|_| o.as_i64().map(|i| i as f32))
        .map_err(|_| ())
}

#[derive(Clone, Copy)]
enum ColorKind {
    Gray,
    Rgb,
}

enum FilterKind {
    Dct,
    /// JPEG wrapped in an extra Flate layer (`[/FlateDecode /DCTDecode]`),
    /// as iLovePDF writes them.
    FlateDct,
    RawFlate,
    RawNone,
}

fn classify_filter(stream: &Stream) -> Option<FilterKind> {
    match stream.filters() {
        Err(_) => Some(FilterKind::RawNone),
        Ok(f) if f.is_empty() => Some(FilterKind::RawNone),
        Ok(f) if f.len() == 1 && f[0] == b"DCTDecode" => Some(FilterKind::Dct),
        Ok(f) if f.len() == 1 && f[0] == b"FlateDecode" => Some(FilterKind::RawFlate),
        Ok(f) if f.len() == 2 && f[0] == b"FlateDecode" && f[1] == b"DCTDecode" => {
            Some(FilterKind::FlateDct)
        }
        _ => None,
    }
}

/// The JPEG bytes of a `Dct`/`FlateDct` image.
fn jpeg_bytes<'a>(stream: &'a Stream, filter: &FilterKind) -> Option<std::borrow::Cow<'a, [u8]>> {
    match filter {
        FilterKind::Dct => Some(std::borrow::Cow::Borrowed(&stream.content)),
        FilterKind::FlateDct => {
            let mut out = Vec::new();
            std::io::Read::read_to_end(
                &mut flate2::read::ZlibDecoder::new(stream.content.as_slice()),
                &mut out,
            )
            .ok()?;
            Some(std::borrow::Cow::Owned(out))
        }
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

fn decode_pixels(
    stream: &Stream,
    color: &ColorKind,
    filter: &FilterKind,
    w: u32,
    h: u32,
) -> Option<Pixels> {
    match filter {
        FilterKind::Dct | FilterKind::FlateDct => {
            let jpeg = jpeg_bytes(stream, filter)?;
            let img = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).ok()?;
            Some(match color {
                ColorKind::Gray => Pixels::Gray(img.into_luma8()),
                ColorKind::Rgb => Pixels::Rgb(img.into_rgb8()),
            })
        }
        FilterKind::RawFlate | FilterKind::RawNone => {
            let bpc = stream
                .dict
                .get(b"BitsPerComponent")
                .and_then(Object::as_i64)
                .unwrap_or(8);
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
/// image can make wildly overstate the image's real on-page footprint. The
/// experimental `page_px` cap is a third one. Whichever wants the smallest
/// result wins.
fn target_dims(w: u32, h: u32, placement: Placement, profile: &Profile) -> (u32, u32) {
    let effective_dpi = placement.dpi;
    let dpi_scale =
        if effective_dpi > 0.0 && effective_dpi > profile.target_dpi * profile.trigger_ratio {
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

    let page_px = profile.experimental.page_px as f32;
    let page_scale = if page_px > 0.0 && placement.page_px > page_px * profile.trigger_ratio {
        (page_px / placement.page_px).clamp(0.05, 1.0)
    } else {
        1.0
    };

    let scale = dpi_scale.min(cap_scale).min(page_scale);
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

fn plan_one(job: Job, profile: &Profile, fallback_pt: (f32, f32)) -> Option<Plan> {
    let x = &profile.experimental;
    let stream = &job.stream;
    let width = stream.dict.get(b"Width").and_then(Object::as_i64).ok()? as u32;
    let height = stream.dict.get(b"Height").and_then(Object::as_i64).ok()? as u32;
    if width == 0 || height == 0 {
        return None;
    }
    let color = job.color?;
    let filter = classify_filter(stream)?;
    let placement = job.placement.unwrap_or_else(|| {
        let (pw, ph) = fallback_pt;
        let dpi_x = width as f32 / (pw / 72.0);
        let dpi_y = height as f32 / (ph / 72.0);
        let dpi = (dpi_x + dpi_y) / 2.0;
        Placement {
            dpi,
            page_px: dpi / 72.0 * pw,
            visible: [0.0, 0.0, 1.0, 1.0],
            via_dicts: 0,
        }
    });
    let original_len = stream.content.len();
    // A scanned page: one image spanning (nearly) the full page width.
    let covers_page = job.placement.is_some() && width as f32 >= 0.8 * placement.page_px;

    let pixels = decode_pixels(stream, &color, &filter, width, height)?;

    // Opaque soft masks are dropped before anything else: the parent then
    // needs no mask handling at all.
    let mut smask = job.smask.as_ref().and_then(|(sid, s)| {
        let alpha = decode_smask(s)?;
        Some((*sid, alpha))
    });
    let smask_dims = smask.as_ref().map(|(_, a)| a.dimensions());
    let drop_smask = x.drop_opaque_smask
        && smask
            .as_ref()
            .is_some_and(|(_, a)| a.pixels().all(|p| p.0[0] == 255));

    // Cropping to the visible part (only with a mask we can crop identically).
    let crop = (x.crop && job.crop_ok)
        .then(|| crop_rect(placement.visible, width, height))
        .flatten()
        .filter(|_| drop_smask || job.smask.is_none() || smask_dims == Some((width, height)));
    let (pixels, width, height, crop_unit) = match crop {
        None => (pixels, width, height, None),
        Some((x0, y0, x1, y1)) => {
            let (cw, ch) = (x1 - x0, y1 - y0);
            let unit = [
                x0 as f32 / width as f32,
                1.0 - y1 as f32 / height as f32,
                x1 as f32 / width as f32,
                1.0 - y0 as f32 / height as f32,
            ];
            if let Some((_, alpha)) = smask.as_mut() {
                *alpha = image::imageops::crop_imm(alpha, x0, y0, cw, ch).to_image();
            }
            let cropped = match pixels {
                Pixels::Gray(i) => {
                    Pixels::Gray(image::imageops::crop_imm(&i, x0, y0, cw, ch).to_image())
                }
                Pixels::Rgb(i) => {
                    Pixels::Rgb(image::imageops::crop_imm(&i, x0, y0, cw, ch).to_image())
                }
            };
            (cropped, cw, ch, Some(unit))
        }
    };

    let (new_w, new_h) = target_dims(width, height, placement, profile);
    let needs_resize = (new_w, new_h) != (width, height);

    let mut pixels = pixels;
    let paper = if covers_page {
        paper_white_point(&pixels)
    } else {
        None
    };
    if let (Some(white_point), true) = (paper, x.scan_whiten) {
        whiten(&mut pixels, white_point);
    }
    let scan = paper.is_some();
    let pixels = match pixels {
        Pixels::Rgb(img) if x.detect_gray && is_near_gray(&img) => {
            Pixels::Gray(image::DynamicImage::ImageRgb8(img).into_luma8())
        }
        p => p,
    };

    let resized = match pixels {
        Pixels::Gray(img) if needs_resize => Pixels::Gray(image::imageops::resize(
            &img,
            new_w,
            new_h,
            image::imageops::FilterType::Lanczos3,
        )),
        Pixels::Rgb(img) if needs_resize => Pixels::Rgb(image::imageops::resize(
            &img,
            new_w,
            new_h,
            image::imageops::FilterType::Lanczos3,
        )),
        p => p,
    };

    let mut dict = stream.dict.clone();
    let encoded = encode_best(&resized, new_w, new_h, profile, scan)?;
    if encoded.content.len() >= original_len && !drop_smask && crop_unit.is_none() {
        return None;
    }
    let content = match encoded.kind {
        EncodedKind::Jpeg { gray } => {
            dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
            dict.remove(b"DecodeParms");
            dict.set(
                "ColorSpace",
                Object::Name(if gray {
                    b"DeviceGray".to_vec()
                } else {
                    b"DeviceRGB".to_vec()
                }),
            );
            dict.set("BitsPerComponent", 8i64);
            encoded.content
        }
        EncodedKind::Palette {
            colorspace,
            bpc,
            parms,
        } => {
            dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
            dict.set("DecodeParms", parms);
            dict.set("ColorSpace", colorspace);
            dict.set("BitsPerComponent", bpc as i64);
            encoded.content
        }
    };
    dict.set("Width", new_w as i64);
    dict.set("Height", new_h as i64);
    dict.remove(b"Decode");

    let smask_action = if drop_smask {
        SmaskAction::Drop
    } else if let Some((sid, alpha)) = smask {
        let (aw, ah) = smask_dims.unwrap_or(alpha.dimensions());
        let (cw, chh) = alpha.dimensions();
        let alpha = if new_w < cw || new_h < chh {
            image::imageops::resize(&alpha, new_w, new_h, image::imageops::FilterType::Lanczos3)
        } else {
            alpha
        };
        let (w, h) = alpha.dimensions();
        if (w, h) == (aw, ah) && !x.palette {
            SmaskAction::Keep
        } else {
            let (content, flate, decode_parms) = encode_alpha(alpha.into_raw(), w, x);
            let old_len = job
                .smask
                .as_ref()
                .map(|(_, s)| s.content.len())
                .unwrap_or(0);
            if (w, h) == (aw, ah) && content.len() >= old_len {
                SmaskAction::Keep
            } else {
                SmaskAction::Replace(SmaskUpdate {
                    id: sid,
                    w,
                    h,
                    content,
                    flate,
                    decode_parms,
                })
            }
        }
    } else {
        SmaskAction::Keep
    };

    Some(Plan {
        id: job.id,
        dict,
        content,
        smask: smask_action,
        crop: crop_unit,
    })
}

fn decode_smask(stream: &Stream) -> Option<GrayImage> {
    let w = stream.dict.get(b"Width").and_then(Object::as_i64).ok()? as u32;
    let h = stream.dict.get(b"Height").and_then(Object::as_i64).ok()? as u32;
    if w == 0 || h == 0 {
        return None;
    }
    let bpc = stream
        .dict
        .get(b"BitsPerComponent")
        .and_then(Object::as_i64)
        .unwrap_or(8);
    // A /Decode array would be dropped on rewrite, inverting the mask.
    if bpc != 8 || stream.dict.has(b"Decode") {
        return None;
    }
    let raw = match classify_filter(stream)? {
        FilterKind::RawFlate => stream.decompressed_content().ok()?,
        FilterKind::RawNone => stream.content.clone(),
        FilterKind::Dct | FilterKind::FlateDct => return None,
    };
    if raw.len() as u64 != w as u64 * h as u64 {
        return None;
    }
    GrayImage::from_raw(w, h, raw)
}

/// Soft masks stay lossless (JPEG ringing on alpha edges is very visible).
/// Experimental levels also try a PNG predictor, which helps a lot on the
/// smooth gradients and large flat areas typical of alpha channels.
fn encode_alpha(raw: Vec<u8>, w: u32, x: &Experimental) -> (Vec<u8>, bool, Option<Object>) {
    if !x.palette {
        let (c, f) = flate_encode(raw);
        return (c, f, None);
    }
    let plain = deflate(&raw, x.zopfli);
    let predicted = deflate(&png_filter(&raw, w as usize, 1), x.zopfli);
    if predicted.len() < plain.len() {
        (predicted, true, Some(png_parms(1, 8, w)))
    } else {
        (plain, true, None)
    }
}

#[allow(clippy::large_enum_variant)] // one per image, short-lived
enum EncodedKind {
    Jpeg {
        gray: bool,
    },
    Palette {
        colorspace: Object,
        bpc: u8,
        parms: Object,
    },
}

struct Encoded {
    content: Vec<u8>,
    kind: EncodedKind,
}

/// Picks the encoding: fixed-quality JPEG for regular levels; for the
/// experimental ones, SSIM-driven JPEG quality and, for few-colour images, a
/// lossless palette whenever it isn't much bigger than the JPEG.
///
/// Scanned pages get a fixed quality instead (the middle of the level's
/// range): SSIM rewards reproducing scanner noise and earlier JPEG artifacts,
/// which drove their quality to the maximum for no visible benefit.
fn encode_best(pixels: &Pixels, w: u32, h: u32, profile: &Profile, scan: bool) -> Option<Encoded> {
    let x = &profile.experimental;
    let (raw, gray) = match pixels {
        Pixels::Gray(img) => (img.as_raw(), true),
        Pixels::Rgb(img) => (img.as_raw(), false),
    };
    let jpeg = if scan && x.ssim_target > 0.0 {
        let q =
            (x.min_jpeg_quality.min(profile.jpeg_quality) as u16 + profile.jpeg_quality as u16) / 2;
        encode_jpeg(raw, w, h, gray, q as u8)
    } else if x.ssim_target > 0.0 {
        jpeg_for_ssim(raw, w, h, gray, profile)
    } else {
        encode_jpeg(raw, w, h, gray, profile.jpeg_quality)
    };
    let palette = if x.palette {
        encode_palette(raw, w, h, gray, x.zopfli)
    } else {
        None
    };
    match (jpeg, palette) {
        // Lossless is worth a small premium: no ringing around text/lines.
        (Some(j), Some(p)) if p.content.len() <= j.len() + j.len() / 8 => Some(p),
        (Some(j), _) => Some(Encoded {
            content: j,
            kind: EncodedKind::Jpeg { gray },
        }),
        (None, p) => p,
    }
}

/// Lowest JPEG quality in `[min_jpeg_quality, jpeg_quality]` whose SSIM
/// against `raw` is at least `ssim_target` (binary search; SSIM is close to
/// monotonic in quality). Falls back to `jpeg_quality` if even that misses.
fn jpeg_for_ssim(raw: &[u8], w: u32, h: u32, gray: bool, profile: &Profile) -> Option<Vec<u8>> {
    let x = &profile.experimental;
    let (mut lo, mut hi) = (
        x.min_jpeg_quality.min(profile.jpeg_quality),
        profile.jpeg_quality,
    );
    let mut best = encode_jpeg(raw, w, h, gray, hi)?;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let candidate = encode_jpeg(raw, w, h, gray, mid)?;
        let decoded =
            image::load_from_memory_with_format(&candidate, image::ImageFormat::Jpeg).ok()?;
        let score = if gray {
            ssim(raw, decoded.into_luma8().as_raw(), w, h, 1)
        } else {
            let decoded = decoded.into_rgb8();
            // Luma drives the score; the worst RGB channel only acts as a
            // guard (with some slack) so chroma damage — a thin coloured line
            // smeared by 4:2:0 subsampling — still vetoes a quality. Using the
            // worst channel outright made scan noise push quality to the max.
            let luma = ssim(&to_luma(raw), &to_luma(decoded.as_raw()), w, h, 1);
            let worst = ssim(raw, decoded.as_raw(), w, h, 3);
            luma.min(worst + CHROMA_SLACK)
        };
        if std::env::var_os("PDFSHRINK_DEBUG").is_some() {
            eprintln!("  {w}x{h} q={mid} ssim={score:.4}");
        }
        if score >= x.ssim_target {
            best = candidate;
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Some(best)
}

/// Slack given to the worst-channel SSIM when it guards the luma SSIM.
const CHROMA_SLACK: f32 = 0.03;

/// BT.601 luma of packed RGB samples.
fn to_luma(rgb: &[u8]) -> Vec<u8> {
    rgb.as_chunks::<3>()
        .0
        .iter()
        .map(|p| ((299 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32 + 500) / 1000) as u8)
        .collect()
}

/// Mean SSIM over 8×8 windows (stride 4), computed per channel and reduced to
/// the *worst* channel.
fn ssim(a: &[u8], b: &[u8], w: u32, h: u32, channels: usize) -> f32 {
    const WIN: usize = 8;
    const STEP: usize = 4;
    const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
    const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
    let (w, h) = (w as usize, h as usize);
    if w < WIN || h < WIN {
        return 1.0;
    }
    let mut worst = f64::MAX;
    for c in 0..channels {
        let mut total = 0.0f64;
        let mut count = 0usize;
        let mut y = 0;
        while y + WIN <= h {
            let mut xx = 0;
            while xx + WIN <= w {
                let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0u64, 0u64, 0u64, 0u64, 0u64);
                for j in 0..WIN {
                    let row = ((y + j) * w + xx) * channels + c;
                    for i in 0..WIN {
                        let pa = a[row + i * channels] as u64;
                        let pb = b[row + i * channels] as u64;
                        sa += pa;
                        sb += pb;
                        saa += pa * pa;
                        sbb += pb * pb;
                        sab += pa * pb;
                    }
                }
                let n = (WIN * WIN) as f64;
                let (ma, mb) = (sa as f64 / n, sb as f64 / n);
                let va = saa as f64 / n - ma * ma;
                let vb = sbb as f64 / n - mb * mb;
                let cov = sab as f64 / n - ma * mb;
                total += ((2.0 * ma * mb + C1) * (2.0 * cov + C2))
                    / ((ma * ma + mb * mb + C1) * (va + vb + C2));
                count += 1;
                xx += STEP;
            }
            y += STEP;
        }
        worst = worst.min(total / count as f64);
    }
    worst as f32
}

/// Scanned-paper detection: most of the image is light, low-saturation
/// "paper" that isn't pure white (a screenshot's background is exactly 255 and
/// doesn't qualify). Returns the level to map to white when cleaning it up:
/// the paper's median tone minus a margin, so paper grain and shading clip to
/// white too.
fn paper_white_point(pixels: &Pixels) -> Option<f32> {
    let (raw, ch): (&[u8], usize) = match pixels {
        Pixels::Gray(i) => (i.as_raw(), 1),
        Pixels::Rgb(i) => (i.as_raw(), 3),
    };
    let mut hist = [0u64; 256];
    let mut total = 0u64;
    for px in raw.chunks_exact(ch) {
        let (l, c) = match *px {
            [g] => (g, 0),
            [r, g, b] => (
                ((r as u16 + g as u16 + b as u16) / 3) as u8,
                r.max(g).max(b) - r.min(g).min(b),
            ),
            _ => return None,
        };
        total += 1;
        if l > 200 && c < 30 {
            hist[l as usize] += 1;
        }
    }
    let paper: u64 = hist.iter().sum();
    if total == 0 || paper * 10 < total * 6 {
        return None;
    }
    let mut acc = 0u64;
    let median = (0..256).find(|&v| {
        acc += hist[v];
        acc * 2 >= paper
    })?;
    if std::env::var_os("PDFSHRINK_DEBUG").is_some() {
        eprintln!(
            "  paper: {:.0}% of pixels, median {median}",
            paper as f64 * 100.0 / total as f64
        );
    }
    // Screenshots and digital pages have a background at 255 exactly (254
    // after a lossy round trip); scanned paper sits a few levels lower.
    if median >= 254 {
        return None;
    }
    Some((median as f32 - 20.0).max(200.0))
}

/// Scanned-paper cleanup: linear levels stretch mapping `white_point` to
/// white. JPEG then stops spending bits on paper grain and shading (~25 %
/// smaller on a 150 dpi exam scan). Linear rather than a threshold, so faint
/// pencil strokes get lighter but never vanish.
fn whiten(pixels: &mut Pixels, white_point: f32) {
    let lut: Vec<u8> = (0..256)
        .map(|v| (v as f32 * 255.0 / white_point).round().min(255.0) as u8)
        .collect();
    let raw: &mut [u8] = match pixels {
        Pixels::Gray(i) => i.as_mut(),
        Pixels::Rgb(i) => i.as_mut(),
    };
    for v in raw.iter_mut() {
        *v = lut[*v as usize];
    }
}

/// True when every pixel's channels are within a couple of levels of each
/// other: the image is gray stored as RGB (common for screenshots/scans).
fn is_near_gray(img: &RgbImage) -> bool {
    img.pixels().all(|p| {
        let [r, g, b] = p.0;
        let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
        mx - mn <= 2
    })
}

/// Lossless `Indexed` encoding for images with at most 256 distinct colours,
/// packed at 1/2/4/8 bits per index, PNG-predicted and deflated.
fn encode_palette(raw: &[u8], w: u32, h: u32, gray: bool, zopfli: bool) -> Option<Encoded> {
    let ch = if gray { 1 } else { 3 };
    let mut index: HashMap<[u8; 3], u8> = HashMap::new();
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut indices = Vec::with_capacity((w * h) as usize);
    for px in raw.chunks_exact(ch) {
        let key = if gray {
            [px[0], 0, 0]
        } else {
            [px[0], px[1], px[2]]
        };
        let i = match index.get(&key) {
            Some(i) => *i,
            None => {
                if palette.len() == 256 {
                    return None;
                }
                let i = palette.len() as u8;
                index.insert(key, i);
                palette.push(key);
                i
            }
        };
        indices.push(i);
    }
    let bpc: u8 = match palette.len() {
        0..=2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        _ => 8,
    };
    let (w, h) = (w as usize, h as usize);
    let row_bytes = (w * bpc as usize).div_ceil(8);
    let mut packed = vec![0u8; row_bytes * h];
    for y in 0..h {
        for x in 0..w {
            let v = indices[y * w + x];
            let bit = x * bpc as usize;
            packed[y * row_bytes + bit / 8] |= v << (8 - bpc as usize - bit % 8);
        }
    }
    // PNG filters operate on bytes, with "bytes per pixel" = 1 for < 8 bpc.
    let filtered = png_filter(&packed, row_bytes, 1);
    let content = deflate(&filtered, zopfli);

    let base = if gray { "DeviceGray" } else { "DeviceRGB" };
    let table: Vec<u8> = palette.iter().flat_map(|c| c[..ch].to_vec()).collect();
    let colorspace = Object::Array(vec![
        Object::Name(b"Indexed".to_vec()),
        Object::Name(base.as_bytes().to_vec()),
        Object::Integer(palette.len() as i64 - 1),
        Object::String(table, StringFormat::Hexadecimal),
    ]);
    Some(Encoded {
        content,
        kind: EncodedKind::Palette {
            colorspace,
            bpc,
            parms: png_parms(1, bpc, w as u32),
        },
    })
}

fn png_parms(colors: i64, bpc: u8, columns: u32) -> Object {
    let mut d = Dictionary::new();
    d.set("Predictor", 15i64);
    d.set("Colors", colors);
    d.set("BitsPerComponent", bpc as i64);
    d.set("Columns", columns as i64);
    Object::Dictionary(d)
}

/// Applies PNG row filters (the PDF `/Predictor 15` format: one filter-type
/// byte per row), picking per row the filter with the smallest sum of
/// absolute signed residuals — libpng's standard heuristic.
fn png_filter(data: &[u8], row_bytes: usize, bpp: usize) -> Vec<u8> {
    let rows = data.len() / row_bytes;
    let mut out = Vec::with_capacity(rows * (row_bytes + 1));
    let zero = vec![0u8; row_bytes];
    let mut cand: [Vec<u8>; 5] = Default::default();
    for y in 0..rows {
        let cur = &data[y * row_bytes..(y + 1) * row_bytes];
        let prev = if y == 0 {
            &zero[..]
        } else {
            &data[(y - 1) * row_bytes..y * row_bytes]
        };
        for (t, c) in cand.iter_mut().enumerate() {
            c.clear();
            for i in 0..row_bytes {
                let a = if i >= bpp { cur[i - bpp] } else { 0 };
                let b = prev[i];
                let cc = if i >= bpp { prev[i - bpp] } else { 0 };
                let pred = match t {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((a as u16 + b as u16) / 2) as u8,
                    _ => paeth(a, b, cc),
                };
                c.push(cur[i].wrapping_sub(pred));
            }
        }
        let best = (0..5)
            .min_by_key(|&t| {
                cand[t]
                    .iter()
                    .map(|&v| (v as i8).unsigned_abs() as u64)
                    .sum::<u64>()
            })
            .unwrap();
        out.push(best as u8);
        out.extend_from_slice(&cand[best]);
    }
    out
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = (
        (p - a as i16).abs(),
        (p - b as i16).abs(),
        (p - c as i16).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn deflate(data: &[u8], zopfli: bool) -> Vec<u8> {
    // Zopfli's cost on multi-megabyte bitmaps isn't worth its few percent.
    if zopfli
        && data.len() <= 512 * 1024
        && let Some(z) = crate::zopfli_pass::zopfli_zlib(data)
    {
        return z;
    }
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    let _ = enc.write_all(data);
    enc.finish().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_via_lopdf(content: Vec<u8>, parms: Object) -> Vec<u8> {
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        dict.set("DecodeParms", parms);
        Stream::new(dict, content).decompressed_content().unwrap()
    }

    #[test]
    fn png_filter_round_trips_through_lopdf() {
        let (w, h) = (37usize, 11usize);
        let raw: Vec<u8> = (0..w * h * 3).map(|i| ((i * 31) ^ (i / 7)) as u8).collect();
        let filtered = png_filter(&raw, w * 3, 3);
        let decoded = decode_via_lopdf(deflate(&filtered, false), png_parms(3, 8, w as u32));
        assert_eq!(decoded, raw);
    }

    #[test]
    fn palette_encoding_is_lossless() {
        // 5 colours -> packed at 4 bits per index, odd width to exercise row padding.
        let colors = [
            [0, 0, 0],
            [255, 0, 0],
            [0, 255, 0],
            [10, 20, 30],
            [255, 255, 255],
        ];
        let (w, h) = (13u32, 7u32);
        let raw: Vec<u8> = (0..w * h)
            .flat_map(|i| colors[(i as usize * 7 / 3) % 5])
            .collect();
        let enc = encode_palette(&raw, w, h, false, false).unwrap();
        let EncodedKind::Palette {
            colorspace,
            bpc,
            parms,
        } = enc.kind
        else {
            panic!("expected a palette encoding")
        };
        assert_eq!(bpc, 4);
        let table = match &colorspace.as_array().unwrap()[3] {
            Object::String(t, _) => t.clone(),
            _ => panic!(),
        };
        let packed = decode_via_lopdf(enc.content, parms);
        let row_bytes = (w as usize * 4).div_ceil(8);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let byte = packed[y * row_bytes + x / 2];
                let idx = if x % 2 == 0 { byte >> 4 } else { byte & 0x0f } as usize;
                let px = &raw[(y * w as usize + x) * 3..][..3];
                assert_eq!(&table[idx * 3..idx * 3 + 3], px);
            }
        }
    }

    #[test]
    fn palette_gives_up_beyond_256_colours() {
        let raw: Vec<u8> = (0..300u32)
            .flat_map(|i| [(i % 256) as u8, (i / 256) as u8, 0])
            .collect();
        assert!(encode_palette(&raw, 300, 1, false, false).is_none());
    }

    #[test]
    fn ssim_is_one_for_identical_and_drops_with_noise() {
        let (w, h) = (64u32, 48u32);
        let a: Vec<u8> = (0..w * h).map(|i| ((i % w) * 4) as u8).collect();
        assert!((ssim(&a, &a, w, h, 1) - 1.0).abs() < 1e-6);
        let b: Vec<u8> = a
            .iter()
            .enumerate()
            .map(|(i, &v)| v.wrapping_add(if i % 3 == 0 { 40 } else { 0 }))
            .collect();
        assert!(ssim(&a, &b, w, h, 1) < 0.9);
    }

    #[test]
    fn near_gray_detection() {
        let gray = RgbImage::from_fn(8, 8, |x, _| {
            image::Rgb([x as u8 * 10, x as u8 * 10 + 1, x as u8 * 10])
        });
        assert!(is_near_gray(&gray));
        let color = RgbImage::from_fn(8, 8, |x, _| image::Rgb([x as u8 * 10, 0, 0]));
        assert!(!is_near_gray(&color));
    }
}
