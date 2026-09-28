mod install;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use pdfshrink_core::{compress_file, CompressOptions, Config, EngineChoice, Level, Outcome};

#[derive(Parser)]
#[command(name = "pdfshrink", version, about = "Compress PDFs while keeping them PDFs.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// PDF files to compress.
    files: Vec<PathBuf>,

    /// Compression level: lossless, low, medium or high. Defaults to the configured default.
    #[arg(short = 'l', long, value_name = "LEVEL")]
    level: Option<String>,

    /// Compression engine: rust, gs or best. Defaults to the configured default.
    #[arg(short = 'e', long, value_name = "ENGINE")]
    engine: Option<String>,

    /// Also show a macOS notification for each file (used by the Quick Action).
    #[arg(long)]
    notify: bool,

    /// Number of files to compress in parallel (defaults to the number of CPUs).
    #[arg(short = 'j', long, value_name = "N")]
    jobs: Option<usize>,
}

#[derive(Subcommand)]
enum Command {
    /// Read or change a persisted default (used by every front end: this CLI, the app, the Quick Action).
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Install the Finder Quick Action and/or a `pdfshrink` symlink on the PATH.
    Install {
        /// Install the "PdfShrinker" Quick Action into Finder's right-click menu.
        #[arg(long)]
        quick_action: bool,
        /// Symlink this binary as /usr/local/bin/pdfshrink.
        #[arg(long)]
        cli_link: bool,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print a config value (`level` or `engine`).
    Get { key: String },
    /// Persist a config value (`level` or `engine`).
    Set { key: String, value: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match &cli.command {
        Some(Command::Config { action }) => run_config(action),
        Some(Command::Install { quick_action, cli_link }) => install::run(*quick_action, *cli_link),
        None => run_compress(cli),
    }
}

fn run_config(action: &ConfigAction) -> ExitCode {
    match action {
        ConfigAction::Get { key } => {
            let cfg = Config::load();
            match key.as_str() {
                "level" | "default_level" => println!("{}", cfg.default_level.as_str()),
                "engine" => println!("{}", cfg.engine.as_str()),
                other => {
                    eprintln!("pdfshrink: unknown config key '{other}' (expected: level, engine)");
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
                        eprintln!("pdfshrink: unknown level '{value}' (expected: lossless, low, medium, high)");
                        return ExitCode::from(1);
                    }
                },
                "engine" => match EngineChoice::parse(value) {
                    Some(e) => cfg.engine = e,
                    None => {
                        eprintln!("pdfshrink: unknown engine '{value}' (expected: rust, gs, best)");
                        return ExitCode::from(1);
                    }
                },
                other => {
                    eprintln!("pdfshrink: unknown config key '{other}' (expected: level, engine)");
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
                eprintln!("pdfshrink: unknown level '{s}' (expected: lossless, low, medium, high)");
                return ExitCode::from(1);
            }
        },
        None => defaults.default_level,
    };
    let engine = match cli.engine.as_deref() {
        Some(s) => match EngineChoice::parse(s) {
            Some(e) => e,
            None => {
                eprintln!("pdfshrink: unknown engine '{s}' (expected: rust, gs, best)");
                return ExitCode::from(1);
            }
        },
        None => defaults.engine,
    };
    let opts = CompressOptions { level, engine };

    if let Some(jobs) = cli.jobs {
        let _ = rayon::ThreadPoolBuilder::new().num_threads(jobs).build_global();
    }

    let results: Vec<(PathBuf, pdfshrink_core::Result<Outcome>)> =
        cli.files.par_iter().map(|f| (f.clone(), compress_file(f, &opts))).collect();

    let mut had_error = false;
    let mut had_not_smaller = false;

    for (file, result) in &results {
        match result {
            Ok(Outcome::Compressed { output, report }) => {
                println!(
                    "{}  ->  {}   {} -> {}  (-{:.0}%)",
                    file.display(),
                    output.display(),
                    human_size(report.input_size),
                    human_size(report.output_size),
                    report.ratio() * 100.0,
                );
                if cli.notify {
                    notify(&format!("{}: -{:.0} %", file_name(output), report.ratio() * 100.0));
                }
            }
            Ok(Outcome::NotSmaller) => {
                had_not_smaller = true;
                println!("{}: already optimal, nothing written", file.display());
                if cli.notify {
                    notify(&format!("{}: déjà optimal", file_name(file)));
                }
            }
            Err(e) => {
                had_error = true;
                eprintln!("{}: {e}", file.display());
                if cli.notify {
                    notify(&format!("{}: échec — {e}", file_name(file)));
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
    const UNITS: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
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
    p.file_name().and_then(|s| s.to_str()).unwrap_or("fichier").to_string()
}

fn notify(message: &str) {
    let script = format!(
        "display notification {} with title \"PdfShrinker\"",
        osascript_quote(message)
    );
    let _ = std::process::Command::new("osascript").arg("-e").arg(script).output();
}

fn osascript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
