const { core, event, dialog } = window.__TAURI__;
const { t } = window.I18N;

const levelSelect = document.getElementById("level-select");
const levelHint = document.getElementById("level-hint");
const setDefaultCheckbox = document.getElementById("set-default");
const compressBtn = document.getElementById("compress-btn");
const clearBtn = document.getElementById("clear-btn");
const pickFilesBtn = document.getElementById("pick-files");
const dropZone = document.getElementById("drop-zone");
const fileList = document.getElementById("file-list");
const installBtn = document.getElementById("install-btn");
const installStatus = document.getElementById("install-status");
const buildInfoEl = document.getElementById("build-info");
const buildInfoTooltip = document.getElementById("build-info-tooltip");

// The Finder service and the PATH symlink are macOS-only integrations.
const IS_WINDOWS = navigator.userAgent.includes("Windows");
installBtn.hidden = IS_WINDOWS;
installStatus.hidden = IS_WINDOWS;

/** path -> { li, detail, render() } */
const files = new Map();
let buildInfo = null;

function buildInfoText() {
  if (!buildInfo) return "";
  const dirty = buildInfo.dirty ? t("info.dirtySuffix") : "";
  return t("info.build", { version: buildInfo.version, commit: buildInfo.commit, dirty });
}

function renderBuildInfo() {
  if (!buildInfo) return;
  buildInfoEl.textContent = "ⓘ";
  const text = buildInfoText();
  // `title` is a harmless fallback (hover, after a delay); the tooltip below,
  // toggled by click, is the primary way this is actually shown.
  buildInfoEl.title = text;
  if (!buildInfoTooltip.hidden) buildInfoTooltip.textContent = text;
}

function hideBuildInfoTooltip() {
  buildInfoTooltip.hidden = true;
  buildInfoEl.classList.remove("active");
}

buildInfoEl.addEventListener("click", (e) => {
  e.stopPropagation();
  if (buildInfoTooltip.hidden) {
    buildInfoTooltip.textContent = buildInfoText();
    buildInfoTooltip.hidden = false;
    buildInfoEl.classList.add("active");
  } else {
    hideBuildInfoTooltip();
  }
});
document.addEventListener("click", (e) => {
  if (!buildInfoTooltip.hidden && !buildInfoTooltip.contains(e.target)) hideBuildInfoTooltip();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") hideBuildInfoTooltip();
});

async function loadBuildInfo() {
  try {
    buildInfo = await core.invoke("get_build_info");
    renderBuildInfo();
  } catch (e) {
    console.error(e);
  }
}

function humanSize(bytes) {
  if (bytes == null) return "";
  const units = window.I18N.sizeUnits();
  let size = bytes;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${bytes} ${units[0]}` : `${size.toFixed(1)} ${units[unit]}`;
}

function fileName(path) {
  return path.split("/").pop() || path;
}

const ICONS = {
  run: '<path d="M5.5 3.5v9l7-4.5z" fill="currentColor"/>',
  queued:
    '<circle cx="8" cy="8" r="5.5" fill="none" stroke="currentColor" stroke-width="1.5"/>' +
    '<path d="M8 5v3.2l2 1.3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>',
  running:
    '<path d="M8 2.5a5.5 5.5 0 1 1-5.5 5.5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>',
  done: '<path d="M3.5 8.5l3 3 6-7" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>',
  same: '<path d="M4 6.5h8M4 9.5h8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>',
  error:
    '<circle cx="8" cy="8" r="6" fill="currentColor"/>' +
    '<path d="M8 4.8v3.8" stroke="white" stroke-width="1.7" stroke-linecap="round"/><circle cx="8" cy="11.2" r="1" fill="white"/>',
  reveal:
    '<circle cx="7" cy="7" r="4" fill="none" stroke="currentColor" stroke-width="1.6"/>' +
    '<path d="M10 10l3.5 3.5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>',
  remove:
    '<path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.7 8.5h5.6l.7-8.5M7 7v4M9 7v4" fill="none" ' +
    'stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>',
};

function icon(name) {
  return `<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">${ICONS[name]}</svg>`;
}

