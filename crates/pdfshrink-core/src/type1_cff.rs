//! Converts embedded Type 1 fonts (`/FontFile`) to CFF (`/FontFile3`,
//! `/Subtype /Type1C`) — experimental levels only.
//!
//! Type 1 programs are `eexec`-encrypted and their charstrings encrypted
//! again, so Flate can't compress them; CFF stores the same outlines as plain,
//! compact Type 2 charstrings, typically 3–5× smaller. PDF viewers accept a
//! CFF program for a `/Type1` font dictionary, so only the font descriptor
//! changes.
//!
//! Outlines are preserved exactly: each Type 1 charstring is interpreted
//! (subroutines expanded, flex turned into its two curves, `seac` kept as a
//! Type 2 accented `endchar`) into an absolute path, then re-encoded. Hints
//! are simplified: all stems of a glyph are kept as one sorted,
//! non-overlapping set (hint replacement is dropped), which only matters for
//! hinted rasterization at small sizes. The font's built-in encoding is kept,
//! since TeX fonts' PDF encodings are expressed relative to it.
//!
//! Any construct the converter doesn't understand (counter-control or
//! multiple-master OtherSubrs, a malformed charstring…) leaves that font as
//! Type 1.

use std::collections::{BTreeMap, HashMap};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::cff_tables::STANDARD_STRINGS;
use crate::type1_merge::{Entry, Type1};

/// Returns the number of font programs converted.
pub fn convert_type1_to_cff(doc: &mut Document) -> usize {
    let mut targets: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
    for (fd_id, obj) in &doc.objects {
        let Ok(fd) = obj.as_dict() else { continue };
        if fd.get(b"Type").and_then(Object::as_name).ok() != Some(b"FontDescriptor") {
            continue;
        }
        if let Ok(ff) = fd.get(b"FontFile").and_then(Object::as_reference) {
            targets.entry(ff).or_default().push(*fd_id);
        }
    }

    let mut converted = 0;
    for (ff_id, fds) in targets {
        let Some(cff) = doc
            .get_object(ff_id)
            .and_then(Object::as_stream)
            .ok()
            .and_then(Type1::from_stream)
            .and_then(|t1| to_cff(&t1))
        else {
            continue;
        };
        let old_len = doc
            .get_object(ff_id)
            .and_then(Object::as_stream)
            .map(|s| s.content.len())
            .unwrap_or(usize::MAX);
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Type1C".to_vec()));
        let mut stream = Stream::new(dict, cff);
        let _ = stream.compress();
        if stream.content.len() >= old_len {
            continue;
        }
        let new_id = doc.add_object(Object::Stream(stream));
        for fd_id in fds {
            if let Ok(fd) = doc.get_dictionary_mut(fd_id) {
                fd.remove(b"FontFile");
                fd.set("FontFile3", Object::Reference(new_id));
            }
        }
        converted += 1;
    }
    if converted > 0 {
        doc.prune_objects();
    }
    converted
}

// ---------------------------------------------------------------------------
// Type 1 font-level values (cleartext part and private dictionary)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum PsVal {
    Num(f64),
    Bool(bool),
    Name(Vec<u8>),
    Str(Vec<u8>),
    Array(Vec<f64>),
}

/// Value following `/key` in PostScript source (first occurrence).
fn ps_value(src: &[u8], key: &str) -> Option<PsVal> {
    let needle = format!("/{key}");
    let mut from = 0;
    let pos = loop {
        let p = src
            .get(from..)?
            .windows(needle.len())
            .position(|w| w == needle.as_bytes())?
            + from;
        // Must be the whole name, not a prefix (/BlueValues vs /BlueValuesX).
        let next = src.get(p + needle.len()).copied().unwrap_or(b' ');
        if next.is_ascii_alphanumeric() {
            from = p + 1;
            continue;
        }
        break p + needle.len();
    };
    let mut i = pos;
    while i < src.len() && src[i].is_ascii_whitespace() {
        i += 1;
    }
    let c = *src.get(i)?;
    match c {
        b'(' => {
            let mut depth = 0;
            let mut out = Vec::new();
            let mut j = i;
            while j < src.len() {
                let b = src[j];
                if b == b'\\' {
                    if let Some(&n) = src.get(j + 1) {
                        out.push(n);
                    }
                    j += 2;
                    continue;
                }
                if b == b'(' {
                    depth += 1;
                    if depth == 1 {
                        j += 1;
                        continue;
                    }
                } else if b == b')' {
                    depth -= 1;
                    if depth == 0 {
                        return Some(PsVal::Str(out));
                    }
                }
                out.push(b);
                j += 1;
            }
            None
        }
        b'[' | b'{' => {
            let close = if c == b'[' { b']' } else { b'}' };
            let end = src[i + 1..].iter().position(|&b| b == close)? + i + 1;
            let nums = std::str::from_utf8(&src[i + 1..end])
                .ok()?
                .split_ascii_whitespace()
                .map(|t| t.parse::<f64>())
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            Some(PsVal::Array(nums))
        }
        b'/' => {
            let end = src[i + 1..]
                .iter()
                .position(|b| b.is_ascii_whitespace() || b"/[]{}()".contains(b))
                .map_or(src.len(), |p| p + i + 1);
            Some(PsVal::Name(src[i + 1..end].to_vec()))
        }
        _ => {
            let end = src[i..]
                .iter()
                .position(|b| b.is_ascii_whitespace() || b"/[]{}()".contains(b))
                .map_or(src.len(), |p| p + i);
            let tok = std::str::from_utf8(&src[i..end]).ok()?;
            match tok {
                "true" => Some(PsVal::Bool(true)),
                "false" => Some(PsVal::Bool(false)),
                t => t.parse::<f64>().ok().map(PsVal::Num),
            }
        }
    }
}

