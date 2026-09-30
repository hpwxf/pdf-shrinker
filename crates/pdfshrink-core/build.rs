//! Bakes the git commit (and whether the tree was dirty) this crate was built
//! from into `PDFSHRINK_GIT_COMMIT`/`PDFSHRINK_GIT_DIRTY` env vars, consumed by
//! `version.rs` via `env!()`. Both the CLI and the app read this back at
//! runtime through `pdfshrink_core::build_info()` — see that module for why
//! this lives here rather than in each front end.

use std::process::Command;

fn main() {
    let commit = run_git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = run_git(&["status", "--porcelain"]).map(|out| !out.is_empty()).unwrap_or(false);

    println!("cargo:rustc-env=PDFSHRINK_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=PDFSHRINK_GIT_DIRTY={dirty}");

    // Re-run when HEAD moves or the index changes (staging/unstaging affects
    // the dirty flag), and fall back to always re-running if we can't find
    // .git (e.g. building from a source tarball with no repo at all).
    match run_git(&["rev-parse", "--git-dir"]) {
        Some(git_dir) => {
            println!("cargo:rerun-if-changed={git_dir}/HEAD");
            println!("cargo:rerun-if-changed={git_dir}/index");
        }
        None => println!("cargo:rerun-if-changed=build.rs"),
    }
}

fn run_git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}
