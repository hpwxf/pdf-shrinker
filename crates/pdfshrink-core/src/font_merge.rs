//! Merges per-page subsets of the same TrueType CID font into one font program
//! (every level, see `Profile::merge_fonts`).
//!
//! Slide exports (Keynote, PowerPoint via Quartz, …) embed a fresh subset of
//! each font on every page: 100 subsets of Courier New at ~10 KB each, when
//! one merged program is ~30 KB. Those subsetters keep the original glyph IDs
//! (unused glyphs are emptied, `numGlyphs` truncated after the highest one
//! used), and the fonts are `CIDFontType2` with an `Identity` CID→GID map, so
//! merging is a per-GID union of the `glyf` data — no content stream or
//! `/W` array has to change.
//!
//! Subsets are only merged when that's provably consistent: same base font
//! name, same hinting programs (`fpgm`/`prep`/`cvt `), and byte-identical glyph
//! data for every GID more than one of them defines. A subset conflicting with
//! a group simply starts (or joins) another group.

use std::collections::{BTreeMap, HashMap};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

/// How many font descriptors point at each `FontFile2` program. A program
/// can only be rewritten in place when every one of them gets repointed.
fn fontfile2_users(doc: &Document) -> HashMap<ObjectId, usize> {
    let mut users = HashMap::new();
    for obj in doc.objects.values() {
        if let Ok(ff) = obj
            .as_dict()
            .and_then(|d| d.get(b"FontFile2"))
            .and_then(Object::as_reference)
        {
            *users.entry(ff).or_insert(0) += 1;
        }
    }
    users
}

/// Returns the number of font programs merged away.
pub fn merge_truetype_subsets(doc: &mut Document) -> usize {
    // FontFile2 stream id -> (base name, list of FontDescriptor ids using it).
    let mut by_file: BTreeMap<ObjectId, (Vec<u8>, Vec<ObjectId>)> = BTreeMap::new();
    for obj in doc.objects.values() {
        let Ok(cid_font) = obj.as_dict() else {
            continue;
        };
        if cid_font.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"CIDFontType2") {
            continue;
        }
        let identity = match cid_font.get(b"CIDToGIDMap") {
            Err(_) => true,
            Ok(Object::Name(n)) => n == b"Identity",
            Ok(_) => false,
        };
        if !identity {
            continue;
        }
        let Ok(fd_id) = cid_font
            .get(b"FontDescriptor")
            .and_then(Object::as_reference)
        else {
            continue;
        };
        let Ok(fd) = doc.get_dictionary(fd_id) else {
            continue;
        };
        let Ok(ff_id) = fd.get(b"FontFile2").and_then(Object::as_reference) else {
            continue;
        };
        let name = fd
            .get(b"FontName")
            .and_then(Object::as_name)
            .map(base_name)
            .unwrap_or_default();
        let e = by_file.entry(ff_id).or_insert_with(|| (name, Vec::new()));
        if !e.1.contains(&fd_id) {
            e.1.push(fd_id);
        }
    }

    // Only the CIDFontType2/Identity descriptors collected above get
    // repointed; anything else sharing a font file keeps the original.
    // Ordered: groups create objects, and their numbering must not depend
    // on hashing (reproducible output).
    let mut groups: BTreeMap<Vec<u8>, Vec<(ObjectId, Sfnt)>> = BTreeMap::new();
    for (ff_id, (name, _)) in &by_file {
        if name.is_empty() {
            continue;
        }
        let Some(bytes) = doc
            .get_object(*ff_id)
            .and_then(Object::as_stream)
            .ok()
            .and_then(|s| s.decompressed_content().ok().or_else(|| plain(s)))
        else {
            continue;
        };
        if let Some(sfnt) = Sfnt::parse(&bytes) {
            groups.entry(name.clone()).or_default().push((*ff_id, sfnt));
        }
    }

    let users = fontfile2_users(doc);
    let mut merged_away = 0;
    let mut replaced = false;
    for (_, subsets) in groups {
        for cluster in cluster_consistent(subsets) {
            if cluster.len() == 1 && users[&cluster[0].0] != by_file[&cluster[0].0].1.len() {
                continue;
            }
            let Some(bytes) = build_merged(&cluster) else {
                continue;
            };
            let mut dict = Dictionary::new();
            dict.set("Length1", bytes.len() as i64);
            let mut stream = Stream::new(dict, bytes);
            let _ = stream.compress();
            // A lone program is only rewritten (name table trimmed, …) if that
            // makes it smaller.
            let before: usize = cluster
                .iter()
                .filter_map(|(id, _)| doc.get_object(*id).and_then(Object::as_stream).ok())
                .map(|s| s.content.len())
                .sum();
            if stream.content.len() >= before {
                continue;
            }
            replaced = true;
            let new_id = doc.add_object(Object::Stream(stream));
            for (ff_id, _) in &cluster {
                for fd_id in &by_file[ff_id].1 {
                    if let Ok(fd) = doc.get_dictionary_mut(*fd_id) {
                        fd.set("FontFile2", Object::Reference(new_id));
                        // CIDSet lists the CIDs present in the program; the
                        // merged one has more. It's optional (and deprecated
                        // in PDF 2.0), so drop rather than widen it.
                        fd.remove(b"CIDSet");
                    }
                }
            }
            merged_away += cluster.len() - 1;
        }
    }
    if replaced {
        doc.prune_objects();
    }
    merged_away
}

