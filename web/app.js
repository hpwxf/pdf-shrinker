// Web version of the app (app/ui/app.js): same list and statuses, but the
// files come from the browser, are compressed by the WebAssembly engine in a
// worker (worker.js), one job at a time, and are downloaded instead of
// written next to the original. Nothing is uploaded.

const { t } = window.I18N;

const levelSelect = document.getElementById("level-select");
const levelHint = document.getElementById("level-hint");
const jpegWarning = document.getElementById("jpeg-warning");
const compareAllCheckbox = document.getElementById("compare-all");
const compressBtn = document.getElementById("compress-btn");
const clearBtn = document.getElementById("clear-btn");
const pickFilesBtn = document.getElementById("pick-files");
const fileInput = document.getElementById("file-input");
const dropZone = document.getElementById("drop-zone");
const fileList = document.getElementById("file-list");
const engineStatus = document.getElementById("engine-status");
const buildInfoEl = document.getElementById("build-info");
const buildInfoTooltip = document.getElementById("build-info-tooltip");
const versionEl = document.getElementById("version");

/** Every level, in the order the select lists them (and comparisons show them). */
const ALL_LEVELS = [...levelSelect.options].map((o) => o.value);

// The level is remembered per viewer (the desktop app keeps it in its config file).
const LEVEL_KEY = "pdfshrinker.level";
try {
  const saved = localStorage.getItem(LEVEL_KEY);
  if (ALL_LEVELS.includes(saved)) levelSelect.value = saved;
} catch {
  // localStorage can throw (blocked site data): keep the default.
}

/** id -> entry: { file, li, row, statusBtn, detail, download, remove, levelList, state, compare, levels, results } */
const files = new Map();
let nextId = 1;

/** Engine state: `differs` (level -> whether its output can differ from the desktop app's). */
let engine = { status: "loading", build: null, differs: {}, error: null };

// --- Rendering ---------------------------------------------------------------

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
  download:
    '<path d="M8 2.5v7.5M4.8 7l3.2 3.2L11.2 7M3.5 13h9" fill="none" stroke="currentColor" ' +
    'stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>',
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
  compared: ["done", true, "file.rerun"],
  queued: ["queued", false, "file.queued"],
  running: ["running", false, "file.running"],
  compressed: ["done", true, "file.rerun"],
  not_smaller: ["same", true, "file.rerun"],
  error: ["error", true, "file.retry"],
};

/** Files "Compress all" picks up: never run yet, or failed. */
const RUNNABLE = new Set(["pending", "error"]);

function formatScore(x) {
  return x.toLocaleString(window.I18N.getLocale(), {
    minimumFractionDigits: 3,
    maximumFractionDigits: 3,
  });
}

function fidelityText(f) {
  if (!f) return [t("file.noImages"), ""];
  return [
    t("file.fidelity", { value: formatScore(f.mean) }),
    t("file.fidelityTip", { images: f.images, min: formatScore(f.min) }),
  ];
}

function sizeText(s) {
  return `${humanSize(s.inputSize)} → ${humanSize(s.outputSize)} · −${s.pct} %`;
}

/** "report.pdf" at "high" -> "report-high.pdf" (compare mode) or "report-compressed.pdf". */
function outputName(name, level, compare) {
  const stem = name.replace(/\.pdf$/i, "");
  return `${stem}-${compare ? level : "compressed"}.pdf`;
}

/** Points a download link at a level's result, or hides it. */
function setDownload(link, s) {
  link.hidden = s.kind !== "compressed";
  if (s.kind === "compressed") {
    link.href = s.url;
    link.download = s.downloadName;
  } else {
    link.removeAttribute("href");
  }
  link.title = t("web.download");
}

