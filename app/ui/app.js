const { core, event, dialog } = window.__TAURI__;
const { t } = window.I18N;

const levelSelect = document.getElementById("level-select");
const engineSelect = document.getElementById("engine-select");
const engineOptionGs = document.getElementById("engine-option-gs");
const engineOptionBest = document.getElementById("engine-option-best");
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

/** path -> { li, detail, render() } */
const files = new Map();
let gsAvailable = true;
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

/** Renders a file's current status in the active language from its last known state. */
function renderFileStatus(entry) {
  entry.detail.innerHTML = "";
  const s = entry.state;
  if (s.kind === "pending") {
    entry.detail.className = "file-detail";
    entry.detail.textContent = t("file.pending");
  } else if (s.kind === "compressing") {
    entry.detail.className = "file-detail";
    entry.detail.textContent = t("file.compressing");
  } else if (s.kind === "compressed") {
    entry.detail.className = "file-detail ok";
    entry.detail.textContent = `${humanSize(s.inputSize)} → ${humanSize(s.outputSize)}  (-${s.pct} %)  `;
    const reveal = document.createElement("button");
    reveal.className = "reveal-link";
    reveal.textContent = t("file.reveal");
    reveal.addEventListener("click", () => core.invoke("reveal_in_finder", { path: s.output }));
    entry.detail.append(reveal);
  } else if (s.kind === "not_smaller") {
    entry.detail.className = "file-detail";
    entry.detail.textContent = t("file.notSmaller");
  } else if (s.kind === "error") {
    entry.detail.className = "file-detail err";
    entry.detail.textContent = s.message || t("file.failed");
  }
}

function addFiles(paths) {
  for (const path of paths) {
    if (files.has(path) || !path.toLowerCase().endsWith(".pdf")) continue;

    const li = document.createElement("li");
    const name = document.createElement("div");
    name.className = "file-name";
    name.textContent = fileName(path);
    const detail = document.createElement("div");
    detail.className = "file-detail";
    li.append(name, detail);
    fileList.append(li);

    const entry = { li, detail, state: { kind: "pending" } };
    files.set(path, entry);
    renderFileStatus(entry);
  }
  compressBtn.disabled = files.size === 0;
}

function clearFiles() {
  files.clear();
  fileList.innerHTML = "";
  compressBtn.disabled = true;
}

function updateEngineOptionLabels() {
  engineOptionGs.textContent = gsAvailable ? t("engine.gs") : t("engine.gsUnavailable");
  engineOptionGs.disabled = !gsAvailable;
  engineOptionBest.textContent = gsAvailable ? t("engine.best") : t("engine.bestGsUnavailable");
}

async function loadConfig() {
  const cfg = await core.invoke("get_config");
  levelSelect.value = cfg.level;
  engineSelect.value = cfg.engine;
  gsAvailable = cfg.gs_available;
  updateEngineOptionLabels();
}

async function onLevelOrEngineChange() {
  if (!setDefaultCheckbox.checked) return;
  try {
    await core.invoke("set_default_level", { level: levelSelect.value });
    await core.invoke("set_default_engine", { engine: engineSelect.value });
  } catch (e) {
    console.error(e);
  }
}

levelSelect.addEventListener("change", onLevelOrEngineChange);
engineSelect.addEventListener("change", onLevelOrEngineChange);
setDefaultCheckbox.addEventListener("change", onLevelOrEngineChange);

document.querySelectorAll(".lang-btn").forEach((btn) => {
  btn.addEventListener("click", () => window.I18N.setLocale(btn.dataset.lang));
});

window.I18N.onChange(() => {
  updateEngineOptionLabels();
  for (const entry of files.values()) renderFileStatus(entry);
  renderBuildInfo();
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

event.listen("opened-files", (e) => addFiles(e.payload ?? []));

event.listen("compress-result", (e) => {
  const r = e.payload;
  const entry = files.get(r.input);
  if (!entry) return;

  if (r.status === "compressed") {
    const pct = r.input_size > 0 ? Math.round((1 - r.output_size / r.input_size) * 100) : 0;
    entry.state = { kind: "compressed", inputSize: r.input_size, outputSize: r.output_size, pct, output: r.output };
  } else if (r.status === "not_smaller") {
    entry.state = { kind: "not_smaller" };
  } else {
    entry.state = { kind: "error", message: r.message };
  }
  renderFileStatus(entry);
});

compressBtn.addEventListener("click", async () => {
  compressBtn.disabled = true;
  for (const entry of files.values()) {
    entry.state = { kind: "compressing" };
    renderFileStatus(entry);
  }
  try {
    await core.invoke("compress_files", {
      paths: [...files.keys()],
      level: levelSelect.value,
      engine: engineSelect.value,
    });
  } catch (e) {
    console.error(e);
  } finally {
    compressBtn.disabled = files.size === 0;
  }
});

installBtn.addEventListener("click", async () => {
  installBtn.disabled = true;
  installStatus.textContent = t("install.installing");
  try {
    const result = await core.invoke("install_integrations");
    const describe = (label, okLabel, outcome) => {
      if (!outcome) return null;
      return outcome.Ok !== undefined ? okLabel : t("install.itemFailed", { label, error: outcome.Err ?? "" });
    };
    installStatus.textContent = [
      describe(t("install.quickActionLabel"), t("install.quickActionOk"), result.quick_action),
      describe(t("install.cliLabel"), t("install.cliOk"), result.cli_link),
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
updateEngineOptionLabels();
loadConfig();
loadBuildInfo();