/// Simple (single-byte) `/TrueType` fonts, as Word and Office write them: one
/// subset per use, named `ABCDEF+TimesNewRoman`. These subsets keep the
/// original GIDs too, but each still carries the *whole* font's `cmap`, `hmtx`,
/// `loca` and `post` (thousands of glyphs for a few dozen used). Merging
/// subsets of one font is the same per-GID union, plus a `cmap` union; the
/// result's tables are cut to the highest glyph actually kept, and its `cmap`
/// to the entries that point at a drawn glyph (or at a space).
///
/// Returns the number of font programs merged away. A font program used by a
/// single descriptor is trimmed too, if that makes it smaller.
pub fn merge_simple_truetype(doc: &mut Document) -> usize {
    let mut by_file: BTreeMap<ObjectId, (Vec<u8>, Vec<ObjectId>)> = BTreeMap::new();
    // Programs we must not touch: also used by a CID font, or by a font whose
    // glyphs may be found through the `post` names we drop.
    let mut excluded: Vec<ObjectId> = Vec::new();
    let mut usage: HashMap<ObjectId, Usage> = HashMap::new();
    for obj in doc.objects.values() {
        let Ok(font) = obj.as_dict() else {
            continue;
        };
        let subtype = font.get(b"Subtype").and_then(Object::as_name).ok();
        let simple = subtype == Some(b"TrueType");
        if !simple && subtype != Some(b"CIDFontType2") {
            continue;
        }
        let Ok(fd_id) = font.get(b"FontDescriptor").and_then(Object::as_reference) else {
            continue;
        };
        let Ok(fd) = doc.get_dictionary(fd_id) else {
            continue;
        };
        let Ok(ff_id) = fd.get(b"FontFile2").and_then(Object::as_reference) else {
            continue;
        };
        let named_encoding = match font.get(b"Encoding") {
            Err(_) => true,
            Ok(Object::Name(_)) => true,
            Ok(_) => false, // /Differences: glyph names, resolved through `post`
        };
        if !simple || !named_encoding {
            excluded.push(ff_id);
            continue;
        }
        let name = fd
            .get(b"FontName")
            .and_then(Object::as_name)
            .map(base_name)
            .unwrap_or_default();
        let e = by_file.entry(ff_id).or_insert_with(|| (name, Vec::new()));
        if !e.1.contains(&fd_id) {
            e.1.push(fd_id);
        }
        let code = |key: &[u8], default: i64| {
            font.get(key)
                .and_then(Object::as_i64)
                .unwrap_or(default)
                .clamp(0, 255) as u16
        };
        let (first, last) = (code(b"FirstChar", 0), code(b"LastChar", 255));
        let u = usage.entry(ff_id).or_insert(Usage {
            lo: first,
            hi: last,
            mac_roman: false,
        });
        u.lo = u.lo.min(first);
        u.hi = u.hi.max(last);
        u.mac_roman |= matches!(
            font.get(b"Encoding"),
            Ok(Object::Name(n)) if n.as_slice() == b"MacRomanEncoding"
        );
    }

    // Ordered: groups create objects, and their numbering must not depend
    // on hashing (reproducible output).
    let mut groups: BTreeMap<Vec<u8>, Vec<SimpleSubset>> = BTreeMap::new();
    for (ff_id, (name, _)) in &by_file {
        if name.is_empty() || excluded.contains(ff_id) {
            continue;
        }
        let Some(stream) = doc.get_object(*ff_id).and_then(Object::as_stream).ok() else {
            continue;
        };
        let Some(bytes) = stream.decompressed_content().ok().or_else(|| plain(stream)) else {
            continue;
        };
        let stored = stream.content.len();
        let Some(mut sfnt) = Sfnt::parse(&bytes) else {
            continue;
        };
        let Some(cmap) = sfnt.table(b"cmap").and_then(|t| parse_cmap(t)) else {
            continue;
        };
        if let Some(u) = usage.get(ff_id).filter(|u| !u.mac_roman) {
            reduce_to_reachable(&mut sfnt, &cmap, u);
        }
        groups.entry(name.clone()).or_default().push(SimpleSubset {
            id: *ff_id,
            sfnt,
            cmap,
            stored,
        });
    }

    let users = fontfile2_users(doc);
    let mut merged_away = 0;
    let mut replaced = false;
    for (_, subsets) in groups {
        for cluster in cluster_by(subsets, simple_consistent) {
            if cluster
                .iter()
                .any(|s| users[&s.id] != by_file[&s.id].1.len())
            {
                continue;
            }
            let Some(bytes) = build_simple(&cluster) else {
                continue;
            };
            let mut dict = Dictionary::new();
            dict.set("Length1", bytes.len() as i64);
            let mut stream = Stream::new(dict, bytes);
            let _ = stream.compress();
            // Only keep it if it beats what it replaces.
            let before: usize = cluster.iter().map(|s| s.stored).sum();
            if stream.content.len() >= before {
                continue;
            }
            replaced = true;
            let new_id = doc.add_object(Object::Stream(stream));
            for s in &cluster {
                for fd_id in &by_file[&s.id].1 {
                    if let Ok(fd) = doc.get_dictionary_mut(*fd_id) {
                        fd.set("FontFile2", Object::Reference(new_id));
                    }
                }
            }
            merged_away += cluster.len() - 1;
        }
    }
    if replaced {
        doc.prune_objects();
    }
    merged_away
}