/** Status kind -> [icon, whether the status button (re)runs the file, its tooltip key]. */
const STATUS = {
  pending: ["run", true, "file.run"],
  queued: ["queued", false, "file.queued"],
  running: ["running", false, "file.running"],
  compressed: ["done", true, "file.rerun"],
  not_smaller: ["same", true, "file.rerun"],
  error: ["error", true, "file.retry"],
};

/** Files "Compress all" picks up: never run yet, or failed. */
const RUNNABLE = new Set(["pending", "error"]);

/** Renders a file's row (one line) in the active language from its last known state. */
function renderFileStatus(entry) {
  const s = entry.state;
  const [iconName, runnable, tip] = STATUS[s.kind];
  entry.li.className = `file-row ${s.kind}`;
  entry.statusBtn.innerHTML = icon(iconName);
  entry.statusBtn.disabled = !runnable;
  entry.statusBtn.title = t(tip);

  entry.detail.title = "";
  entry.reveal.hidden = s.kind !== "compressed";
  if (s.kind === "compressed") {
    entry.detail.textContent = `${humanSize(s.inputSize)} → ${humanSize(s.outputSize)} · −${s.pct} %`;
  } else if (s.kind === "not_smaller") {
    entry.detail.textContent = t("file.notSmaller");
  } else if (s.kind === "error") {
    entry.detail.textContent = s.message || t("file.failed");
    entry.detail.title = s.message || "";
  } else if (s.kind === "running") {
    entry.detail.textContent = t("file.running");
  } else {
    entry.detail.textContent = "";
  }
  entry.reveal.title = t(IS_WINDOWS ? "file.revealExplorer" : "file.reveal");
  // Removing a queued/running file wouldn't stop its compression: wait for it.
  const busy = s.kind === "queued" || s.kind === "running";
  entry.remove.disabled = busy;
  entry.remove.title = t("file.remove");
}

function updateCompressAll() {
  compressBtn.disabled = ![...files.values()].some((e) => RUNNABLE.has(e.state.kind));
}

function setState(entry, state) {
  entry.state = state;
  renderFileStatus(entry);
  updateCompressAll();
}

/** Compresses `paths` (in order, one at a time) at the selected level. */
async function run(paths) {
  paths = paths.filter((p) => files.has(p));
  if (paths.length === 0) return;
  for (const p of paths) setState(files.get(p), { kind: "queued" });
  try {
    await core.invoke("compress_files", { paths, level: levelSelect.value });
  } catch (e) {
    for (const p of paths) {
      const entry = files.get(p);
      if (entry && (entry.state.kind === "queued" || entry.state.kind === "running")) {
        setState(entry, { kind: "error", message: String(e) });
      }
    }
  }
}

function addFiles(paths) {
  for (const path of paths) {
    if (files.has(path) || !path.toLowerCase().endsWith(".pdf")) continue;

    const li = document.createElement("li");
    const statusBtn = document.createElement("button");
    statusBtn.className = "status-btn";
    statusBtn.addEventListener("click", () => run([path]));
    const name = document.createElement("span");
    name.className = "file-name";
    name.textContent = fileName(path);
    name.title = path;
    const detail = document.createElement("span");
    detail.className = "file-detail";
    const reveal = document.createElement("button");
    reveal.className = "icon-btn";
    reveal.innerHTML = icon("reveal");
    reveal.addEventListener("click", () => {
      const s = files.get(path)?.state;
      if (s?.output) core.invoke("reveal_in_finder", { path: s.output });
    });
    const remove = document.createElement("button");
    remove.className = "icon-btn remove-btn";
    remove.innerHTML = icon("remove");
    remove.addEventListener("click", () => removeFile(path));
    li.append(statusBtn, name, detail, reveal, remove);
    fileList.append(li);

    const entry = { li, statusBtn, detail, reveal, remove, state: { kind: "pending" } };
    files.set(path, entry);
    renderFileStatus(entry);
  }
  updateCompressAll();
}

function removeFile(path) {
  const entry = files.get(path);
  if (!entry) return;
  entry.li.remove();
  files.delete(path);
  updateCompressAll();
}

