use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PdfShrinkError {
    #[error("cannot read {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),

    #[error("{0} is not a valid PDF: {1}")]
    InvalidPdf(PathBuf, #[source] lopdf::Error),

    #[error("{0} is password-protected/encrypted; encrypted PDFs are not supported yet")]
    Encrypted(PathBuf),

    #[error("failed to write output {0}: {1}")]
    Write(PathBuf, #[source] std::io::Error),

    #[error("compressed output for {0} failed to reload as a valid PDF: {1}")]
    OutputVerification(PathBuf, #[source] lopdf::Error),

    #[error("page count changed while compressing {0} ({1} -> {2}); output was discarded")]
    PageCountMismatch(PathBuf, usize, usize),

    #[error("Ghostscript engine requested but `gs` was not found on this system")]
    GhostscriptNotFound,

    #[error("Ghostscript exited with an error while processing {0}: {1}")]
    GhostscriptFailed(PathBuf, String),

    #[error("config error: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, PdfShrinkError>;
