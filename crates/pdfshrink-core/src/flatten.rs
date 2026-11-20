//! Annotation flattening (D-027): the appearance stream of each markup
//! annotation and form field is drawn into its page's content with `Do`, then
//! the annotation is deleted. The page looks the same, nothing can be moved or
//! edited any more, and text stays text (selectable, searchable) — the
//! appearance streams are Form XObjects with real text operators, they are
//! never rasterized.
//!
//! Placement follows ISO 32000 §12.5.5: the form's `/BBox`, transformed by its
//! `/Matrix`, is mapped onto the annotation's `/Rect`. The original content is
//! wrapped in `q … Q` first, so an unbalanced `q` or a leftover CTM cannot
//! move the drawings. Annotations without a usable appearance are left alone:
//! no appearance is ever generated.

use std::collections::{BTreeMap, HashSet};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

/// What [`flatten_annotations`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FlattenStats {
    /// Annotations removed from the pages (drawn, or hidden and dropped).
    pub flattened: usize,
    /// Annotations of a flattenable kind left in place: no usable appearance,
    /// optional content, or form fields whose appearances are stale.
    pub kept: usize,
}

/// Markup annotations and form widgets: what a user adds when filling in or
/// marking up a document. Links, sticky notes (`Text`, whose comment would be
/// lost) with their popups, attachments, multimedia and `Redact` stay.
const FLATTENED: &[&[u8]] = &[
    b"FreeText",
    b"Ink",
    b"Stamp",
    b"Square",
    b"Circle",
    b"Line",
    b"Polygon",
    b"PolyLine",
    b"Highlight",
    b"Underline",
    b"StrikeOut",
    b"Squiggly",
    b"Caret",
    b"Widget",
];

const FLAG_HIDDEN: i64 = 1 << 1;
const FLAG_NO_ROTATE: i64 = 1 << 4;
const FLAG_NO_VIEW: i64 = 1 << 5;

type Mat = [f32; 6];

/// Flattens every page's markup annotations and form widgets; removes the
/// flattened widgets from `/AcroForm`, and `/AcroForm` itself once it has no
/// field left.
pub fn flatten_annotations(doc: &mut Document) -> FlattenStats {
    let mut stats = FlattenStats::default();
    let stale_widgets = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .and_then(|a| doc.dereference(a).ok())
        .and_then(|(_, a)| a.as_dict().ok())
        .and_then(|a| a.get(b"NeedAppearances").ok())
        .and_then(|n| n.as_bool().ok())
        .unwrap_or(false);

    let mut removed_widgets: HashSet<ObjectId> = HashSet::new();
    for page_id in doc.get_pages().into_values() {
        flatten_page(
            doc,
            page_id,
            stale_widgets,
            &mut stats,
            &mut removed_widgets,
        );
    }
    if !removed_widgets.is_empty() {
        prune_acroform(doc, &removed_widgets);
    }
    stats
}

/// One annotation's fate on its page.
enum Plan {
    Keep,
    /// Removed; drawn with this XObject and matrix unless `None`.
    Remove(Option<(ObjectId, Mat)>),
}