fn num_of(v: Option<PsVal>) -> Option<f64> {
    match v? {
        PsVal::Num(n) => Some(n),
        PsVal::Array(a) => a.first().copied(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Type 1 charstring interpreter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Curve([f64; 6]),
}

#[derive(Debug, Default)]
struct Glyph {
    width: f64,
    segs: Vec<Seg>,
    hstems: Vec<(f64, f64)>,
    vstems: Vec<(f64, f64)>,
    /// Type 2 accented-character arguments: (adx − asb, ady, bchar, achar).
    seac: Option<(f64, f64, f64, f64)>,
}

struct Interp<'a> {
    subrs: &'a BTreeMap<usize, Entry>,
    stack: Vec<f64>,
    ps: Vec<f64>,
    x: f64,
    y: f64,
    sbx: f64,
    sby: f64,
    flex: Option<Vec<(f64, f64)>>,
    g: Glyph,
    done: bool,
}

/// Why a charstring couldn't be converted.
#[derive(Debug)]
struct Unsupported;

type R<T> = Result<T, Unsupported>;

impl Interp<'_> {
    fn pop(&mut self) -> R<f64> {
        self.stack.pop().ok_or(Unsupported)
    }

    fn args<const N: usize>(&mut self) -> R<[f64; N]> {
        if self.stack.len() < N {
            return Err(Unsupported);
        }
        let start = self.stack.len() - N;
        let mut out = [0.0; N];
        out.copy_from_slice(&self.stack[start..]);
        self.stack.clear();
        Ok(out)
    }

    fn move_to(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        if self.flex.is_none() {
            self.g.segs.push(Seg::Move(self.x, self.y));
        }
    }

    fn line_to(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        self.g.segs.push(Seg::Line(self.x, self.y));
    }

    fn curve_to(&mut self, d: [f64; 6]) {
        let (x1, y1) = (self.x + d[0], self.y + d[1]);
        let (x2, y2) = (x1 + d[2], y1 + d[3]);
        let (x3, y3) = (x2 + d[4], y2 + d[5]);
        self.x = x3;
        self.y = y3;
        self.g.segs.push(Seg::Curve([x1, y1, x2, y2, x3, y3]));
    }

    fn run(&mut self, cs: &[u8], depth: u32) -> R<()> {
        if depth > 10 {
            return Err(Unsupported);
        }
        let mut i = 0;
        while i < cs.len() && !self.done {
            let v = cs[i];
            i += 1;
            if v >= 32 {
                let n = match v {
                    32..=246 => v as f64 - 139.0,
                    247..=250 => {
                        let w = *cs.get(i).ok_or(Unsupported)? as f64;
                        i += 1;
                        (v as f64 - 247.0) * 256.0 + w + 108.0
                    }
                    251..=254 => {
                        let w = *cs.get(i).ok_or(Unsupported)? as f64;
                        i += 1;
                        -(v as f64 - 251.0) * 256.0 - w - 108.0
                    }
                    _ => {
                        let b = cs.get(i..i + 4).ok_or(Unsupported)?;
                        i += 4;
                        i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64
                    }
                };
                self.stack.push(n);
                continue;
            }
            let op = if v == 12 {
                let e = *cs.get(i).ok_or(Unsupported)?;
                i += 1;
                1200 + e as u16
            } else {
                v as u16
            };
            match op {
                13 => {
                    let [sbx, wx] = self.args()?;
                    self.sbx = sbx;
                    self.sby = 0.0;
                    self.x = sbx;
                    self.y = 0.0;
                    self.g.width = wx;
                }
                1207 => {
                    let [sbx, sby, wx, _wy] = self.args()?;
                    self.sbx = sbx;
                    self.sby = sby;
                    self.x = sbx;
                    self.y = sby;
                    self.g.width = wx;
                }
                21 => {
                    let [dx, dy] = self.args()?;
                    self.move_to(dx, dy);
                }
                22 => {
                    let [dx] = self.args()?;
                    self.move_to(dx, 0.0);
                }
                4 => {
                    let [dy] = self.args()?;
                    self.move_to(0.0, dy);
                }
                5 => {
                    let [dx, dy] = self.args()?;
                    self.line_to(dx, dy);
                }
                6 => {
                    let [dx] = self.args()?;
                    self.line_to(dx, 0.0);
                }
                7 => {
                    let [dy] = self.args()?;
                    self.line_to(0.0, dy);
                }
                8 => {
                    let d = self.args::<6>()?;
                    self.curve_to(d);
                }
                30 => {
                    let [dy1, dx2, dy2, dx3] = self.args()?;
                    self.curve_to([0.0, dy1, dx2, dy2, dx3, 0.0]);
                }
                31 => {
                    let [dx1, dx2, dy2, dy3] = self.args()?;
                    self.curve_to([dx1, 0.0, dx2, dy2, 0.0, dy3]);
                }
                9 => self.stack.clear(), // closepath: implicit in Type 2
                10 => {
                    let idx = self.pop()?;
                    if idx < 0.0 {
                        return Err(Unsupported);
                    }
                    // Missing entries are `return` stubs dropped by the parser.
                    if let Some(sub) = self.subrs.get(&(idx as usize)) {
                        let plain = sub.plain.clone();
                        self.run(&plain, depth + 1)?;
                    }
                }
                11 => return Ok(()),
                14 => {
                    self.stack.clear();
                    self.done = true;
                }
                1 => {
                    let [y, dy] = self.args()?;
                    self.g.hstems.push((y + self.sby, dy));
                }
                3 => {
                    let [x, dx] = self.args()?;
                    self.g.vstems.push((x + self.sbx, dx));
                }
                1202 => {
                    let a = self.args::<6>()?;
                    for k in 0..3 {
                        self.g.hstems.push((a[2 * k] + self.sby, a[2 * k + 1]));
                    }
                }
                1201 => {
                    let a = self.args::<6>()?;
                    for k in 0..3 {
                        self.g.vstems.push((a[2 * k] + self.sbx, a[2 * k + 1]));
                    }
                }
                1200 => self.stack.clear(), // dotsection
                1206 => {
                    let [asb, adx, ady, bchar, achar] = self.args()?;
                    self.g.seac = Some((adx - asb, ady, bchar, achar));
                    self.done = true;
                }
                1212 => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    if b == 0.0 {
                        return Err(Unsupported);
                    }
                    self.stack.push(a / b);
                }
                1216 => {
                    let other = self.pop()?;
                    let n = self.pop()? as usize;
                    if self.stack.len() < n {
                        return Err(Unsupported);
                    }
                    let args = self.stack.split_off(self.stack.len() - n);
                    self.other_subr(other as i64, &args)?;
                }
                1217 => {
                    let v = self.ps.pop().ok_or(Unsupported)?;
                    self.stack.push(v);
                }
                1233 => {
                    let [x, y] = self.args()?;
                    self.x = x;
                    self.y = y;
                }
                _ => return Err(Unsupported),
            }
        }
        Ok(())
    }

    fn other_subr(&mut self, n: i64, args: &[f64]) -> R<()> {
        match n {
            // Flex start: from here, rmovetos only collect points.
            1 => self.flex = Some(Vec::new()),
            // Flex point.
            2 => self
                .flex
                .as_mut()
                .ok_or(Unsupported)?
                .push((self.x, self.y)),
            // Flex end: reference point + 6 control/end points → 2 curves.
            0 => {
                let pts = self.flex.take().ok_or(Unsupported)?;
                if pts.len() != 7 || args.len() != 3 {
                    return Err(Unsupported);
                }
                let c = |a: (f64, f64), b: (f64, f64), e: (f64, f64)| {
                    Seg::Curve([a.0, a.1, b.0, b.1, e.0, e.1])
                };
                self.g.segs.push(c(pts[1], pts[2], pts[3]));
                self.g.segs.push(c(pts[4], pts[5], pts[6]));
                self.x = pts[6].0;
                self.y = pts[6].1;
                // `pop pop setcurrentpoint` reads back x then y.
                self.ps = vec![args[2], args[1]];
            }
            // Hint replacement: hands the subr number back for `pop callsubr`.
            3 => self.ps = vec![*args.first().ok_or(Unsupported)?],
            _ => return Err(Unsupported),
        }
        Ok(())
    }
}

