//! Finishes the job of sloppy CFF subsetters (every level, see
//! `Profile::cff_subset`).
//!
//! Some exporters "subset" a CFF font (`/FontFile3`) by emptying the unused
//! glyphs but keep everything else: Canva's CID-keyed Montserrat subsets are
//! 41 KB for ~20 glyphs (2 KB of charstrings) — a 1946-entry CharStrings
//! INDEX, 1721 glyph names nothing reads (CID-keyed fonts select glyphs by
//! CID), and every global subroutine. A Noto Serif CJK "subset" with 2 glyphs
//! kept all of its 27 000 local subroutines (540 KB).
//!
//! The pass walks every charstring (Type 2 interpreter limited to what's
//! needed to follow `callsubr`/`callgsubr` and skip `hintmask` bytes), then
//! rewrites the program:
//! - subroutines no glyph reaches become a 1-byte `return`, and each subr
//!   INDEX is cut after its last used entry — but never below the size that
//!   would change the subr-number bias (107/1131/32768), so no charstring
//!   changes;
//! - CID-keyed fonts whose `.notdef` draws nothing also lose every glyph that
//!   draws nothing: the charset (GID → CID) is rebuilt over the remaining
//!   glyphs, so CIDs in content streams still select the same outlines, and
//!   a CID no longer present falls back to the (equally blank) `.notdef`.
//!   Advance widths come from the PDF `/W` array, not the font;
//! - CID-keyed fonts lose the strings no DICT refers to.
//!
//! DICTs are copied byte for byte except for offsets (and remapped string
//! ids). Anything unexpected — a computed subroutine number, an unknown
//! operator, CFF2, an OpenType wrapper — leaves the program as it is, as
//! does a rewrite that isn't smaller. Such a program is still re-deflated
//! when that alone shrinks the stream (exporters often compress fonts
//! poorly: 10 KB on an 86 KB Myriad Pro).

use std::collections::HashMap;

use lopdf::{Document, Object, ObjectId, Stream};

/// Returns the number of font programs rewritten (or just re-deflated).
pub fn subset_cff_programs(doc: &mut Document) -> usize {
    let mut targets: Vec<ObjectId> = Vec::new();
    for obj in doc.objects.values() {
        let Ok(fd) = obj.as_dict() else { continue };
        if fd.get(b"Type").and_then(Object::as_name).ok() != Some(b"FontDescriptor") {
            continue;
        }
        if let Ok(ff) = fd.get(b"FontFile3").and_then(Object::as_reference) {
            targets.push(ff);
        }
    }
    targets.sort();
    targets.dedup();

    let mut rewritten = 0;
    for id in targets {
        let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
            continue;
        };
        let subtype = stream.dict.get(b"Subtype").and_then(Object::as_name).ok();
        if !matches!(subtype, Some(b"Type1C" | b"CIDFontType0C")) {
            continue;
        }
        let Some(data) = decoded(stream) else {
            continue;
        };
        // The rewrite only when the program itself shrinks; otherwise the
        // original bytes, which may still deflate better than they were.
        let cff = subset(&data)
            .filter(|c| c.len() < data.len())
            .unwrap_or(data);
        let mut dict = stream.dict.clone();
        dict.remove(b"Filter");
        dict.remove(b"DecodeParms");
        let mut new = Stream::new(dict, cff);
        let _ = new.compress();
        if new.content.len() >= stream.content.len() {
            continue;
        }
        // Rewritten in place: same object id, so every descriptor sharing the
        // program keeps pointing at it (the rewrite is valid for all of them).
        doc.objects.insert(id, Object::Stream(new));
        rewritten += 1;
    }
    rewritten
}

fn decoded(s: &Stream) -> Option<Vec<u8>> {
    match s.dict.get(b"Filter") {
        Err(_) => Some(s.content.clone()),
        Ok(_) => s.decompressed_content().ok(),
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn rd16(d: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?) as usize)
}

/// An INDEX at `off`: its items and the offset just past it.
fn read_index(d: &[u8], off: usize) -> Option<(Vec<&[u8]>, usize)> {
    let n = rd16(d, off)?;
    if n == 0 {
        return Some((Vec::new(), off + 2));
    }
    let os = *d.get(off + 2)? as usize;
    if !(1..=4).contains(&os) {
        return None;
    }
    let offs_at = off + 3;
    let rd_off = |i: usize| -> Option<usize> {
        let b = d.get(offs_at + i * os..offs_at + (i + 1) * os)?;
        Some(b.iter().fold(0usize, |a, &x| (a << 8) | x as usize))
    };
    let base = offs_at + (n + 1) * os - 1;
    let mut items = Vec::with_capacity(n);
    let mut prev = rd_off(0)?;
    for i in 1..=n {
        let cur = rd_off(i)?;
        if cur < prev {
            return None;
        }
        items.push(d.get(base + prev..base + cur)?);
        prev = cur;
    }
    Some((items, base + prev))
}

