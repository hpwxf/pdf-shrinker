//! WebAssembly bindings for `pdfshrink-core`, used by the web front end
//! (`web/`): the whole compression runs in the browser, the server only
//! serves static files.
//!
//! The core is built without its `native` feature: no threads (images are
//! processed one after another), no config file or macOS integration, and
//! JPEGs go through the pure-Rust encoder instead of mozjpeg — see
//! [`level_differs_from_desktop`].

use std::path::Path;

use pdfshrink_core::{ImageFidelity, Level};
use wasm_bindgen::prelude::*;

/// A compressed PDF and what was done to it.
#[wasm_bindgen]
pub struct Compressed {
    output: Vec<u8>,
    input_size: u64,
    images_resampled: usize,
    fidelity: Option<ImageFidelity>,
}

#[wasm_bindgen]
impl Compressed {
    /// The new file's bytes (copied out to a `Uint8Array`).
    #[wasm_bindgen(getter)]
    pub fn output(&self) -> Vec<u8> {
        self.output.clone()
    }

    #[wasm_bindgen(getter, js_name = inputSize)]
    pub fn input_size(&self) -> f64 {
        self.input_size as f64
    }

    #[wasm_bindgen(getter, js_name = outputSize)]
    pub fn output_size(&self) -> f64 {
        self.output.len() as f64
    }

    #[wasm_bindgen(getter, js_name = imagesResampled)]
    pub fn images_resampled(&self) -> usize {
        self.images_resampled
    }

    /// Mean image fidelity (luma SSIM), when images were re-encoded.
    #[wasm_bindgen(getter, js_name = fidelityMean)]
    pub fn fidelity_mean(&self) -> Option<f32> {
        self.fidelity.map(|f| f.mean)
    }

    #[wasm_bindgen(getter, js_name = fidelityMin)]
    pub fn fidelity_min(&self) -> Option<f32> {
        self.fidelity.map(|f| f.min)
    }

    #[wasm_bindgen(getter, js_name = fidelityImages)]
    pub fn fidelity_images(&self) -> Option<usize> {
        self.fidelity.map(|f| f.images)
    }
}

fn parse_level(level: &str) -> Result<Level, JsError> {
    Level::parse(level).ok_or_else(|| JsError::new(&format!("unknown level '{level}'")))
}

/// Compresses `input` (a whole PDF) at `level` (`lossless` … `extreme-max`).
/// `name` only labels error messages. The result may be no smaller than the
/// input: the caller checks `outputSize`.
#[wasm_bindgen]
pub fn compress(input: &[u8], level: &str, name: &str) -> Result<Compressed, JsError> {
    let mut profile = parse_level(level)?.profile();
    profile.measure_fidelity = true;
    let (output, report) = pdfshrink_core::compress_bytes(input, Path::new(name), &profile)
        .map_err(|e| JsError::new(&e.to_string()))?;
    Ok(Compressed {
        output,
        input_size: report.input_size,
        images_resampled: report.images_resampled,
        fidelity: report.image_fidelity,
    })
}

/// Whether `level` may give a different (usually slightly bigger) file here
/// than in the desktop app and CLI: it re-encodes JPEGs, and this build uses
/// the pure-Rust encoder instead of mozjpeg.
#[wasm_bindgen(js_name = levelDiffersFromDesktop)]
pub fn level_differs_from_desktop(level: &str) -> Result<bool, JsError> {
    Ok(parse_level(level)?.differs_from_desktop())
}

/// `"0.2.0 (210bb07)"`.
#[wasm_bindgen(js_name = buildInfo)]
pub fn build_info() -> String {
    pdfshrink_core::build_info().short()
}