fn interpret(plain: &[u8], subrs: &BTreeMap<usize, Entry>) -> R<Glyph> {
    let mut it = Interp {
        subrs,
        stack: Vec::new(),
        ps: Vec::new(),
        x: 0.0,
        y: 0.0,
        sbx: 0.0,
        sby: 0.0,
        flex: None,
        g: Glyph::default(),
        done: false,
    };
    it.run(plain, 0)?;
    if !it.done || it.flex.is_some() {
        return Err(Unsupported);
    }
    Ok(it.g)
}

// ---------------------------------------------------------------------------
// Type 2 charstring encoder
// ---------------------------------------------------------------------------

fn snap(v: f64) -> f64 {
    let r = v.round();
    if (v - r).abs() < 1e-6 { r } else { v }
}

fn t2_num(out: &mut Vec<u8>, v: f64) {
    let v = snap(v);
    if v.fract() == 0.0 && (-32768.0..=32767.0).contains(&v) {
        let i = v as i32;
        match i {
            -107..=107 => out.push((i + 139) as u8),
            108..=1131 => {
                let j = i - 108;
                out.extend_from_slice(&[((j >> 8) + 247) as u8, (j & 0xff) as u8]);
            }
            -1131..=-108 => {
                let j = -i - 108;
                out.extend_from_slice(&[((j >> 8) + 251) as u8, (j & 0xff) as u8]);
            }
            _ => {
                out.push(28);
                out.extend_from_slice(&(i as i16).to_be_bytes());
            }
        }
    } else {
        out.push(255);
        out.extend_from_slice(&((v * 65536.0).round() as i32).to_be_bytes());
    }
}