/// One DICT entry: operator (escaped ones as `1200 + b1`), operand values
/// (reals as their approximate value) and the raw operand bytes.
#[derive(Clone, Debug)]
struct Entry {
    op: u16,
    vals: Vec<f64>,
    raw: Vec<u8>,
}

fn read_dict(b: &[u8]) -> Option<Vec<Entry>> {
    let mut out = Vec::new();
    let (mut vals, mut start, mut i) = (Vec::new(), 0, 0);
    while i < b.len() {
        let c = b[i];
        match c {
            0..=21 => {
                let raw = b[start..i].to_vec();
                let op = if c == 12 {
                    i += 1;
                    1200 + *b.get(i)? as u16
                } else {
                    c as u16
                };
                i += 1;
                out.push(Entry {
                    op,
                    vals: std::mem::take(&mut vals),
                    raw,
                });
                start = i;
            }
            28 => {
                vals.push(i16::from_be_bytes(b.get(i + 1..i + 3)?.try_into().ok()?) as f64);
                i += 3;
            }
            29 => {
                vals.push(i32::from_be_bytes(b.get(i + 1..i + 5)?.try_into().ok()?) as f64);
                i += 5;
            }
            30 => {
                // Real: nibbles until an 0xf nibble.
                let mut s = String::new();
                loop {
                    i += 1;
                    let x = *b.get(i)?;
                    let mut end = false;
                    for nib in [x >> 4, x & 15] {
                        match nib {
                            0..=9 => s.push((b'0' + nib) as char),
                            0xa => s.push('.'),
                            0xb => s.push('E'),
                            0xc => s.push_str("E-"),
                            0xe => s.push('-'),
                            0xf => {
                                end = true;
                                break;
                            }
                            _ => return None,
                        }
                    }
                    if end {
                        break;
                    }
                }
                i += 1;
                vals.push(s.parse().unwrap_or(0.0));
            }
            32..=246 => {
                vals.push(c as f64 - 139.0);
                i += 1;
            }
            247..=250 => {
                vals.push(((c as f64 - 247.0) * 256.0) + *b.get(i + 1)? as f64 + 108.0);
                i += 2;
            }
            251..=254 => {
                vals.push(-((c as f64 - 251.0) * 256.0) - *b.get(i + 1)? as f64 - 108.0);
                i += 2;
            }
            _ => return None,
        }
    }
    (start == b.len()).then_some(out)
}

fn get(d: &[Entry], op: u16) -> Option<&[f64]> {
    d.iter().find(|e| e.op == op).map(|e| e.vals.as_slice())
}

fn offset(d: &[Entry], op: u16) -> Option<usize> {
    let v = *get(d, op)?.first()?;
    (v >= 0.0 && v.fract() == 0.0).then_some(v as usize)
}

const OP_CHARSET: u16 = 15;
const OP_ENCODING: u16 = 16;
const OP_CHARSTRINGS: u16 = 17;
const OP_PRIVATE: u16 = 18;
const OP_SUBRS: u16 = 19;
const OP_CHARSTRING_TYPE: u16 = 1206;
const OP_ROS: u16 = 1230;
const OP_FDARRAY: u16 = 1236;
const OP_FDSELECT: u16 = 1237;
/// Operators whose operands are all string ids (`ROS` handled separately).
const SID_OPS: [u16; 9] = [0, 1, 2, 3, 4, 1200, 1221, 1222, 1238];
/// First non-standard string id.
const N_STD_STRINGS: usize = 391;

/// A Private DICT and its local subroutines.
struct Private<'a> {
    dict: Vec<Entry>,
    subrs: Vec<&'a [u8]>,
}

fn read_private<'a>(d: &'a [u8], font_dict: &[Entry]) -> Option<Private<'a>> {
    let Some(pv) = get(font_dict, OP_PRIVATE) else {
        return Some(Private {
            dict: Vec::new(),
            subrs: Vec::new(),
        });
    };
    let (&size, &off) = (pv.first()?, pv.get(1)?);
    let (size, off) = (size as usize, off as usize);
    let dict = read_dict(d.get(off..off + size)?)?;
    let subrs = match offset(&dict, OP_SUBRS) {
        Some(rel) => read_index(d, off + rel)?.0,
        None => Vec::new(),
    };
    Some(Private { dict, subrs })
}

struct Cff<'a> {
    name_index: &'a [u8],
    top: Vec<Entry>,
    strings: Vec<&'a [u8]>,
    gsubrs: Vec<&'a [u8]>,
    charstrings: Vec<&'a [u8]>,
    /// CID-keyed: GID → CID; FD dicts and FDSelect (GID → FD).
    cids: Vec<u16>,
    fd_dicts: Vec<Vec<Entry>>,
    fd_select: Vec<u8>,
    /// One per FD (CID-keyed) or the font's single one.
    privates: Vec<Private<'a>>,
    /// Non-CID: charset and encoding tables copied verbatim (`None` when the
    /// Top DICT names a predefined one).
    charset_raw: Option<&'a [u8]>,
    encoding_raw: Option<&'a [u8]>,
}

