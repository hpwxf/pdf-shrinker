//! Re-deflates Flate streams with Zopfli (experimental levels only): the same
//! `FlateDecode` format every reader understands, typically 5–8 % smaller than
//! zlib's best level, at a much higher CPU cost — hence parallel, and fewer
//! iterations for big streams.

use std::num::NonZeroU64;

use lopdf::{Document, Object, ObjectId};
use rayon::prelude::*;

/// Streams bigger than this (decoded) are left as they are: Zopfli's time
/// grows faster than its gains on them.
const MAX_DECODED: usize = 4 << 20;

/// Returns bytes saved.
pub fn rezopfli_streams(doc: &mut Document) -> u64 {
    let jobs: Vec<(ObjectId, Vec<u8>, usize)> = doc
        .objects
        .iter()
        .filter_map(|(id, obj)| {
            let Object::Stream(s) = obj else { return None };
            let only_flate = matches!(s.filters(), Ok(f) if f.len() == 1 && f[0] == b"FlateDecode");
            if !only_flate {
                return None;
            }
            // Predictor-encoded streams would need the predictor re-applied;
            // decompressed_content() hands back the un-predicted samples.
            if s.dict.has(b"DecodeParms") {
                return None;
            }
            let decoded = s.decompressed_content().ok()?;
            (decoded.len() <= MAX_DECODED).then_some((*id, decoded, s.content.len()))
        })
        .collect();

    let results: Vec<(ObjectId, Vec<u8>, usize)> = jobs
        .into_par_iter()
        .filter_map(|(id, decoded, old_len)| {
            let out = zopfli_zlib(&decoded)?;
            (out.len() < old_len).then_some((id, out, old_len))
        })
        .collect();

    let mut saved = 0u64;
    for (id, bytes, old_len) in results {
        if let Some(Object::Stream(s)) = doc.objects.get_mut(&id) {
            saved += (old_len - bytes.len()) as u64;
            s.set_content(bytes);
        }
    }
    saved
}

pub fn zopfli_zlib(data: &[u8]) -> Option<Vec<u8>> {
    let iterations = match data.len() {
        0..=65_536 => 15,
        65_537..=262_144 => 5,
        _ => 2,
    };
    let options = zopfli::Options {
        iteration_count: NonZeroU64::new(iterations)?,
        ..Default::default()
    };
    let mut out = Vec::new();
    zopfli::compress(options, zopfli::Format::Zlib, data, &mut out).ok()?;
    Some(out)
}
