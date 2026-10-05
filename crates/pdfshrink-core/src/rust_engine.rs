use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use lopdf::{Document, Object, ObjectId};

use crate::engine::{Engine, Report};
use crate::image_ops;
use crate::level::Profile;
use crate::{PdfShrinkError, Result};
use crate::{cff_subset, deep_dedup, font_merge, type1_cff, type1_merge, zopfli_pass};

/// Pure-Rust compression engine: structural cleanup (dead-object pruning, stream
/// dedup, Flate recompression) plus, when the profile asks for it, image
/// downsampling/JPEG re-encoding. Always available — no external process.
pub struct RustEngine;

impl Engine for RustEngine {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn compress(&self, input: &Path, output: &Path, profile: &Profile) -> Result<Report> {
        let bytes = std::fs::read(input).map_err(|e| PdfShrinkError::Io(input.to_path_buf(), e))?;
        let (compressed, report) = compress_bytes(&bytes, input, profile)?;
        std::fs::write(output, compressed)
            .map_err(|e| PdfShrinkError::Write(output.to_path_buf(), e))?;
        Ok(report)
    }
}

/// Compresses a whole PDF held in memory, returning the new file's bytes —
/// what [`RustEngine`] runs between reading and writing files, and the entry
/// point of the WebAssembly build. `name` only labels errors. The result is
/// not checked against the input size: callers decide what "not smaller"
/// means for them.
pub fn compress_bytes(input: &[u8], name: &Path, profile: &Profile) -> Result<(Vec<u8>, Report)> {
    let mut doc =
        Document::load_mem(input).map_err(|e| PdfShrinkError::InvalidPdf(name.to_path_buf(), e))?;

    if doc.is_encrypted() {
        return Err(PdfShrinkError::Encrypted(name.to_path_buf()));
    }

    let original_pages = doc.get_pages().len();
    let mut phase = PhaseLog::new();

    doc.prune_objects();
    dedup_streams(&mut doc);
    phase.done("prune+dedup");
    if profile.deep_dedup {
        let n = deep_dedup::deep_dedup(&mut doc);
        phase.done(&format!("deep dedup ({n} merged)"));
    }
    if profile.merge_fonts {
        let n = font_merge::merge_truetype_subsets(&mut doc)
            + font_merge::merge_simple_truetype(&mut doc)
            + type1_merge::merge_type1_subsets(&mut doc);
        phase.done(&format!("font merge ({n} programs merged)"));
    }
    if profile.cff {
        let n = type1_cff::convert_type1_to_cff(&mut doc);
        phase.done(&format!("Type 1 → CFF ({n} programs converted)"));
    }
    if profile.cff_subset {
        let n = cff_subset::subset_cff_programs(&mut doc);
        phase.done(&format!("CFF subset ({n} programs rewritten)"));
    }

    let (images_resampled, images_skipped, image_fidelity) = if profile.resample_images {
        image_ops::resample_images(&mut doc, profile)
    } else {
        (0, 0, None)
    };
    phase.done(&format!(
        "images ({images_resampled} re-encoded, {images_skipped} skipped)"
    ));
    if profile.deep_dedup {
        // Identical inputs re-encode to identical outputs: catch pairs that
        // differed only in encoding before resampling.
        let n = deep_dedup::deep_dedup(&mut doc);
        phase.done(&format!("deep dedup ({n} merged)"));
    }

    // Flate-compress any stream that isn't compressed yet (fonts, content
    // streams, …). A no-op for streams that already have a /Filter.
    doc.compress();
    if profile.zopfli {
        let saved = zopfli_pass::rezopfli_streams(&mut doc);
        phase.done(&format!("zopfli ({saved} bytes saved)"));
    }

    let mut output = Vec::new();
    doc.save_modern(&mut output)
        .map_err(|e| PdfShrinkError::Write(name.to_path_buf(), e))?;

    let reloaded = Document::load_mem(&output)
        .map_err(|e| PdfShrinkError::OutputVerification(name.to_path_buf(), e))?;
    let new_pages = reloaded.get_pages().len();
    if new_pages != original_pages {
        return Err(PdfShrinkError::PageCountMismatch(
            name.to_path_buf(),
            original_pages,
            new_pages,
        ));
    }

    let report = Report {
        input_size: input.len() as u64,
        output_size: output.len() as u64,
        images_resampled,
        images_skipped,
        image_fidelity,
    };
    Ok((output, report))
}

/// Per-phase timings on stderr when `PDFSHRINK_DEBUG` is set. The clock is
/// only read then: `Instant::now()` panics on wasm32-unknown-unknown.
struct PhaseLog {
    last: Option<std::time::Instant>,
}

impl PhaseLog {
    fn new() -> Self {
        PhaseLog {
            last: std::env::var_os("PDFSHRINK_DEBUG").map(|_| std::time::Instant::now()),
        }
    }

    fn done(&mut self, what: &str) {
        if let Some(last) = &mut self.last {
            eprintln!("  [{:>6.1}s] {what}", last.elapsed().as_secs_f32());
            *last = std::time::Instant::now();
        }
    }
}

/// Merge stream objects that are byte-for-byte identical (same dictionary, same
/// content) into a single object, rewriting every reference throughout the
/// document, then dropping the now-unreferenced duplicates.
fn dedup_streams(doc: &mut Document) {
    let mut buckets: HashMap<(u64, usize), Vec<ObjectId>> = HashMap::new();
    for (id, obj) in doc.objects.iter() {
        if let Object::Stream(s) = obj {
            let mut hasher = DefaultHasher::new();
            s.content.hash(&mut hasher);
            buckets
                .entry((hasher.finish(), s.content.len()))
                .or_default()
                .push(*id);
        }
    }

    let mut remap: HashMap<ObjectId, ObjectId> = HashMap::new();
    for ids in buckets.into_values() {
        if ids.len() < 2 {
            continue;
        }
        let mut ids = ids;
        ids.sort();
        let canonical = ids[0];
        let Some(Object::Stream(canon)) = doc.objects.get(&canonical) else {
            continue;
        };
        let canon_dict = canon.dict.clone();
        let canon_content = canon.content.clone();
        for &dup in &ids[1..] {
            if let Some(Object::Stream(s)) = doc.objects.get(&dup)
                && s.dict == canon_dict
                && s.content == canon_content
            {
                remap.insert(dup, canonical);
            }
        }
    }

    if remap.is_empty() {
        return;
    }

    doc.traverse_objects(|obj| {
        if let Object::Reference(rid) = obj
            && let Some(&canon) = remap.get(rid)
        {
            *obj = Object::Reference(canon);
        }
    });

    doc.prune_objects();
}