function clearFiles() {
  files.clear();
  fileList.innerHTML = "";
  updateCompressAll();
}

/** "extreme-max" -> "extremeMax", the i18n key naming convention. */
function levelKey(level) {
  return level.replace(/-(\w)/g, (_, c) => c.toUpperCase());
}

function renderLevelHint() {
  levelHint.textContent = t(`levelHint.${levelKey(levelSelect.value)}`);
}

async function loadConfig() {
  const cfg = await core.invoke("get_config");
  levelSelect.value = cfg.level;
  renderLevelHint();
}

async function onLevelChange() {
  renderLevelHint();
  if (!setDefaultCheckbox.checked) return;
  try {
    await core.invoke("set_default_level", { level: levelSelect.value });
  } catch (e) {
    console.error(e);
  }
}

levelSelect.addEventListener("change", onLevelChange);
setDefaultCheckbox.addEventListener("change", onLevelChange);

document.querySelectorAll(".lang-btn").forEach((btn) => {
  btn.addEventListener("click", () => window.I18N.setLocale(btn.dataset.lang));
});

window.I18N.onChange(() => {
  for (const entry of files.values()) renderFileStatus(entry);
  renderBuildInfo();
  renderLevelHint();
});

pickFilesBtn.addEventListener("click", async () => {
  const selected = await dialog.open({
    multiple: true,
    filters: [{ name: "PDF", extensions: ["pdf"] }],
  });
  if (!selected) return;
  addFiles(Array.isArray(selected) ? selected : [selected]);
});

clearBtn.addEventListener("click", clearFiles);

["dragenter", "dragover"].forEach((name) =>
  dropZone.addEventListener(name, (e) => {
    e.preventDefault();
    dropZone.classList.add("drag-over");
  })
);
["dragleave", "drop"].forEach((name) =>
  dropZone.addEventListener(name, () => dropZone.classList.remove("drag-over"))
);

event.listen("tauri://drag-drop", (e) => {
  dropZone.classList.remove("drag-over");
  addFiles(e.payload.paths ?? []);
});

// Listener first, then ask for files that arrived before it existed (cold start).
event
  .listen("opened-files", (e) => addFiles(e.payload ?? []))
  .then(() => core.invoke("frontend_ready"))
  .then((paths) => paths.length && addFiles(paths));

event.listen("compress-started", (e) => {
  const entry = files.get(e.payload);
  if (entry) setState(entry, { kind: "running" });
});

event.listen("compress-result", (e) => {
  const r = e.payload;
  const entry = files.get(r.input);
  if (!entry) return;

  if (r.status === "compressed") {
    const pct = r.input_size > 0 ? Math.round((1 - r.output_size / r.input_size) * 100) : 0;
    setState(entry, { kind: "compressed", inputSize: r.input_size, outputSize: r.output_size, pct, output: r.output });
  } else if (r.status === "not_smaller") {
    setState(entry, { kind: "not_smaller" });
  } else {
    setState(entry, { kind: "error", message: r.message });
  }
});

compressBtn.addEventListener("click", () =>
  run([...files].filter(([, e]) => RUNNABLE.has(e.state.kind)).map(([p]) => p))
);

installBtn.addEventListener("click", async () => {
  installBtn.disabled = true;
  installStatus.textContent = t("install.installing");
  try {
    const result = await core.invoke("install_integrations");
    const describe = (label, okKey, outcome) => {
      if (!outcome) return null;
      return outcome.Ok !== undefined
        ? t(okKey, { path: outcome.Ok })
        : t("install.itemFailed", { label, error: outcome.Err ?? "" });
    };
    installStatus.textContent = [
      describe(t("install.serviceLabel"), "install.serviceOk", result.finder_service),
      describe(t("install.cliLabel"), "install.cliOk", result.cli_link),
    ]
      .filter(Boolean)
      .join(" · ");
  } catch (e) {
    installStatus.textContent = String(e);
  } finally {
    installBtn.disabled = false;
  }
});

window.I18N.applyToDom();
renderLevelHint();
loadConfig();
loadBuildInfo();