/// Char codes a simple font's dictionaries can show, and how they name them.
struct Usage {
    lo: u16,
    hi: u16,
    /// `/MacRomanEncoding`: codes above 0x7F mean other characters, so we
    /// don't try to work out which glyphs are reachable.
    mac_roman: bool,
}

/// Unicode for the WinAnsi codes 0x80..=0x9F (undefined ones show a bullet).
const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0x2022, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x2022, 0x017D, 0x2022, 0x2022, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x2022, 0x017E, 0x0178,
];

/// Blanks every glyph no char code in `u`'s range can reach. Fonts embedded
/// whole (Office "Save as PDF" on a form, thousands of glyphs) shrink to the
/// ~200 an 8-bit encoding can name; real subsets are unaffected. A glyph's
/// components are kept with it.
fn reduce_to_reachable(sfnt: &mut Sfnt, cmap: &[CmapSub], u: &Usage) {
    let mut unicodes: Vec<u16> = vec![0x20, 0x2D, 0x2022];
    for c in u.lo..=u.hi {
        unicodes.push(match c {
            0x80..=0x9F => CP1252_HIGH[(c - 0x80) as usize],
            _ => c,
        });
        unicodes.push(c);
        unicodes.push(0xF000 + c);
    }
    let mut keep = vec![false; sfnt.glyphs.len()];
    let mut mark = |g: u16| {
        if let Some(k) = keep.get_mut(g as usize) {
            *k = true;
        }
    };
    mark(0);
    for sub in cmap {
        for (&code, &gid) in &sub.map {
            let reached = if sub.platform == 1 {
                // Raw Mac codes. A WinAnsi name above 0x7F can sit at any Mac code.
                (u.lo..=u.hi).contains(&code) || (u.hi >= 0x80 && (0x80..=0xFF).contains(&code))
            } else {
                unicodes.contains(&code)
            };
            if reached {
                mark(gid);
            }
        }
    }
    // Composite glyphs pull their components in.
    let mut g = 0;
    while g < keep.len() {
        if keep[g] {
            for part in components(&sfnt.glyphs[g]) {
                if let Some(k) = keep.get_mut(part as usize)
                    && !*k
                {
                    *k = true;
                    g = g.min(part as usize); // revisit from the new one
                }
            }
        }
        g += 1;
    }
    for (g, k) in keep.iter().enumerate() {
        if !k {
            sfnt.glyphs[g].clear();
        }
    }
}

/// Glyph IDs a composite `glyf` entry refers to (none for a simple glyph).
fn components(glyph: &[u8]) -> Vec<u16> {
    let mut parts = Vec::new();
    if rd16(glyph, 0).is_none_or(|n| n < 0x8000) {
        return parts; // numberOfContours >= 0: simple glyph
    }
    let mut at = 10;
    while let (Some(flags), Some(gid)) = (rd16(glyph, at), rd16(glyph, at + 2)) {
        parts.push(gid);
        at += 4 + if flags & 0x0001 != 0 { 4 } else { 2 };
        at += if flags & 0x0008 != 0 {
            2
        } else if flags & 0x0040 != 0 {
            4
        } else if flags & 0x0080 != 0 {
            8
        } else {
            0
        };
        if flags & 0x0020 == 0 {
            break;
        }
    }
    parts
}

struct SimpleSubset {
    id: ObjectId,
    sfnt: Sfnt,
    cmap: Vec<CmapSub>,
    /// Size of the stored (compressed) stream, for the "is it smaller" check.
    stored: usize,
}

/// One `cmap` subtable: (platform, encoding) and its code → GID entries.
struct CmapSub {
    platform: u16,
    encoding: u16,
    map: BTreeMap<u16, u16>,
}

fn simple_consistent(a: &SimpleSubset, b: &SimpleSubset) -> bool {
    if !consistent(&a.sfnt, &b.sfnt) {
        return false;
    }
    // The same char code must lead to the same glyph in both.
    a.cmap.iter().all(|x| {
        b.cmap
            .iter()
            .filter(|y| (y.platform, y.encoding) == (x.platform, x.encoding))
            .all(|y| {
                x.map
                    .iter()
                    .all(|(code, gid)| y.map.get(code).is_none_or(|g| g == gid))
            })
    })
}

/// Parses formats 0, 4 and 6; anything else (12, 2…) makes the font ineligible
/// rather than silently losing a subtable.
fn parse_cmap(b: &[u8]) -> Option<Vec<CmapSub>> {
    let n = rd16(b, 2)? as usize;
    let mut subs = Vec::new();
    for i in 0..n {
        let platform = rd16(b, 4 + 8 * i)?;
        let encoding = rd16(b, 6 + 8 * i)?;
        let o = rd32(b, 8 + 8 * i)? as usize;
        let mut map = BTreeMap::new();
        match rd16(b, o)? {
            0 => {
                for c in 0..256usize {
                    let g = *b.get(o + 6 + c)?;
                    if g != 0 {
                        map.insert(c as u16, g as u16);
                    }
                }
            }
            4 => {
                let segx2 = rd16(b, o + 6)? as usize;
                let (ends, starts) = (o + 14, o + 16 + segx2);
                let (deltas, ranges) = (starts + segx2, starts + 2 * segx2);
                for s in 0..segx2 / 2 {
                    let end = rd16(b, ends + 2 * s)? as usize;
                    let start = rd16(b, starts + 2 * s)? as usize;
                    let delta = rd16(b, deltas + 2 * s)?;
                    let range_off = rd16(b, ranges + 2 * s)? as usize;
                    for c in start..=end.min(0xFFFE) {
                        let g = if range_off == 0 {
                            (c as u16).wrapping_add(delta)
                        } else {
                            match rd16(b, ranges + 2 * s + range_off + 2 * (c - start))? {
                                0 => 0,
                                v => v.wrapping_add(delta),
                            }
                        };
                        if g != 0 {
                            map.insert(c as u16, g);
                        }
                    }
                }
            }
            6 => {
                let first = rd16(b, o + 6)?;
                let count = rd16(b, o + 8)? as usize;
                for i in 0..count {
                    let g = rd16(b, o + 10 + 2 * i)?;
                    if g != 0 {
                        map.insert(first.checked_add(i as u16)?, g);
                    }
                }
            }
            _ => return None,
        }
        subs.push(CmapSub {
            platform,
            encoding,
            map,
        });
    }
    Some(subs)
}