fn flatten_page(
    doc: &mut Document,
    page_id: ObjectId,
    stale_widgets: bool,
    stats: &mut FlattenStats,
    removed_widgets: &mut HashSet<ObjectId>,
) {
    let Some(annots) = page_annots(doc, page_id) else {
        return;
    };
    let rotate = inherited(doc, page_id, b"Rotate")
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0)
        .rem_euclid(360);

    let mut plans: Vec<Plan> = Vec::with_capacity(annots.len());
    let mut removed: HashSet<ObjectId> = HashSet::new();
    for annot in &annots {
        let plan = plan_annotation(doc, annot, rotate, stale_widgets, stats);
        if let (Plan::Remove(_), Ok(id)) = (&plan, annot.as_reference()) {
            removed.insert(id);
            if subtype(doc, annot) == Some(b"Widget".as_slice()) {
                removed_widgets.insert(id);
            }
        }
        plans.push(plan);
    }
    // A popup goes with the annotation it belongs to.
    for (annot, plan) in annots.iter().zip(plans.iter_mut()) {
        if matches!(plan, Plan::Keep)
            && subtype(doc, annot) == Some(b"Popup".as_slice())
            && let Some(parent) = annot_dict(doc, annot)
                .and_then(|d| d.get(b"Parent").ok())
                .and_then(|p| p.as_reference().ok())
            && removed.contains(&parent)
        {
            *plan = Plan::Remove(None);
        }
    }
    if plans.iter().all(|p| matches!(p, Plan::Keep)) {
        return;
    }

    let draws: Vec<(ObjectId, Mat)> = plans
        .iter()
        .filter_map(|p| match p {
            Plan::Remove(Some(draw)) => Some(*draw),
            _ => None,
        })
        .collect();
    let kept: Vec<Object> = annots
        .into_iter()
        .zip(&plans)
        .filter(|(_, p)| matches!(p, Plan::Keep))
        .map(|(a, _)| a)
        .collect();

    if !draws.is_empty() {
        let names = add_xobjects(doc, page_id, &draws);
        let mut tail = b"Q\n".to_vec();
        for ((_, m), name) in draws.iter().zip(&names) {
            tail.extend_from_slice(
                format!(
                    "q {} {} {} {} {} {} cm /{name} Do Q\n",
                    num(m[0]),
                    num(m[1]),
                    num(m[2]),
                    num(m[3]),
                    num(m[4]),
                    num(m[5])
                )
                .as_bytes(),
            );
        }
        wrap_contents(doc, page_id, tail);
    }

    if let Ok(page) = doc.get_dictionary_mut(page_id) {
        if kept.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", Object::Array(kept));
        }
    }
}

fn plan_annotation(
    doc: &Document,
    annot: &Object,
    rotate: i64,
    stale_widgets: bool,
    stats: &mut FlattenStats,
) -> Plan {
    let Some(dict) = annot_dict(doc, annot) else {
        return Plan::Keep;
    };
    let Some(kind) = dict.get(b"Subtype").and_then(Object::as_name).ok() else {
        return Plan::Keep;
    };
    if !FLATTENED.contains(&kind) || annot.as_reference().is_err() {
        return Plan::Keep;
    }
    if (kind == b"Widget" && stale_widgets) || dict.has(b"OC") {
        stats.kept += 1;
        return Plan::Keep;
    }
    let flags = dict.get(b"F").and_then(Object::as_i64).unwrap_or(0);
    if flags & (FLAG_HIDDEN | FLAG_NO_VIEW) != 0 {
        stats.flattened += 1;
        return Plan::Remove(None);
    }
    let Some(form_id) = appearance(doc, dict) else {
        stats.kept += 1;
        return Plan::Keep;
    };
    let Some(rect) = dict
        .get(b"Rect")
        .ok()
        .and_then(|r| doc.dereference(r).ok())
        .and_then(|(_, r)| rect_of(r))
    else {
        stats.kept += 1;
        return Plan::Keep;
    };
    let Ok(Object::Stream(form)) = doc.get_object(form_id) else {
        stats.kept += 1;
        return Plan::Keep;
    };
    let Some(bbox) = form
        .dict
        .get(b"BBox")
        .ok()
        .and_then(|b| doc.dereference(b).ok())
        .and_then(|(_, b)| rect_of(b))
    else {
        stats.kept += 1;
        return Plan::Keep;
    };
    let matrix = form
        .dict
        .get(b"Matrix")
        .ok()
        .and_then(|m| doc.dereference(m).ok())
        .and_then(|(_, m)| matrix_of(m))
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    stats.flattened += 1;
    let Some(mut placement) = placement(bbox, matrix, rect) else {
        // Nothing to draw (an empty signature field: `/BBox [0 0 0 0]`).
        return Plan::Remove(None);
    };
    if flags & FLAG_NO_ROTATE != 0 && rotate != 0 {
        // Kept upright on screen: turn it back by the page rotation, around
        // the top-left corner of its rectangle (what qpdf does).
        let (x, y) = (rect[0], rect[3]);
        let (s, c) = (rotate as f32).to_radians().sin_cos();
        placement = mul(
            mul(placement, [1.0, 0.0, 0.0, 1.0, -x, -y]),
            [c, s, -s, c, x, y],
        );
    }
    Plan::Remove(Some((form_id, placement)))
}

