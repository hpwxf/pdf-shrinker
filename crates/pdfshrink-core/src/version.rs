//! Build provenance — the crate version plus the exact git commit (and
//! whether the tree was dirty) it was built from, so "which build is this?"
//! always has an answer. Baked in at compile time by `build.rs`, but only
//! `env!()`'d here in `pdfshrink-core`: the CLI and the app front ends get it
//! through the plain function call below rather than each needing their own
//! `build.rs`, since `env!()` only resolves within the crate that writes it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: &'static str,
    pub commit: &'static str,
    pub dirty: bool,
}

impl BuildInfo {
    /// `"0.1.0 (210bb07)"`, or `"0.1.0 (210bb07-dirty)"` when built from an
    /// uncommitted working tree.
    pub fn short(&self) -> String {
        if self.dirty {
            format!("{} ({}-dirty)", self.version, self.commit)
        } else {
            format!("{} ({})", self.version, self.commit)
        }
    }
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION"),
        commit: env!("PDFSHRINK_GIT_COMMIT"),
        dirty: env!("PDFSHRINK_GIT_DIRTY") == "true",
    }
}
