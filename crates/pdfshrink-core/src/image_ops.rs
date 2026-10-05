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
//! Depending on the level (see [`Profile`]), per image:
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

use image::{GrayImage, RgbImage};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};

use crate::engine::ImageFidelity;
use crate::image_codecs;
use crate::jpeg::{self, JpegColor};
use crate::level::Profile;
use crate::placement::{self, Placement};

/// Downsample/re-encode every eligible image in `doc` according to `profile`.
/// Returns `(images_resampled, images_skipped, fidelity)`, the last one only
/// with `Profile::measure_fidelity`.
pub fn resample_images(
    doc: &mut Document,
    profile: &Profile,
) -> (usize, usize, Option<ImageFidelity>) {
    let placements = placement::compute_image_placement(doc);
    let fallback_pt = fallback_page_size_pt(doc);
    let smask_ids = collect_smask_ids(doc);
    let refcounts = if profile.crop && placements.complete {
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

    let plans: Vec<Plan> = crate::par::filter_map(jobs, |job| plan_one(job, profile, fallback_pt));

    let resampled = plans.len();
    let fidelity = ImageFidelity::from_scores(plans.iter().filter_map(|p| p.fidelity));
    // Soft masks can be shared by several images (after dedup); resize each
    // once, to the largest size any of its parents asked for.
    let mut smask_updates: HashMap<ObjectId, SmaskUpdate> = HashMap::new();
    let mut new_smasks: Vec<(ObjectId, SmaskUpdate)> = Vec::new();
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
            SmaskAction::Create(u) => new_smasks.push((target, u)),
        }
    }
    for (image_id, u) in new_smasks {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        dict.set("Subtype", Object::Name(b"Image".to_vec()));
        let mask_id = doc.add_object(Stream::new(dict, Vec::new()));
        smask_updates.insert(mask_id, SmaskUpdate { id: mask_id, ..u });
        if let Some(Object::Stream(s)) = doc.objects.get_mut(&image_id) {
            s.dict.set("SMask", Object::Reference(mask_id));
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

    (resampled, candidates - resampled, fidelity)
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
    /// With `Profile::measure_fidelity`: luma SSIM of the result against the
    /// source at the source's size, and the image's area on the page (weight).
    fidelity: Option<(f32, f64)>,
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
    /// New soft mask (from a JPEG 2000's own alpha); `id` is unused.
    Create(SmaskUpdate),
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

/// Image XObjects, stencil masks included (those only ever take the
/// lossless bi-level path).
fn is_candidate_image(stream: &Stream) -> bool {
    stream
        .dict
        .get(b"Subtype")
        .and_then(Object::as_name)
        .map(|n| n == b"Image")
        .unwrap_or(false)
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

/// Colour spaces the engine can decode.
#[derive(Clone)]
enum ColorKind {
    Gray,
    Rgb,
    Cmyk,
    /// `[/Indexed base hival lookup]`: `lut` holds `hival + 1` entries of
    /// `base`'s component count.
    Indexed {
        base: Base,
        lut: Vec<u8>,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum Base {
    Gray,
    Rgb,
    Cmyk,
}

impl Base {
    fn channels(self) -> usize {
        match self {
            Base::Gray => 1,
            Base::Rgb => 3,
            Base::Cmyk => 4,
        }
    }
}

enum FilterKind {
    Dct,
    /// JPEG wrapped in an extra Flate layer (`[/FlateDecode /DCTDecode]`),
    /// as iLovePDF writes them.
    FlateDct,
    Jpx,
    /// CCITT Group 4 (only taken by the bi-level path).
    CcittG4,
    RawFlate,
    RawNone,
}

fn classify_filter(stream: &Stream) -> Option<FilterKind> {
    match stream.filters() {
        Err(_) => Some(FilterKind::RawNone),
        Ok(f) if f.is_empty() => Some(FilterKind::RawNone),
        Ok(f) if f.len() == 1 && f[0] == b"DCTDecode" => Some(FilterKind::Dct),
        Ok(f) if f.len() == 1 && f[0] == b"JPXDecode" => Some(FilterKind::Jpx),
        Ok(f) if f.len() == 1 && f[0] == b"CCITTFaxDecode" => {
            // Group 4 only (K < 0), without byte-aligned rows.
            let p = stream
                .dict
                .get(b"DecodeParms")
                .and_then(Object::as_dict)
                .ok();
            let k = p
                .and_then(|p| p.get(b"K").and_then(Object::as_i64).ok())
                .unwrap_or(0);
            let aligned = p
                .and_then(|p| p.get(b"EncodedByteAlign").and_then(Object::as_bool).ok())
                .unwrap_or(false);
            (k < 0 && !aligned).then_some(FilterKind::CcittG4)
        }
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

fn base_of(doc: &Document, cs: &Object) -> Option<Base> {
    let resolved: &Object = match cs {
        Object::Reference(rid) => doc.get_object(*rid).ok()?,
        other => other,
    };
    match resolved {
        Object::Name(name) => match name.as_slice() {
            b"DeviceGray" | b"CalGray" | b"G" => Some(Base::Gray),
            b"DeviceRGB" | b"CalRGB" | b"RGB" => Some(Base::Rgb),
            b"DeviceCMYK" | b"CMYK" => Some(Base::Cmyk),
            _ => None,
        },
        Object::Array(arr) => {
            let first = arr.first()?.as_name().ok()?;
            match first {
                b"ICCBased" => {
                    let icc_id = arr.get(1)?.as_reference().ok()?;
                    let icc_stream = doc.get_object(icc_id).ok()?.as_stream().ok()?;
                    match icc_stream.dict.get(b"N").and_then(Object::as_i64).ok()? {
                        1 => Some(Base::Gray),
                        3 => Some(Base::Rgb),
                        4 => Some(Base::Cmyk),
                        _ => None,
                    }
                }
                b"CalGray" => Some(Base::Gray),
                b"CalRGB" => Some(Base::Rgb),
                _ => None,
            }
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
    if let Object::Array(arr) = resolved
        && matches!(
            arr.first().and_then(|o| o.as_name().ok()),
            Some(b"Indexed" | b"I")
        )
    {
        let base = base_of(doc, arr.get(1)?)?;
        let hival = arr.get(2)?.as_i64().ok()?.clamp(0, 255) as usize;
        let lut = match arr.get(3)? {
            Object::String(bytes, _) => bytes.clone(),
            Object::Reference(r) => {
                let s = doc.get_object(*r).ok()?.as_stream().ok()?;
                s.decompressed_content()
                    .ok()
                    .unwrap_or_else(|| s.content.clone())
            }
            _ => return None,
        };
        if lut.len() < (hival + 1) * base.channels() {
            return None;
        }
        return Some(ColorKind::Indexed { base, lut });
    }
    Some(match base_of(doc, cs_obj)? {
        Base::Gray => ColorKind::Gray,
        Base::Rgb => ColorKind::Rgb,
        Base::Cmyk => ColorKind::Cmyk,
    })
}

/// 4-channel CMYK pixels (the `image` crate has no CMYK buffer type).
struct CmykImage {
    w: u32,
    h: u32,
    data: Vec<u8>,
}

impl CmykImage {
    fn channel(&self, c: usize) -> GrayImage {
        let plane = self.data.as_chunks::<4>().0.iter().map(|p| p[c]).collect();
        GrayImage::from_raw(self.w, self.h, plane).expect("plane size")
    }

    fn from_channels(planes: [GrayImage; 4]) -> CmykImage {
        let (w, h) = planes[0].dimensions();
        let mut data = Vec::with_capacity(w as usize * h as usize * 4);
        for i in 0..(w * h) as usize {
            for p in &planes {
                data.push(p.as_raw()[i]);
            }
        }
        CmykImage { w, h, data }
    }

    fn crop(&self, x0: u32, y0: u32, cw: u32, ch: u32) -> CmykImage {
        let mut data = Vec::with_capacity(cw as usize * ch as usize * 4);
        for y in y0..y0 + ch {
            let start = (y as usize * self.w as usize + x0 as usize) * 4;
            data.extend_from_slice(&self.data[start..start + cw as usize * 4]);
        }
        CmykImage { w: cw, h: ch, data }
    }

    /// Resized channel by channel (no colour mixing between channels).
    fn resize(&self, w: u32, h: u32) -> CmykImage {
        let planes = [0, 1, 2, 3].map(|c| {
            image::imageops::resize(
                &self.channel(c),
                w,
                h,
                image::imageops::FilterType::Lanczos3,
            )
        });
        CmykImage::from_channels(planes)
    }
}

enum Pixels {
    Gray(GrayImage),
    Rgb(RgbImage),
    Cmyk(CmykImage),
}

/// A decoded image plus what the re-encoding must know about its source.
struct Decoded {
    pixels: Pixels,
    /// CMYK only: the pixels are the source JPEG's *stored* samples (Adobe
    /// marker, possibly inverted), so the source `/Decode` must be kept
    /// as-is. Otherwise CMYK pixels are stored inverted with
    /// `/Decode [1 0 1 0 1 0 1 0]` (the Photoshop/img2pdf convention).
    cmyk_passthrough: bool,
    /// Came from a palette image: a lossless palette re-encoding is always
    /// worth trying (it can't have more colours unless resized).
    from_palette: bool,
    /// JPEG 2000 alpha channel, for `/SMaskInData`.
    jpx_alpha: Option<GrayImage>,
}

fn bpc_of(stream: &Stream) -> i64 {
    stream
        .dict
        .get(b"BitsPerComponent")
        .and_then(Object::as_i64)
        .unwrap_or(8)
}

fn invert(mut v: Vec<u8>) -> Vec<u8> {
    for b in v.iter_mut() {
        *b = 255 - *b;
    }
    v
}

fn pixels_from(base: Base, w: u32, h: u32, data: Vec<u8>) -> Option<Pixels> {
    if data.len() != w as usize * h as usize * base.channels() {
        return None;
    }
    Some(match base {
        Base::Gray => Pixels::Gray(GrayImage::from_raw(w, h, data)?),
        Base::Rgb => Pixels::Rgb(RgbImage::from_raw(w, h, data)?),
        // Non-passthrough CMYK is stored inverted (see `Decoded`).
        Base::Cmyk => Pixels::Cmyk(CmykImage {
            w,
            h,
            data: invert(data),
        }),
    })
}

fn decode_pixels(
    stream: &Stream,
    color: Option<&ColorKind>,
    filter: &FilterKind,
    w: u32,
    h: u32,
) -> Option<Decoded> {
    let plain = |pixels| Decoded {
        pixels,
        cmyk_passthrough: false,
        from_palette: false,
        jpx_alpha: None,
    };
    match filter {
        FilterKind::Dct | FilterKind::FlateDct => {
            let jpeg = jpeg_bytes(stream, filter)?;
            match color? {
                ColorKind::Cmyk => {
                    let (dw, dh, data) = jpeg::decode_cmyk(&jpeg)?;
                    if (dw, dh) != (w, h) {
                        return None;
                    }
                    if image_codecs::jpeg_has_adobe_marker(&jpeg) {
                        Some(Decoded {
                            pixels: Pixels::Cmyk(CmykImage { w, h, data }),
                            cmyk_passthrough: true,
                            from_palette: false,
                            jpx_alpha: None,
                        })
                    } else if stream.dict.has(b"Decode") {
                        None
                    } else {
                        Some(plain(Pixels::Cmyk(CmykImage {
                            w,
                            h,
                            data: invert(data),
                        })))
                    }
                }
                ColorKind::Gray | ColorKind::Rgb => {
                    // Decoded like the re-encoded candidates (see `jpeg.rs`),
                    // not through `image`'s zune-jpeg, which misreads some
                    // valid baseline files.
                    let (dw, dh, decoded, data) = jpeg::decode(&jpeg)?;
                    Some(plain(match (color?, decoded) {
                        (ColorKind::Gray, JpegColor::Gray) => {
                            Pixels::Gray(GrayImage::from_raw(dw, dh, data)?)
                        }
                        (ColorKind::Gray, JpegColor::Rgb) => {
                            Pixels::Gray(GrayImage::from_raw(dw, dh, to_luma(&data))?)
                        }
                        (ColorKind::Rgb, JpegColor::Rgb) => {
                            Pixels::Rgb(RgbImage::from_raw(dw, dh, data)?)
                        }
                        (ColorKind::Rgb, JpegColor::Gray) => Pixels::Rgb(RgbImage::from_raw(
                            dw,
                            dh,
                            data.iter().flat_map(|&v| [v, v, v]).collect(),
                        )?),
                        // A 4-channel JPEG under a gray/RGB colour space.
                        _ => return None,
                    }))
                }
                ColorKind::Indexed { .. } => None,
            }
        }
        FilterKind::CcittG4 => None,
        FilterKind::Jpx => {
            // A /Decode array or an explicit palette colour space would need
            // handling this decoder doesn't do.
            if stream.dict.has(b"Decode") || matches!(color, Some(ColorKind::Indexed { .. })) {
                return None;
            }
            let jpx = image_codecs::decode_jpx(&stream.content)?;
            if (jpx.width, jpx.height) != (w, h) {
                return None;
            }
            let base = match jpx.channels {
                1 => Base::Gray,
                3 => Base::Rgb,
                _ => Base::Cmyk,
            };
            // An explicit /ColorSpace must agree on the component count.
            let declared = match color {
                None => None,
                Some(ColorKind::Gray) => Some(Base::Gray),
                Some(ColorKind::Rgb) => Some(Base::Rgb),
                Some(ColorKind::Cmyk) => Some(Base::Cmyk),
                Some(ColorKind::Indexed { .. }) => return None,
            };
            if declared.is_some_and(|d| d != base) {
                return None;
            }
            let jpx_alpha = jpx.alpha.and_then(|a| GrayImage::from_raw(w, h, a));
            let mut d = plain(pixels_from(base, w, h, jpx.data)?);
            d.jpx_alpha = jpx_alpha;
            Some(d)
        }
        FilterKind::RawFlate | FilterKind::RawNone => {
            let bpc = bpc_of(stream);
            let raw = match filter {
                FilterKind::RawFlate => stream.decompressed_content().ok()?,
                _ => stream.content.clone(),
            };
            if stream.dict.has(b"Decode") && !matches!(color?, ColorKind::Gray | ColorKind::Rgb) {
                return None;
            }
            match color? {
                ColorKind::Indexed { base, lut } => {
                    let idx =
                        image_codecs::unpack_samples(&raw, w as usize, h as usize, bpc as u8)?;
                    let n = base.channels();
                    let hival = lut.len() / n - 1;
                    let mut data = Vec::with_capacity(idx.len() * n);
                    for i in idx {
                        let e = (i as usize).min(hival) * n;
                        data.extend_from_slice(&lut[e..e + n]);
                    }
                    let mut d = plain(pixels_from(*base, w, h, data)?);
                    d.from_palette = true;
                    Some(d)
                }
                c => {
                    let base = match c {
                        ColorKind::Gray => Base::Gray,
                        ColorKind::Rgb => Base::Rgb,
                        _ => Base::Cmyk,
                    };
                    let raw = match bpc {
                        8 => raw,
                        16 => image_codecs::samples_16_to_8(&raw),
                        _ => return None,
                    };
                    let expected = w as usize * h as usize * base.channels();
                    if raw.len() < expected {
                        return None;
                    }
                    let mut raw = raw;
                    raw.truncate(expected);
                    Some(plain(pixels_from(base, w, h, raw)?))
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
/// page-relative `page_px` cap is a third one. Whichever wants the smallest
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

    let page_px = profile.page_px as f32;
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
    let stream = &job.stream;
    let width = stream.dict.get(b"Width").and_then(Object::as_i64).ok()? as u32;
    let height = stream.dict.get(b"Height").and_then(Object::as_i64).ok()? as u32;
    if width == 0 || height == 0 {
        return None;
    }
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
    if let Some(plan) = plan_bilevel(&job, &filter, width, height, placement, profile) {
        return plan;
    }
    // A colour space is required, except for JPEG 2000 (it can carry its own).
    if job.color.is_none() && !matches!(filter, FilterKind::Jpx) {
        return None;
    }
    let original_len = stream.content.len();
    // A scanned page: one image spanning (nearly) the full page width.
    let covers_page = job.placement.is_some() && width as f32 >= 0.8 * placement.page_px;

    let Decoded {
        pixels,
        cmyk_passthrough,
        from_palette,
        jpx_alpha,
    } = decode_pixels(stream, job.color.as_ref(), &filter, width, height)?;

    // A soft mask with `/Matte` says the parent's colours were pre-blended
    // with that colour, given in the parent's colour space: the parent must
    // keep the same kind of colour space (no gray or palette conversion), or
    // the matte would no longer match it. The mask may be shared by other
    // parents, so it can't be rewritten for this one instead.
    let keep_color_space = job
        .smask
        .as_ref()
        .is_some_and(|(_, s)| s.dict.has(b"Matte"));
    if keep_color_space && from_palette {
        return None;
    }

    // Opaque soft masks are dropped before anything else: the parent then
    // needs no mask handling at all. A JPEG 2000 image's own alpha channel
    // stands in for a soft mask when `/SMaskInData` asks for it.
    let smask_in_data = stream
        .dict
        .get(b"SMaskInData")
        .and_then(Object::as_i64)
        .unwrap_or(0)
        != 0;
    let mut smask = job.smask.as_ref().and_then(|(sid, s)| {
        let alpha = decode_smask(s)?;
        Some((Some(*sid), alpha))
    });
    if job.smask.is_none()
        && smask_in_data
        && let Some(alpha) = jpx_alpha
    {
        smask = Some((None, alpha));
    }
    let smask_dims = smask.as_ref().map(|(_, a)| a.dimensions());
    let drop_smask = profile.drop_opaque_smask
        && smask
            .as_ref()
            .is_some_and(|(_, a)| a.pixels().all(|p| p.0[0] == 255));

    // Cropping to the visible part (only with a mask we can crop identically).
    let crop = (profile.crop && job.crop_ok)
        .then(|| crop_rect(placement.visible, width, height))
        .flatten()
        .filter(|_| drop_smask || smask.is_none() || smask_dims == Some((width, height)));
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
                Pixels::Cmyk(i) => Pixels::Cmyk(i.crop(x0, y0, cw, ch)),
            };
            (cropped, cw, ch, Some(unit))
        }
    };

    let (new_w, new_h) = target_dims(width, height, placement, profile);
    let needs_resize = (new_w, new_h) != (width, height);

    // Fidelity is measured against the decoded source as drawn (after the
    // crop, before whitening), luma only.
    let reference = profile.measure_fidelity.then(|| luma_of(&pixels)).flatten();

    let mut pixels = pixels;
    let paper = if covers_page {
        paper_white_point(&pixels)
    } else {
        None
    };
    if let (Some(white_point), true) = (paper, profile.scan_whiten) {
        whiten(&mut pixels, white_point);
    }
    let scan = paper.is_some();
    let pixels = match pixels {
        Pixels::Rgb(img) if profile.detect_gray && !keep_color_space && is_near_gray(&img) => {
            Pixels::Gray(image::DynamicImage::ImageRgb8(img).into_luma8())
        }
        p => p,
    };

    // Few-colour images can lose more than they gain from resampling: the
    // new edge pixels add colours, so the lossless palette no longer applies
    // and a JPEG of the smaller image can outweigh the full-size lossless
    // one. Also tried, keeping the smallest: black-and-white content stored
    // as 8-bit gray (a common way to save scans) as CCITT G4 at the
    // bi-level resolution, and any other ≤256-colour image as a palette at
    // its own resolution. A soft mask still follows the downsampled size
    // (it needn't match its image's), so the comparison is image to image.
    let alternative = if keep_color_space || !profile.palette {
        None
    } else {
        match &pixels {
            Pixels::Gray(img) if is_black_and_white(img) => {
                encode_black_and_white(img, placement, profile)
            }
            Pixels::Gray(img) if needs_resize => {
                encode_palette(img.as_raw(), width, height, true, false).map(|e| (e, width, height))
            }
            Pixels::Rgb(img) if needs_resize => {
                encode_palette(img.as_raw(), width, height, false, false)
                    .map(|e| (e, width, height))
            }
            _ => None,
        }
    };
    // What a native-resolution palette would show (lossless, but possibly
    // whitened); a CCITT alternative is decoded back instead.
    let native_luma = (reference.is_some()
        && matches!(
            alternative,
            Some((
                Encoded {
                    kind: EncodedKind::Palette { .. },
                    ..
                },
                _,
                _
            ))
        ))
    .then(|| luma_of(&pixels))
    .flatten();

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
        Pixels::Cmyk(img) if needs_resize => Pixels::Cmyk(img.resize(new_w, new_h)),
        p => p,
    };

    let mut dict = stream.dict.clone();
    // Whatever the source was, the output isn't JPEG 2000 any more.
    let was_jpx = matches!(filter, FilterKind::Jpx);
    dict.remove(b"SMaskInData");
    let encoded = encode_best(
        &resized,
        new_w,
        new_h,
        profile,
        scan,
        from_palette,
        !keep_color_space,
    )?;
    let (mask_w, mask_h) = (new_w, new_h);
    let (encoded, new_w, new_h, alt_chosen) = match alternative {
        Some((alt, w, h)) if alt.content.len() < encoded.content.len() => (alt, w, h, true),
        _ => (encoded, new_w, new_h, false),
    };
    let fidelity = reference.as_deref().and_then(|r| {
        let shown = match &encoded.kind {
            EncodedKind::Palette { .. } if alt_chosen => native_luma?,
            EncodedKind::Palette { .. } => luma_of(&resized)?,
            kind => decoded_luma(&encoded.content, kind, new_w, new_h)?,
        };
        let score = luma_fidelity(r, width, height, shown, new_w, new_h, placement.dpi);
        if std::env::var_os("PDFSHRINK_DEBUG").is_some() {
            let kind = match &encoded.kind {
                EncodedKind::Jpeg { .. } => "jpeg",
                EncodedKind::Palette { .. } => "palette",
                EncodedKind::Ccitt { .. } => "ccitt",
            };
            eprintln!(
                "  fidelity {:?} {width}x{height} -> {new_w}x{new_h} {kind}: {score:.3}",
                job.id
            );
        }
        Some((score, on_page_area(width, height, placement)))
    });
    let smask_created = matches!(smask, Some((None, _)));
    if encoded.content.len() >= original_len
        && !drop_smask
        && crop_unit.is_none()
        && !(was_jpx && smask_created)
    {
        return None;
    }
    let content = match encoded.kind {
        EncodedKind::Jpeg { color } => {
            dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
            dict.remove(b"DecodeParms");
            match color {
                JpegColor::Gray | JpegColor::Rgb => {
                    dict.set(
                        "ColorSpace",
                        Object::Name(if color == JpegColor::Gray {
                            b"DeviceGray".to_vec()
                        } else {
                            b"DeviceRGB".to_vec()
                        }),
                    );
                    dict.remove(b"Decode");
                }
                JpegColor::Cmyk => {
                    // Keep a CMYK (Device or ICC) colour space as it was;
                    // palette or JPX sources get DeviceCMYK.
                    let keep_cs = !from_palette && dict.has(b"ColorSpace");
                    if !keep_cs {
                        dict.set("ColorSpace", Object::Name(b"DeviceCMYK".to_vec()));
                    }
                    if !cmyk_passthrough {
                        dict.set(
                            "Decode",
                            Object::Array([1, 0, 1, 0, 1, 0, 1, 0].map(Object::Integer).to_vec()),
                        );
                    }
                }
            }
            dict.set("BitsPerComponent", 8i64);
            encoded.content
        }
        EncodedKind::Ccitt { parms } => {
            dict.set("Filter", Object::Name(b"CCITTFaxDecode".to_vec()));
            dict.set("DecodeParms", parms);
            dict.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
            dict.set("BitsPerComponent", 1i64);
            dict.remove(b"Decode");
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
            dict.remove(b"Decode");
            encoded.content
        }
    };
    dict.set("Width", new_w as i64);
    dict.set("Height", new_h as i64);

    let smask_action = if drop_smask {
        SmaskAction::Drop
    } else if let Some((sid, alpha)) = smask {
        let (aw, ah) = smask_dims.unwrap_or(alpha.dimensions());
        let (cw, chh) = alpha.dimensions();
        let alpha = if mask_w < cw || mask_h < chh {
            image::imageops::resize(
                &alpha,
                mask_w,
                mask_h,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            alpha
        };
        let (w, h) = alpha.dimensions();
        match sid {
            // Alpha that was inside the JPEG 2000: becomes a real /SMask.
            None => {
                let (content, flate, decode_parms) = encode_alpha(alpha.into_raw(), w, profile);
                SmaskAction::Create(SmaskUpdate {
                    id: (0, 0),
                    w,
                    h,
                    content,
                    flate,
                    decode_parms,
                })
            }
            Some(sid) if (w, h) == (aw, ah) && !profile.palette => {
                let _ = sid;
                SmaskAction::Keep
            }
            Some(sid) => {
                let (content, flate, decode_parms) = encode_alpha(alpha.into_raw(), w, profile);
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
        fidelity,
    })
}

/// Bi-level images need about twice the resolution of gray/colour ones to
/// stay legible (no anti-aliasing), so their targets are scaled by this.
const BILEVEL_RESOLUTION_FACTOR: f32 = 2.0;

/// 1-bit images (raw, Flate or CCITT G4, stencil masks included):
/// downsampled when over-resolved — to twice the profile's colour targets —
/// and re-encoded as CCITT Group 4, if that's smaller. Without
/// downsampling the re-encoding is lossless. Returns `Some(plan)` when the
/// image is bi-level (whether or not it gained), `None` to fall through to
/// the general path.
fn plan_bilevel(
    job: &Job,
    filter: &FilterKind,
    w: u32,
    h: u32,
    placement: Placement,
    profile: &Profile,
) -> Option<Option<Plan>> {
    let stream = &job.stream;
    let is_mask = stream
        .dict
        .get(b"ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false);
    let one_bit_gray = bpc_of(stream) == 1 && matches!(job.color, Some(ColorKind::Gray));
    if !is_mask && !one_bit_gray {
        return None;
    }
    let parms = stream
        .dict
        .get(b"DecodeParms")
        .and_then(Object::as_dict)
        .ok();
    let raw = match filter {
        FilterKind::RawFlate => stream.decompressed_content().ok(),
        FilterKind::RawNone => Some(stream.content.clone()),
        FilterKind::CcittG4 => {
            let black_is_1 = parms
                .and_then(|p| p.get(b"BlackIs1").and_then(Object::as_bool).ok())
                .unwrap_or(false);
            let columns = parms
                .and_then(|p| p.get(b"Columns").and_then(Object::as_i64).ok())
                .unwrap_or(1728);
            (columns == w as i64)
                .then(|| image_codecs::decode_g4(&stream.content, w, h, black_is_1))
                .flatten()
        }
        // JBIG2 and others: left alone.
        _ => None,
    };
    let Some(raw) = raw else {
        return Some(None);
    };

    let (new_w, new_h) = target_dims(w, h, placement, &bilevel_profile(profile));
    // Downsampling is the only loss here: measured against the source bits.
    let reference =
        (profile.measure_fidelity && (new_w, new_h) != (w, h)).then(|| bits_to_luma(&raw, w, h));
    let bits = if (new_w, new_h) != (w, h) {
        // Which sample value paints: 0 unless a /Decode [1 0] inverts it.
        let inverted = stream
            .dict
            .get(b"Decode")
            .and_then(Object::as_array)
            .ok()
            .and_then(|d| d.first().and_then(|v| v.as_i64().ok()))
            == Some(1);
        image_codecs::downsample_bilevel(&raw, w, h, new_w, new_h, u8::from(inverted))
    } else {
        raw
    };
    let fidelity = profile.measure_fidelity.then(|| {
        let score = match &reference {
            None => 1.0, // lossless
            Some(r) => luma_fidelity(
                r,
                w,
                h,
                bits_to_luma(&bits, new_w, new_h),
                new_w,
                new_h,
                placement.dpi,
            ),
        };
        (score, on_page_area(w, h, placement))
    });
    let plan = image_codecs::encode_g4(&bits, new_w, new_h)
        .filter(|g4| g4.len() < stream.content.len())
        .map(|g4| {
            let mut dict = stream.dict.clone();
            dict.set("Filter", Object::Name(b"CCITTFaxDecode".to_vec()));
            dict.set("DecodeParms", g4_parms(new_w, new_h));
            dict.set("Width", new_w as i64);
            dict.set("Height", new_h as i64);
            Plan {
                id: job.id,
                dict,
                content: g4,
                smask: SmaskAction::Keep,
                crop: None,
                fidelity,
            }
        });
    Some(plan)
}

/// Luma samples of gray or RGB pixels (`None` for CMYK: not measured).
fn luma_of(pixels: &Pixels) -> Option<Vec<u8>> {
    match pixels {
        Pixels::Gray(img) => Some(img.as_raw().clone()),
        Pixels::Rgb(img) => Some(to_luma(img.as_raw())),
        Pixels::Cmyk(_) => None,
    }
}

/// Packed 1-bit rows as 0/255 luma.
fn bits_to_luma(bits: &[u8], w: u32, h: u32) -> Vec<u8> {
    let row = (w as usize).div_ceil(8);
    let mut out = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let bit = (bits[y * row + x / 8] >> (7 - x % 8)) & 1;
            out.push(if bit == 1 { 255 } else { 0 });
        }
    }
    out
}

/// Luma of an encoded JPEG (gray or RGB) or CCITT image, as a viewer decodes it.
fn decoded_luma(content: &[u8], kind: &EncodedKind, w: u32, h: u32) -> Option<Vec<u8>> {
    match kind {
        EncodedKind::Jpeg {
            color: JpegColor::Gray | JpegColor::Rgb,
        } => match jpeg::decode(content)? {
            (.., JpegColor::Gray, gray) => Some(gray),
            (.., JpegColor::Rgb, rgb) => Some(to_luma(&rgb)),
            _ => None,
        },
        EncodedKind::Ccitt { .. } => {
            let bits = image_codecs::decode_g4(content, w, h, false)?;
            Some(bits_to_luma(&bits, w, h))
        }
        _ => None,
    }
}

/// Resolution fidelity is judged at: a Retina screen at 100 % zoom. Detail
/// beyond it (a 600 dpi image downsampled to 150 dpi) doesn't show unless
/// zoomed in, and shouldn't sink the score.
const FIDELITY_DPI: f32 = 144.0;

/// Largest size (pixels) fidelity is computed at.
const FIDELITY_MAX_PX: f32 = 4_000_000.0;

/// SSIM of `shown` (`sw`×`sh`) against `reference` (`w`×`h`, drawn at `dpi`),
/// both brought to the image's size at [`FIDELITY_DPI`] (never above the
/// source's): what downsampling and lossy encoding cost together, as seen.
fn luma_fidelity(
    reference: &[u8],
    w: u32,
    h: u32,
    shown: Vec<u8>,
    sw: u32,
    sh: u32,
    dpi: f32,
) -> f32 {
    let mut scale = if dpi > FIDELITY_DPI {
        FIDELITY_DPI / dpi
    } else {
        1.0
    };
    // Huge images (a 121 Mpx A0 map) would take seconds: judged a bit more
    // zoomed out instead.
    let px = w as f32 * h as f32 * scale * scale;
    if px > FIDELITY_MAX_PX {
        scale *= (FIDELITY_MAX_PX / px).sqrt();
    }
    let (ew, eh) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    let fit = |buf: Vec<u8>, bw: u32, bh: u32| -> Vec<u8> {
        if (bw, bh) == (ew, eh) {
            return buf;
        }
        let mut img = GrayImage::from_raw(bw, bh, buf).expect("luma buffer size");
        if bw > 2 * ew && bh > 2 * eh {
            // A box average down to twice the target first: much faster than
            // a filtered resize on big sources. Not all the way: both sides
            // must end with the same filter, or SSIM measures the filters.
            img = image::imageops::thumbnail(&img, 2 * ew, 2 * eh);
        }
        image::imageops::resize(&img, ew, eh, image::imageops::FilterType::Triangle).into_raw()
    };
    let reference = fit(reference.to_vec(), w, h);
    let shown = fit(shown, sw, sh);
    ssim(&reference, &shown, ew, eh, 1)
}

/// Area the image covers on the page, in square inches (its fidelity's weight).
fn on_page_area(w: u32, h: u32, placement: Placement) -> f64 {
    let dpi = if placement.dpi > 0.0 {
        placement.dpi as f64
    } else {
        72.0
    };
    w as f64 * h as f64 / (dpi * dpi)
}

/// `profile` with its resolution targets scaled for bi-level images.
fn bilevel_profile(profile: &Profile) -> Profile {
    let mut p = *profile;
    p.target_dpi *= BILEVEL_RESOLUTION_FACTOR;
    p.max_dimension = (p.max_dimension as f32 * BILEVEL_RESOLUTION_FACTOR) as u32;
    p.page_px = (p.page_px as f32 * BILEVEL_RESOLUTION_FACTOR) as u32;
    p
}

fn g4_parms(w: u32, h: u32) -> Object {
    let mut parms = Dictionary::new();
    parms.set("K", -1i64);
    parms.set("Columns", w as i64);
    parms.set("Rows", h as i64);
    parms.set("BlackIs1", false);
    Object::Dictionary(parms)
}

fn is_black_and_white(img: &GrayImage) -> bool {
    img.pixels().all(|p| p.0[0] == 0 || p.0[0] == 255)
}

/// A gray image holding only black and white pixels, as 1-bit CCITT G4
/// downsampled (if at all) to the bi-level targets, like a 1-bit source.
fn encode_black_and_white(
    img: &GrayImage,
    placement: Placement,
    profile: &Profile,
) -> Option<(Encoded, u32, u32)> {
    let (w, h) = img.dimensions();
    let row = (w as usize).div_ceil(8);
    let mut bits = vec![0u8; row * h as usize];
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[0] == 255 {
            bits[y as usize * row + x as usize / 8] |= 1 << (7 - x % 8);
        }
    }
    let (nw, nh) = target_dims(w, h, placement, &bilevel_profile(profile));
    if (nw, nh) != (w, h) {
        bits = image_codecs::downsample_bilevel(&bits, w, h, nw, nh, 0);
    }
    let content = image_codecs::encode_g4(&bits, nw, nh)?;
    Some((
        Encoded {
            content,
            kind: EncodedKind::Ccitt {
                parms: g4_parms(nw, nh),
            },
        },
        nw,
        nh,
    ))
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
        FilterKind::Dct | FilterKind::FlateDct | FilterKind::Jpx | FilterKind::CcittG4 => {
            return None;
        }
    };
    if raw.len() as u64 != w as u64 * h as u64 {
        return None;
    }
    GrayImage::from_raw(w, h, raw)
}

/// Soft masks stay lossless (JPEG ringing on alpha edges is very visible).
/// With `palette` on, a PNG predictor is also tried, which helps a lot on the
/// smooth gradients and large flat areas typical of alpha channels.
fn encode_alpha(raw: Vec<u8>, w: u32, profile: &Profile) -> (Vec<u8>, bool, Option<Object>) {
    if !profile.palette {
        let (c, f) = flate_encode(raw);
        return (c, f, None);
    }
    let plain = deflate(&raw, profile.zopfli);
    let predicted = deflate(&png_filter(&raw, w as usize, 1), profile.zopfli);
    if predicted.len() < plain.len() {
        (predicted, true, Some(png_parms(1, 8, w)))
    } else {
        (plain, true, None)
    }
}

#[allow(clippy::large_enum_variant)] // one per image, short-lived
enum EncodedKind {
    Jpeg {
        color: JpegColor,
    },
    Palette {
        colorspace: Object,
        bpc: u8,
        parms: Object,
    },
    /// 1-bit DeviceGray, CCITT Group 4.
    Ccitt {
        parms: Object,
    },
}

struct Encoded {
    content: Vec<u8>,
    kind: EncodedKind,
}

/// Picks the encoding: JPEG at a fixed quality, or at the lowest quality
/// meeting `ssim_target` when set, and, for few-colour images (`palette`), a
/// lossless palette whenever it isn't much bigger than the JPEG
/// (`allow_palette` false: never, the colour space must stay as it is).
///
/// Scanned pages get a fixed quality instead, the middle of the level's
/// `[min_jpeg_quality, jpeg_quality]` range (also at levels without SSIM
/// search): SSIM rewards reproducing scanner noise and earlier JPEG
/// artifacts, which drove their quality to the maximum for no visible
/// benefit.
///
/// Palette sources (`from_palette`) always try the lossless palette: unless
/// resized, they can't have more than 256 colours.
fn encode_best(
    pixels: &Pixels,
    w: u32,
    h: u32,
    profile: &Profile,
    scan: bool,
    from_palette: bool,
    allow_palette: bool,
) -> Option<Encoded> {
    let (raw, color) = match pixels {
        Pixels::Gray(img) => (img.as_raw().as_slice(), JpegColor::Gray),
        Pixels::Rgb(img) => (img.as_raw().as_slice(), JpegColor::Rgb),
        Pixels::Cmyk(img) => (img.data.as_slice(), JpegColor::Cmyk),
    };
    let jpeg = if scan && profile.min_jpeg_quality > 0 {
        let q = (profile.min_jpeg_quality.min(profile.jpeg_quality) as u16
            + profile.jpeg_quality as u16)
            / 2;
        jpeg::encode(profile.jpeg_encoder, raw, w, h, color, q as u8)
    } else if profile.ssim_target > 0.0 {
        jpeg_for_ssim(raw, w, h, color, profile)
    } else {
        jpeg::encode(profile.jpeg_encoder, raw, w, h, color, profile.jpeg_quality)
    };
    let palette = if allow_palette && (profile.palette || from_palette) && color != JpegColor::Cmyk
    {
        encode_palette(raw, w, h, color == JpegColor::Gray, profile.zopfli)
    } else {
        None
    };
    match (jpeg, palette) {
        // Lossless is worth a small premium: no ringing around text/lines.
        (Some(j), Some(p)) if p.content.len() <= j.len() + j.len() / 8 => Some(p),
        (Some(j), _) => Some(Encoded {
            content: j,
            kind: EncodedKind::Jpeg { color },
        }),
        (None, p) => p,
    }
}

/// Lowest JPEG quality in `[min_jpeg_quality, jpeg_quality]` whose SSIM
/// against `raw` is at least `ssim_target` (binary search; SSIM is close to
/// monotonic in quality). Falls back to `jpeg_quality` if even that misses.
fn jpeg_for_ssim(
    raw: &[u8],
    w: u32,
    h: u32,
    color: JpegColor,
    profile: &Profile,
) -> Option<Vec<u8>> {
    let (mut lo, mut hi) = (
        profile.min_jpeg_quality.min(profile.jpeg_quality),
        profile.jpeg_quality,
    );
    let mut best = jpeg::encode(profile.jpeg_encoder, raw, w, h, color, hi)?;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let candidate = jpeg::encode(profile.jpeg_encoder, raw, w, h, color, mid)?;
        let score = match color {
            JpegColor::Gray => {
                let (.., decoded) = jpeg::decode(&candidate)?;
                ssim(raw, &decoded, w, h, 1)
            }
            JpegColor::Rgb => {
                let (.., decoded) = jpeg::decode(&candidate)?;
                // Luma drives the score; the worst RGB channel only acts as a
                // guard (with some slack) so chroma damage — a thin coloured line
                // smeared by 4:2:0 subsampling — still vetoes a quality. Using the
                // worst channel outright made scan noise push quality to the max.
                let luma = ssim(&to_luma(raw), &to_luma(&decoded), w, h, 1);
                let worst = ssim(raw, &decoded, w, h, 3);
                luma.min(worst + CHROMA_SLACK)
            }
            JpegColor::Cmyk => {
                // No luma to lean on: mean of the four channels, with the worst
                // one as a guard.
                let (_, _, decoded) = jpeg::decode_cmyk(&candidate)?;
                let per: Vec<f32> = (0..4)
                    .map(|c| ssim_channel(raw, &decoded, w, h, 4, c))
                    .collect();
                let mean = per.iter().sum::<f32>() / 4.0;
                let worst = per.iter().copied().fold(f32::MAX, f32::min);
                mean.min(worst + CHROMA_SLACK)
            }
        };
        if std::env::var_os("PDFSHRINK_DEBUG").is_some() {
            eprintln!("  {w}x{h} q={mid} ssim={score:.4}");
        }
        if score >= profile.ssim_target {
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
    (0..channels)
        .map(|c| ssim_channel(a, b, w, h, channels, c))
        .fold(f32::MAX, f32::min)
}

/// Mean SSIM over 8×8 windows (stride 4) of channel `c` of interleaved
/// `channels`-channel samples.
fn ssim_channel(a: &[u8], b: &[u8], w: u32, h: u32, channels: usize, c: usize) -> f32 {
    const WIN: usize = 8;
    const STEP: usize = 4;
    const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
    const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
    let (w, h) = (w as usize, h as usize);
    if w < WIN || h < WIN {
        return 1.0;
    }
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
    (total / count as f64) as f32
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
        // Print-oriented CMYK is never a desk-scanner page.
        Pixels::Cmyk(_) => return None,
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
        Pixels::Cmyk(_) => return,
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
    fn fidelity_is_judged_at_the_viewing_resolution() {
        let (w, h) = (400u32, 300u32);
        let src: Vec<u8> = (0..w * h)
            .map(|i| {
                if ((i % w) / 2 + (i / w) / 2) % 2 == 0 {
                    30
                } else {
                    220
                }
            })
            .collect();
        // Identical: 1.
        assert!((luma_fidelity(&src, w, h, src.clone(), w, h, 144.0) - 1.0).abs() < 1e-6);
        // Halved: a fine checkerboard turns gray, a real loss at 144 dpi…
        let img = GrayImage::from_raw(w, h, src.clone()).unwrap();
        let half =
            image::imageops::resize(&img, w / 2, h / 2, image::imageops::FilterType::Triangle);
        let at_144 = luma_fidelity(&src, w, h, half.clone().into_raw(), w / 2, h / 2, 144.0);
        assert!(at_144 < 0.5, "{at_144}");
        // …but invisible when the image is drawn at 600 dpi.
        let at_600 = luma_fidelity(&src, w, h, half.into_raw(), w / 2, h / 2, 600.0);
        assert!(at_600 > 0.95, "{at_600}");
    }

    #[test]
    fn image_fidelity_is_an_area_weighted_mean() {
        let f = ImageFidelity::from_scores([(1.0, 3.0), (0.8, 1.0)].into_iter()).unwrap();
        assert!((f.mean - 0.95).abs() < 1e-6);
        assert_eq!((f.min, f.images), (0.8, 2));
        assert!(ImageFidelity::from_scores(std::iter::empty()).is_none());
    }

    #[test]
    fn black_and_white_gray_becomes_g4_at_the_bilevel_target() {
        let (w, h) = (1200u32, 900u32);
        let img = GrayImage::from_fn(w, h, |x, y| {
            image::Luma([if (x / 3 + y / 7) % 5 == 0 { 0 } else { 255 }])
        });
        assert!(is_black_and_white(&img));
        // 600 dpi on the page; medium: 150 dpi colour target, 300 dpi bi-level.
        let placement = Placement {
            dpi: 600.0,
            page_px: 2400.0,
            visible: [0.0, 0.0, 1.0, 1.0],
            via_dicts: 1,
        };
        let (enc, nw, nh) =
            encode_black_and_white(&img, placement, &crate::Level::Medium.profile()).unwrap();
        assert_eq!((nw, nh), (600, 450));
        assert!(matches!(enc.kind, EncodedKind::Ccitt { .. }));
        let bits = image_codecs::decode_g4(&enc.content, nw, nh, false).unwrap();
        assert_eq!(bits.len(), (nw as usize).div_ceil(8) * nh as usize);

        let mut gray = img.clone();
        gray.put_pixel(5, 5, image::Luma([128]));
        assert!(!is_black_and_white(&gray));
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