impl<'a> Cff<'a> {
    fn cid_keyed(&self) -> bool {
        get(&self.top, OP_ROS).is_some()
    }

    fn parse(d: &'a [u8]) -> Option<Self> {
        if *d.first()? != 1 {
            return None; // CFF2, OpenType wrapper, garbage
        }
        let hdr = *d.get(2)? as usize;
        let (names, after_names) = read_index(d, hdr)?;
        let (tops, after_tops) = read_index(d, after_names)?;
        if names.len() != 1 || tops.len() != 1 {
            return None;
        }
        let top = read_dict(tops[0])?;
        if get(&top, OP_CHARSTRING_TYPE).is_some_and(|v| v != [2.0]) {
            return None;
        }
        let (strings, after_strings) = read_index(d, after_tops)?;
        let (gsubrs, _) = read_index(d, after_strings)?;
        let (charstrings, _) = read_index(d, offset(&top, OP_CHARSTRINGS)?)?;
        let n = charstrings.len();
        if n == 0 || n > 65535 {
            return None;
        }
        let mut cff = Cff {
            name_index: &d[hdr..after_names],
            top,
            strings,
            gsubrs,
            charstrings,
            cids: Vec::new(),
            fd_dicts: Vec::new(),
            fd_select: Vec::new(),
            privates: Vec::new(),
            charset_raw: None,
            encoding_raw: None,
        };
        if cff.cid_keyed() {
            let cs_off = offset(&cff.top, OP_CHARSET)?;
            if cs_off <= 2 {
                return None;
            }
            cff.cids = read_charset(d, cs_off, n)?.0;
            let (fds, _) = read_index(d, offset(&cff.top, OP_FDARRAY)?)?;
            for fd in fds {
                let dict = read_dict(fd)?;
                cff.privates.push(read_private(d, &dict)?);
                cff.fd_dicts.push(dict);
            }
            cff.fd_select = read_fd_select(d, offset(&cff.top, OP_FDSELECT)?, n)?;
            if cff
                .fd_select
                .iter()
                .any(|&f| f as usize >= cff.fd_dicts.len())
            {
                return None;
            }
        } else {
            cff.privates.push(read_private(d, &cff.top)?);
            cff.fd_select = vec![0; n];
            if let Some(o) = offset(&cff.top, OP_CHARSET).filter(|&o| o > 2) {
                let (_, end) = read_charset(d, o, n)?;
                cff.charset_raw = Some(d.get(o..end)?);
            }
            if let Some(o) = offset(&cff.top, OP_ENCODING).filter(|&o| o > 1) {
                cff.encoding_raw = Some(d.get(o..encoding_end(d, o)?)?);
            }
        }
        Some(cff)
    }
}

/// Charset at `off` for `n` glyphs: GID → SID/CID (`.notdef` included) and
/// the table's end offset.
fn read_charset(d: &[u8], off: usize, n: usize) -> Option<(Vec<u16>, usize)> {
    let mut out = vec![0u16];
    let fmt = *d.get(off)?;
    let mut o = off + 1;
    while out.len() < n {
        match fmt {
            0 => {
                out.push(rd16(d, o)? as u16);
                o += 2;
            }
            1 | 2 => {
                let first = rd16(d, o)?;
                let left = if fmt == 1 {
                    *d.get(o + 2)? as usize
                } else {
                    rd16(d, o + 2)?
                };
                o += if fmt == 1 { 3 } else { 4 };
                for k in 0..=left {
                    out.push(u16::try_from(first + k).ok()?);
                }
            }
            _ => return None,
        }
    }
    out.truncate(n);
    Some((out, o))
}

fn encoding_end(d: &[u8], off: usize) -> Option<usize> {
    let fmt = *d.get(off)?;
    let n = *d.get(off + 1)? as usize;
    let mut o = off
        + 2
        + match fmt & 0x7f {
            0 => n,
            1 => 2 * n,
            _ => return None,
        };
    if fmt & 0x80 != 0 {
        o += 1 + 3 * *d.get(o)? as usize;
    }
    (o <= d.len()).then_some(o)
}

