use std::path::PathBuf;

use serde::Serialize;
use tauri::{Emitter, Manager};

use pdfshrink_core::{compress_file, CompressOptions, Config, Engine, EngineChoice, GhostscriptEngine, Level, Outcome};

#[derive(Clone, Serialize)]
struct ConfigDto {
    level: String,
    engine: String,
    gs_available: bool,
}

fn config_dto() -> ConfigDto {
    let cfg = Config::load();
    ConfigDto {
        level: cfg.default_level.as_str().to_string(),
        engine: cfg.engine.as_str().to_string(),
        gs_available: GhostscriptEngine.is_available(),
    }
}

#[derive(Clone, Serialize)]
struct CompressResult {
    input: String,
    status: &'static str, // "compressed" | "not_smaller" | "error"
    output: Option<String>,
    input_size: Option<u64>,
    output_size: Option<u64>,
    message: Option<String>,
}

#[tauri::command]
fn get_config() -> ConfigDto {
    config_dto()
}

#[derive(Clone, Serialize)]
struct BuildInfoDto {
    version: String,
    commit: String,
    dirty: bool,
}

#[tauri::command]
fn get_build_info() -> BuildInfoDto {
    let b = pdfshrink_core::build_info();
    BuildInfoDto {
        version: b.version.to_string(),
        commit: b.commit.to_string(),
        dirty: b.dirty,
    }
}

#[tauri::command]
fn set_default_level(level: String) -> Result<ConfigDto, String> {
    let mut cfg = Config::load();
    cfg.default_level = Level::parse(&level).ok_or_else(|| format!("niveau inconnu : {level}"))?;
    cfg.save().map_err(|e| e.to_string())?;
    Ok(config_dto())
}

#[tauri::command]
fn set_default_engine(engine: String) -> Result<ConfigDto, String> {
    let mut cfg = Config::load();
    cfg.engine = EngineChoice::parse(&engine).ok_or_else(|| format!("moteur inconnu : {engine}"))?;
    cfg.save().map_err(|e| e.to_string())?;
    Ok(config_dto())
}

/// Compresses each file and emits a `compress-result` event as soon as its
/// result is known, so the UI can update incrementally instead of waiting for
/// the whole batch.
#[tauri::command]
fn compress_files(app: tauri::AppHandle, paths: Vec<String>, level: String, engine: String) -> Result<(), String> {
    let level = Level::parse(&level).ok_or_else(|| format!("niveau inconnu : {level}"))?;
    let engine = EngineChoice::parse(&engine).ok_or_else(|| format!("moteur inconnu : {engine}"))?;
    let opts = CompressOptions { level, engine };

    for path in paths {
        let input = PathBuf::from(&path);
        let result = match compress_file(&input, &opts) {
            Ok(Outcome::Compressed { output, report }) => CompressResult {
                input: path.clone(),
                status: "compressed",
                output: Some(output.display().to_string()),
                input_size: Some(report.input_size),
                output_size: Some(report.output_size),
                message: None,
            },
            Ok(Outcome::NotSmaller) => CompressResult {
                input: path.clone(),
                status: "not_smaller",
                output: None,
                input_size: None,
                output_size: None,
                message: None,
            },
            Err(e) => CompressResult {
                input: path.clone(),
                status: "error",
                output: None,
                input_size: None,
                output_size: None,
                message: Some(e.to_string()),
            },
        };
        let _ = app.emit("compress-result", &result);
    }

    Ok(())
}

#[tauri::command]
fn reveal_in_finder(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Clone, Serialize)]
struct InstallResult {
    quick_action: Option<Result<String, String>>,
    cli_link: Option<Result<String, String>>,
}

#[tauri::command]
fn install_integrations() -> InstallResult {
    let exec = match pdfshrink_core::integration::resolve_exec_path() {
        Ok(p) => p,
        Err(e) => {
            let msg = Err(e.to_string());
            return InstallResult {
                quick_action: Some(msg.clone()),
                cli_link: Some(msg),
            };
        }
    };

    let quick_action = pdfshrink_core::integration::install_quick_action(&exec)
        .map(|p| p.display().to_string())
        .map_err(|e| e.to_string());
    let cli_link = pdfshrink_core::integration::install_cli_symlink(&exec).map(|p| p.display().to_string());

    InstallResult {
        quick_action: Some(quick_action),
        cli_link: Some(cli_link),
    }
}

/// Files macOS hands us via "Open With…" / drag-onto-the-Dock-icon, both at
/// cold start and while the app is already running.
fn forward_opened_files(app: &tauri::AppHandle, urls: Vec<tauri::Url>) {
    let paths: Vec<String> = urls
        .into_iter()
        .filter_map(|url| url.to_file_path().ok())
        .map(|p| p.display().to_string())
        .collect();
    if paths.is_empty() {
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_focus();
    }
    let _ = app.emit("opened-files", &paths);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_build_info,
            set_default_level,
            set_default_engine,
            compress_files,
            reveal_in_finder,
            install_integrations,
        ])
        .setup(|app| {
            // Files passed on the initial launch (double-click on a PDF via
            // "Open With", or a cold Quick-Action-less launch with args).
            let opened: Vec<tauri::Url> = app
                .env()
                .args_os
                .iter()
                .skip(1)
                .filter_map(|a| a.to_str())
                .filter_map(|s| tauri::Url::from_file_path(s).ok())
                .collect();
            if !opened.is_empty() {
                forward_opened_files(&app.handle().clone(), opened);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Opened { urls } = event {
                forward_opened_files(app_handle, urls);
            }
        });
}
