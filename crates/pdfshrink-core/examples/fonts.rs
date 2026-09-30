//! Diagnostic: embedded font programs by FontDescriptor name, with sizes.
use lopdf::{Document, Object};
use std::collections::{HashMap, HashSet};

/// (descriptors, bytes, distinct font file ids) per font name + FontFile key.
type Agg = HashMap<String, (usize, u64, HashSet<(u32, u16)>)>;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let doc = Document::load(&path).unwrap();
    let mut agg: Agg = HashMap::new();
    for obj in doc.objects.values() {
        let Ok(d) = obj.as_dict() else { continue };
        if d.get(b"Type").and_then(Object::as_name).ok() != Some(b"FontDescriptor") {
            continue;
        }
        let name = d
            .get(b"FontName")
            .and_then(Object::as_name)
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        let base = name.split('+').next_back().unwrap_or("").to_string();
        for k in [&b"FontFile"[..], b"FontFile2", b"FontFile3"] {
            if let Ok(r) = d.get(k).and_then(Object::as_reference) {
                let len = doc
                    .get_object(r)
                    .and_then(Object::as_stream)
                    .map(|s| s.content.len() as u64)
                    .unwrap_or(0);
                let e = agg
                    .entry(format!("{base} {}", String::from_utf8_lossy(k)))
                    .or_default();
                e.0 += 1;
                if e.2.insert(r) {
                    e.1 += len;
                }
            }
        }
    }
    let mut v: Vec<_> = agg.into_iter().collect();
    v.sort_by_key(|(_, (_, b, _))| std::cmp::Reverse(*b));
    for (k, (n, b, u)) in v {
        println!(
            "{k:<45} desc={n:<4} files={:<4} {:>8.1} KB",
            u.len(),
            b as f64 / 1e3
        );
    }
}