fn read_fd_select(d: &[u8], off: usize, n: usize) -> Option<Vec<u8>> {
    match *d.get(off)? {
        0 => Some(d.get(off + 1..off + 1 + n)?.to_vec()),
        3 => {
            let nr = rd16(d, off + 1)?;
            let mut out = Vec::with_capacity(n);
            for r in 0..nr {
                let at = off + 3 + 3 * r;
                let first = rd16(d, at)?;
                let fd = *d.get(at + 2)?;
                let end = rd16(d, at + 3)?; // next range's first, or the sentinel
                if first != out.len() || end < first {
                    return None;
                }
                out.extend(std::iter::repeat_n(fd, end - first));
            }
            (out.len() == n).then_some(out)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Charstring walk
// ---------------------------------------------------------------------------

fn bias(n: usize) -> i64 {
    match n {
        0..1240 => 107,
        1240..33900 => 1131,
        _ => 32768,
    }
}

/// Smallest INDEX length that keeps the bias of an `n`-entry subr INDEX.
fn bias_floor(n: usize) -> usize {
    match n {
        0..1240 => 0,
        1240..33900 => 1240,
        _ => 33900,
    }
}

/// Upper bound on interpreted operators per program, so a pathological font
/// (exponential subr fan-out) can't stall the run.
const OP_BUDGET: usize = 20_000_000;

struct Walk<'a, 'u> {
    gsubrs: &'a [&'a [u8]],
    lsubrs: &'a [&'a [u8]],
    used_g: &'u mut [bool],
    used_l: &'u mut [bool],
    budget: &'u mut usize,
    stack: Vec<f64>,
    stems: usize,
    draws: bool,
    ended: bool,
    seac: bool,
}

#[derive(Debug)]
struct Unsupported;

impl Walk<'_, '_> {
    fn run(&mut self, cs: &[u8], depth: u32) -> Result<(), Unsupported> {
        if depth > 16 {
            return Err(Unsupported);
        }
        let mut i = 0;
        while i < cs.len() && !self.ended {
            *self.budget = self.budget.checked_sub(1).ok_or(Unsupported)?;
            let b = cs[i];
            let at = i;
            let arg = |k: usize| cs.get(at + k).copied().ok_or(Unsupported);
            match b {
                28 => {
                    self.stack
                        .push(i16::from_be_bytes([arg(1)?, arg(2)?]) as f64);
                    i += 3;
                    continue;
                }
                32..=246 => {
                    self.stack.push(b as f64 - 139.0);
                    i += 1;
                    continue;
                }
                247..=250 => {
                    self.stack
                        .push((b as f64 - 247.0) * 256.0 + arg(1)? as f64 + 108.0);
                    i += 2;
                    continue;
                }
                251..=254 => {
                    self.stack
                        .push(-(b as f64 - 251.0) * 256.0 - arg(1)? as f64 - 108.0);
                    i += 2;
                    continue;
                }
                255 => {
                    let v = i32::from_be_bytes([arg(1)?, arg(2)?, arg(3)?, arg(4)?]);
                    self.stack.push(v as f64 / 65536.0);
                    i += 5;
                    continue;
                }
                _ => {}
            }
            i += 1;
            match b {
                1 | 3 | 18 | 23 => self.stems += self.stack.len() / 2,
                19 | 20 => {
                    // Arguments left on the stack are an implicit vstemhm.
                    self.stems += self.stack.len() / 2;
                    i += self.stems.div_ceil(8);
                }
                10 | 29 => {
                    // Arguments flow through subroutine calls: no clear.
                    self.call(b == 29, depth + 1)?;
                    continue;
                }
                11 => return Ok(()),
                14 => {
                    // endchar with 4 (+ width) arguments is a seac accent.
                    self.seac |= self.stack.len() >= 4;
                    self.ended = true;
                }
                4..=8 | 21 | 22 | 24..=27 | 30 | 31 => self.draws = true,
                12 => {
                    let e = arg(1)?;
                    i += 1;
                    match e {
                        0 => {}                       // dotsection (deprecated)
                        34..=37 => self.draws = true, // flex family
                        _ => return Err(Unsupported), // arithmetic/storage
                    }
                }
                _ => return Err(Unsupported), // reserved, CFF2 blend/vsindex
            }
            self.stack.clear();
        }
        Ok(())
    }

    fn call(&mut self, global: bool, depth: u32) -> Result<(), Unsupported> {
        let n = self.stack.pop().ok_or(Unsupported)?;
        if n.fract() != 0.0 {
            return Err(Unsupported);
        }
        let len = if global {
            self.gsubrs.len()
        } else {
            self.lsubrs.len()
        };
        let k = usize::try_from(n as i64 + bias(len)).map_err(|_| Unsupported)?;
        if k >= len {
            return Err(Unsupported);
        }
        let body = if global {
            self.used_g[k] = true;
            self.gsubrs[k]
        } else {
            self.used_l[k] = true;
            self.lsubrs[k]
        };
        self.run(body, depth)
    }
}

// ---------------------------------------------------------------------------
// Rewrite
// ---------------------------------------------------------------------------

/// The subsetted program, or `None` when the font can't be handled.
fn subset(d: &[u8]) -> Option<Vec<u8>> {
    let cff = Cff::parse(d)?;
    let cid = cff.cid_keyed();
    let n = cff.charstrings.len();

    let mut used_g = vec![false; cff.gsubrs.len()];
    let mut used_l: Vec<Vec<bool>> = cff
        .privates
        .iter()
        .map(|p| vec![false; p.subrs.len()])
        .collect();
    let mut budget = OP_BUDGET;
    let mut draws = vec![false; n];
    for (g, cs) in cff.charstrings.iter().enumerate() {
        let fd = cff.fd_select[g] as usize;
        let mut w = Walk {
            gsubrs: &cff.gsubrs,
            lsubrs: &cff.privates[fd].subrs,
            used_g: &mut used_g,
            used_l: &mut used_l[fd],
            budget: &mut budget,
            stack: Vec::new(),
            stems: 0,
            draws: false,
            ended: false,
            seac: false,
        };
        w.run(cs, 0).ok()?;
        if cid && w.seac {
            return None; // not allowed in CID-keyed fonts: don't guess
        }
        draws[g] = w.draws;
    }

    let keep: Vec<usize> = if cid && !draws[0] {
        (0..n).filter(|&g| g == 0 || draws[g]).collect()
    } else {
        (0..n).collect()
    };

    // String ids: CID-keyed fonts only reference strings from their DICTs.
    let (strings, sid_map) = if cid {
        let mut refs: Vec<usize> = Vec::new();
        for dict in std::iter::once(&cff.top).chain(&cff.fd_dicts) {
            for e in dict {
                let sids = match e.op {
                    OP_ROS => e.vals.get(..2).unwrap_or(&[]),
                    op if SID_OPS.contains(&op) => &e.vals[..],
                    _ => &[],
                };
                refs.extend(sids.iter().map(|&v| v as usize));
            }
        }
        refs.retain(|&s| s >= N_STD_STRINGS);
        refs.sort_unstable();
        refs.dedup();
        let mut map = HashMap::new();
        let mut strings = Vec::new();
        for s in refs {
            strings.push(*cff.strings.get(s - N_STD_STRINGS)?);
            map.insert(s, N_STD_STRINGS + map.len());
        }
        (strings, Some(map))
    } else {
        (cff.strings.clone(), None)
    };
    let sid_bytes = |e: &Entry| -> Option<Option<Vec<u8>>> {
        let Some(map) = &sid_map else {
            return Some(None);
        };
        let n_sids = match e.op {
            OP_ROS => 2,
            op if SID_OPS.contains(&op) => e.vals.len(),
            _ => return Some(None),
        };
        let mut out = Vec::new();
        for (k, &v) in e.vals.iter().enumerate() {
            let v = v as usize;
            let v = if k < n_sids && v >= N_STD_STRINGS {
                *map.get(&v)?
            } else {
                v
            };
            dict_int(&mut out, i32::try_from(v).ok()?);
        }
        Some(Some(out))
    };

    // Subroutines and Private DICTs (Subrs right after its DICT).
    let gsubr_index = stub_index(&cff.gsubrs, &used_g).unwrap_or_else(|| index::<&[u8]>(&[]));
    let mut private_blocks: Vec<(usize, Vec<u8>)> = Vec::new(); // (DICT size, DICT + Subrs)
    for (p, used) in cff.privates.iter().zip(&used_l) {
        let subrs = stub_index(&p.subrs, used);
        let entries: Vec<Entry> = p
            .dict
            .iter()
            .filter(|e| e.op != OP_SUBRS)
            .cloned()
            .collect();
        let mut dict = write_dict(&entries, |_| Some(None))?;
        if subrs.is_some() {
            let len = dict.len() + 6;
            dict_int5(&mut dict, len);
            dict_op(&mut dict, OP_SUBRS);
        }
        let size = dict.len();
        dict.extend(subrs.unwrap_or_default());
        private_blocks.push((size, dict));
    }

    let charstrings = index(&keep.iter().map(|&g| cff.charstrings[g]).collect::<Vec<_>>());
    let (charset, fd_select) = if cid {
        let cids: Vec<u16> = keep.iter().map(|&g| cff.cids[g]).collect();
        let fds: Vec<u8> = keep.iter().map(|&g| cff.fd_select[g]).collect();
        (Some(write_cid_charset(&cids)), Some(write_fd_select(&fds)))
    } else {
        (cff.charset_raw.map(<[u8]>::to_vec), None)
    };
    let encoding = cff.encoding_raw.map(<[u8]>::to_vec);

    // Layout: every offset is written as a 5-byte integer, so DICT sizes
    // don't depend on the values and one pass with placeholders suffices.
    #[derive(Default, Clone, Copy)]
    struct Offsets {
        charset: usize,
        encoding: usize,
        fd_select: usize,
        charstrings: usize,
        fd_array: usize,
        private: usize,
    }
    let top_dict = |o: &Offsets| {
        write_dict(&cff.top, |e| {
            let off = |v: usize| {
                let mut b = Vec::new();
                dict_int5(&mut b, v);
                Some(Some(b))
            };
            match e.op {
                OP_CHARSET if charset.is_some() => off(o.charset),
                OP_ENCODING if encoding.is_some() => off(o.encoding),
                OP_CHARSTRINGS => off(o.charstrings),
                OP_FDSELECT => off(o.fd_select),
                OP_FDARRAY => off(o.fd_array),
                OP_PRIVATE => {
                    let mut b = Vec::new();
                    dict_int5(&mut b, private_blocks.first()?.0);
                    dict_int5(&mut b, o.private);
                    Some(Some(b))
                }
                _ => sid_bytes(e),
            }
        })
    };
    let fd_array = |first_private: usize| -> Option<Vec<u8>> {
        let mut at = first_private;
        let mut dicts = Vec::new();
        for (fd, (size, block)) in cff.fd_dicts.iter().zip(&private_blocks) {
            let this = at;
            dicts.push(write_dict(fd, |e| {
                if e.op == OP_PRIVATE {
                    let mut b = Vec::new();
                    dict_int5(&mut b, *size);
                    dict_int5(&mut b, this);
                    Some(Some(b))
                } else {
                    sid_bytes(e)
                }
            })?);
            at += block.len();
        }
        Some(index(&dicts))
    };

    let string_index = index(&strings);
    let mut o = Offsets::default();
    let top_len = index(&[top_dict(&o)?]).len();
    let mut pos = 4 + cff.name_index.len() + top_len + string_index.len() + gsubr_index.len();
    for (slot, table) in [
        (&mut o.charset, &charset),
        (&mut o.encoding, &encoding),
        (&mut o.fd_select, &fd_select),
    ] {
        *slot = pos;
        pos += table.as_ref().map_or(0, Vec::len);
    }
    o.charstrings = pos;
    pos += charstrings.len();
    if cid {
        o.fd_array = pos;
        pos += fd_array(0)?.len();
    }
    o.private = pos;
    let fd_array = if cid {
        fd_array(o.private)?
    } else {
        Vec::new()
    };

    let mut out = vec![1, 0, 4, 4];
    out.extend_from_slice(cff.name_index);
    out.extend(index(&[top_dict(&o)?]));
    out.extend(string_index);
    out.extend(gsubr_index);
    for table in [charset, encoding, fd_select].into_iter().flatten() {
        out.extend(table);
    }
    out.extend(charstrings);
    out.extend(fd_array);
    for (_, block) in private_blocks {
        out.extend(block);
    }

    // Self-check: the result parses back with the expected glyph count.
    let back = Cff::parse(&out)?;
    (back.charstrings.len() == keep.len()).then_some(out)
}

/// A subr INDEX keeping only the used entries (others become `return`),
/// truncated after the last used one as far as the bias allows. `None` when
/// none is used.
fn stub_index(subrs: &[&[u8]], used: &[bool]) -> Option<Vec<u8>> {
    let last = used.iter().rposition(|&u| u)?;
    let n = (last + 1).max(bias_floor(subrs.len()));
    let items: Vec<&[u8]> = (0..n)
        .map(|k| if used[k] { subrs[k] } else { &[11u8][..] })
        .collect();
    Some(index(&items))
}

fn write_cid_charset(cids: &[u16]) -> Vec<u8> {
    // Format 2 ranges (from GID 1), or format 0 when that's smaller.
    let mut ranges: Vec<(u16, u16)> = Vec::new(); // (first, nLeft)
    for &c in &cids[1..] {
        match ranges.last_mut() {
            Some((first, left)) if c as u32 == *first as u32 + *left as u32 + 1 => *left += 1,
            _ => ranges.push((c, 0)),
        }
    }
    let mut out = Vec::new();
    if 4 * ranges.len() < 2 * (cids.len() - 1) {
        out.push(2);
        for (first, left) in ranges {
            out.extend_from_slice(&first.to_be_bytes());
            out.extend_from_slice(&left.to_be_bytes());
        }
    } else {
        out.push(0);
        for &c in &cids[1..] {
            out.extend_from_slice(&c.to_be_bytes());
        }
    }
    out
}

fn write_fd_select(fds: &[u8]) -> Vec<u8> {
    let mut ranges: Vec<(usize, u8)> = Vec::new();
    for (g, &fd) in fds.iter().enumerate() {
        if ranges.last().is_none_or(|r| r.1 != fd) {
            ranges.push((g, fd));
        }
    }
    let mut out = Vec::new();
    if 3 * ranges.len() + 4 < fds.len() {
        out.push(3);
        out.extend_from_slice(&(ranges.len() as u16).to_be_bytes());
        for (first, fd) in ranges {
            out.extend_from_slice(&(first as u16).to_be_bytes());
            out.push(fd);
        }
        out.extend_from_slice(&(fds.len() as u16).to_be_bytes());
    } else {
        out.push(0);
        out.extend_from_slice(fds);
    }
    out
}

/// Writes `entries`, taking each one's operands from `subst` when it returns
/// `Some(Some(bytes))`, verbatim on `Some(None)`; `None` aborts.
fn write_dict(
    entries: &[Entry],
    subst: impl Fn(&Entry) -> Option<Option<Vec<u8>>>,
) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for e in entries {
        match subst(e)? {
            Some(b) => out.extend(b),
            None => out.extend_from_slice(&e.raw),
        }
        dict_op(&mut out, e.op);
    }
    Some(out)
}

