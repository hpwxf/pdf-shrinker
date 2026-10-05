mod install;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use pdfshrink_core::{Config, Level, Outcome, compress_file_with};

#[derive(Parser)]
#[command(
    name = "pdfshrink",
    version,
    about = "Smaller is better — and still beautiful."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// PDF files to compress.
    files: Vec<PathBuf>,

    /// Compression level (see "Levels" below). Defaults to the configured default.
    #[arg(short = 'l', long, value_name = "LEVEL")]
    level: Option<String>,

    /// Also show a macOS notification for each file (used by the Finder service).
    #[arg(long)]
    notify: bool,

    /// Override one profile parameter (repeatable), e.g. `--tune page_px=1800 --tune ssim=0.97`.
    /// For experimenting with variants; see `pdfshrink-core`'s `Profile::tune` for the keys.
    #[arg(long, value_name = "KEY=VALUE")]
    tune: Vec<String>,

    /// Output file suffix: writes `<name>-<SUFFIX>.pdf` (default: `compressed`).
    #[arg(long, value_name = "SUFFIX", default_value = "compressed")]
    suffix: String,

    /// Number of files to compress in parallel (defaults to the number of CPUs).
    #[arg(short = 'j', long, value_name = "N")]
    jobs: Option<usize>,
}

#[derive(Subcommand)]
enum Command {
    /// Read or change a persisted default (used by every front end: this CLI, the app, the Finder service).
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Install the Finder service and/or a `pdfshrink` symlink on the PATH.
    Install {
        /// Install the "PdfShrinker" service into Finder's right-click Services menu.
        #[arg(long, alias = "quick-action")]
        finder_service: bool,
        /// Symlink this binary as `pdfshrink` in /usr/local/bin (or ~/.local/bin).
        #[arg(long)]
        cli_link: bool,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print a config value (`level`).
    Get { key: String },
    /// Persist a config value (`level`).
    Set { key: String, value: String },
}

fn main() -> ExitCode {
    // `--version`/`-V` prints the git commit (and whether the build was made
    // from a dirty tree) alongside the crate version, not just the crate
    // version clap's derive `version` shorthand would print on its own.
    let version: &'static str = pdfshrink_core::build_info().short().leak();
    let matches = Cli::command()
        .version(version)
        .after_help(levels_help())
        .get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    match &cli.command {
        Some(Command::Config { action }) => run_config(action),
        Some(Command::Install {
            finder_service,
            cli_link,
        }) => install::run(*finder_service, *cli_link),
        None => run_compress(cli),
    }
}

fn level_names() -> String {
    Level::ALL.map(|l| l.as_str()).join(", ")
}

/// "Levels" section of `--help`: one line per level, each adding to the
/// previous one's settings.
fn levels_help() -> String {
    let mut help = String::from("Levels (each one adds to the previous):\n");
    for level in Level::ALL {
        help.push_str(&format!("  {:<12} {}\n", level.as_str(), level.summary()));
    }
    help
}

fn run_config(action: &ConfigAction) -> ExitCode {
    match action {
        ConfigAction::Get { key } => {
            let cfg = Config::load();
            match key.as_str() {
                "level" | "default_level" => println!("{}", cfg.default_level.as_str()),
                other => {
                    eprintln!("pdfshrink: unknown config key '{other}' (expected: level)");
                    return ExitCode::from(1);
                }
            }
            ExitCode::SUCCESS
        }
        ConfigAction::Set { key, value } => {
            let mut cfg = Config::load();
            match key.as_str() {
                "level" | "default_level" => match Level::parse(value) {
                    Some(l) => cfg.default_level = l,
                    None => {
                        eprintln!(
                            "pdfshrink: unknown level '{value}' (expected: {})",
                            level_names()
                        );
                        return ExitCode::from(1);
                    }
                },
                other => {
                    eprintln!("pdfshrink: unknown config key '{other}' (expected: level)");
                    return ExitCode::from(1);
                }
            }
            match cfg.save() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("pdfshrink: {e}");
                    ExitCode::from(1)
                }
            }
        }
    }
}

fn run_compress(cli: Cli) -> ExitCode {
    if cli.files.is_empty() {
        eprintln!("pdfshrink: no input files given (see --help)");
        return ExitCode::from(1);
    }

    let defaults = Config::load();

    let level = match cli.level.as_deref() {
        Some(s) => match Level::parse(s) {
            Some(l) => l,
            None => {
                eprintln!(
                    "pdfshrink: unknown level '{s}' (expected: {})",
                    level_names()
                );
                return ExitCode::from(1);
            }
        },
        None => defaults.default_level,
    };
    let mut profile = level.profile();
    for kv in &cli.tune {
        if let Err(e) = profile.tune(kv) {
            eprintln!("pdfshrink: --tune {e}");
            return ExitCode::from(1);
        }
    }

    if let Some(jobs) = cli.jobs {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build_global();
    }

    let results: Vec<(PathBuf, pdfshrink_core::Result<Outcome>)> = cli
        .files
        .par_iter()
        .map(|f| (f.clone(), compress_file_with(f, &profile, &cli.suffix)))
        .collect();

    let mut had_error = false;
    let mut had_not_smaller = false;

    for (file, result) in &results {
        match result {
            Ok(Outcome::Compressed { output, report }) => {
                // With `--tune fidelity=1`.
                let fidelity = report
                    .image_fidelity
                    .map(|f| {
                        format!(
                            "  image fidelity {:.3} (min {:.3}, {} images)",
                            f.mean, f.min, f.images
                        )
                    })
                    .unwrap_or_default();
                println!(
                    "{}  ->  {}   {} -> {}  (-{:.0}%){fidelity}",
                    file.display(),
                    output.display(),
                    human_size(report.input_size),
                    human_size(report.output_size),
                    report.ratio() * 100.0,
                );
                if cli.notify {
                    notify(&format!(
                        "{}: -{:.0} %",
                        file_name(output),
                        report.ratio() * 100.0
                    ));
                }
            }
            Ok(Outcome::NotSmaller) => {
                had_not_smaller = true;
                println!("{}: already optimal, nothing written", file.display());
                if cli.notify {
                    notify(&format!("{}: already optimal", file_name(file)));
                }
            }
            Err(e) => {
                had_error = true;
                eprintln!("{}: {e}", file.display());
                if cli.notify {
                    notify(&format!("{}: failed — {e}", file_name(file)));
                }
            }
        }
    }

    if had_error {
        ExitCode::from(1)
    } else if had_not_smaller {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string()
}

fn notify(message: &str) {
    let script = format!(
        "display notification {} with title \"PdfShrinker\"",
        osascript_quote(message)
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output();
}

fn osascript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