function renderFileStatus(entry) {
  const s = entry.state;
  const [iconName, runnable, tip] = STATUS[s.kind];
  entry.row.className = `file-row ${s.kind}`;
  entry.statusBtn.innerHTML = icon(iconName);
  entry.statusBtn.disabled = !runnable || engine.status !== "ready";
  entry.statusBtn.title = t(tip);

  entry.detail.title = "";
  setDownload(entry.download, s);
  if (s.kind === "compressed") {
    const [fid, fidTip] = fidelityText(s.fidelity);
    entry.detail.textContent = `${sizeText(s)} · ${fid}`;
    entry.detail.title = fidTip;
  } else if (s.kind === "compared") {
    entry.detail.textContent = t("file.compared", { n: entry.levels.length });
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
  const busy = s.kind === "queued" || s.kind === "running";
  entry.remove.disabled = busy;
  entry.remove.title = t("file.remove");
  renderLevelResults(entry);
}

function renderLevelResults(entry) {
  entry.levelList.hidden = !entry.compare;
  if (!entry.compare) return;
  entry.levelList.innerHTML = "";
  for (const level of entry.levels) {
    const s = entry.results[level];
    const li = document.createElement("li");
    li.className = `level-result ${s.kind}`;
    const name = document.createElement("span");
    name.className = "level-name";
    name.textContent = t(`level.${levelKey(level)}`);
    const size = document.createElement("span");
    size.className = "level-size";
    const fid = document.createElement("span");
    fid.className = "level-fidelity";
    const download = document.createElement("a");
    download.className = "icon-btn";
    download.innerHTML = icon("download");
    setDownload(download, s);
    // Not `hidden`: the cell must stay in the grid to keep the columns aligned.
    download.hidden = false;
    download.style.visibility = s.kind === "compressed" ? "visible" : "hidden";
    if (s.kind === "compressed") {
      size.textContent = sizeText(s);
      [fid.textContent, fid.title] = fidelityText(s.fidelity);
    } else if (s.kind === "not_smaller") {
      size.textContent = t("file.notSmaller");
    } else if (s.kind === "error") {
      size.textContent = s.message || t("file.failed");
      size.title = s.message || "";
    } else if (s.kind === "running") {
      size.textContent = t("file.running");
    } else {
      size.textContent = t("file.queued");
    }
    li.append(name, size, fid, download);
    entry.levelList.append(li);
  }
}

function updateCompressAll() {
  compressBtn.disabled =
    engine.status !== "ready" || ![...files.values()].some((e) => RUNNABLE.has(e.state.kind));
}

function setState(entry, state) {
  entry.state = state;
  renderFileStatus(entry);
  updateCompressAll();
}

/** "extreme-max" -> "extremeMax", the i18n key naming convention. */
function levelKey(level) {
  return level.replace(/-(\w)/g, (_, c) => c.toUpperCase());
}

function renderLevelHint() {
  const compare = compareAllCheckbox.checked;
  levelHint.textContent = t(`levelHint.${compare ? "compare" : levelKey(levelSelect.value)}`);
  // Warn whenever a level that will run re-encodes JPEGs differently from
  // the desktop app (known once the engine is loaded).
  const levels = compare ? ALL_LEVELS : [levelSelect.value];
  const differs = levels.some((l) => engine.differs[l]);
  jpegWarning.hidden = !differs;
  jpegWarning.textContent = differs ? t(compare ? "web.jpegWarningCompare" : "web.jpegWarning") : "";
}

function renderEngineStatus() {
  engineStatus.hidden = engine.status === "ready";
  if (engine.status === "loading") engineStatus.textContent = t("web.loading");
  if (engine.status === "failed") engineStatus.textContent = t("web.engineFailed", { error: engine.error });
}

/** Full version info for the "ⓘ" tooltip, like the desktop app's, plus the JPEG encoder. */
function buildInfoText() {
  const b = engine.build;
  if (!b) return "";
  const dirty = b.dirty ? t("info.dirtySuffix") : "";
  // `JpegEncoder` names -> the library's own name.
  const jpeg = { rust: "jpeg-encoder", mozjpeg: "mozjpeg" }[b.jpegEncoder] ?? b.jpegEncoder;
  return t("web.build", { version: b.version, commit: b.commit, dirty, jpeg });
}

function renderBuildInfo() {
  const b = engine.build;
  buildInfoEl.hidden = !b;
  versionEl.hidden = !b;
  if (!b) return;
  buildInfoEl.textContent = "ⓘ";
  buildInfoEl.title = buildInfoText();
  versionEl.textContent = t("web.version", { version: b.version, commit: b.commit + (b.dirty ? "+" : "") });
  if (!buildInfoTooltip.hidden) buildInfoTooltip.textContent = buildInfoText();
}

function renderAll() {
  for (const entry of files.values()) renderFileStatus(entry);
  renderLevelHint();
  renderEngineStatus();
  renderBuildInfo();
  updateCompressAll();
}

// --- Engine (worker) ---------------------------------------------------------

/** Jobs waiting for the worker: { id, level }. The head one is running. */
const queue = [];
let worker = null;

function startWorker() {
  worker = new Worker("worker.js", { type: "module" });
  worker.onmessage = ({ data }) => onWorkerMessage(data);
  worker.onerror = (e) => {
    // Failing to load the module (e.g. served from file://).
    if (engine.status === "loading") {
      engine = { ...engine, status: "failed", error: e.message || "worker error" };
      renderAll();
    }
  };
}

function onWorkerMessage(msg) {
  if (msg.type === "ready") {
    engine = { status: "ready", build: msg.build, differs: msg.differs, error: null };
    renderAll();
    pump();
    return;
  }
  if (msg.type === "failed") {
    engine = { ...engine, status: "failed", error: msg.message };
    renderAll();
    return;
  }
  queue.shift();
  const entry = files.get(msg.id);
  if (entry) recordResult(entry, msg);
  if (msg.crashed) {
    // A trapped module is unusable: start over with a fresh one.
    worker.terminate();
    engine = { ...engine, status: "loading" };
    renderAll();
    startWorker();
    return;
  }
  pump();
}

function recordResult(entry, msg) {
  let state;
  if (msg.type === "error") {
    state = { kind: "error", message: msg.crashed ? t("web.crashed") : msg.message };
  } else if (msg.outputSize >= msg.inputSize) {
    state = { kind: "not_smaller" };
  } else {
    const blob = new Blob([msg.output], { type: "application/pdf" });
    state = {
      kind: "compressed",
      inputSize: msg.inputSize,
      outputSize: msg.outputSize,
      pct: msg.inputSize > 0 ? Math.round((1 - msg.outputSize / msg.inputSize) * 100) : 0,
      fidelity: msg.fidelity,
      url: URL.createObjectURL(blob),
      downloadName: outputName(entry.file.name, msg.level, entry.compare),
    };
  }
  if (!entry.compare) {
    setState(entry, state);
    return;
  }
  entry.results[msg.level] = state;
  const done = entry.levels.every((l) => !["queued", "running"].includes(entry.results[l].kind));
  setState(entry, done ? { kind: "compared" } : { kind: "running" });
}

/** Sends the next queued job to the worker, if it is idle. */
async function pump() {
  if (engine.status !== "ready" || queue.length === 0 || queue[0].sent) return;
  const job = queue[0];
  const entry = files.get(job.id);
  job.sent = true;
  if (entry.compare) entry.results[job.level] = { kind: "running" };
  setState(entry, { kind: "running" });
  let bytes;
  try {
    bytes = await entry.file.arrayBuffer();
  } catch (e) {
    onWorkerMessage({ type: "error", id: job.id, level: job.level, message: String(e) });
    return;
  }
  worker.postMessage({ id: job.id, level: job.level, name: entry.file.name, bytes }, [bytes]);
}

// --- Actions -----------------------------------------------------------------

function revokeResults(entry) {
  for (const s of [entry.state, ...Object.values(entry.results)]) {
    if (s.url) URL.revokeObjectURL(s.url);
  }
}

/** Queues `ids` at the selected level, or at every level with "compare all levels". */
function run(ids) {
  ids = ids.filter((id) => files.has(id));
  if (ids.length === 0) return;
  const compare = compareAllCheckbox.checked;
  const levels = compare ? ALL_LEVELS : [levelSelect.value];
  for (const id of ids) {
    const entry = files.get(id);
    revokeResults(entry);
    entry.compare = compare;
    entry.levels = levels;
    entry.results = Object.fromEntries(levels.map((l) => [l, { kind: "queued" }]));
    setState(entry, { kind: "queued" });
    for (const level of levels) queue.push({ id, level });
  }
  pump();
}

function addFiles(list) {
  for (const file of list) {
    const isPdf = file.type === "application/pdf" || file.name.toLowerCase().endsWith(".pdf");
    if (!isPdf) continue;
    const id = nextId++;

    const li = document.createElement("li");
    li.className = "file-item";
    const row = document.createElement("div");
    const statusBtn = document.createElement("button");
    statusBtn.className = "status-btn";
    statusBtn.addEventListener("click", () => run([id]));
    const name = document.createElement("span");
    name.className = "file-name";
    name.textContent = file.name;
    name.title = file.name;
    const detail = document.createElement("span");
    detail.className = "file-detail";
    const download = document.createElement("a");
    download.className = "icon-btn";
    download.innerHTML = icon("download");
    const remove = document.createElement("button");
    remove.className = "icon-btn remove-btn";
    remove.innerHTML = icon("remove");
    remove.addEventListener("click", () => removeFile(id));
    row.append(statusBtn, name, detail, download, remove);
    const levelList = document.createElement("ul");
    levelList.className = "level-results";
    levelList.hidden = true;
    li.append(row, levelList);
    fileList.append(li);

    const entry = {
      file,
      li,
      row,
      statusBtn,
      detail,
      download,
      remove,
      levelList,
      compare: false,
      levels: [],
      results: {},
      state: { kind: "pending" },
    };
    files.set(id, entry);
    renderFileStatus(entry);
  }
  updateCompressAll();
}

function removeFile(id) {
  const entry = files.get(id);
  if (!entry) return;
  revokeResults(entry);
  entry.li.remove();
  files.delete(id);
  updateCompressAll();
}

/** Drops every file not being compressed (queued jobs of removed files are skipped). */
function clearFiles() {
  for (const [id, entry] of files) {
    if (entry.state.kind !== "queued" && entry.state.kind !== "running") removeFile(id);
  }
}

// --- Wiring ------------------------------------------------------------------

levelSelect.addEventListener("change", () => {
  try {
    localStorage.setItem(LEVEL_KEY, levelSelect.value);
  } catch {
    // Best effort only.
  }
  renderLevelHint();
});
compareAllCheckbox.addEventListener("change", renderLevelHint);

document.querySelectorAll(".lang-btn").forEach((btn) => {
  btn.addEventListener("click", () => window.I18N.setLocale(btn.dataset.lang));
});
window.I18N.onChange(renderAll);

pickFilesBtn.addEventListener("click", () => fileInput.click());
fileInput.addEventListener("change", () => {
  addFiles([...fileInput.files]);
  fileInput.value = "";
});
clearBtn.addEventListener("click", clearFiles);
compressBtn.addEventListener("click", () =>
  run([...files].filter(([, e]) => RUNNABLE.has(e.state.kind)).map(([id]) => id))
);

// Dropping anywhere on the page adds the files (and doesn't open them in the tab).
["dragenter", "dragover"].forEach((name) =>
  document.addEventListener(name, (e) => {
    e.preventDefault();
    dropZone.classList.add("drag-over");
  })
);
document.addEventListener("dragleave", (e) => {
  if (e.relatedTarget === null) dropZone.classList.remove("drag-over");
});
document.addEventListener("drop", (e) => {
  e.preventDefault();
  dropZone.classList.remove("drag-over");
  addFiles([...(e.dataTransfer?.files ?? [])]);
});

buildInfoEl.addEventListener("click", (e) => {
  e.stopPropagation();
  buildInfoTooltip.hidden = !buildInfoTooltip.hidden;
  buildInfoEl.classList.toggle("active", !buildInfoTooltip.hidden);
  buildInfoTooltip.textContent = buildInfoText();
});
document.addEventListener("click", (e) => {
  if (!buildInfoTooltip.hidden && !buildInfoTooltip.contains(e.target)) {
    buildInfoTooltip.hidden = true;
    buildInfoEl.classList.remove("active");
  }
});

window.I18N.applyToDom();
startWorker();
renderAll();