/// Codes for which an *empty* glyph is legitimate (space, no-break space, and
/// their symbolic-font `0xF0xx` form): the cmap entry must survive.
fn is_space_code(code: u16) -> bool {
    matches!(code, 0x20 | 0xA0 | 0xF020 | 0xF0A0)
}

fn build_simple(cluster: &[SimpleSubset]) -> Option<Vec<u8>> {
    let sfnts: Vec<&Sfnt> = cluster.iter().map(|s| &s.sfnt).collect();
    let drawn = |g: usize| {
        sfnts
            .iter()
            .any(|s| s.glyphs.get(g).is_some_and(|d| !d.is_empty()))
    };

    // Union of the cmaps, restricted to entries worth keeping.
    let mut subtables: BTreeMap<(u16, u16), BTreeMap<u16, u16>> = BTreeMap::new();
    for s in cluster {
        for sub in &s.cmap {
            let dst = subtables.entry((sub.platform, sub.encoding)).or_default();
            for (&code, &gid) in &sub.map {
                if drawn(gid as usize) || is_space_code(code) {
                    dst.insert(code, gid);
                }
            }
        }
    }
    let highest_drawn = (0..sfnts.iter().map(|s| s.glyphs.len()).max()?)
        .rev()
        .find(|&g| drawn(g))
        .unwrap_or(0);
    let highest_mapped = subtables
        .values()
        .flat_map(|m| m.values())
        .map(|&g| g as usize)
        .max()
        .unwrap_or(0);
    let n = highest_drawn.max(highest_mapped) + 1;
    // A glyph past the original's last one can't be kept.
    if n > sfnts.iter().map(|s| s.glyphs.len()).max()? {
        return None;
    }
    merge_tables(&sfnts, Some(n), Some(write_cmap(&subtables)?))
}