fn index<T: AsRef<[u8]>>(items: &[T]) -> Vec<u8> {
    let mut out = (items.len() as u16).to_be_bytes().to_vec();
    if items.is_empty() {
        return out;
    }
    let total: usize = items.iter().map(|i| i.as_ref().len()).sum::<usize>() + 1;
    let off_size: usize = match total {
        0..=0xff => 1,
        0x100..=0xffff => 2,
        0x10000..=0xff_ffff => 3,
        _ => 4,
    };
    out.push(off_size as u8);
    let mut off = 1usize;
    out.extend_from_slice(&(off as u32).to_be_bytes()[4 - off_size..]);
    for it in items {
        off += it.as_ref().len();
        out.extend_from_slice(&(off as u32).to_be_bytes()[4 - off_size..]);
    }
    for it in items {
        out.extend_from_slice(it.as_ref());
    }
    out
}

fn dict_int(out: &mut Vec<u8>, v: i32) {
    match v {
        -107..=107 => out.push((v + 139) as u8),
        108..=1131 => {
            let j = v - 108;
            out.extend_from_slice(&[((j >> 8) + 247) as u8, (j & 0xff) as u8]);
        }
        -1131..=-108 => {
            let j = -v - 108;
            out.extend_from_slice(&[((j >> 8) + 251) as u8, (j & 0xff) as u8]);
        }
        -32768..=32767 => {
            out.push(28);
            out.extend_from_slice(&(v as i16).to_be_bytes());
        }
        _ => {
            out.push(29);
            out.extend_from_slice(&v.to_be_bytes());
        }
    }
}

