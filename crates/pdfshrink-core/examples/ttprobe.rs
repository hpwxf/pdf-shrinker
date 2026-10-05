//! Diagnostic: simple `/TrueType` fonts (not CID) — how each subset maps char codes to glyphs.
//! Usage: ttprobe file.pdf [BaseFontSubstring]
use lopdf::{Document, Object};

fn rd16(b: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_be_bytes(b.get(o..o + 2)?.try_into().ok()?) as usize)
}
fn rd32(b: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize)
}

fn table<'a>(b: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let n = rd16(b, 4)?;
    for i in 0..n {
        let r = 12 + 16 * i;
        if b.get(r..r + 4)? == tag {
            let (o, l) = (rd32(b, r + 8)?, rd32(b, r + 12)?);
            return b.get(o..o + l);
        }
    }
    None
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let filter = std::env::args().nth(2).unwrap_or_default();
    let doc = Document::load(&path).unwrap();
    for (id, obj) in &doc.objects {
        let Ok(d) = obj.as_dict() else { continue };
        let sub = d.get(b"Subtype").and_then(Object::as_name).ok();
        if sub == Some(b"CIDFontType2") {
            let bf = d
                .get(b"BaseFont")
                .and_then(Object::as_name)
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default();
            if bf.contains(&filter) {
                let map = match d.get(b"CIDToGIDMap") {
                    Ok(Object::Name(n)) => String::from_utf8_lossy(n).into_owned(),
                    Ok(Object::Reference(r)) => format!("stream {r:?}"),
                    _ => "none".into(),
                };
                println!("{id:?} CIDFontType2 {bf} CIDToGIDMap={map}");
                let bytes = d
                    .get(b"FontDescriptor")
                    .ok()
                    .and_then(|o| doc.dereference(o).ok())
                    .and_then(|(_, o)| o.as_dict().ok())
                    .and_then(|fd| fd.get(b"FontFile2").and_then(Object::as_reference).ok())
                    .and_then(|ff| doc.get_object(ff).and_then(Object::as_stream).ok())
                    .and_then(|s| s.decompressed_content().ok());
                if let Some(b) = bytes {
                    let ng = table(&b, b"maxp").and_then(|m| rd16(m, 4)).unwrap_or(0);
                    let (head, loca) = (table(&b, b"head"), table(&b, b"loca"));
                    let (mut used, mut first) = (Vec::new(), None);
                    if let (Some(head), Some(loca)) = (head, loca) {
                        let long = rd16(head, 50) == Some(1);
                        let off = |g: usize| {
                            if long {
                                rd32(loca, 4 * g)
                            } else {
                                rd16(loca, 2 * g).map(|v| v * 2)
                            }
                        };
                        used = (0..ng).filter(|&g| off(g + 1) > off(g)).collect();
                        first = off(0);
                    }
                    let sizes: Vec<String> = (0..rd16(&b, 4).unwrap_or(0))
                        .map(|i| {
                            let r = 12 + 16 * i;
                            format!(
                                "{}={}",
                                String::from_utf8_lossy(&b[r..r + 4]),
                                rd32(&b, r + 12).unwrap_or(0)
                            )
                        })
                        .collect();
                    println!(
                        "   numGlyphs={ng} nonEmpty={} gids={:?}.. glyf0={first:?} tables: {}",
                        used.len(),
                        &used[..used.len().min(14)],
                        sizes.join(" ")
                    );
                }
            }
            continue;
        }
        if sub != Some(b"TrueType") {
            continue;
        }
        let name = d
            .get(b"BaseFont")
            .and_then(Object::as_name)
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        if !name.contains(&filter) {
            continue;
        }
        let enc = match d.get(b"Encoding") {
            Ok(Object::Name(n)) => String::from_utf8_lossy(n).into_owned(),
            Ok(Object::Dictionary(_)) | Ok(Object::Reference(_)) => "dict/ref".into(),
            _ => "none".into(),
        };
        let first = d.get(b"FirstChar").and_then(Object::as_i64).unwrap_or(-1);
        let last = d.get(b"LastChar").and_then(Object::as_i64).unwrap_or(-1);
        let Some(ff) = d
            .get(b"FontDescriptor")
            .ok()
            .and_then(|o| doc.dereference(o).ok())
            .and_then(|(_, o)| o.as_dict().ok())
            .and_then(|fd| fd.get(b"FontFile2").and_then(Object::as_reference).ok())
        else {
            continue;
        };
        let Some(bytes) = doc
            .get_object(ff)
            .and_then(Object::as_stream)
            .ok()
            .and_then(|s| s.decompressed_content().ok())
        else {
            continue;
        };
        let (Some(maxp), Some(head), Some(loca)) = (
            table(&bytes, b"maxp"),
            table(&bytes, b"head"),
            table(&bytes, b"loca"),
        ) else {
            continue;
        };
        let ng = rd16(maxp, 4).unwrap_or(0);
        let long = rd16(head, 50) == Some(1);
        let off = |g: usize| {
            if long {
                rd32(loca, 4 * g)
            } else {
                rd16(loca, 2 * g).map(|v| v * 2)
            }
        };
        let used: Vec<usize> = (0..ng).filter(|&g| off(g + 1) > off(g)).collect();
        let tabs: Vec<String> = (0..rd16(&bytes, 4).unwrap_or(0))
            .map(|i| String::from_utf8_lossy(&bytes[12 + 16 * i..16 + 16 * i]).into_owned())
            .collect();
        println!(
            "{:?} {name} enc={enc} chars={first}..{last} numGlyphs={ng} nonEmpty={} len={} tables={}",
            id,
            used.len(),
            bytes.len(),
            tabs.join(",")
        );
        println!("   gids: {:?}", &used[..used.len().min(24)]);
        // cmap subtables and what the used char codes map to
        if let Some(cmap) = table(&bytes, b"cmap") {
            for i in 0..rd16(cmap, 2).unwrap_or(0) {
                let (p, e, o) = (
                    rd16(cmap, 4 + 8 * i).unwrap_or(0),
                    rd16(cmap, 6 + 8 * i).unwrap_or(0),
                    rd32(cmap, 8 + 8 * i).unwrap_or(0),
                );
                let fmt = rd16(cmap, o).unwrap_or(0);
                let mut pairs = Vec::new();
                if fmt == 0 {
                    for c in 0..256 {
                        if let Some(&g) = cmap.get(o + 6 + c)
                            && g != 0
                        {
                            pairs.push((c, g as usize));
                        }
                    }
                } else if fmt == 4 {
                    let segx2 = rd16(cmap, o + 6).unwrap_or(0);
                    let (ends, starts) = (o + 14, o + 16 + segx2);
                    let (deltas, ranges) = (starts + segx2, starts + 2 * segx2);
                    for s in 0..segx2 / 2 {
                        let (end, start) = (
                            rd16(cmap, ends + 2 * s).unwrap_or(0),
                            rd16(cmap, starts + 2 * s).unwrap_or(0),
                        );
                        let delta = rd16(cmap, deltas + 2 * s).unwrap_or(0);
                        let ro = rd16(cmap, ranges + 2 * s).unwrap_or(0);
                        for c in start..=end.min(0xFFFE) {
                            let g = if ro == 0 {
                                (c + delta) & 0xFFFF
                            } else {
                                let a = ranges + 2 * s + ro + 2 * (c - start);
                                match rd16(cmap, a) {
                                    Some(0) | None => 0,
                                    Some(v) => (v + delta) & 0xFFFF,
                                }
                            };
                            if g != 0 {
                                pairs.push((c, g));
                            }
                        }
                    }
                }
                println!(
                    "   cmap({p},{e}) fmt{fmt}: {} entries, e.g. {:?}",
                    pairs.len(),
                    &pairs[..pairs.len().min(10)]
                );
            }
        }
    }
}
