//! Content-level deduplication (every level, see `Profile::deep_dedup`).
//!
//! `rust_engine::dedup_streams` only merges streams that are byte-for-byte
//! identical *including* their dictionary. That misses a lot in real exports:
//! the same picture pasted on 30 slides typically becomes 30 image objects
//! whose Flate bytes may differ (different encoder settings) and whose
//! dictionaries always differ as soon as they each point at their own copy of
//! an otherwise identical `/SMask`.
//!
//! This pass instead keys every object on its *meaning*:
//! - streams: dictionary minus the encoding keys (`Length`, `Filter`,
//!   `DecodeParms`) + decoded content (falling back to the raw bytes and the
//!   full dictionary when the filter chain can't be decoded, e.g. DCT);
//! - a conservative allowlist of plain dictionaries/arrays (fonts, font
//!   descriptors, graphics states, colour spaces, …) that commonly become
//!   identical once the streams they point to have been merged.
//!
//! Merging rewrites references, and can therefore make *more* objects
//! identical (two images whose soft masks just merged), so it is iterated
//! until nothing changes. Among a group of equivalent streams the one with the
//! smallest stored encoding is kept.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use lopdf::{Dictionary, Document, Object, ObjectId};

const MAX_ROUNDS: usize = 8;

/// Returns the number of objects merged away.
pub fn deep_dedup(doc: &mut Document) -> usize {
    let mut total = 0;
    for _ in 0..MAX_ROUNDS {
        let merged = one_round(doc);
        total += merged;
        if merged == 0 {
            break;
        }
    }
    if total > 0 {
        doc.prune_objects();
    }
    total
}

/// Canonical form an object is compared on. `Vec<u8>` so it can be both
/// hashed and compared exactly.
fn canonical_key(obj: &Object) -> Option<Vec<u8>> {
    match obj {
        Object::Stream(s) => {
            let mut out = b"S".to_vec();
            match s.decompressed_content() {
                Ok(decoded) if s.dict.get(b"Filter").is_ok() => {
                    write_dict(&mut out, &s.dict, &[b"Length", b"Filter", b"DecodeParms"]);
                    out.extend_from_slice(b"|D|");
                    out.extend_from_slice(&decoded);
                }
                _ => {
                    write_dict(&mut out, &s.dict, &[b"Length"]);
                    out.extend_from_slice(b"|R|");
                    out.extend_from_slice(&s.content);
                }
            }
            Some(out)
        }
        Object::Dictionary(d) if dict_is_mergeable(d) => {
            let mut out = b"D".to_vec();
            write_dict(&mut out, d, &[]);
            Some(out)
        }
        Object::Array(a) if array_is_mergeable(a) => {
            let mut out = b"A".to_vec();
            write_obj(&mut out, &Object::Array(a.clone()));
            Some(out)
        }
        _ => None,
    }
}

/// Plain dictionaries are only merged when they're of a kind that carries no
/// identity (no back-pointers like `/Parent`, `/P`, no page-tree/outline/
/// annotation semantics).
fn dict_is_mergeable(d: &Dictionary) -> bool {
    if d.has(b"Parent") || d.has(b"P") || d.has(b"Kids") || d.has(b"Next") || d.has(b"Prev") {
        return false;
    }
    match d.get(b"Type").and_then(Object::as_name) {
        Ok(t) => matches!(
            t,
            b"Font" | b"FontDescriptor" | b"ExtGState" | b"Pattern" | b"Encoding" | b"Group"
        ),
        // Untyped dictionaries: shadings, functions, CIDSystemInfo, resource
        // dictionaries, … — only if they look like a shading/function.
        Err(_) => d.has(b"ShadingType") || d.has(b"FunctionType"),
    }
}

/// Arrays of plain values and references (widths, colour spaces like
/// `[/ICCBased 12 0 R]`, `/Decode`, …). Arrays nesting dictionaries are left
/// alone.
fn array_is_mergeable(a: &[Object]) -> bool {
    a.iter().all(|o| {
        matches!(
            o,
            Object::Integer(_)
                | Object::Real(_)
                | Object::Name(_)
                | Object::Boolean(_)
                | Object::Reference(_)
                | Object::Null
        ) || matches!(o, Object::Array(inner) if array_is_mergeable(inner))
    })
}

fn write_dict(out: &mut Vec<u8>, d: &Dictionary, skip: &[&[u8]]) {
    let mut entries: Vec<(&Vec<u8>, &Object)> = d
        .iter()
        .filter(|(k, _)| !skip.contains(&k.as_slice()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    out.push(b'<');
    for (k, v) in entries {
        out.push(b'/');
        out.extend_from_slice(k);
        out.push(b' ');
        write_obj(out, v);
    }
    out.push(b'>');
}

fn write_obj(out: &mut Vec<u8>, o: &Object) {
    match o {
        Object::Dictionary(d) => write_dict(out, d, &[]),
        Object::Array(a) => {
            out.push(b'[');
            for x in a {
                write_obj(out, x);
                out.push(b' ');
            }
            out.push(b']');
        }
        // Debug output is unambiguous for scalars, names, strings and refs.
        other => out.extend_from_slice(format!("{other:?}").as_bytes()),
    }
}

fn stored_len(obj: &Object) -> usize {
    match obj {
        Object::Stream(s) => s.content.len(),
        _ => 0,
    }
}

fn one_round(doc: &mut Document) -> usize {
    let trailer_refs: Vec<ObjectId> = doc
        .trailer
        .iter()
        .filter_map(|(_, v)| v.as_reference().ok())
        .collect();

    // Only hashes are kept for the whole document (decoded images would take
    // gigabytes); full keys are recomputed per candidate group to confirm.
    let mut groups: HashMap<(u64, usize), Vec<ObjectId>> = HashMap::new();
    for (id, obj) in &doc.objects {
        if trailer_refs.contains(id) {
            continue;
        }
        let Some(key) = canonical_key(obj) else {
            continue;
        };
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        groups.entry((h.finish(), key.len())).or_default().push(*id);
    }

    let mut remap: HashMap<ObjectId, ObjectId> = HashMap::new();
    for mut ids in groups.into_values() {
        if ids.len() < 2 {
            continue;
        }
        ids.sort_by_key(|id| (stored_len(&doc.objects[id]), *id));
        let keep = ids[0];
        let Some(keep_key) = canonical_key(&doc.objects[&keep]) else {
            continue;
        };
        for &id in &ids[1..] {
            if canonical_key(&doc.objects[&id]).as_ref() == Some(&keep_key) {
                remap.insert(id, keep);
            }
        }
    }

    if remap.is_empty() {
        return 0;
    }
    let merged = remap.len();
    doc.traverse_objects(|obj| {
        if let Object::Reference(rid) = obj
            && let Some(&canon) = remap.get(rid)
        {
            *obj = Object::Reference(canon);
        }
    });
    for id in remap.keys() {
        doc.objects.remove(id);
    }
    merged
}