fn write_cmap(subtables: &BTreeMap<(u16, u16), BTreeMap<u16, u16>>) -> Option<Vec<u8>> {
    let mut bodies = Vec::new();
    for (&(platform, _), map) in subtables {
        bodies.push(
            if platform == 1 && map.keys().all(|&c| c < 256) && map.values().all(|&g| g < 256) {
                cmap_format0(map)
            } else if platform == 1 {
                cmap_format6(map)?
            } else {
                cmap_format4(map)?
            },
        );
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(subtables.len() as u16).to_be_bytes());
    let mut offset = 4 + 8 * subtables.len();
    for (&(platform, encoding), body) in subtables.keys().zip(&bodies) {
        out.extend_from_slice(&platform.to_be_bytes());
        out.extend_from_slice(&encoding.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        offset += body.len();
    }
    for b in bodies {
        out.extend_from_slice(&b);
    }
    Some(out)
}

fn cmap_format0(map: &BTreeMap<u16, u16>) -> Vec<u8> {
    let mut t = vec![0u8; 262];
    t[2..4].copy_from_slice(&262u16.to_be_bytes());
    for (&c, &g) in map {
        t[6 + c as usize] = g as u8;
    }
    t
}

fn cmap_format6(map: &BTreeMap<u16, u16>) -> Option<Vec<u8>> {
    let first = *map.keys().next().unwrap_or(&0);
    let last = *map.keys().next_back().unwrap_or(&0);
    let count = (last - first) as usize + 1;
    let mut t = Vec::new();
    t.extend_from_slice(&6u16.to_be_bytes());
    t.extend_from_slice(&u16::try_from(10 + 2 * count).ok()?.to_be_bytes());
    t.extend_from_slice(&0u16.to_be_bytes());
    t.extend_from_slice(&first.to_be_bytes());
    t.extend_from_slice(&(count as u16).to_be_bytes());
    for c in first..=last {
        t.extend_from_slice(&map.get(&c).copied().unwrap_or(0).to_be_bytes());
    }
    Some(t)
}

/// Format 4 with one segment per run of consecutive codes whose GIDs are
/// consecutive too (`idDelta` only, no `glyphIdArray`).
fn cmap_format4(map: &BTreeMap<u16, u16>) -> Option<Vec<u8>> {
    let mut segs: Vec<(u16, u16, u16)> = Vec::new(); // start, end, delta
    for (&c, &g) in map.iter().filter(|(c, _)| **c != 0xFFFF) {
        let delta = g.wrapping_sub(c);
        match segs.last_mut() {
            Some((_, end, d)) if *end + 1 == c && *d == delta => *end = c,
            _ => segs.push((c, c, delta)),
        }
    }
    segs.push((0xFFFF, 0xFFFF, 1));
    let n = segs.len();
    let mut pow = 1usize;
    let mut log = 0u16;
    while pow * 2 <= n {
        pow *= 2;
        log += 1;
    }
    let mut t = Vec::new();
    t.extend_from_slice(&4u16.to_be_bytes());
    t.extend_from_slice(&u16::try_from(16 + 8 * n).ok()?.to_be_bytes());
    t.extend_from_slice(&0u16.to_be_bytes());
    t.extend_from_slice(&(2 * n as u16).to_be_bytes());
    t.extend_from_slice(&(2 * pow as u16).to_be_bytes());
    t.extend_from_slice(&log.to_be_bytes());
    t.extend_from_slice(&((2 * n - 2 * pow) as u16).to_be_bytes());
    for &(_, end, _) in &segs {
        t.extend_from_slice(&end.to_be_bytes());
    }
    t.extend_from_slice(&0u16.to_be_bytes());
    for &(start, _, _) in &segs {
        t.extend_from_slice(&start.to_be_bytes());
    }
    for &(_, _, delta) in &segs {
        t.extend_from_slice(&delta.to_be_bytes());
    }
    t.extend(std::iter::repeat_n(0u8, 2 * n)); // idRangeOffset
    Some(t)
}

fn plain(s: &Stream) -> Option<Vec<u8>> {
    s.dict.get(b"Filter").is_err().then(|| s.content.clone())
}

fn base_name(n: &[u8]) -> Vec<u8> {
    // Subset prefix is "ABCDEF+".
    if n.len() > 7 && n[6] == b'+' && n[..6].iter().all(u8::is_ascii_uppercase) {
        n[7..].to_vec()
    } else {
        n.to_vec()
    }
}

/// Greedy clustering: each subset joins the first cluster it's consistent with.
fn cluster_consistent(subsets: Vec<(ObjectId, Sfnt)>) -> Vec<Vec<(ObjectId, Sfnt)>> {
    cluster_by(subsets, |a, b| consistent(&a.1, &b.1))
}

fn cluster_by<T>(items: Vec<T>, ok: impl Fn(&T, &T) -> bool) -> Vec<Vec<T>> {
    let mut clusters: Vec<Vec<T>> = Vec::new();
    'next: for it in items {
        for c in clusters.iter_mut() {
            if c.iter().all(|o| ok(o, &it)) {
                c.push(it);
                continue 'next;
            }
        }
        clusters.push(vec![it]);
    }
    clusters
}

fn consistent(a: &Sfnt, b: &Sfnt) -> bool {
    for tag in [b"fpgm", b"prep", b"cvt ", b"head"] {
        let (ta, tb) = (a.table(tag), b.table(tag));
        let same = match (ta, tb) {
            // head differs in checksum adjustment / modified date; compare the
            // fields that matter for rendering.
            (Some(x), Some(y)) if tag == b"head" => {
                x.len() >= 54 && y.len() >= 54 && x[18..20] == y[18..20] // unitsPerEm
            }
            (x, y) => x == y,
        };
        if !same {
            return false;
        }
    }
    let n = a.glyphs.len().min(b.glyphs.len());
    (0..n).all(|g| a.glyphs[g].is_empty() || b.glyphs[g].is_empty() || a.glyphs[g] == b.glyphs[g])
}

/// A parsed TrueType font: raw tables plus `glyf` split per glyph and `hmtx`
/// expanded to one (advance, lsb) per glyph.
struct Sfnt {
    tables: BTreeMap<[u8; 4], Vec<u8>>,
    glyphs: Vec<Vec<u8>>,
    metrics: Vec<(u16, i16)>,
}

fn rd16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn rd32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

impl Sfnt {
    fn table(&self, tag: &[u8; 4]) -> Option<&Vec<u8>> {
        self.tables.get(tag)
    }

    fn parse(b: &[u8]) -> Option<Sfnt> {
        let version = rd32(b, 0)?;
        if version != 0x0001_0000 && version != u32::from_be_bytes(*b"true") {
            return None;
        }
        let num_tables = rd16(b, 4)? as usize;
        let mut tables = BTreeMap::new();
        for i in 0..num_tables {
            let rec = 12 + 16 * i;
            let tag: [u8; 4] = b.get(rec..rec + 4)?.try_into().ok()?;
            let off = rd32(b, rec + 8)? as usize;
            let len = rd32(b, rec + 12)? as usize;
            tables.insert(tag, b.get(off..off.checked_add(len)?)?.to_vec());
        }
        let head = tables.get(b"head")?;
        let maxp = tables.get(b"maxp")?;
        let hhea = tables.get(b"hhea")?;
        let hmtx = tables.get(b"hmtx")?;
        let loca = tables.get(b"loca")?;
        let glyf = tables.get(b"glyf")?;
        let long_loca = rd16(head, 50)? == 1;
        let num_glyphs = rd16(maxp, 4)? as usize;
        let num_hm = rd16(hhea, 34)? as usize;

        let off = |g: usize| -> Option<usize> {
            if long_loca {
                rd32(loca, 4 * g).map(|v| v as usize)
            } else {
                rd16(loca, 2 * g).map(|v| v as usize * 2)
            }
        };
        let mut glyphs = Vec::with_capacity(num_glyphs);
        for g in 0..num_glyphs {
            let (s, e) = (off(g)?, off(g + 1)?);
            if e < s {
                return None;
            }
            glyphs.push(glyf.get(s..e)?.to_vec());
        }
        let mut metrics = Vec::with_capacity(num_glyphs);
        let mut last_adv = 0u16;
        for g in 0..num_glyphs {
            if g < num_hm {
                last_adv = rd16(hmtx, 4 * g)?;
                metrics.push((last_adv, rd16(hmtx, 4 * g + 2)? as i16));
            } else {
                let lsb = rd16(hmtx, 4 * num_hm + 2 * (g - num_hm)).unwrap_or(0) as i16;
                metrics.push((last_adv, lsb));
            }
        }
        Some(Sfnt {
            tables,
            glyphs,
            metrics,
        })
    }
}