/// The normal appearance to draw: `/AP /N`, or its `/AS` state when `/N` is a
/// dictionary of states. Only indirect streams qualify (XObjects must be).
fn appearance(doc: &Document, annot: &Dictionary) -> Option<ObjectId> {
    let ap = annot.get(b"AP").ok()?;
    let (_, ap) = doc.dereference(ap).ok()?;
    let n = ap.as_dict().ok()?.get(b"N").ok()?;
    let id = match n {
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Stream(_) => Some(*id),
            Object::Dictionary(states) => state(annot, states),
            _ => None,
        },
        Object::Dictionary(states) => state(annot, states),
        _ => None,
    }?;
    matches!(doc.get_object(id), Ok(Object::Stream(_))).then_some(id)
}

fn state(annot: &Dictionary, states: &Dictionary) -> Option<ObjectId> {
    let name = annot.get(b"AS").and_then(Object::as_name).ok()?;
    states.get(name).and_then(Object::as_reference).ok()
}

/// Matrix taking the form (`bbox` transformed by `matrix`) onto `rect`, or
/// `None` when either box is empty.
fn placement(bbox: [f32; 4], matrix: Mat, rect: [f32; 4]) -> Option<Mat> {
    let corners = [
        (bbox[0], bbox[1]),
        (bbox[2], bbox[1]),
        (bbox[2], bbox[3]),
        (bbox[0], bbox[3]),
    ]
    .map(|(x, y)| apply(matrix, x, y));
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in corners {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
    if !(w > 1e-6 && h > 1e-6 && rw > 1e-6 && rh > 1e-6) || !(w * h).is_finite() {
        return None;
    }
    let (sx, sy) = (rw / w, rh / h);
    Some([sx, 0.0, 0.0, sy, rect[0] - x0 * sx, rect[1] - y0 * sy])
}

fn apply(m: Mat, x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// `a` then `b` (PDF row-vector convention, as `cm` concatenates).
fn mul(a: Mat, b: Mat) -> Mat {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// Normalized `[x0 y0 x1 y1]` of a 4-number array.
fn rect_of(obj: &Object) -> Option<[f32; 4]> {
    let a = obj.as_array().ok()?;
    if a.len() != 4 {
        return None;
    }
    let v: Vec<f32> = a.iter().filter_map(number).collect();
    (v.len() == 4).then(|| {
        [
            v[0].min(v[2]),
            v[1].min(v[3]),
            v[0].max(v[2]),
            v[1].max(v[3]),
        ]
    })
}

fn matrix_of(obj: &Object) -> Option<Mat> {
    let a = obj.as_array().ok()?;
    let v: Vec<f32> = a.iter().filter_map(number).collect();
    (a.len() == 6 && v.len() == 6).then(|| [v[0], v[1], v[2], v[3], v[4], v[5]])
}

fn number(o: &Object) -> Option<f32> {
    o.as_float().ok().filter(|v| v.is_finite())
}

/// A content-stream number: up to 6 decimals, no exponent, no `-0`.
fn num(v: f32) -> String {
    let s = format!("{:.6}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn annot_dict<'a>(doc: &'a Document, annot: &'a Object) -> Option<&'a Dictionary> {
    doc.dereference(annot).ok()?.1.as_dict().ok()
}

fn subtype<'a>(doc: &'a Document, annot: &'a Object) -> Option<&'a [u8]> {
    annot_dict(doc, annot)?
        .get(b"Subtype")
        .and_then(Object::as_name)
        .ok()
}

/// The page's `/Annots` entries (array direct or indirect), if any.
fn page_annots(doc: &Document, page_id: ObjectId) -> Option<Vec<Object>> {
    let annots = doc.get_dictionary(page_id).ok()?.get(b"Annots").ok()?;
    let (_, annots) = doc.dereference(annots).ok()?;
    let annots = annots.as_array().ok()?;
    (!annots.is_empty()).then(|| annots.clone())
}

/// An inheritable page attribute, looked up through `/Parent`.
fn inherited<'a>(doc: &'a Document, page_id: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut id = page_id;
    for _ in 0..32 {
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(v) = dict.get(key) {
            return doc.dereference(v).ok().map(|(_, v)| v);
        }
        id = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

/// Adds each form to the page's `/XObject` resources under a fresh name and
/// returns the names. The page gets its own direct `/Resources` and
/// `/XObject` dictionaries (copies of the inherited or shared ones), so no
/// other page sees the new names.
fn add_xobjects(doc: &mut Document, page_id: ObjectId, draws: &[(ObjectId, Mat)]) -> Vec<String> {
    let mut resources = inherited(doc, page_id, b"Resources")
        .and_then(|r| r.as_dict().ok())
        .cloned()
        .unwrap_or_default();
    let mut xobjects = resources
        .get(b"XObject")
        .ok()
        .and_then(|x| doc.dereference(x).ok())
        .and_then(|(_, x)| x.as_dict().ok())
        .cloned()
        .unwrap_or_default();

    let mut assigned: BTreeMap<ObjectId, String> = BTreeMap::new();
    let mut next = 1usize;
    let names = draws
        .iter()
        .map(|(form_id, _)| {
            assigned
                .entry(*form_id)
                .or_insert_with(|| {
                    let name = loop {
                        let candidate = format!("Flat{next}");
                        next += 1;
                        if !xobjects.has(candidate.as_bytes()) {
                            break candidate;
                        }
                    };
                    xobjects.set(name.as_bytes(), Object::Reference(*form_id));
                    name
                })
                .clone()
        })
        .collect();
    resources.set("XObject", Object::Dictionary(xobjects));

    for form_id in assigned.keys() {
        if let Ok(Object::Stream(form)) = doc.get_object_mut(*form_id) {
            form.dict.set("Type", Object::Name(b"XObject".to_vec()));
            form.dict.set("Subtype", Object::Name(b"Form".to_vec()));
        }
    }
    if let Ok(page) = doc.get_dictionary_mut(page_id) {
        page.set("Resources", Object::Dictionary(resources));
    }
    names
}

/// `/Contents` becomes `[q-stream, …original…, tail]`.
fn wrap_contents(doc: &mut Document, page_id: ObjectId, tail: Vec<u8>) {
    let original: Vec<Object> = doc
        .get_dictionary(page_id)
        .ok()
        .and_then(|p| p.get(b"Contents").ok())
        .map(|c| match c {
            Object::Reference(id) => match doc.get_object(*id) {
                Ok(Object::Array(parts)) => parts.clone(),
                _ => vec![c.clone()],
            },
            Object::Array(parts) => parts.clone(),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    let head = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let tail = doc.add_object(Stream::new(Dictionary::new(), tail));
    let mut contents = Vec::with_capacity(original.len() + 2);
    contents.push(Object::Reference(head));
    contents.extend(original);
    contents.push(Object::Reference(tail));
    if let Ok(page) = doc.get_dictionary_mut(page_id) {
        page.set("Contents", Object::Array(contents));
    }
}

/// Removes the flattened widgets from the field tree; a field left without
/// widgets goes too, and `/AcroForm` once no field is left.
fn prune_acroform(doc: &mut Document, removed: &HashSet<ObjectId>) {
    let Some(acroform) = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .cloned()
    else {
        return;
    };
    let Some(fields) = doc
        .dereference(&acroform)
        .ok()
        .and_then(|(_, a)| a.as_dict().ok())
        .and_then(|a| a.get(b"Fields").ok())
        .and_then(|f| doc.dereference(f).ok())
        .and_then(|(_, f)| f.as_array().ok())
        .cloned()
    else {
        return;
    };
    let fields: Vec<Object> = fields
        .into_iter()
        .filter(|f| keep_field(doc, f, removed, 0))
        .collect();

    if fields.is_empty() {
        if let Ok(catalog) = doc.catalog_mut() {
            catalog.remove(b"AcroForm");
        }
        return;
    }
    let target = match acroform {
        Object::Reference(id) => doc.get_dictionary_mut(id).ok(),
        _ => doc.catalog_mut().ok().and_then(|c| {
            c.get_mut(b"AcroForm")
                .ok()
                .and_then(|a| a.as_dict_mut().ok())
        }),
    };
    if let Some(form) = target {
        form.set("Fields", Object::Array(fields));
    }
}

/// Whether `field` survives; rewrites its `/Kids` when some of them go.
fn keep_field(doc: &mut Document, field: &Object, removed: &HashSet<ObjectId>, depth: u32) -> bool {
    let Ok(id) = field.as_reference() else {
        return true;
    };
    if removed.contains(&id) {
        return false;
    }
    if depth > 32 {
        return true;
    }
    let Some(kids) = doc
        .get_dictionary(id)
        .ok()
        .and_then(|d| d.get(b"Kids").ok())
        .and_then(|k| doc.dereference(k).ok())
        .and_then(|(_, k)| k.as_array().ok())
        .cloned()
    else {
        return true;
    };
    if kids.is_empty() {
        return true;
    }
    let before = kids.len();
    let kids: Vec<Object> = kids
        .into_iter()
        .filter(|k| keep_field(doc, k, removed, depth + 1))
        .collect();
    if kids.is_empty() {
        return false;
    }
    if kids.len() != before
        && let Ok(dict) = doc.get_dictionary_mut(id)
    {
        dict.set("Kids", Object::Array(kids));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn form(doc: &mut Document, bbox: [f32; 4], matrix: Option<Mat>, body: &[u8]) -> ObjectId {
        let mut dict = dictionary! {
            "Subtype" => "Form",
            "BBox" => bbox.iter().map(|&v| Object::Real(v)).collect::<Vec<_>>(),
        };
        if let Some(m) = matrix {
            dict.set(
                "Matrix",
                m.iter().map(|&v| Object::Real(v)).collect::<Vec<Object>>(),
            );
        }
        doc.add_object(Stream::new(dict, body.to_vec()))
    }

    fn annot(doc: &mut Document, subtype: &str, rect: [f32; 4], extra: Dictionary) -> ObjectId {
        let mut dict = dictionary! {
            "Type" => "Annot",
            "Subtype" => Object::Name(subtype.as_bytes().to_vec()),
            "Rect" => rect.iter().map(|&v| Object::Real(v)).collect::<Vec<_>>(),
        };
        dict.extend(&extra);
        doc.add_object(dict)
    }

    /// Two pages sharing resources inherited from `/Pages`; every kind of
    /// annotation on the first one.
    fn fixture() -> (Document, ObjectId, ObjectId, Vec<ObjectId>) {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        });
        let text = form(
            &mut doc,
            [0.0, 0.0, 100.0, 20.0],
            None,
            b"BT /Helv 12 Tf 2 5 Td (Hello) Tj ET",
        );
        if let Ok(Object::Stream(s)) = doc.get_object_mut(text) {
            s.dict.set(
                "Resources",
                dictionary! { "Font" => dictionary! { "Helv" => font } },
            );
        }
        let free = annot(
            &mut doc,
            "FreeText",
            [100.0, 700.0, 200.0, 720.0],
            dictionary! { "AP" => dictionary! { "N" => text }, "F" => 4i64 },
        );
        let popup = annot(
            &mut doc,
            "Popup",
            [0.0, 0.0, 10.0, 10.0],
            dictionary! { "Parent" => free },
        );
        let link = annot(&mut doc, "Link", [0.0, 0.0, 10.0, 10.0], dictionary! {});
        let hidden_form = form(&mut doc, [0.0, 0.0, 10.0, 10.0], None, b"0 0 10 10 re f");
        let hidden = annot(
            &mut doc,
            "Square",
            [0.0, 0.0, 10.0, 10.0],
            dictionary! { "AP" => dictionary! { "N" => hidden_form }, "F" => 2i64 },
        );
        let no_ap = annot(&mut doc, "Ink", [0.0, 0.0, 10.0, 10.0], dictionary! {});
        let on = form(&mut doc, [0.0, 0.0, 10.0, 10.0], None, b"0 0 m 10 10 l S");
        let off = form(&mut doc, [0.0, 0.0, 10.0, 10.0], None, b"");
        let rotated = form(
            &mut doc,
            [0.0, 0.0, 10.0, 20.0],
            Some([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]),
            b"0 0 10 20 re S",
        );
        let checkbox = annot(
            &mut doc,
            "Widget",
            [300.0, 300.0, 310.0, 310.0],
            dictionary! {
                "AP" => dictionary! { "N" => dictionary! { "Yes" => on, "Off" => off } },
                "AS" => "Yes",
            },
        );
        let turned = annot(
            &mut doc,
            "Stamp",
            [400.0, 400.0, 440.0, 420.0],
            dictionary! { "AP" => dictionary! { "N" => rotated } },
        );
        let empty_sig = form(&mut doc, [0.0, 0.0, 0.0, 0.0], None, b"q Q");
        let sig = annot(
            &mut doc,
            "Widget",
            [450.0, 20.0, 510.0, 50.0],
            dictionary! { "AP" => dictionary! { "N" => empty_sig }, "FT" => "Sig", "T" => Object::string_literal("Signature") },
        );
        let field = doc.add_object(dictionary! {
            "FT" => "Btn", "T" => Object::string_literal("Check"), "Kids" => vec![Object::Reference(checkbox)],
        });
        if let Ok(d) = doc.get_dictionary_mut(checkbox) {
            d.set("Parent", field);
        }

        let content = doc.add_object(Stream::new(dictionary! {}, b"q 1 0 0 1 50 50 cm".to_vec()));
        let page1 = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content,
            "Annots" => [free, popup, link, hidden, no_ap, checkbox, turned, sig].map(Object::Reference).to_vec(),
        });
        let page2 = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page1), Object::Reference(page2)],
                "Count" => 2i64,
                "MediaBox" => [0i64, 0, 595, 842].map(Object::Integer).to_vec(),
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
            }),
        );
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "AcroForm" => dictionary! { "Fields" => vec![Object::Reference(field), Object::Reference(sig)], "SigFlags" => 3i64 },
        });
        doc.trailer.set("Root", catalog);
        (doc, page1, page2, vec![free, link, no_ap, text, rotated])
    }

    fn page_content(doc: &Document, page: ObjectId) -> String {
        String::from_utf8(doc.get_page_content(page)).unwrap()
    }

    #[test]
    fn flattens_markup_and_widgets_keeps_the_rest() {
        let (mut doc, page1, page2, ids) = fixture();
        let [_, link, no_ap, text, rotated] = ids[..] else {
            unreachable!()
        };
        let stats = flatten_annotations(&mut doc);
        assert_eq!(
            stats,
            FlattenStats {
                flattened: 5,
                kept: 1
            }
        );

        let page = doc.get_dictionary(page1).unwrap();
        let annots = page.get(b"Annots").unwrap().as_array().unwrap();
        assert_eq!(
            annots,
            &vec![Object::Reference(link), Object::Reference(no_ap)]
        );

        let xobjects = page
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"XObject")
            .unwrap()
            .as_dict()
            .unwrap();
        assert_eq!(xobjects.len(), 3, "FreeText, checkbox /Yes, stamp");
        assert!(xobjects.iter().any(|(_, v)| v == &Object::Reference(text)));

        let content = page_content(&doc, page1);
        assert!(content.starts_with("q\n"), "{content}");
        assert!(content.contains("q 1 0 0 1 50 50 cm"));
        // FreeText: bbox 100×20 at the origin onto [100 700 200 720].
        assert!(
            content.contains("Q\nq 1 0 0 1 100 700 cm /Flat"),
            "{content}"
        );
        // The stamp's form is turned a quarter: its box is [-20 0 0 10], 20×10.
        assert!(content.contains("q 2 0 0 2 440 400 cm /Flat"), "{content}");
        assert_eq!(content.matches(" Do Q").count(), 3);

        // The other page's resources were not touched.
        assert!(
            doc.get_dictionary(page2)
                .unwrap()
                .get(b"Resources")
                .is_err()
        );
        let pages = doc
            .get_dictionary(
                doc.catalog()
                    .unwrap()
                    .get(b"Pages")
                    .unwrap()
                    .as_reference()
                    .unwrap(),
            )
            .unwrap();
        let shared = pages.get(b"Resources").unwrap().as_dict().unwrap();
        assert!(!shared.has(b"XObject"));

        assert!(!doc.catalog().unwrap().has(b"AcroForm"));
        assert!(
            matches!(doc.get_object(rotated), Ok(Object::Stream(s)) if s.dict.has_type(b"XObject"))
        );
    }

    #[test]
    fn stale_form_appearances_are_left_alone() {
        let (mut doc, page1, _, _) = fixture();
        let catalog = doc.catalog_mut().unwrap();
        let form = catalog.get_mut(b"AcroForm").unwrap().as_dict_mut().unwrap();
        form.set("NeedAppearances", true);
        let stats = flatten_annotations(&mut doc);
        assert_eq!(stats.flattened, 3, "FreeText, hidden square, stamp");
        assert_eq!(stats.kept, 3, "ink without /AP and both widgets");
        let annots = doc.get_dictionary(page1).unwrap().get(b"Annots").unwrap();
        assert_eq!(annots.as_array().unwrap().len(), 4);
        assert!(doc.catalog().unwrap().has(b"AcroForm"));
    }

    #[test]
    fn partially_flattened_fields_keep_their_other_widgets() {
        let (mut doc, page1, _, _) = fixture();
        // A second widget of the "Check" field, without appearance: it stays.
        let field = doc
            .catalog()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Fields")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .as_reference()
            .unwrap();
        let lonely = annot(
            &mut doc,
            "Widget",
            [0.0, 0.0, 10.0, 10.0],
            dictionary! { "Parent" => field },
        );
        let kids = doc
            .get_dictionary_mut(field)
            .unwrap()
            .get_mut(b"Kids")
            .unwrap();
        kids.as_array_mut().unwrap().push(Object::Reference(lonely));
        let page = doc.get_dictionary_mut(page1).unwrap();
        page.get_mut(b"Annots")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(Object::Reference(lonely));

        flatten_annotations(&mut doc);
        let form = doc
            .catalog()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_dict()
            .unwrap();
        let fields = form.get(b"Fields").unwrap().as_array().unwrap();
        assert_eq!(fields, &vec![Object::Reference(field)], "signature gone");
        let kids = doc
            .get_dictionary(field)
            .unwrap()
            .get(b"Kids")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(kids, &vec![Object::Reference(lonely)]);
    }

    #[test]
    fn no_rotate_turns_back_around_the_top_left_corner() {
        // Page turned 90°: the annotation is rotated +90° around (x0, y1).
        let m = placement(
            [0.0, 0.0, 10.0, 10.0],
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 10.0, 10.0],
        )
        .unwrap();
        let turned = mul(
            mul(m, [1.0, 0.0, 0.0, 1.0, 0.0, -10.0]),
            [0.0, 1.0, -1.0, 0.0, 0.0, 10.0],
        );
        let (x, y) = apply(turned, 10.0, 10.0);
        assert!((x - 0.0).abs() < 1e-5 && (y - 20.0).abs() < 1e-5, "{x} {y}");
    }

    #[test]
    fn numbers_are_written_plainly() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(0.5), "0.5");
        assert_eq!(num(-316.25), "-316.25");
        assert_eq!(num(1e-9), "0");
    }
}
