use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{Emitter, Manager};

use pdfshrink_core::{compress_file, CompressOptions, Config, Level, Outcome};

#[derive(Clone, Serialize)]
struct ConfigDto {
    level: String,
}

fn config_dto() -> ConfigDto {
    let cfg = Config::load();
    ConfigDto {
        level: cfg.default_level.as_str().to_string(),
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
    cfg.default_level = Level::parse(&level).ok_or_else(|| format!("unknown level: {level}"))?;
    cfg.save().map_err(|e| e.to_string())?;
    Ok(config_dto())
}

/// Compresses each file in turn, emitting `compress-started` when one begins
/// and `compress-result` as soon as its result is known, so the UI can update
/// incrementally instead of waiting for the whole batch.
///
/// `async` + `spawn_blocking`: a plain (sync) command runs on the main thread,
/// which would freeze the webview (no repaint, no scrolling) for the whole
/// batch.
#[tauri::command]
async fn compress_files(
    app: tauri::AppHandle,
    paths: Vec<String>,
    level: String,
) -> Result<(), String> {
    let level = Level::parse(&level).ok_or_else(|| format!("unknown level: {level}"))?;
    let opts = CompressOptions { level };

    tauri::async_runtime::spawn_blocking(move || {
        for path in paths {
            let _ = app.emit("compress-started", &path);
            let result = compress_one(path, &opts);
            let _ = app.emit("compress-result", &result);
        }
    })
    .await
    .map_err(|e| e.to_string())
}

fn compress_one(path: String, opts: &CompressOptions) -> CompressResult {
    let empty = |status, message| CompressResult {
        input: path.clone(),
        status,
        output: None,
        input_size: None,
        output_size: None,
        message,
    };
    match compress_file(&PathBuf::from(&path), opts) {
        Ok(Outcome::Compressed { output, report }) => CompressResult {
            output: Some(output.display().to_string()),
            input_size: Some(report.input_size),
            output_size: Some(report.output_size),
            ..empty("compressed", None)
        },
        Ok(Outcome::NotSmaller) => empty("not_smaller", None),
        Err(e) => empty("error", Some(e.to_string())),
    }
}

/// Shows `path` selected in Finder (macOS) or Explorer (Windows).
#[tauri::command]
fn reveal_in_finder(path: String) -> Result<(), String> {
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        // Explorer parses its own command line: `/select,"C:\a b.pdf"` must
        // reach it verbatim, which std's per-argument quoting would break.
        let mut cmd = std::process::Command::new("explorer");
        cmd.raw_arg(format!("/select,\"{path}\""));
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = std::process::Command::new("open");
        cmd.arg("-R").arg(&path);
        cmd
    };
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Clone, Serialize)]
struct InstallResult {
    finder_service: Option<Result<String, String>>,
    cli_link: Option<Result<String, String>>,
}

#[tauri::command]
fn install_integrations() -> InstallResult {
    let exec = match pdfshrink_core::integration::resolve_exec_path() {
        Ok(p) => p,
        Err(e) => {
            let msg = Err(e.to_string());
            return InstallResult {
                finder_service: Some(msg.clone()),
                cli_link: Some(msg),
            };
        }
    };

    let finder_service = pdfshrink_core::integration::install_finder_service(&exec)
        .map(|p| p.display().to_string())
        .map_err(|e| e.to_string());
    let cli_link =
        pdfshrink_core::integration::install_cli_symlink(&exec).map(|p| p.display().to_string());

    InstallResult {
        finder_service: Some(finder_service),
        cli_link: Some(cli_link),
    }
}

/// Files macOS hands us via "Open With…" / drag-onto-the-Dock-icon, both at
/// cold start and while the app is already running (on Windows: the argv of
/// the first instance, or of a later one forwarded by the single-instance
/// plugin).
/// Files received before the webview registered its `opened-files` listener
/// (cold start: the `odoc` event beats the page's JS). Drained by `frontend_ready`.
#[derive(Default)]
struct OpenedFiles {
    ready: bool,
    pending: Vec<String>,
}

/// Called by the UI once its `opened-files` listener is registered: returns
/// what arrived earlier and switches to live emission.
#[tauri::command]
fn frontend_ready(state: tauri::State<'_, Mutex<OpenedFiles>>) -> Vec<String> {
    let mut s = state.lock().unwrap();
    s.ready = true;
    std::mem::take(&mut s.pending)
}

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
    let state = app.state::<Mutex<OpenedFiles>>();
    let mut s = state.lock().unwrap();
    if s.ready {
        drop(s);
        let _ = app.emit("opened-files", &paths);
    } else {
        s.pending.extend(paths);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    // macOS keeps one instance per app and sends it an `odoc` event instead;
    // Windows starts a new process per "Open with", so hand its files over to
    // the running one and let it exit.
    #[cfg(windows)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
        let cwd = PathBuf::from(cwd);
        let urls = argv
            .iter()
            .skip(1)
            .filter_map(|a| tauri::Url::from_file_path(cwd.join(a)).ok())
            .collect();
        forward_opened_files(app, urls);
    }));

    builder
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(OpenedFiles::default()))
        .invoke_handler(tauri::generate_handler![
            frontend_ready,
            get_config,
            get_build_info,
            set_default_level,
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
            // Only macOS has the `odoc` event; Windows goes through argv and
            // the single-instance plugin above.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = event {
                forward_opened_files(app_handle, urls);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app_handle, event);
        });
}
