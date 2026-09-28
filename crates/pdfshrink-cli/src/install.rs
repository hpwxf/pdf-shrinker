//! Thin CLI wrapper around `pdfshrink_core::integration` (shared with the app).

use std::process::ExitCode;

pub fn run(quick_action: bool, cli_link: bool) -> ExitCode {
    // With neither flag, do both — that's what a first run from a freshly
    // installed app should do.
    let (quick_action, cli_link) = if !quick_action && !cli_link { (true, true) } else { (quick_action, cli_link) };

    let exec = match pdfshrink_core::integration::resolve_exec_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("pdfshrink: could not resolve the executable to install: {e}");
            return ExitCode::from(1);
        }
    };

    let mut ok = true;

    if quick_action {
        match pdfshrink_core::integration::install_quick_action(&exec) {
            Ok(path) => println!("Quick Action installed: {}", path.display()),
            Err(e) => {
                eprintln!("pdfshrink: could not install the Quick Action: {e}");
                ok = false;
            }
        }
    }

    if cli_link {
        match pdfshrink_core::integration::install_cli_symlink(&exec) {
            Ok(path) => println!("Command-line symlink installed: {}", path.display()),
            Err(e) => {
                eprintln!("pdfshrink: could not install the command-line symlink: {e}");
                ok = false;
            }
        }
    }

    if ok { ExitCode::SUCCESS } else { ExitCode::from(1) }
}
