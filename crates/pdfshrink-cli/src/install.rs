//! Thin CLI wrapper around `pdfshrink_core::integration` (shared with the app).

use std::process::ExitCode;

pub fn run(finder_service: bool, cli_link: bool) -> ExitCode {
    // With neither flag, do both — that's what a first run from a freshly
    // installed app should do.
    let (finder_service, cli_link) = if !finder_service && !cli_link {
        (true, true)
    } else {
        (finder_service, cli_link)
    };

    let exec = match pdfshrink_core::integration::resolve_exec_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("pdfshrink: could not resolve the executable to install: {e}");
            return ExitCode::from(1);
        }
    };

    let mut ok = true;

    if finder_service {
        match pdfshrink_core::integration::install_finder_service(&exec) {
            Ok(path) => println!(
                "Finder service installed: {} (right-click a PDF in Finder \u{203a} Services \u{203a} PdfShrinker)",
                path.display()
            ),
            Err(e) => {
                eprintln!("pdfshrink: could not install the Finder service: {e}");
                ok = false;
            }
        }
    }

    if cli_link {
        match pdfshrink_core::integration::install_cli_symlink(&exec) {
            Ok(path) => {
                println!("Command-line symlink installed: {}", path.display());
                let dir = path.parent().unwrap_or(&path);
                if !pdfshrink_core::integration::is_on_path(dir) {
                    println!(
                        "note: {} is not on your PATH — add it to use `pdfshrink` directly.",
                        dir.display()
                    );
                }
            }
            Err(e) => {
                eprintln!("pdfshrink: could not install the command-line symlink: {e}");
                ok = false;
            }
        }
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
