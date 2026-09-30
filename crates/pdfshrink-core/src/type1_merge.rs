//! Merges subsets of the same Type 1 font (`/FontFile`) into one program
//! (experimental levels only).
//!
//! LaTeX documents that include PDF figures carry one subset of each Computer
//! Modern font per figure (a thesis: 32 × CMR10, 29 × CMMI10, …). Type 1
//! programs are `eexec`-encrypted, so Flate barely compresses them and every
//! copy costs its full size. Type 1 subsetters keep glyph *names* and `Subrs`
//! *indices* from the original font (unused subroutines are simply dropped
//! off the end), so a union — glyphs by name, subroutines by index — is a
//! valid font that renders every subset's text unchanged: simple fonts look
//! glyphs up by name through their `/Encoding`.
//!
//! Only merged when provably consistent: same base font name, identical
//! cleartext part (ignoring the built-in encoding's entries, which are merged
//! and must not conflict), identical private dictionary outside `Subrs` and
//! `CharStrings`, and identical decrypted charstrings/subroutines wherever two
//! subsets both define one.

use std::collections::{BTreeMap, HashMap};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

const EEXEC_KEY: u16 = 55665;
const CHARSTRING_KEY: u16 = 4330;
/// Type 1 charstring `return` operator.
const RETURN: u8 = 11;

