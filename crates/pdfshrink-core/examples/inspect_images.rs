//! Diagnostic: lists every Image XObject in a PDF with size/filter info.
//! `cargo run --release -p pdfshrink-core --example inspect_images -- file.pdf`
use std::env;

use lopdf::{Document, Object};

fn main() {
    let path = env::args()
        .nth(1)
        .expect("usage: inspect_images <file.pdf>");
    let doc = Document::load(&path).unwrap();

    let mut total_content = 0u64;
    let mut count = 0u64;
    let mut over_2000 = 0u64;

    for (id, obj) in doc.objects.iter() {
        let Object::Stream(s) = obj else { continue };
        let is_image = s
            .dict
            .get(b"Subtype")
            .and_then(Object::as_name)
            .map(|n| n == b"Image")
            .unwrap_or(false);
        if !is_image {
            continue;
        }
        let w = s.dict.get(b"Width").and_then(Object::as_i64).unwrap_or(-1);
        let h = s.dict.get(b"Height").and_then(Object::as_i64).unwrap_or(-1);
        let filter = s
            .filters()
            .map(|v| {
                v.iter()
                    .map(|f| String::from_utf8_lossy(f).to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|_| "none".to_string());
        let cs = s
            .dict
            .get(b"ColorSpace")
            .map(|o| format!("{o:?}"))
            .unwrap_or_default();
        count += 1;
        total_content += s.content.len() as u64;
        if w.max(h) > 2000 {
            over_2000 += 1;
        }
        println!(
            "{:?}  {}x{}  filter={filter}  len={}  cs={}",
            id,
            w,
            h,
            s.content.len(),
            cs
        );
    }
    eprintln!(
        "--- {count} images, {} MB total content, {over_2000} with longest side > 2000px ---",
        total_content / 1_000_000
    );
}