/// Sorted, de-duplicated, non-overlapping stems (Type 2 requires it without
/// hint masks), at most `max`.
fn clean_stems(stems: &[(f64, f64)], max: usize) -> Vec<(f64, f64)> {
    let mut s: Vec<(f64, f64)> = stems.to_vec();
    s.sort_by(|a, b| {
        let (la, lb) = (a.0.min(a.0 + a.1), b.0.min(b.0 + b.1));
        la.partial_cmp(&lb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut prev_hi = f64::NEG_INFINITY;
    for (p, d) in s {
        let (lo, hi) = (p.min(p + d), p.max(p + d));
        if lo <= prev_hi || out.len() == max {
            continue;
        }
        out.push((p, d));
        prev_hi = hi;
    }
    out
}

struct T2 {
    out: Vec<u8>,
    width: Option<f64>,
}

impl T2 {
    fn op(&mut self, args: &[f64], op: &[u8]) {
        if let Some(w) = self.width.take() {
            t2_num(&mut self.out, w);
        }
        for &a in args {
            t2_num(&mut self.out, a);
        }
        self.out.extend_from_slice(op);
    }
}

fn encode_t2(g: &Glyph) -> Vec<u8> {
    let mut t = T2 {
        out: Vec::new(),
        width: (snap(g.width) != 0.0).then_some(g.width),
    };
    for (stems, op) in [(&g.hstems, 1u8), (&g.vstems, 3u8)] {
        let stems = clean_stems(stems, 23);
        if stems.is_empty() {
            continue;
        }
        let mut args = Vec::with_capacity(stems.len() * 2);
        let mut last = 0.0;
        for (p, d) in stems {
            args.push(p - last);
            args.push(d);
            last = p + d;
        }
        t.op(&args, &[op]);
    }

    // Collapse consecutive moves (empty subpaths).
    let mut segs: Vec<Seg> = Vec::with_capacity(g.segs.len());
    for s in &g.segs {
        if let (Seg::Move(..), Some(Seg::Move(..))) = (s, segs.last()) {
            segs.pop();
        }
        segs.push(*s);
    }
    while let Some(Seg::Move(..)) = segs.last() {
        segs.pop();
    }
    if !matches!(segs.first(), None | Some(Seg::Move(..))) {
        segs.insert(0, Seg::Move(0.0, 0.0));
    }

    let (mut cx, mut cy) = (0.0f64, 0.0f64);
    let mut i = 0;
    while i < segs.len() {
        match segs[i] {
            Seg::Move(x, y) => {
                let (dx, dy) = (snap(x - cx), snap(y - cy));
                if dy == 0.0 {
                    t.op(&[dx], &[22]);
                } else if dx == 0.0 {
                    t.op(&[dy], &[4]);
                } else {
                    t.op(&[dx, dy], &[21]);
                }
                cx = x;
                cy = y;
                i += 1;
            }
            Seg::Line(..) => {
                // A run of lines: alternating h/v chains, the rest as rlineto.
                let mut args = Vec::new();
                let mut kind: Option<(u8, bool)> = None; // (op, next is horizontal)
                let mut rl = Vec::new();
                let flush_rl = |t: &mut T2, rl: &mut Vec<f64>| {
                    if !rl.is_empty() {
                        t.op(rl, &[5]);
                        rl.clear();
                    }
                };
                while let Some(Seg::Line(x, y)) = segs.get(i).copied() {
                    let (dx, dy) = (snap(x - cx), snap(y - cy));
                    let horiz = dy == 0.0;
                    let vert = dx == 0.0 && !horiz;
                    let continues =
                        matches!(kind, Some((_, next_h)) if (next_h && horiz) || (!next_h && vert));
                    if continues && args.len() < 46 {
                        args.push(if horiz { dx } else { dy });
                        if let Some((op, h)) = kind {
                            kind = Some((op, !h));
                        }
                    } else if horiz || vert {
                        if let Some((op, _)) = kind.take() {
                            t.op(&args, &[op]);
                            args.clear();
                        }
                        flush_rl(&mut t, &mut rl);
                        args.push(if horiz { dx } else { dy });
                        kind = Some((if horiz { 6 } else { 7 }, !horiz));
                    } else {
                        if let Some((op, _)) = kind.take() {
                            t.op(&args, &[op]);
                            args.clear();
                        }
                        if rl.len() >= 46 {
                            flush_rl(&mut t, &mut rl);
                        }
                        rl.push(dx);
                        rl.push(dy);
                    }
                    cx = x;
                    cy = y;
                    i += 1;
                }
                if let Some((op, _)) = kind {
                    t.op(&args, &[op]);
                }
                flush_rl(&mut t, &mut rl);
            }
            Seg::Curve(_) => {
                let mut rr = Vec::new();
                while let Some(Seg::Curve(c)) = segs.get(i).copied() {
                    let d = [
                        snap(c[0] - cx),
                        snap(c[1] - cy),
                        snap(c[2] - c[0]),
                        snap(c[3] - c[1]),
                        snap(c[4] - c[2]),
                        snap(c[5] - c[3]),
                    ];
                    let hv = d[1] == 0.0 && d[4] == 0.0;
                    let vh = d[0] == 0.0 && d[5] == 0.0;
                    if hv || vh {
                        if !rr.is_empty() {
                            t.op(&rr, &[8]);
                            rr.clear();
                        }
                        if hv {
                            t.op(&[d[0], d[2], d[3], d[5]], &[31]);
                        } else {
                            t.op(&[d[1], d[2], d[3], d[4]], &[30]);
                        }
                    } else {
                        if rr.len() >= 48 {
                            t.op(&rr, &[8]);
                            rr.clear();
                        }
                        rr.extend_from_slice(&d);
                    }
                    cx = c[4];
                    cy = c[5];
                    i += 1;
                }
                if !rr.is_empty() {
                    t.op(&rr, &[8]);
                }
            }
        }
    }
    match g.seac {
        Some((adx, ady, b, a)) => t.op(&[adx, ady, b, a], &[14]),
        None => t.op(&[], &[14]),
    }
    t.out
}

// ---------------------------------------------------------------------------
// CFF writer
// ---------------------------------------------------------------------------

fn index(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = (items.len() as u16).to_be_bytes().to_vec();
    if items.is_empty() {
        return out;
    }
    let total: usize = items.iter().map(Vec::len).sum::<usize>() + 1;
    let off_size: u8 = match total {
        0..=0xff => 1,
        0x100..=0xffff => 2,
        0x10000..=0xff_ffff => 3,
        _ => 4,
    };
    out.push(off_size);
    let mut off = 1usize;
    let push_off = |out: &mut Vec<u8>, v: usize| {
        let b = (v as u32).to_be_bytes();
        out.extend_from_slice(&b[4 - off_size as usize..]);
    };
    push_off(&mut out, off);
    for it in items {
        off += it.len();
        push_off(&mut out, off);
    }
    for it in items {
        out.extend_from_slice(it);
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

/// Fixed-size (5-byte) integer, for offsets patched after layout.
fn dict_int5(out: &mut Vec<u8>, v: i32) {
    out.push(29);
    out.extend_from_slice(&v.to_be_bytes());
}

fn dict_num(out: &mut Vec<u8>, v: f64) {
    let v = snap(v);
    if v.fract() == 0.0 && v.abs() < 2e9 {
        dict_int(out, v as i32);
        return;
    }
    let s = format!("{v}");
    let mut nibbles: Vec<u8> = Vec::new();
    for ch in s.bytes() {
        nibbles.push(match ch {
            b'0'..=b'9' => ch - b'0',
            b'.' => 0xa,
            b'-' => 0xe,
            _ => return dict_int(out, v.round() as i32),
        });
    }
    nibbles.push(0xf);
    if nibbles.len() % 2 == 1 {
        nibbles.push(0xf);
    }
    out.push(30);
    for p in nibbles.chunks(2) {
        out.push((p[0] << 4) | p[1]);
    }
}

fn dict_op(out: &mut Vec<u8>, op: u16) {
    if op >= 1200 {
        out.extend_from_slice(&[12, (op - 1200) as u8]);
    } else {
        out.push(op as u8);
    }
}

struct Strings {
    standard: HashMap<&'static [u8], u16>,
    custom: Vec<Vec<u8>>,
    custom_ids: HashMap<Vec<u8>, u16>,
}

impl Strings {
    fn new() -> Self {
        Strings {
            standard: STANDARD_STRINGS
                .iter()
                .enumerate()
                .map(|(i, s)| (s.as_bytes(), i as u16))
                .collect(),
            custom: Vec::new(),
            custom_ids: HashMap::new(),
        }
    }
    fn sid(&mut self, s: &[u8]) -> u16 {
        if let Some(&i) = self.standard.get(s) {
            return i;
        }
        if let Some(&i) = self.custom_ids.get(s) {
            return i;
        }
        let i = 391 + self.custom.len() as u16;
        self.custom.push(s.to_vec());
        self.custom_ids.insert(s.to_vec(), i);
        i
    }
}

fn to_cff(t1: &Type1) -> Option<Vec<u8>> {
    let clear = &t1.clear;
    let private: Vec<u8> = [t1.head.as_slice(), t1.middle.as_slice()].concat();

    let font_name = match ps_value(clear, "FontName")? {
        PsVal::Name(n) => n,
        _ => return None,
    };

    // Charstrings → Type 2.
    let mut glyphs: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    for (name, entry) in &t1.charstrings {
        let g = interpret(&entry.plain, &t1.subrs).ok()?;
        glyphs.insert(name.clone(), encode_t2(&g));
    }
    let notdef = glyphs
        .remove(b".notdef".as_slice())
        .unwrap_or_else(|| vec![14]);

    // Glyph order: .notdef, then (custom encoding) encoded glyphs by first
    // code, then the rest.
    let standard_encoding = t1.encoding.is_empty();
    if standard_encoding && !clear.windows(16).any(|w| w == b"StandardEncoding") {
        return None;
    }
    let mut order: Vec<Vec<u8>> = Vec::new();
    let mut codes: Vec<u8> = Vec::new();
    let mut supplements: Vec<(u8, Vec<u8>)> = Vec::new();
    if !standard_encoding {
        for (&code, name) in &t1.encoding {
            if code > 255 || !glyphs.contains_key(name) {
                continue;
            }
            if order.contains(name) {
                supplements.push((code as u8, name.clone()));
            } else {
                order.push(name.clone());
                codes.push(code as u8);
            }
        }
    }
    if codes.len() > 255 {
        return None;
    }
    for name in glyphs.keys() {
        if !order.contains(name) {
            order.push(name.clone());
        }
    }

    let mut strings = Strings::new();
    let mut charset = vec![0u8];
    for name in &order {
        charset.extend_from_slice(&strings.sid(name).to_be_bytes());
    }
    let encoding = (!standard_encoding).then(|| {
        let mut e = vec![
            if supplements.is_empty() { 0 } else { 0x80 },
            codes.len() as u8,
        ];
        e.extend_from_slice(&codes);
        if !supplements.is_empty() {
            e.push(supplements.len() as u8);
            for (code, name) in &supplements {
                e.push(*code);
                e.extend_from_slice(&strings.sid(name).to_be_bytes());
            }
        }
        e
    });
    let mut charstrings = vec![notdef];
    charstrings.extend(order.iter().map(|n| glyphs[n].clone()));

    // Private DICT.
    let mut priv_dict = Vec::new();
    for (key, op) in [
        ("BlueValues", 6u16),
        ("OtherBlues", 7),
        ("FamilyBlues", 8),
        ("FamilyOtherBlues", 9),
        ("StemSnapH", 1212),
        ("StemSnapV", 1213),
    ] {
        if let Some(PsVal::Array(a)) = ps_value(&private, key)
            && !a.is_empty()
        {
            let mut prev = 0.0;
            for v in a {
                dict_num(&mut priv_dict, v - prev);
                prev = v;
            }
            dict_op(&mut priv_dict, op);
        }
    }
    for (key, op, default) in [
        ("StdHW", 10u16, f64::NAN),
        ("StdVW", 11, f64::NAN),
        ("BlueScale", 1209, 0.039625),
        ("BlueShift", 1210, 7.0),
        ("BlueFuzz", 1211, 1.0),
        ("LanguageGroup", 1217, 0.0),
        ("ExpansionFactor", 1218, 0.06),
    ] {
        if let Some(v) = num_of(ps_value(&private, key))
            && v != default
        {
            dict_num(&mut priv_dict, v);
            dict_op(&mut priv_dict, op);
        }
    }
    if let Some(PsVal::Bool(true)) = ps_value(&private, "ForceBold") {
        dict_int(&mut priv_dict, 1);
        dict_op(&mut priv_dict, 1214);
    }

    // Top DICT (offsets patched after layout, hence fixed-size ints).
    let top_strings: Vec<(u16, Option<u16>)> = [
        ("version", 0u16),
        ("Notice", 1),
        ("FullName", 2),
        ("FamilyName", 3),
        ("Weight", 4),
    ]
    .iter()
    .map(|(k, op)| {
        let sid = match ps_value(clear, k) {
            Some(PsVal::Str(s)) => Some(strings.sid(&s)),
            _ => None,
        };
        (*op, sid)
    })
    .collect();
    let bbox = match ps_value(clear, "FontBBox") {
        Some(PsVal::Array(a)) if a.len() == 4 => a,
        _ => vec![0.0; 4],
    };
    let matrix = match ps_value(clear, "FontMatrix") {
        Some(PsVal::Array(a)) if a.len() == 6 => Some(a),
        _ => None,
    };
    let italic = num_of(ps_value(clear, "ItalicAngle")).unwrap_or(0.0);
    let ul_pos = num_of(ps_value(clear, "UnderlinePosition")).unwrap_or(-100.0);
    let ul_thick = num_of(ps_value(clear, "UnderlineThickness")).unwrap_or(50.0);
    let fixed = matches!(ps_value(clear, "isFixedPitch"), Some(PsVal::Bool(true)));

    let top = |charset_off: i32, enc_off: i32, cs_off: i32, priv_off: i32| -> Vec<u8> {
        let mut d = Vec::new();
        for (op, sid) in &top_strings {
            if let Some(sid) = sid {
                dict_int(&mut d, *sid as i32);
                dict_op(&mut d, *op);
            }
        }
        if fixed {
            dict_int(&mut d, 1);
            dict_op(&mut d, 1201);
        }
        if italic != 0.0 {
            dict_num(&mut d, italic);
            dict_op(&mut d, 1202);
        }
        if ul_pos != -100.0 {
            dict_num(&mut d, ul_pos);
            dict_op(&mut d, 1203);
        }
        if ul_thick != 50.0 {
            dict_num(&mut d, ul_thick);
            dict_op(&mut d, 1204);
        }
        for v in &bbox {
            dict_num(&mut d, *v);
        }
        dict_op(&mut d, 5);
        if let Some(m) = &matrix
            && m != &[0.001, 0.0, 0.0, 0.001, 0.0, 0.0]
        {
            for v in m {
                dict_num(&mut d, *v);
            }
            dict_op(&mut d, 1207);
        }
        dict_int5(&mut d, charset_off);
        dict_op(&mut d, 15);
        if enc_off != 0 {
            dict_int5(&mut d, enc_off);
            dict_op(&mut d, 16);
        }
        dict_int5(&mut d, cs_off);
        dict_op(&mut d, 17);
        dict_int5(&mut d, priv_dict.len() as i32);
        dict_int5(&mut d, priv_off);
        dict_op(&mut d, 18);
        d
    };

    let header = [1u8, 0, 4, 4];
    let name_idx = index(&[font_name]);
    let string_idx = index(&strings.custom);
    let gsubr_idx = index(&[]);
    let cs_idx = index(&charstrings);
    let enc_bytes = encoding.unwrap_or_default();
    let placeholder = top(0, if enc_bytes.is_empty() { 0 } else { 1 }, 0, 0);
    let top_idx_len = index(&[placeholder]).len();
    let charset_off =
        header.len() + name_idx.len() + top_idx_len + string_idx.len() + gsubr_idx.len();
    let enc_off = charset_off + charset.len();
    let cs_off = enc_off + enc_bytes.len();
    let priv_off = cs_off + cs_idx.len();
    let top_dict = top(
        charset_off as i32,
        if enc_bytes.is_empty() {
            0
        } else {
            enc_off as i32
        },
        cs_off as i32,
        priv_off as i32,
    );
    let top_idx = index(&[top_dict]);
    debug_assert_eq!(top_idx.len(), top_idx_len);

    let mut out = header.to_vec();
    out.extend_from_slice(&name_idx);
    out.extend_from_slice(&top_idx);
    out.extend_from_slice(&string_idx);
    out.extend_from_slice(&gsubr_idx);
    out.extend_from_slice(&charset);
    out.extend_from_slice(&enc_bytes);
    out.extend_from_slice(&cs_idx);
    out.extend_from_slice(&priv_dict);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs(ops: &[&[u8]]) -> Vec<u8> {
        ops.concat()
    }
    fn n(v: i32) -> Vec<u8> {
        let mut o = Vec::new();
        match v {
            -107..=107 => o.push((v + 139) as u8),
            _ => {
                o.push(255);
                o.extend_from_slice(&v.to_be_bytes());
            }
        }
        o
    }

    #[test]
    fn hsbw_offsets_the_outline_and_sets_width() {
        // 50 500 hsbw  10 20 rmoveto  100 0 rlineto  0 100 rlineto  closepath endchar
        let t1 = cs(&[
            &n(50),
            &n(500),
            &[13],
            &n(10),
            &n(20),
            &[21],
            &n(100),
            &n(0),
            &[5],
            &n(0),
            &n(100),
            &[5],
            &[9],
            &[14],
        ]);
        let g = interpret(&t1, &BTreeMap::new()).unwrap();
        assert_eq!(g.width, 500.0);
        assert_eq!(
            g.segs,
            vec![
                Seg::Move(60.0, 20.0),
                Seg::Line(160.0, 20.0),
                Seg::Line(160.0, 120.0)
            ]
        );
        let t2 = encode_t2(&g);
        // width, rmoveto 60 20, hlineto 100 vlineto... (alternating chain), endchar
        let mut want = Vec::new();
        t2_num(&mut want, 500.0);
        t2_num(&mut want, 60.0);
        t2_num(&mut want, 20.0);
        want.push(21);
        t2_num(&mut want, 100.0);
        t2_num(&mut want, 100.0);
        want.push(6);
        want.push(14);
        assert_eq!(t2, want);
    }

    #[test]
    fn flex_becomes_two_curves() {
        // Standard flex via othersubrs 1/2/0 with 7 rmoveto points.
        let mut t1 = cs(&[&n(0), &n(500), &[13], &n(0), &n(0), &[21]]);
        t1.extend(cs(&[&n(0), &n(1), &[12, 16]])); // 0 1 callothersubr
        let pts = [(10, 0), (5, 5), (5, 0), (5, -5), (5, -5), (5, 0), (5, 5)];
        for (dx, dy) in pts {
            t1.extend(cs(&[&n(dx), &n(dy), &[21], &n(0), &n(2), &[12, 16]]));
        }
        // 50 x y 3 0 callothersubr pop pop setcurrentpoint
        t1.extend(cs(&[
            &n(50),
            &n(40),
            &n(0),
            &n(3),
            &n(0),
            &[12, 16],
            &[12, 17],
            &[12, 17],
            &[12, 33],
            &[14],
        ]));
        let g = interpret(&t1, &BTreeMap::new()).unwrap();
        assert_eq!(g.segs.len(), 3);
        assert_eq!(g.segs[1], Seg::Curve([15.0, 5.0, 20.0, 5.0, 25.0, 0.0]));
        assert_eq!(g.segs[2], Seg::Curve([30.0, -5.0, 35.0, -5.0, 40.0, 0.0]));
    }

    #[test]
    fn overlapping_stems_are_dropped() {
        let s = clean_stems(
            &[(100.0, 50.0), (120.0, 40.0), (200.0, 20.0), (0.0, -21.0)],
            23,
        );
        assert_eq!(s, vec![(0.0, -21.0), (100.0, 50.0), (200.0, 20.0)]);
    }

    #[test]
    fn dict_reals_use_nibbles() {
        let mut o = Vec::new();
        dict_num(&mut o, 0.04379);
        assert_eq!(o, vec![30, 0x0a, 0x04, 0x37, 0x9f]);
    }

    #[test]
    fn ps_values_are_read() {
        let src = b"/FontName /CMR10 def /FontBBox {-40 -250 1009 750} readonly def /Notice (Copyright \\(c\\) AMS) def /isFixedPitch false def";
        assert_eq!(
            ps_value(src, "FontName"),
            Some(PsVal::Name(b"CMR10".to_vec()))
        );
        assert_eq!(
            ps_value(src, "FontBBox"),
            Some(PsVal::Array(vec![-40.0, -250.0, 1009.0, 750.0]))
        );
        assert_eq!(
            ps_value(src, "Notice"),
            Some(PsVal::Str(b"Copyright (c) AMS".to_vec()))
        );
        assert_eq!(ps_value(src, "isFixedPitch"), Some(PsVal::Bool(false)));
    }

    #[test]
    fn toy_type1_converts_to_a_well_formed_cff() {
        // .notdef: 0 500 hsbw endchar;  A: 20 600 hsbw 0 0 rmoveto 100 0 rlineto 0 100 rlineto endchar
        let notdef = cs(&[&n(0), &n(500), &[13], &[14]]);
        let a = cs(&[
            &n(20),
            &n(600),
            &[13],
            &n(0),
            &n(0),
            &[21],
            &n(100),
            &n(0),
            &[5],
            &n(0),
            &n(100),
            &[5],
            &[9],
            &[14],
        ]);
        let stream =
            crate::type1_merge::tests::toy(&[(".notdef", &notdef), ("A", &a)], 0, &[(65, "A")]);
        let t1 = Type1::from_stream(&stream).unwrap();
        let cff = to_cff(&t1).unwrap();
        assert_eq!(&cff[..4], &[1, 0, 4, 4]);
        // Name INDEX: 1 entry, offSize 1, offsets 1..4, "Toy".
        assert_eq!(&cff[4..11], &[0, 1, 1, 1, 4, b'T', b'o']);
        // Custom encoding: format 0, one code (65) for glyph 1 ("A").
        assert!(cff.windows(3).any(|w| w == [0, 1, 65]));
    }
}
