//! Diagnostic: where do a PDF's bytes go? Breaks stored stream bytes down by
//! kind (image, soft mask, font, page content, form XObject, other) and counts
//! images whose *decoded* samples are identical (duplicates byte-level dedup
//! can't see because the encoded bytes or dictionaries differ).
//! `cargo run --release -p pdfshrink-core --example analyze -- file.pdf`
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::env;
use std::hash::{Hash, Hasher};

use lopdf::{Document, Object, ObjectId};

fn main() {
    let path = env::args().nth(1).expect("usage: analyze <file.pdf>");
    let doc = Document::load(&path).unwrap();

    let mut smasks: HashSet<ObjectId> = HashSet::new();
    let mut fonts: HashSet<ObjectId> = HashSet::new();
    let mut contents: HashSet<ObjectId> = HashSet::new();
    for obj in doc.objects.values() {
        let dict = match obj {
            Object::Stream(s) => &s.dict,
            Object::Dictionary(d) => d,
            _ => continue,
        };
        if let Ok(r) = dict.get(b"SMask").and_then(Object::as_reference) {
            smasks.insert(r);
        }
        for k in [&b"FontFile"[..], b"FontFile2", b"FontFile3"] {
            if let Ok(r) = dict.get(k).and_then(Object::as_reference) {
                fonts.insert(r);
            }
        }
    }
    for (_, pid) in doc.get_pages() {
        for c in doc.get_page_contents(pid) {
            contents.insert(c);
        }
    }

    let mut by_kind: HashMap<&str, (u64, u64)> = HashMap::new();
    let mut pixel_hash: HashMap<u64, Vec<(ObjectId, usize)>> = HashMap::new();
    let mut filters: HashMap<String, (u64, u64)> = HashMap::new();
    for (id, obj) in &doc.objects {
        let Object::Stream(s) = obj else { continue };
        let sub = s.dict.get(b"Subtype").and_then(Object::as_name).ok();
        let kind = if smasks.contains(id) {
            "smask"
        } else if sub == Some(b"Image") {
            "image"
        } else if fonts.contains(id) {
            "font"
        } else if contents.contains(id) {
            "page-content"
        } else if sub == Some(b"Form") {
            "form-xobject"
        } else if s.dict.get(b"Type").and_then(Object::as_name).ok() == Some(b"ObjStm") {
            "objstm"
        } else {
            "other-stream"
        };
        let e = by_kind.entry(kind).or_default();
        e.0 += 1;
        e.1 += s.content.len() as u64;

        if kind == "image" || kind == "smask" {
            let f = s
                .filters()
                .map(|v| {
                    v.iter()
                        .map(|x| String::from_utf8_lossy(x).into_owned())
                        .collect::<Vec<_>>()
                        .join("+")
                })
                .unwrap_or_else(|_| "none".into());
            let e = filters.entry(format!("{kind}:{f}")).or_default();
            e.0 += 1;
            e.1 += s.content.len() as u64;

            let decoded = s
                .decompressed_content()
                .unwrap_or_else(|_| s.content.clone());
            let mut h = DefaultHasher::new();
            decoded.hash(&mut h);
            s.dict
                .get(b"Width")
                .ok()
                .map(|o| format!("{o:?}"))
                .hash(&mut h);
            s.dict
                .get(b"Height")
                .ok()
                .map(|o| format!("{o:?}"))
                .hash(&mut h);
            pixel_hash
                .entry(h.finish())
                .or_default()
                .push((*id, s.content.len()));
        }
    }

    let total: u64 = std::fs::metadata(&path).unwrap().len();
    println!("file: {} bytes, {} pages", total, doc.get_pages().len());
    let mut kinds: Vec<_> = by_kind.into_iter().collect();
    kinds.sort_by_key(|(_, (_, b))| std::cmp::Reverse(*b));
    for (k, (n, b)) in kinds {
        println!("  {k:<14} {n:>6} streams  {:>8.2} MB", b as f64 / 1e6);
    }
    println!("image filters:");
    let mut fl: Vec<_> = filters.into_iter().collect();
    fl.sort_by_key(|(_, (_, b))| std::cmp::Reverse(*b));
    for (k, (n, b)) in fl {
        println!("  {k:<30} {n:>6}  {:>8.2} MB", b as f64 / 1e6);
    }
    let mut dup_objs = 0usize;
    let mut dup_bytes = 0u64;
    for v in pixel_hash.values() {
        if v.len() > 1 {
            dup_objs += v.len() - 1;
            dup_bytes += v[1..].iter().map(|(_, l)| *l as u64).sum::<u64>();
        }
    }
    println!(
        "decoded-identical image duplicates: {dup_objs} objects, {:.2} MB reclaimable",
        dup_bytes as f64 / 1e6
    );

    // Image resolutions (soft masks excluded), colour spaces, total pixels.
    let mut buckets = [0usize; 4];
    let mut megapixels = 0f64;
    let mut max_side = 0i64;
    let mut spaces: HashMap<String, usize> = HashMap::new();
    for (id, obj) in &doc.objects {
        let Object::Stream(s) = obj else { continue };
        if smasks.contains(id)
            || s.dict.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Image")
        {
            continue;
        }
        let w = s.dict.get(b"Width").and_then(Object::as_i64).unwrap_or(0);
        let h = s.dict.get(b"Height").and_then(Object::as_i64).unwrap_or(0);
        megapixels += (w * h) as f64 / 1e6;
        max_side = max_side.max(w.max(h));
        buckets[match w.max(h) {
            0..=255 => 0,
            256..=1023 => 1,
            1024..=2000 => 2,
            _ => 3,
        }] += 1;
        let cs = match s.dict.get(b"ColorSpace") {
            Ok(Object::Name(n)) => String::from_utf8_lossy(n).into_owned(),
            Ok(Object::Array(a)) => a
                .first()
                .and_then(|o| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default(),
            Ok(Object::Reference(r)) => match doc.get_object(*r) {
                Ok(Object::Name(n)) => String::from_utf8_lossy(n).into_owned(),
                Ok(Object::Array(a)) => a
                    .first()
                    .and_then(|o| o.as_name().ok())
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_default(),
                _ => "?".into(),
            },
            _ => "none".into(),
        };
        *spaces.entry(cs).or_default() += 1;
    }
    println!(
        "image sizes (longest side): <256: {}, 256-1023: {}, 1024-2000: {}, >2000: {}; largest {} px; {:.1} Mpx total",
        buckets[0], buckets[1], buckets[2], buckets[3], max_side, megapixels
    );
    let mut sp: Vec<_> = spaces.into_iter().collect();
    sp.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!(
        "image colour spaces: {}",
        sp.iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Fonts: dictionaries by subtype vs distinct embedded programs; ICC profiles.
    let mut font_dicts: HashMap<String, usize> = HashMap::new();
    let mut icc = 0usize;
    for obj in doc.objects.values() {
        let dict = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => {
                if s.dict.has(b"N") && s.dict.has(b"Alternate")
                    || s.dict.get(b"N").is_ok() && !s.dict.has(b"Subtype") && !s.dict.has(b"Type")
                {
                    icc += 1;
                }
                continue;
            }
            _ => continue,
        };
        if dict.get(b"Type").and_then(Object::as_name).ok() == Some(b"Font")
            && let Ok(st) = dict.get(b"Subtype").and_then(Object::as_name)
        {
            *font_dicts
                .entry(String::from_utf8_lossy(st).into_owned())
                .or_default() += 1;
        }
    }
    let mut fd: Vec<_> = font_dicts.into_iter().collect();
    fd.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!(
        "font dictionaries: {}; embedded font programs: {}",
        fd.iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(", "),
        fonts.len()
    );
    println!("ICC profiles: {icc}; total objects: {}", doc.objects.len());
}