fn build_merged(cluster: &[(ObjectId, Sfnt)]) -> Option<Vec<u8>> {
    let sfnts: Vec<&Sfnt> = cluster.iter().map(|(_, s)| s).collect();
    merge_tables(&sfnts, None, None)
}

/// Union of `cluster`'s glyphs. `limit` truncates the glyph count (default: the
/// largest subset's), `cmap` replaces the (otherwise copied) `cmap` table.
fn merge_tables(cluster: &[&Sfnt], limit: Option<usize>, cmap: Option<Vec<u8>>) -> Option<Vec<u8>> {
    let n = match limit {
        Some(n) => n,
        None => cluster.iter().map(|s| s.glyphs.len()).max()?,
    };
    // Base for the tables we copy verbatim: the subset covering the most GIDs.
    let base = *cluster.iter().max_by_key(|s| s.glyphs.len())?;

    let mut glyphs: Vec<&[u8]> = vec![&[]; n];
    let mut metrics: Vec<(u16, i16)> = vec![(0, 0); n];
    for s in cluster {
        for g in 0..s.glyphs.len().min(n) {
            if glyphs[g].is_empty() && !s.glyphs[g].is_empty() {
                glyphs[g] = &s.glyphs[g];
                metrics[g] = s.metrics[g];
            }
        }
    }
    // Empty glyphs (e.g. space) still have an advance: take it from any subset.
    for g in 0..n {
        if glyphs[g].is_empty()
            && let Some(m) = cluster
                .iter()
                .find(|s| g < s.metrics.len() && s.metrics[g].0 != 0)
                .map(|s| s.metrics[g])
        {
            metrics[g] = m;
        }
    }

    let mut glyf = Vec::new();
    let mut loca = Vec::with_capacity(4 * (n + 1));
    for g in &glyphs {
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        glyf.extend_from_slice(g);
        while glyf.len() % 4 != 0 {
            glyf.push(0);
        }
    }
    loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());

    let mut hmtx = Vec::with_capacity(4 * n);
    for (adv, lsb) in &metrics {
        hmtx.extend_from_slice(&adv.to_be_bytes());
        hmtx.extend_from_slice(&lsb.to_be_bytes());
    }

    let mut tables = base.tables.clone();
    // Per-glyph tables we don't rebuild: their glyph count would be wrong.
    for tag in [
        b"hdmx", b"LTSH", b"VDMX", b"kern", b"GPOS", b"GSUB", b"GDEF", b"DSIG", b"JSTF",
    ] {
        tables.remove(tag);
    }
    let mut head = tables.get(b"head")?.clone();
    head.get_mut(50..52)?.copy_from_slice(&1u16.to_be_bytes()); // long loca
    head.get_mut(8..12)?.copy_from_slice(&0u32.to_be_bytes()); // checksum adj, set below
    let mut maxp = tables.get(b"maxp")?.clone();
    maxp.get_mut(4..6)?
        .copy_from_slice(&(n as u16).to_be_bytes());
    if maxp.len() >= 32 {
        // maxp v1.0 limits (maxPoints, maxContours, …): max over the cluster.
        for field in (6..32).step_by(2) {
            let m = cluster
                .iter()
                .filter_map(|s| s.table(b"maxp").and_then(|t| rd16(t, field)))
                .max()?;
            maxp[field..field + 2].copy_from_slice(&m.to_be_bytes());
        }
    }
    let mut hhea = tables.get(b"hhea")?.clone();
    hhea.get_mut(34..36)?
        .copy_from_slice(&(n as u16).to_be_bytes());
    let adv_max = metrics.iter().map(|m| m.0).max().unwrap_or(0);
    hhea.get_mut(10..12)?
        .copy_from_slice(&adv_max.to_be_bytes());
    // post format 2 carries one name per glyph; format 3 carries none.
    if let Some(post) = tables.get_mut(b"post")
        && post.len() >= 32
    {
        post.truncate(32);
        post[0..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
    }

    tables.insert(*b"head", head);
    tables.insert(*b"maxp", maxp);
    tables.insert(*b"hhea", hhea);
    tables.insert(*b"hmtx", hmtx);
    tables.insert(*b"loca", loca);
    tables.insert(*b"glyf", glyf);
    if let Some(name) = tables.get(b"name").and_then(|n| trim_name(n)) {
        tables.insert(*b"name", name);
    }
    if let Some(c) = cmap {
        tables.insert(*b"cmap", c);
    }
    Some(write_sfnt(&tables))
}

