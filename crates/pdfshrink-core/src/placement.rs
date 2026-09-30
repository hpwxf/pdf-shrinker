//! Figures out how big, in points, each image XObject is actually drawn on the
//! page, by walking page (and nested Form XObject) content streams and tracking
//! the current transformation matrix. That size, combined with the image's pixel
//! dimensions, gives its *effective DPI* — the number the compression profiles
//! compare against their target resolution.

use std::collections::{HashMap, HashSet};

use lopdf::content::Operation;
use lopdf::{Dictionary, Document, Object, ObjectId};

/// A 2D affine transform, stored as PDF's `[a b c d e f]` operand order.
type Mat = [f32; 6];

const IDENTITY: Mat = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `m1` applied first, then `m2` (PDF's `cm` semantics: `CTM' = m1 x CTM`).
fn concat(m1: Mat, m2: Mat) -> Mat {
    let [a1, b1, c1, d1, e1, f1] = m1;
    let [a2, b2, c2, d2, e2, f2] = m2;
    [
        a1 * a2 + b1 * c2,
        a1 * b2 + b1 * d2,
        c1 * a2 + d1 * c2,
        c1 * b2 + d1 * d2,
        e1 * a2 + f1 * c2 + e2,
        e1 * b2 + f1 * d2 + f2,
    ]
}

/// Lengths, in target-space units, of the transformed unit-square edge vectors —
/// i.e. how many points wide/tall a unit image placed with this CTM ends up.
fn placed_size(m: Mat) -> (f32, f32) {
    let [a, b, c, d, _, _] = m;
    ((a * a + b * b).sqrt(), (c * c + d * d).sqrt())
}

fn operand_f32(op: &Object) -> Option<f32> {
    op.as_f32()
        .ok()
        .or_else(|| op.as_i64().ok().map(|i| i as f32))
}

fn matrix_from_operands(ops: &[Object]) -> Option<Mat> {
    if ops.len() < 6 {
        return None;
    }
    Some([
        operand_f32(&ops[0])?,
        operand_f32(&ops[1])?,
        operand_f32(&ops[2])?,
        operand_f32(&ops[3])?,
        operand_f32(&ops[4])?,
        operand_f32(&ops[5])?,
    ])
}

/// How an image XObject is drawn, over all of its placements.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    /// Effective on-page DPI; the smallest over all placements (the most
    /// demanding one, which would show resampling artifacts first).
    pub dpi: f32,
    /// How many image pixels span the (displayed) width of the page it's
    /// drawn on, i.e. `dpi` rescaled by that page's width; again the smallest
    /// over all placements. Width rather than longest side because that's
    /// what viewers fit to the screen: a 1512×14400 pt "web page" export is
    /// read at its width, not squeezed to fit its height. Lets a profile
    /// express its target resolution relative to the page, which `dpi` alone
    /// can't do for oversized pages like slide exports (1920×1080 pt, where
    /// 96 DPI means 2560 px across).
    pub page_px: f32,
    /// Part of the image that can ever be visible, as `[u0, v0, u1, v1]` in
    /// image space (the unit square, `v` pointing up as in PDF): the union,
    /// over all placements, of the bounding box of the image area that falls
    /// inside the page's MediaBox. Clipping paths are ignored, so this can
    /// only overestimate what's visible. An image drawn oversized and cropped
    /// by the page edge (slide-deck "full-bleed" backgrounds) has a visible
    /// area well under the full `[0, 0, 1, 1]`.
    pub visible: [f32; 4],
    /// Number of distinct `/XObject` resource dictionaries the walk resolved
    /// this image through. Compared against the document-wide reference count
    /// to know whether *every* use of the image was seen (see `complete`).
    pub via_dicts: usize,
}

/// Result of [`compute_image_placement`].
pub struct Placements {
    pub map: HashMap<ObjectId, Placement>,
    /// False if any page or form couldn't be decoded, or the walk hit its
    /// depth/operation budget: some placements may then be missing, and
    /// anything relying on having seen *all* of them (cropping) must not run.
    pub complete: bool,
}

/// Placement of every image XObject that could be located while walking
/// content streams, keyed by object id.
///
/// Images that are never reached by a `Do` operator (used only from a pattern,
/// an annotation appearance stream, etc.) are simply absent from the map; the
/// caller is expected to apply a conservative fallback for those.
pub fn compute_image_placement(doc: &Document) -> Placements {
    let mut dpi = HashMap::new();
    let mut via: HashMap<ObjectId, HashSet<usize>> = HashMap::new();
    let mut complete = true;
    for (_, page_id) in doc.get_pages() {
        // Per page: a global budget used to run out partway through long
        // documents, silently leaving every image on later pages to the
        // conservative fallback.
        let mut budget = 0u32;
        let (page_width_pt, page_box) = page_geometry(doc, page_id);
        let Ok(content) = doc.get_and_decode_page_content(page_id) else {
            complete = false;
            continue;
        };
        let Ok((resources, extra_ids)) = doc.get_page_resources(page_id) else {
            complete = false;
            continue;
        };
        let extra_dicts: Vec<&Dictionary> = extra_ids
            .iter()
            .filter_map(|id| doc.get_dictionary(*id).ok())
            .collect();
        let mut resource_stack: Vec<&Dictionary> = Vec::new();
        if let Some(r) = resources {
            resource_stack.push(r);
        }
        resource_stack.extend(extra_dicts);

        let mut walk = Walk {
            doc,
            page_width_pt,
            page_box,
            out: &mut dpi,
            via: &mut via,
            budget: &mut budget,
            complete: &mut complete,
        };
        walk.ops(&content.operations, IDENTITY, &resource_stack, 0);
    }
    for (id, dicts) in via {
        if let Some(p) = dpi.get_mut(&id) {
            p.via_dicts = dicts.len();
        }
    }
    Placements { map: dpi, complete }
}

/// The page's displayed width — its MediaBox (inherited through the page
/// tree) width, or height when `/Rotate` is 90 or 270 — in points, and the
/// MediaBox itself as `[x0, y0, x1, y1]`; US Letter's if it can't be read.
fn page_geometry(doc: &Document, page_id: ObjectId) -> (f32, [f32; 4]) {
    let mut cur = Some(page_id);
    let mut hops = 0;
    let mut rotate = None;
    while let Some(id) = cur {
        let Ok(dict) = doc.get_dictionary(id) else {
            break;
        };
        if rotate.is_none() {
            rotate = dict.get(b"Rotate").and_then(Object::as_i64).ok();
        }
        if let Ok(bbox) = dict.get(b"MediaBox").and_then(Object::as_array)
            && bbox.len() == 4
            && let (Some(x0), Some(y0), Some(x1), Some(y1)) = (
                operand_f32(&bbox[0]),
                operand_f32(&bbox[1]),
                operand_f32(&bbox[2]),
                operand_f32(&bbox[3]),
            )
        {
            let (w, h) = ((x1 - x0).abs(), (y1 - y0).abs());
            let quarter_turn = rotate.unwrap_or(0).rem_euclid(180) == 90;
            let width = if quarter_turn { h } else { w }.max(1.0);
            return (width, [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]);
        }
        cur = dict.get(b"Parent").and_then(Object::as_reference).ok();
        hops += 1;
        if hops > 32 {
            break;
        }
    }
    (612.0, [0.0, 0.0, 612.0, 792.0])
}

/// Bounding box, in image space, of the part of the unit square that `ctm`
/// maps inside `page_box`; `None` if none of it does (or `ctm` is singular).
fn visible_in_image_space(ctm: Mat, page_box: [f32; 4]) -> Option<[f32; 4]> {
    let [a, b, c, d, e, f] = ctm;
    let det = a * d - b * c;
    if det.abs() < 1e-9 {
        return None;
    }
    let to_page = |u: f32, v: f32| (a * u + c * v + e, b * u + d * v + f);
    let to_image = |x: f32, y: f32| {
        let (x, y) = (x - e, y - f);
        ((d * x - c * y) / det, (a * y - b * x) / det)
    };
    // Sutherland–Hodgman: clip the placed parallelogram to the page box.
    let mut poly: Vec<(f32, f32)> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
        .iter()
        .map(|&(u, v)| to_page(u, v))
        .collect();
    let [x0, y0, x1, y1] = page_box;
    // (signed distance inside the half-plane, boundary coordinate)
    type Edge = (fn((f32, f32), f32) -> f32, f32);
    let edges: [Edge; 4] = [
        (|p, k| p.0 - k, x0),
        (|p, k| k - p.0, x1),
        (|p, k| p.1 - k, y0),
        (|p, k| k - p.1, y1),
    ];
    for (inside, k) in edges {
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            let (dp, dq) = (inside(p, k), inside(q, k));
            if dp >= 0.0 {
                out.push(p);
            }
            if (dp >= 0.0) != (dq >= 0.0) {
                let t = dp / (dp - dq);
                out.push((p.0 + t * (q.0 - p.0), p.1 + t * (q.1 - p.1)));
            }
        }
        poly = out;
        if poly.is_empty() {
            return None;
        }
    }
    let mut r = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for (x, y) in poly {
        let (u, v) = to_image(x, y);
        r = [r[0].min(u), r[1].min(v), r[2].max(u), r[3].max(v)];
    }
    Some([
        r[0].clamp(0.0, 1.0),
        r[1].clamp(0.0, 1.0),
        r[2].clamp(0.0, 1.0),
        r[3].clamp(0.0, 1.0),
    ])
}

/// Resolves an XObject name; also returns the address of the `/XObject`
/// dictionary it was found in, as an identity for `Placement::via_dicts`.
fn find_xobject(
    doc: &Document,
    name: &[u8],
    resource_stack: &[&Dictionary],
) -> Option<(ObjectId, usize)> {
    for res in resource_stack {
        if let Ok(xobjects) = doc.get_dict_in_dict(res, b"XObject")
            && let Ok(obj) = xobjects.get(name)
            && let Ok(id) = obj.as_reference()
        {
            return Some((id, xobjects as *const Dictionary as usize));
        }
    }
    None
}

// Bounds (per page) so a pathological/cyclical set of nested forms can't blow
// up memory or time.
const MAX_DEPTH: u32 = 12;
const MAX_OPS: u32 = 300_000;

struct Walk<'a, 'b> {
    doc: &'a Document,
    page_width_pt: f32,
    page_box: [f32; 4],
    out: &'b mut HashMap<ObjectId, Placement>,
    via: &'b mut HashMap<ObjectId, HashSet<usize>>,
    budget: &'b mut u32,
    complete: &'b mut bool,
}

impl<'a> Walk<'a, '_> {
    fn ops(&mut self, ops: &[Operation], ctm: Mat, resource_stack: &[&'a Dictionary], depth: u32) {
        if depth > MAX_DEPTH {
            *self.complete = false;
            return;
        }
        let doc = self.doc;
        let mut stack: Vec<Mat> = Vec::new();
        let mut cur = ctm;
        for op in ops {
            *self.budget += 1;
            if *self.budget > MAX_OPS {
                *self.complete = false;
                return;
            }
            match op.operator.as_str() {
                "q" => stack.push(cur),
                "Q" => {
                    if let Some(m) = stack.pop() {
                        cur = m;
                    }
                }
                "cm" => {
                    if let Some(m) = matrix_from_operands(&op.operands) {
                        cur = concat(m, cur);
                    }
                }
                "Do" => {
                    let Some(Object::Name(name)) = op.operands.first() else {
                        continue;
                    };
                    let Some((id, dict_addr)) = find_xobject(doc, name, resource_stack) else {
                        continue;
                    };
                    let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
                        continue;
                    };
                    match stream.dict.get(b"Subtype").and_then(Object::as_name) {
                        Ok(b"Image") => {
                            self.via.entry(id).or_default().insert(dict_addr);
                            self.record_image(stream, cur, id)
                        }
                        Ok(b"Form") => self.descend_into_form(stream, cur, resource_stack, depth),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    fn record_image(&mut self, stream: &lopdf::Stream, ctm: Mat, id: ObjectId) {
        let (Ok(w), Ok(h)) = (
            stream.dict.get(b"Width").and_then(Object::as_i64),
            stream.dict.get(b"Height").and_then(Object::as_i64),
        ) else {
            return;
        };
        let (width_pt, height_pt) = placed_size(ctm);
        if width_pt <= 0.01 || height_pt <= 0.01 {
            return;
        }
        let dpi_x = w as f32 / (width_pt / 72.0);
        let dpi_y = h as f32 / (height_pt / 72.0);
        let effective = (dpi_x + dpi_y) / 2.0;
        let page_px = effective / 72.0 * self.page_width_pt;
        let entry = self.out.entry(id).or_insert(Placement {
            dpi: f32::MAX,
            page_px: f32::MAX,
            visible: [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
            via_dicts: 0,
        });
        entry.dpi = entry.dpi.min(effective);
        entry.page_px = entry.page_px.min(page_px);
        if let Some([u0, v0, u1, v1]) = visible_in_image_space(ctm, self.page_box) {
            let r = &mut entry.visible;
            *r = [r[0].min(u0), r[1].min(v0), r[2].max(u1), r[3].max(v1)];
        }
    }

    fn descend_into_form(
        &mut self,
        stream: &'a lopdf::Stream,
        ctm: Mat,
        resource_stack: &[&'a Dictionary],
        depth: u32,
    ) {
        let form_matrix = stream
            .dict
            .get(b"Matrix")
            .and_then(Object::as_array)
            .ok()
            .and_then(|a| matrix_from_operands(a))
            .unwrap_or(IDENTITY);
        let new_ctm = concat(form_matrix, ctm);

        let Ok(content_bytes) = stream.get_plain_content() else {
            *self.complete = false;
            return;
        };
        let Ok(content) = lopdf::content::Content::decode(&content_bytes) else {
            *self.complete = false;
            return;
        };

        let mut new_stack: Vec<&Dictionary> = Vec::with_capacity(resource_stack.len() + 1);
        if let Ok(own_resources) = self.doc.get_dict_in_dict(&stream.dict, b"Resources") {
            new_stack.push(own_resources);
        }
        new_stack.extend_from_slice(resource_stack);

        self.ops(&content.operations, new_ctm, &new_stack, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn fully_visible_image() {
        let ctm = [100.0, 0.0, 0.0, 50.0, 10.0, 10.0];
        let v = visible_in_image_space(ctm, [0.0, 0.0, 200.0, 200.0]).unwrap();
        assert!(close(v, [0.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn image_overflowing_the_page_is_cropped_to_it() {
        // 400×400 pt image centred on a 200×200 pt page: only the middle half
        // of it (in both directions) can ever be seen.
        let ctm = [400.0, 0.0, 0.0, 400.0, -100.0, -100.0];
        let v = visible_in_image_space(ctm, [0.0, 0.0, 200.0, 200.0]).unwrap();
        assert!(close(v, [0.25, 0.25, 0.75, 0.75]), "{v:?}");
    }

    #[test]
    fn flipped_image_maps_back_correctly() {
        // Drawn upside down (d < 0), hanging off the top of the page.
        let ctm = [100.0, 0.0, 0.0, -100.0, 0.0, 150.0];
        let v = visible_in_image_space(ctm, [0.0, 0.0, 100.0, 100.0]).unwrap();
        assert!(close(v, [0.0, 0.5, 1.0, 1.0]), "{v:?}");
    }

    #[test]
    fn off_page_image_is_invisible() {
        let ctm = [50.0, 0.0, 0.0, 50.0, 500.0, 500.0];
        assert!(visible_in_image_space(ctm, [0.0, 0.0, 100.0, 100.0]).is_none());
    }
}
