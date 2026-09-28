//! Figures out how big, in points, each image XObject is actually drawn on the
//! page, by walking page (and nested Form XObject) content streams and tracking
//! the current transformation matrix. That size, combined with the image's pixel
//! dimensions, gives its *effective DPI* — the number the compression profiles
//! compare against their target resolution.

use std::collections::HashMap;

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
    op.as_f32().ok().or_else(|| op.as_i64().ok().map(|i| i as f32))
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

/// Effective on-page DPI for every image XObject that could be located while
/// walking content streams, keyed by object id. When an image is drawn more than
/// once, the *smallest* DPI (its largest/most demanding placement) wins, since
/// that is the placement that would show resampling artifacts first.
///
/// Images that are never reached by a `Do` operator (used only from a pattern,
/// an annotation appearance stream, etc.) are simply absent from the map; the
/// caller is expected to apply a conservative fallback for those.
pub fn compute_image_dpi(doc: &Document) -> HashMap<ObjectId, f32> {
    let mut dpi = HashMap::new();
    let mut budget = 0u32;
    for (_, page_id) in doc.get_pages() {
        let Ok(content) = doc.get_and_decode_page_content(page_id) else {
            continue;
        };
        let Ok((resources, extra_ids)) = doc.get_page_resources(page_id) else {
            continue;
        };
        let extra_dicts: Vec<&Dictionary> = extra_ids.iter().filter_map(|id| doc.get_dictionary(*id).ok()).collect();
        let mut resource_stack: Vec<&Dictionary> = Vec::new();
        if let Some(r) = resources {
            resource_stack.push(r);
        }
        resource_stack.extend(extra_dicts);

        walk_ops(doc, &content.operations, IDENTITY, &resource_stack, &mut dpi, 0, &mut budget);
    }
    dpi
}

fn find_xobject(doc: &Document, name: &[u8], resource_stack: &[&Dictionary]) -> Option<ObjectId> {
    for res in resource_stack {
        if let Ok(xobjects) = doc.get_dict_in_dict(res, b"XObject")
            && let Ok(obj) = xobjects.get(name)
            && let Ok(id) = obj.as_reference()
        {
            return Some(id);
        }
    }
    None
}

// Bounds so a pathological/cyclical set of nested forms can't blow up memory or time.
const MAX_DEPTH: u32 = 12;
const MAX_OPS: u32 = 300_000;

fn walk_ops(
    doc: &Document,
    ops: &[Operation],
    ctm: Mat,
    resource_stack: &[&Dictionary],
    dpi: &mut HashMap<ObjectId, f32>,
    depth: u32,
    budget: &mut u32,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let mut stack: Vec<Mat> = Vec::new();
    let mut cur = ctm;
    for op in ops {
        *budget += 1;
        if *budget > MAX_OPS {
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
                let Some(id) = find_xobject(doc, name, resource_stack) else {
                    continue;
                };
                let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
                    continue;
                };
                match stream.dict.get(b"Subtype").and_then(Object::as_name) {
                    Ok(b"Image") => record_image_dpi(stream, cur, id, dpi),
                    Ok(b"Form") => descend_into_form(doc, stream, cur, resource_stack, dpi, depth, budget),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn record_image_dpi(stream: &lopdf::Stream, ctm: Mat, id: ObjectId, dpi: &mut HashMap<ObjectId, f32>) {
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
    let entry = dpi.entry(id).or_insert(f32::MAX);
    if effective < *entry {
        *entry = effective;
    }
}

fn descend_into_form(
    doc: &Document,
    stream: &lopdf::Stream,
    ctm: Mat,
    resource_stack: &[&Dictionary],
    dpi: &mut HashMap<ObjectId, f32>,
    depth: u32,
    budget: &mut u32,
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
        return;
    };
    let Ok(content) = lopdf::content::Content::decode(&content_bytes) else {
        return;
    };

    let mut new_stack: Vec<&Dictionary> = Vec::with_capacity(resource_stack.len() + 1);
    if let Ok(own_resources) = doc.get_dict_in_dict(&stream.dict, b"Resources") {
        new_stack.push(own_resources);
    }
    new_stack.extend_from_slice(resource_stack);

    walk_ops(doc, &content.operations, new_ctm, &new_stack, dpi, depth + 1, budget);
}