/// Keeps only the English family/style/full/PostScript names (IDs 1 to 6) of a
/// `name` table: Skia and Office write copyright, licence and trademark
/// strings that run to several KB per subset, and no PDF viewer reads them.
fn trim_name(name: &[u8]) -> Option<Vec<u8>> {
    if rd16(name, 0)? != 0 {
        return None;
    }
    let count = rd16(name, 2)? as usize;
    let strings = rd16(name, 4)? as usize;
    let mut kept: Vec<([u16; 4], &[u8])> = Vec::new();
    for i in 0..count {
        let r = 6 + 12 * i;
        let (platform, encoding, language, id) = (
            rd16(name, r)?,
            rd16(name, r + 2)?,
            rd16(name, r + 4)?,
            rd16(name, r + 6)?,
        );
        let (len, off) = (rd16(name, r + 8)? as usize, rd16(name, r + 10)? as usize);
        let english = (platform == 3 && language == 0x0409) || (platform == 1 && language == 0);
        if english && (1..=6).contains(&id) {
            let s = name.get(strings + off..strings + off + len)?;
            kept.push(([platform, encoding, language, id], s));
        }
    }
    if kept.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(kept.len() as u16).to_be_bytes());
    out.extend_from_slice(&((6 + 12 * kept.len()) as u16).to_be_bytes());
    let mut off = 0usize;
    for (key, s) in &kept {
        for v in key.iter().take(3) {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&key[3].to_be_bytes());
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(&(off as u16).to_be_bytes());
        off += s.len();
    }
    for (_, s) in &kept {
        out.extend_from_slice(s);
    }
    Some(out)
}

fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in data.chunks(4) {
        let mut w = [0u8; 4];
        w[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(w));
    }
    sum
}

