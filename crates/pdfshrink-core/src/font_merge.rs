//! Merges per-page subsets of the same TrueType CID font into one font program
//! (experimental levels only).
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
    let mut groups: HashMap<Vec<u8>, Vec<(ObjectId, Sfnt)>> = HashMap::new();
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

    let mut merged_away = 0;
    for (_, subsets) in groups {
        if subsets.len() < 2 {
            continue;
        }
        for cluster in cluster_consistent(subsets) {
            if cluster.len() < 2 {
                continue;
            }
            let Some(bytes) = build_merged(&cluster) else {
                continue;
            };
            let mut dict = Dictionary::new();
            dict.set("Length1", bytes.len() as i64);
            let mut stream = Stream::new(dict, bytes);
            let _ = stream.compress();
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
    if merged_away > 0 {
        doc.prune_objects();
    }
    merged_away
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
    let mut clusters: Vec<Vec<(ObjectId, Sfnt)>> = Vec::new();
    'next: for (id, s) in subsets {
        for c in clusters.iter_mut() {
            if c.iter().all(|(_, o)| consistent(o, &s)) {
                c.push((id, s));
                continue 'next;
            }
        }
        clusters.push(vec![(id, s)]);
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
    let n = cluster.iter().map(|(_, s)| s.glyphs.len()).max()?;
    // Base for the tables we copy verbatim: the subset covering the most GIDs.
    let base = &cluster.iter().max_by_key(|(_, s)| s.glyphs.len())?.1;

    let mut glyphs: Vec<&[u8]> = vec![&[]; n];
    let mut metrics: Vec<(u16, i16)> = vec![(0, 0); n];
    for (_, s) in cluster {
        for g in 0..s.glyphs.len() {
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
                .find(|(_, s)| g < s.metrics.len() && s.metrics[g].0 != 0)
                .map(|(_, s)| s.metrics[g])
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
        b"hdmx", b"LTSH", b"VDMX", b"kern", b"GPOS", b"GSUB", b"GDEF",
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
                .filter_map(|(_, s)| s.table(b"maxp").and_then(|t| rd16(t, field)))
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
    Some(write_sfnt(&tables))
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

    #[test]
    fn subset_prefix_is_stripped() {
        assert_eq!(base_name(b"ABCDEF+CourierNewPSMT"), b"CourierNewPSMT");
        assert_eq!(base_name(b"CourierNewPSMT"), b"CourierNewPSMT");
    }
}