/// Fixed-size (5-byte) integer, for offsets.
fn dict_int5(out: &mut Vec<u8>, v: usize) {
    out.push(29);
    out.extend_from_slice(&(v as i32).to_be_bytes());
}

fn dict_op(out: &mut Vec<u8>, op: u16) {
    if op >= 1200 {
        out.extend_from_slice(&[12, (op - 1200) as u8]);
    } else {
        out.push(op as u8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op5(out: &mut Vec<u8>, vals: &[usize], op: u16) {
        for &v in vals {
            dict_int5(out, v);
        }
        dict_op(out, op);
    }

    /// A CFF with one FD, laid out by hand: `top_extra` is appended to the
    /// Top DICT (already encoded), `cid` selects a CID-keyed font (ROS with
    /// string ids 391/392, FDSelect, FDArray) or a name-keyed one.
    fn build(
        cid: bool,
        strings: &[&[u8]],
        gsubrs: &[&[u8]],
        lsubrs: &[&[u8]],
        charstrings: &[&[u8]],
    ) -> Vec<u8> {
        let n = charstrings.len();
        let name = index(&[b"T".as_slice()]);
        let strings = index(strings);
        let gsubrs = index(gsubrs);
        let mut charset = vec![0u8];
        for g in 1..n {
            charset.extend_from_slice(&(g as u16 * 10).to_be_bytes()); // CID or SID = 10·GID
        }
        let fd_select: Vec<u8> = std::iter::once(0)
            .chain(std::iter::repeat_n(0, n))
            .collect();
        let cs = index(charstrings);
        let lsubrs = index(lsubrs);
        let mut private = Vec::new();
        dict_int(&mut private, 500);
        dict_op(&mut private, 20); // defaultWidthX
        let private_len = private.len() + 6;
        op5(&mut private, &[private_len], OP_SUBRS);

        let top = |o: [usize; 5]| {
            let mut t = Vec::new();
            if cid {
                for v in [391, 392, 0] {
                    dict_int(&mut t, v);
                }
                dict_op(&mut t, OP_ROS);
                dict_int(&mut t, 1000);
                dict_op(&mut t, 1234); // CIDCount
            } else {
                dict_int(&mut t, 391);
                dict_op(&mut t, 2); // FullName
            }
            op5(&mut t, &[o[0]], OP_CHARSET);
            op5(&mut t, &[o[2]], OP_CHARSTRINGS);
            if cid {
                op5(&mut t, &[o[1]], OP_FDSELECT);
                op5(&mut t, &[o[3]], OP_FDARRAY);
            } else {
                op5(&mut t, &[private.len(), o[4]], OP_PRIVATE);
            }
            t
        };
        let fd_array = |priv_off: usize| {
            let mut fd = Vec::new();
            op5(&mut fd, &[private.len(), priv_off], OP_PRIVATE);
            index(&[fd])
        };
        let top_len = index(&[top([0; 5])]).len();
        let charset_off = 4 + name.len() + top_len + strings.len() + gsubrs.len();
        let fd_select_off = charset_off + charset.len();
        let cs_off = fd_select_off + if cid { fd_select.len() } else { 0 };
        let fd_array_off = cs_off + cs.len();
        let priv_off = fd_array_off + if cid { fd_array(0).len() } else { 0 };

        let mut out = vec![1, 0, 4, 4];
        out.extend(name);
        out.extend(index(&[top([
            charset_off,
            fd_select_off,
            cs_off,
            fd_array_off,
            priv_off,
        ])]));
        out.extend(strings);
        out.extend(gsubrs);
        out.extend(charset);
        if cid {
            out.extend(fd_select);
        }
        out.extend(cs);
        if cid {
            out.extend(fd_array(priv_off));
        }
        out.extend(private);
        out.extend(lsubrs);
        out
    }

    // Subr numbers are biased by -107 for INDEXes of < 1240 entries.
    const CALL_G0: &[u8] = &[139 - 107, 29]; // callgsubr 0
    const CALL_L1: &[u8] = &[139 - 106, 10]; // callsubr 1

    #[test]
    fn cid_font_loses_blank_glyphs_unused_subrs_and_strings() {
        let src = build(
            true,
            &[b"Adobe", b"Identity", b"glyph-name-1", b"glyph-name-2"],
            &[&[139, 139, 21, 11], b"\x8b\x8b\x8b\x8b\x8b\x8b\x08\x0b"], // rmoveto; rrcurveto
            &[b"\x0b", &[150, 139, 5, 11]],                              // rlineto
            &[
                &[14],                                                      // .notdef: blank
                &[14],                                                      // CID 10: blank
                &[CALL_G0, &[14]].concat(),                                 // CID 20: global subr 0
                &[&[139, 149, 1, 0x13, 0x80][..], CALL_L1, &[14]].concat(), // CID 30: hstem, hintmask, local subr 1
                &[139, 139, 0x13, 0x80, 14], // CID 40: implicit vstem + hintmask, blank
            ],
        );
        let out = subset(&src).expect("subset");
        assert!(out.len() < src.len());
        let c = Cff::parse(&out).unwrap();
        assert_eq!(c.cids, vec![0, 20, 30]);
        assert_eq!(c.charstrings.len(), 3);
        assert_eq!(c.charstrings[1], [CALL_G0, &[14]].concat());
        // Global subr 1 unused, past the last used one: truncated.
        assert_eq!(c.gsubrs.len(), 1);
        // Local subr 0 unused but before subr 1: kept as a `return` stub.
        assert_eq!(c.privates[0].subrs, vec![&[11u8][..], &[150, 139, 5, 11]]);
        // Only the ROS strings survive, renumbered from 391.
        assert_eq!(c.strings, vec![&b"Adobe"[..], b"Identity"]);
        assert_eq!(get(&c.top, OP_ROS), Some(&[391.0, 392.0, 0.0][..]));
        assert_eq!(get(&c.privates[0].dict, 20), Some(&[500.0][..]));
    }

    #[test]
    fn name_keyed_font_keeps_glyphs_and_strings() {
        let src = build(
            false,
            &[b"Full Name", b"a", b"b"],
            &[b"\x0b", b"\x0b"],
            &[&[139, 139, 21, 11], b"\x0b", b"\x0b"],
            &[&[14], &[14], &[&[139 - 107, 10][..], &[14]].concat()],
        );
        let out = subset(&src).expect("subset");
        let c = Cff::parse(&out).unwrap();
        assert_eq!(c.charstrings.len(), 3); // glyphs are selected by name: all kept
        assert_eq!(c.strings.len(), 3);
        assert_eq!(c.charset_raw.unwrap().len(), 1 + 2 * 2);
        assert!(c.gsubrs.is_empty());
        assert_eq!(c.privates[0].subrs, vec![&[139u8, 139, 21, 11][..]]);
    }

    #[test]
    fn computed_subr_number_is_left_alone() {
        // 1 1 add callsubr: the subr number comes from arithmetic.
        let src = build(
            false,
            &[b"Full Name", b"a"],
            &[],
            &[b"\x0b", b"\x0b", b"\x0b"],
            &[&[14], &[140, 140, 12, 10, 10, 14]],
        );
        assert!(subset(&src).is_none());
    }
}