fn write_sfnt(tables: &BTreeMap<[u8; 4], Vec<u8>>) -> Vec<u8> {
    let num = tables.len() as u16;
    let mut pow = 1u16;
    let mut log = 0u16;
    while pow * 2 <= num {
        pow *= 2;
        log += 1;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&num.to_be_bytes());
    out.extend_from_slice(&(pow * 16).to_be_bytes());
    out.extend_from_slice(&log.to_be_bytes());
    out.extend_from_slice(&(num * 16 - pow * 16).to_be_bytes());

    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    let mut head_pos = None;
    for (tag, data) in tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        if tag == b"head" {
            head_pos = Some(offset);
        }
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    out.extend_from_slice(&body);
    if let Some(p) = head_pos {
        let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[p + 8..p + 12].copy_from_slice(&adj.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake TrueType program with `n` glyphs where only `present` are
    /// non-empty; glyph `g`'s data is 12 bytes derived from `g`, so two subsets
    /// of the "same font" agree on every glyph they share.
    fn fake_subset(n: usize, present: &[usize]) -> Vec<u8> {
        let mut glyphs: Vec<Vec<u8>> = vec![Vec::new(); n];
        for &g in present {
            glyphs[g] = vec![0, 1, g as u8, 0, 0, 0, 0, 0, 0, 10, 0, 10];
        }
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        for g in &glyphs {
            loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
            glyf.extend_from_slice(g);
        }
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        let mut head = vec![0u8; 54];
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        head[50..52].copy_from_slice(&1u16.to_be_bytes());
        let mut maxp = vec![0u8; 32];
        maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&(n as u16).to_be_bytes());
        let mut hhea = vec![0u8; 36];
        hhea[34..36].copy_from_slice(&(n as u16).to_be_bytes());
        let hmtx: Vec<u8> = (0..n)
            .flat_map(|g| {
                let adv = if present.contains(&g) {
                    500 + g as u16
                } else {
                    0
                };
                [adv.to_be_bytes(), 0u16.to_be_bytes()].concat()
            })
            .collect();
        let mut t = BTreeMap::new();
        t.insert(*b"head", head);
        t.insert(*b"maxp", maxp);
        t.insert(*b"hhea", hhea);
        t.insert(*b"hmtx", hmtx);
        t.insert(*b"loca", loca);
        t.insert(*b"glyf", glyf);
        t.insert(*b"fpgm", vec![1, 2, 3, 4]);
        write_sfnt(&t)
    }

    #[test]
    fn merged_program_is_the_union_of_its_subsets() {
        let a = Sfnt::parse(&fake_subset(6, &[0, 3, 5])).unwrap();
        let b = Sfnt::parse(&fake_subset(9, &[0, 1, 8])).unwrap();
        assert!(consistent(&a, &b));
        let merged = build_merged(&[((1, 0), a), ((2, 0), b)]).unwrap();
        let m = Sfnt::parse(&merged).unwrap();
        assert_eq!(m.glyphs.len(), 9);
        for g in 0..9 {
            assert_eq!(
                !m.glyphs[g].is_empty(),
                [0, 1, 3, 5, 8].contains(&g),
                "glyph {g}"
            );
        }
        assert_eq!(m.metrics[5].0, 505);
        assert_eq!(m.metrics[8].0, 508);
        assert_eq!(checksum(&merged), 0xB1B0_AFBA, "whole-file checksum");
    }

    #[test]
    fn subsets_disagreeing_on_a_glyph_are_not_merged() {
        let a = Sfnt::parse(&fake_subset(4, &[2])).unwrap();
        let mut b = Sfnt::parse(&fake_subset(4, &[2])).unwrap();
        b.glyphs[2][4] = 99;
        assert!(!consistent(&a, &b));
        let clusters = cluster_consistent(vec![((1, 0), a), ((2, 0), b)]);
        assert_eq!(clusters.len(), 2);
    }

    fn map(pairs: &[(u16, u16)]) -> BTreeMap<u16, u16> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn written_cmap_reads_back_in_every_format() {
        let mut subtables = BTreeMap::new();
        // Mac Roman, gids < 256: format 0.
        subtables.insert((1, 0), map(&[(32, 3), (65, 36), (66, 37)]));
        // Unicode: runs with a constant delta, a lone code, a high code: format 4.
        subtables.insert(
            (3, 1),
            map(&[(32, 3), (65, 36), (66, 37), (67, 38), (0x2019, 200)]),
        );
        // Symbolic: 0xF0xx codes.
        subtables.insert((3, 0), map(&[(0xF020, 3), (0xF041, 300)]));
        let bytes = write_cmap(&subtables).unwrap();
        let parsed = parse_cmap(&bytes).unwrap();
        assert_eq!(parsed.len(), 3);
        for sub in parsed {
            assert_eq!(sub.map, subtables[&(sub.platform, sub.encoding)]);
        }
        // Mac subtable with a gid above 255: format 6.
        let mut big = BTreeMap::new();
        big.insert((1, 0), map(&[(32, 3), (40, 400)]));
        let parsed = parse_cmap(&write_cmap(&big).unwrap()).unwrap();
        assert_eq!(parsed[0].map, big[&(1, 0)]);
    }

    #[test]
    fn cmaps_that_send_a_code_to_different_glyphs_are_not_merged() {
        let subset = |pairs: &[(u16, u16)]| SimpleSubset {
            id: (1, 0),
            sfnt: Sfnt::parse(&fake_subset(8, &[2, 5])).unwrap(),
            cmap: vec![CmapSub {
                platform: 3,
                encoding: 1,
                map: map(pairs),
            }],
            stored: 0,
        };
        let a = subset(&[(65, 2), (66, 5)]);
        assert!(simple_consistent(&a, &subset(&[(65, 2), (67, 5)])));
        assert!(!simple_consistent(&a, &subset(&[(65, 5)])));
    }

    #[test]
    fn name_table_keeps_only_the_english_essentials() {
        let strings: [&[u8]; 3] = [b"Copyright (c) someone", b"Times", b"Regular"];
        let records = [(0u16, 0usize), (1, 1), (2, 2)]; // (nameID, string)
        let mut t = Vec::new();
        t.extend_from_slice(&[0, 0, 0, 3]);
        t.extend_from_slice(&(6 + 12 * 3u16).to_be_bytes());
        let mut off = 0u16;
        for (id, s) in records {
            for v in [3u16, 1, 0x0409, id, strings[s].len() as u16, off] {
                t.extend_from_slice(&v.to_be_bytes());
            }
            off += strings[s].len() as u16;
        }
        for s in strings {
            t.extend_from_slice(s);
        }
        let trimmed = trim_name(&t).unwrap();
        assert_eq!(rd16(&trimmed, 2), Some(2), "copyright (ID 0) dropped");
        assert!(trimmed.windows(5).any(|w| w == b"Times"));
        assert!(!trimmed.windows(9).any(|w| w == b"Copyright"));
    }

    #[test]
    fn a_whole_font_is_cut_to_the_glyphs_its_encoding_reaches() {
        // 10 glyphs, all drawn. Glyph 6 is a composite of 7 (an accented letter).
        let mut sfnt = Sfnt::parse(&fake_subset(10, &(0..10).collect::<Vec<_>>())).unwrap();
        let mut composite = vec![0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0];
        composite.extend_from_slice(&[0x00, 0x00, 0, 7, 0, 0]); // flags, glyph 7, 2 arg bytes
        sfnt.glyphs[6] = composite;
        let cmap = vec![CmapSub {
            platform: 3,
            encoding: 1,
            // 'A'..'C' -> 1..3, e-acute (0xE9) -> 6, a letter outside the range -> 9
            map: map(&[(65, 1), (66, 2), (67, 3), (0xE9, 6), (0x4E2D, 9)]),
        }];
        let used = Usage {
            lo: 32,
            hi: 255,
            mac_roman: false,
        };
        reduce_to_reachable(&mut sfnt, &cmap, &used);
        let drawn: Vec<usize> = (0..10).filter(|&g| !sfnt.glyphs[g].is_empty()).collect();
        assert_eq!(
            drawn,
            vec![0, 1, 2, 3, 6, 7],
            "9 is unreachable, 7 comes with 6"
        );
        // Codes 32..=66 only: C and e-acute are out of range, so their glyphs go.
        let mut sfnt = Sfnt::parse(&fake_subset(10, &(0..10).collect::<Vec<_>>())).unwrap();
        let used = Usage {
            lo: 32,
            hi: 66,
            mac_roman: false,
        };
        reduce_to_reachable(&mut sfnt, &cmap, &used);
        let drawn: Vec<usize> = (0..10).filter(|&g| !sfnt.glyphs[g].is_empty()).collect();
        assert_eq!(drawn, vec![0, 1, 2]);
    }

    #[test]
    fn subset_prefix_is_stripped() {
        assert_eq!(base_name(b"ABCDEF+CourierNewPSMT"), b"CourierNewPSMT");
        assert_eq!(base_name(b"CourierNewPSMT"), b"CourierNewPSMT");
    }
}