/// Returns the number of font programs merged away.
pub fn merge_type1_subsets(doc: &mut Document) -> usize {
    // FontFile stream id -> (base name, FontDescriptors using it).
    let mut by_file: BTreeMap<ObjectId, (Vec<u8>, Vec<ObjectId>)> = BTreeMap::new();
    for (fd_id, obj) in &doc.objects {
        let Ok(fd) = obj.as_dict() else { continue };
        if fd.get(b"Type").and_then(Object::as_name).ok() != Some(b"FontDescriptor") {
            continue;
        }
        let Ok(ff_id) = fd.get(b"FontFile").and_then(Object::as_reference) else {
            continue;
        };
        let name = fd
            .get(b"FontName")
            .and_then(Object::as_name)
            .map(base_name)
            .unwrap_or_default();
        let e = by_file.entry(ff_id).or_insert_with(|| (name, Vec::new()));
        if !e.1.contains(fd_id) {
            e.1.push(*fd_id);
        }
    }

    let mut groups: HashMap<Vec<u8>, Vec<(ObjectId, Type1)>> = HashMap::new();
    for (ff_id, (name, _)) in &by_file {
        if name.is_empty() {
            continue;
        }
        let Some(font) = doc
            .get_object(*ff_id)
            .and_then(Object::as_stream)
            .ok()
            .and_then(Type1::from_stream)
        else {
            continue;
        };
        groups.entry(name.clone()).or_default().push((*ff_id, font));
    }

    let mut merged_away = 0;
    for (_, fonts) in groups {
        if fonts.len() < 2 {
            continue;
        }
        let mut clusters: Vec<Vec<(ObjectId, Type1)>> = Vec::new();
        'next: for (id, f) in fonts {
            for c in clusters.iter_mut() {
                if c.iter().all(|(_, o)| o.consistent_with(&f)) {
                    c.push((id, f));
                    continue 'next;
                }
            }
            clusters.push(vec![(id, f)]);
        }
        for cluster in clusters {
            if cluster.len() < 2 {
                continue;
            }
            let fonts: Vec<&Type1> = cluster.iter().map(|(_, f)| f).collect();
            let Some((program, l1, l2, l3)) = build_merged(&fonts) else {
                continue;
            };
            let mut dict = Dictionary::new();
            dict.set("Length1", l1 as i64);
            dict.set("Length2", l2 as i64);
            dict.set("Length3", l3 as i64);
            let mut stream = Stream::new(dict, program);
            let _ = stream.compress();
            let new_id = doc.add_object(Object::Stream(stream));
            for (ff_id, _) in &cluster {
                for fd_id in &by_file[ff_id].1 {
                    if let Ok(fd) = doc.get_dictionary_mut(*fd_id) {
                        fd.set("FontFile", Object::Reference(new_id));
                        // /CharSet lists the glyphs present; optional, and now
                        // too narrow.
                        fd.remove(b"CharSet");
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

fn base_name(n: &[u8]) -> Vec<u8> {
    if n.len() > 7 && n[6] == b'+' && n[..6].iter().all(u8::is_ascii_uppercase) {
        n[7..].to_vec()
    } else {
        n.to_vec()
    }
}

/// A parsed Type 1 program.
pub(crate) struct Type1 {
    /// Cleartext part with the built-in encoding's `dup <code> /<name> put`
    /// entries removed (they're in `encoding`); `enc_at` is where they go back.
    pub(crate) clear: Vec<u8>,
    /// `clear` with the subset prefix of `/FontName /ABCDEF+Name` removed:
    /// what consistency is checked on.
    clear_key: Vec<u8>,
    enc_at: Option<usize>,
    pub(crate) encoding: BTreeMap<u32, Vec<u8>>,
    /// Decrypted private part, split around `Subrs` and `CharStrings`.
    pub(crate) head: Vec<u8>,
    pub(crate) middle: Vec<u8>,
    tail: Vec<u8>,
    /// Encrypted entries as stored, plus their decrypted form (minus the
    /// random `lenIV` prefix) for comparisons.
    /// Subroutines that aren't a bare `return` stub (subsetters replace the
    /// unused ones with that).
    pub(crate) subrs: BTreeMap<usize, Entry>,
    pub(crate) n_subrs: usize,
    pub(crate) len_iv: usize,
    pub(crate) charstrings: BTreeMap<Vec<u8>, Entry>,
    /// Spellings of the `RD` / `NP` / `ND` procedures used by this font.
    rd: Vec<u8>,
    np: Vec<u8>,
    nd: Vec<u8>,
    /// Bytes after the encrypted part (the 512 zeros + `cleartomark`).
    trailer: Vec<u8>,
}

pub(crate) struct Entry {
    pub(crate) stored: Vec<u8>,
    pub(crate) plain: Vec<u8>,
}

fn decrypt(data: &[u8], key: u16, skip: usize) -> Vec<u8> {
    let mut r = key;
    let mut out = Vec::with_capacity(data.len());
    for &c in data {
        out.push(c ^ (r >> 8) as u8);
        r = (c as u16)
            .wrapping_add(r)
            .wrapping_mul(52845)
            .wrapping_add(22719);
    }
    out.split_off(skip.min(out.len()))
}

fn encrypt(plain: &[u8], key: u16) -> Vec<u8> {
    let mut r = key;
    plain
        .iter()
        .map(|&p| {
            let c = p ^ (r >> 8) as u8;
            r = (c as u16)
                .wrapping_add(r)
                .wrapping_mul(52845)
                .wrapping_add(22719);
            c
        })
        .collect()
}

/// Minimal cursor over PostScript-ish bytes.
struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cur<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn token(&mut self) -> Option<&'a [u8]> {
        self.ws();
        let s = self.i;
        while self.i < self.b.len() && !self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
        (self.i > s).then(|| &self.b[s..self.i])
    }
    fn int(&mut self) -> Option<usize> {
        std::str::from_utf8(self.token()?).ok()?.parse().ok()
    }
    fn peek_token(&mut self) -> Option<&'a [u8]> {
        let save = self.i;
        let t = self.token();
        self.i = save;
        t
    }
    /// `<len> <RD> <binary>`: returns (RD spelling, binary).
    fn binary(&mut self) -> Option<(&'a [u8], &'a [u8])> {
        let len = self.int()?;
        let rd = self.token()?;
        self.i += 1; // the single space after RD
        let data = self.b.get(self.i..self.i + len)?;
        self.i += len;
        Some((rd, data))
    }
    /// The procedure closing an entry: `NP`/`ND`/`|`/`|-`, or `noaccess put|def`.
    fn closer(&mut self) -> Option<Vec<u8>> {
        let t = self.token()?;
        if t == b"noaccess" {
            let t2 = self.token()?;
            return Some([t, b" ", t2].concat());
        }
        Some(t.to_vec())
    }
}

/// Removes the `ABCDEF+` subset tag following `/FontName /`.
fn strip_subset_prefix(clear: &[u8]) -> Vec<u8> {
    let mut out = clear.to_vec();
    if let Some(p) = find(&out, b"/FontName /", 0) {
        let s = p + 11;
        if out.len() > s + 7
            && out[s + 6] == b'+'
            && out[s..s + 6].iter().all(u8::is_ascii_uppercase)
        {
            out.drain(s..s + 7);
        }
    }
    out
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

impl Type1 {
    pub(crate) fn from_stream(s: &Stream) -> Option<Type1> {
        let l1 = s.dict.get(b"Length1").and_then(Object::as_i64).ok()? as usize;
        let l2 = s.dict.get(b"Length2").and_then(Object::as_i64).ok()? as usize;
        let data = s
            .decompressed_content()
            .ok()
            .or_else(|| s.dict.get(b"Filter").is_err().then(|| s.content.clone()))?;
        let clear = data.get(..l1)?;
        let enc = data.get(l1..l1 + l2)?;
        // Binary eexec only (hex-encoded sections are rare in PDFs).
        if enc.len() < 4 || enc[..4].iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let trailer = data[l1 + l2..].to_vec();
        let private = decrypt(enc, EEXEC_KEY, 4);
        let len_iv = {
            let at = find(&private, b"/lenIV", 0);
            match at {
                Some(p) => Cur {
                    b: &private,
                    i: p + 6,
                }
                .int()?,
                None => 4,
            }
        };

        // Subrs (optional) then CharStrings.
        let (head, subrs, n_subrs, subrs_end, np, rd_s) = match find(&private, b"/Subrs", 0) {
            Some(p) => {
                let mut c = Cur {
                    b: &private,
                    i: p + 6,
                };
                let n = c.int()?;
                if c.token()? != b"array" {
                    return None;
                }
                let mut subrs = BTreeMap::new();
                let (mut np, mut rd) = (Vec::new(), Vec::new());
                while c.peek_token() == Some(b"dup") {
                    c.token();
                    let idx = c.int()?;
                    let (r, data) = c.binary()?;
                    rd = r.to_vec();
                    np = c.closer()?;
                    let plain = decrypt(data, CHARSTRING_KEY, len_iv);
                    if plain != [RETURN] {
                        subrs.insert(
                            idx,
                            Entry {
                                stored: data.to_vec(),
                                plain,
                            },
                        );
                    }
                }
                (private[..p].to_vec(), subrs, n, c.i, np, rd)
            }
            None => (Vec::new(), BTreeMap::new(), 0, 0, Vec::new(), Vec::new()),
        };
        let cs = find(&private, b"/CharStrings", subrs_end)?;
        let head = if head.is_empty() && subrs_end == 0 {
            private[..cs].to_vec()
        } else {
            head
        };
        let middle = if subrs_end == 0 {
            Vec::new()
        } else {
            private[subrs_end..cs].to_vec()
        };
        let mut c = Cur {
            b: &private,
            i: cs + 12,
        };
        c.int()?;
        for want in [&b"dict"[..], b"dup", b"begin"] {
            if c.token()? != want {
                return None;
            }
        }
        let mut charstrings = BTreeMap::new();
        let (mut nd, mut rd) = (Vec::new(), rd_s);
        loop {
            c.ws();
            if c.b.get(c.i) != Some(&b'/') {
                break;
            }
            let name = c.token()?[1..].to_vec();
            let (r, data) = c.binary()?;
            rd = r.to_vec();
            nd = c.closer()?;
            charstrings.insert(
                name,
                Entry {
                    stored: data.to_vec(),
                    plain: decrypt(data, CHARSTRING_KEY, len_iv),
                },
            );
        }
        c.ws();
        let tail = private[c.i..].to_vec();
        if charstrings.is_empty() || rd.is_empty() || nd.is_empty() {
            return None;
        }
        let np = if np.is_empty() { b"NP".to_vec() } else { np };

        // Built-in encoding entries in the cleartext part.
        let mut encoding = BTreeMap::new();
        let mut stripped = Vec::with_capacity(clear.len());
        let mut enc_at = None;
        let mut i = 0;
        while i < clear.len() {
            if clear[i..].starts_with(b"dup ") {
                let mut c = Cur { b: clear, i: i + 4 };
                if let (Some(code), Some(name), Some(b"put")) = (c.int(), c.token(), c.token())
                    && name.first() == Some(&b'/')
                {
                    encoding.insert(code as u32, name[1..].to_vec());
                    enc_at.get_or_insert(stripped.len());
                    i = c.i;
                    while i < clear.len()
                        && (clear[i] == b' ' || clear[i] == b'\r' || clear[i] == b'\n')
                    {
                        i += 1;
                    }
                    continue;
                }
            }
            stripped.push(clear[i]);
            i += 1;
        }

        let clear_key = strip_subset_prefix(&stripped);
        Some(Type1 {
            clear: stripped,
            clear_key,
            enc_at,
            encoding,
            head,
            middle,
            tail,
            subrs,
            n_subrs,
            len_iv,
            charstrings,
            rd,
            np,
            nd,
            trailer,
        })
    }

    fn consistent_with(&self, o: &Type1) -> bool {
        self.clear_key == o.clear_key
            && self.len_iv == o.len_iv
            && self.head == o.head
            && self.middle == o.middle
            && self.tail == o.tail
            && self.rd == o.rd
            && self.nd == o.nd
            && self
                .encoding
                .iter()
                .all(|(k, v)| o.encoding.get(k).is_none_or(|w| w == v))
            && self
                .charstrings
                .iter()
                .all(|(k, v)| o.charstrings.get(k).is_none_or(|w| w.plain == v.plain))
            && self
                .subrs
                .iter()
                .all(|(k, v)| o.subrs.get(k).is_none_or(|w| w.plain == v.plain))
    }
}

/// Returns the merged program and its Length1/2/3.
fn build_merged(fonts: &[&Type1]) -> Option<(Vec<u8>, usize, usize, usize)> {
    let base = fonts[0];
    let mut encoding: BTreeMap<u32, &[u8]> = BTreeMap::new();
    let mut charstrings: BTreeMap<&[u8], &[u8]> = BTreeMap::new();
    let mut subrs: BTreeMap<usize, &[u8]> = BTreeMap::new();
    let mut n_subrs = 0;
    for f in fonts {
        for (k, v) in &f.encoding {
            encoding.entry(*k).or_insert(v);
        }
        for (k, v) in &f.charstrings {
            charstrings.entry(k).or_insert(&v.stored);
        }
        for (k, v) in &f.subrs {
            subrs.entry(*k).or_insert(&v.stored);
        }
        n_subrs = n_subrs.max(f.n_subrs);
    }
    // Indices no subset really defines get a `return` stub again.
    let stub = encrypt(
        &[vec![0u8; base.len_iv], vec![RETURN]].concat(),
        CHARSTRING_KEY,
    );
    for i in 0..n_subrs {
        subrs.entry(i).or_insert(&stub);
    }

    let mut clear = base.clear.clone();
    if let Some(at) = base.enc_at {
        let entries: Vec<u8> = encoding
            .iter()
            .flat_map(|(code, name)| {
                [
                    format!("dup {code} /").into_bytes(),
                    name.to_vec(),
                    b" put\n".to_vec(),
                ]
                .concat()
            })
            .collect();
        clear.splice(at..at, entries);
    } else if !encoding.is_empty() {
        return None;
    }

    let mut private = Vec::new();
    // 4 leading bytes, required and ignored by readers.
    private.extend_from_slice(b"\0\0\0\0");
    private.extend_from_slice(&base.head);
    if base.n_subrs > 0 {
        // Newline *before* each entry: `middle` already holds whatever
        // followed the last one.
        private.extend_from_slice(format!("/Subrs {n_subrs} array").as_bytes());
        for (i, data) in &subrs {
            private.extend_from_slice(format!("\ndup {i} {} ", data.len()).as_bytes());
            private.extend_from_slice(&base.rd);
            private.push(b' ');
            private.extend_from_slice(data);
            private.push(b' ');
            private.extend_from_slice(&base.np);
        }
        private.extend_from_slice(&base.middle);
    }
    private.extend_from_slice(
        format!("/CharStrings {} dict dup begin\n", charstrings.len()).as_bytes(),
    );
    for (name, data) in &charstrings {
        private.push(b'/');
        private.extend_from_slice(name);
        private.extend_from_slice(format!(" {} ", data.len()).as_bytes());
        private.extend_from_slice(&base.rd);
        private.push(b' ');
        private.extend_from_slice(data);
        private.push(b' ');
        private.extend_from_slice(&base.nd);
        private.push(b'\n');
    }
    private.extend_from_slice(&base.tail);

    // Readers tell binary from hex eexec by the first 4 ciphertext bytes: make
    // sure they aren't all hex digits.
    let mut enc = encrypt(&private, EEXEC_KEY);
    let mut seed = 0u8;
    while enc[..4].iter().all(u8::is_ascii_hexdigit) {
        seed = seed.wrapping_add(1);
        private[0] = seed;
        enc = encrypt(&private, EEXEC_KEY);
    }

    let (l1, l2, l3) = (clear.len(), enc.len(), base.trailer.len());
    let mut out = clear;
    out.extend_from_slice(&enc);
    out.extend_from_slice(&base.trailer);
    Some((out, l1, l2, l3))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn eexec_round_trips() {
        let plain = b"\x01\x02\x03\x04/Private 8 dict dup begin";
        let enc = encrypt(plain, EEXEC_KEY);
        assert_eq!(decrypt(&enc, EEXEC_KEY, 4), &plain[4..]);
    }

    /// A toy Type 1 program with the given glyphs, `Subrs` count and
    /// built-in encoding entries.
    pub(crate) fn toy(glyphs: &[(&str, &[u8])], n_subrs: usize, enc: &[(u32, &str)]) -> Stream {
        let mut clear = b"%!PS-AdobeFont-1.0: Toy 001\n/FontName /Toy def\n/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\n".to_vec();
        for (c, n) in enc {
            clear.extend_from_slice(format!("dup {c} /{n} put\n").as_bytes());
        }
        clear.extend_from_slice(b"readonly def\ncurrentfile eexec\n");
        let cs = |p: &[u8]| encrypt(&[b"\0\0\0\0".as_slice(), p].concat(), CHARSTRING_KEY);
        let mut private = b"XXXXdup /Private 8 dict dup begin /RD{string currentfile exch readstring pop}executeonly def\n".to_vec();
        private.extend_from_slice(format!("/Subrs {n_subrs} array\n").as_bytes());
        for i in 0..n_subrs {
            let d = cs(&[i as u8, 11]);
            private.extend_from_slice(format!("dup {i} {} RD ", d.len()).as_bytes());
            private.extend_from_slice(&d);
            private.extend_from_slice(b" NP\n");
        }
        private.extend_from_slice(b"ND\n");
        private.extend_from_slice(
            format!("/CharStrings {} dict dup begin\n", glyphs.len()).as_bytes(),
        );
        for (n, p) in glyphs {
            let d = cs(p);
            private.extend_from_slice(format!("/{n} {} RD ", d.len()).as_bytes());
            private.extend_from_slice(&d);
            private.extend_from_slice(b" ND\n");
        }
        private.extend_from_slice(b"end\nend\nreadonly put\nmark currentfile closefile\n");
        let enc = encrypt(&private, EEXEC_KEY);
        let trailer = b"0000000000\ncleartomark\n".to_vec();
        let mut dict = Dictionary::new();
        dict.set("Length1", clear.len() as i64);
        dict.set("Length2", enc.len() as i64);
        dict.set("Length3", trailer.len() as i64);
        Stream::new(dict, [clear, enc, trailer].concat())
    }

    #[test]
    fn subsets_merge_into_their_union() {
        let a =
            Type1::from_stream(&toy(&[(".notdef", b"n"), ("A", b"aa")], 2, &[(65, "A")])).unwrap();
        let b =
            Type1::from_stream(&toy(&[(".notdef", b"n"), ("B", b"bb")], 4, &[(66, "B")])).unwrap();
        assert!(a.consistent_with(&b));
        let (program, l1, l2, l3) = build_merged(&[&a, &b]).unwrap();
        let mut dict = Dictionary::new();
        dict.set("Length1", l1 as i64);
        dict.set("Length2", l2 as i64);
        dict.set("Length3", l3 as i64);
        let m = Type1::from_stream(&Stream::new(dict, program)).unwrap();
        assert_eq!(
            m.charstrings.keys().cloned().collect::<Vec<_>>(),
            vec![b".notdef".to_vec(), b"A".to_vec(), b"B".to_vec()]
        );
        assert_eq!(m.charstrings[&b"B".to_vec()].plain, b"bb");
        assert_eq!(m.n_subrs, 4);
        assert_eq!(m.encoding.len(), 2);
        assert!(m.consistent_with(&a) && m.consistent_with(&b));
    }

    #[test]
    fn conflicting_glyphs_are_not_merged() {
        let a = Type1::from_stream(&toy(&[("A", b"aa")], 2, &[])).unwrap();
        let b = Type1::from_stream(&toy(&[("A", b"zz")], 2, &[])).unwrap();
        assert!(!a.consistent_with(&b));
    }

    #[test]
    fn conflicting_encodings_are_not_merged() {
        let a = Type1::from_stream(&toy(&[("A", b"aa"), ("B", b"bb")], 2, &[(65, "A")])).unwrap();
        let b = Type1::from_stream(&toy(&[("A", b"aa"), ("B", b"bb")], 2, &[(65, "B")])).unwrap();
        assert!(!a.consistent_with(&b));
    }
}
