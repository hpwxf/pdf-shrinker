const { core, event, dialog } = window.__TAURI__;

const levelSelect = document.getElementById("level-select");
const engineSelect = document.getElementById("engine-select");
const setDefaultCheckbox = document.getElementById("set-default");
const compressBtn = document.getElementById("compress-btn");
const clearBtn = document.getElementById("clear-btn");
const pickFilesBtn = document.getElementById("pick-files");
const dropZone = document.getElementById("drop-zone");
const fileList = document.getElementById("file-list");
const installBtn = document.getElementById("install-btn");
const installStatus = document.getElementById("install-status");

/** path -> { li, status, detail } */
const files = new Map();

function humanSize(bytes) {
  if (bytes == null) return "";
  const units = ["o", "Ko", "Mo", "Go"];
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

function addFiles(paths) {
  for (const path of paths) {
    if (files.has(path) || !path.toLowerCase().endsWith(".pdf")) continue;

    const li = document.createElement("li");
    const name = document.createElement("div");
    name.className = "file-name";
    name.textContent = fileName(path);
    const detail = document.createElement("div");
    detail.className = "file-detail";
    detail.textContent = "en attente";
    li.append(name, detail);
    fileList.append(li);

    files.set(path, { li, detail });
  }
  compressBtn.disabled = files.size === 0;
}

function clearFiles() {
  files.clear();
  fileList.innerHTML = "";
  compressBtn.disabled = true;
}

async function loadConfig() {
  const cfg = await core.invoke("get_config");
  levelSelect.value = cfg.level;
  engineSelect.value = cfg.engine;
  const gsOption = engineSelect.querySelector('option[value="gs"]');
  const bestOption = engineSelect.querySelector('option[value="best"]');
  if (!cfg.gs_available) {
    gsOption.disabled = true;
    gsOption.textContent = "Ghostscript (non installé)";
    bestOption.textContent = "Meilleur des deux (Ghostscript indisponible)";
  }
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
    entry.detail.className = "file-detail ok";
    const pct = r.input_size > 0 ? Math.round((1 - r.output_size / r.input_size) * 100) : 0;
    entry.detail.textContent = `${humanSize(r.input_size)} → ${humanSize(r.output_size)}  (-${pct} %)  `;
    const reveal = document.createElement("button");
    reveal.className = "reveal-link";
    reveal.textContent = "Afficher dans le Finder";
    reveal.addEventListener("click", () => core.invoke("reveal_in_finder", { path: r.output }));
    entry.detail.append(reveal);
  } else if (r.status === "not_smaller") {
    entry.detail.className = "file-detail";
    entry.detail.textContent = "déjà optimal, rien à faire";
  } else {
    entry.detail.className = "file-detail err";
    entry.detail.textContent = r.message || "échec";
  }
});

compressBtn.addEventListener("click", async () => {
  compressBtn.disabled = true;
  for (const [, entry] of files) {
    entry.detail.className = "file-detail";
    entry.detail.textContent = "compression…";
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
  installStatus.textContent = "Installation…";
  try {
    const result = await core.invoke("install_integrations");
    const describe = (label, okLabel, outcome) => {
      if (!outcome) return null;
      return outcome.Ok !== undefined ? okLabel : `${label} : échec (${outcome.Err ?? ""})`;
    };
    installStatus.textContent = [
      describe("Action rapide", "Action rapide installée", result.quick_action),
      describe("Outil en ligne de commande", "outil en ligne de commande installé (/usr/local/bin/pdfshrink)", result.cli_link),
    ]
      .filter(Boolean)
      .join(" · ");
  } catch (e) {
    installStatus.textContent = String(e);
  } finally {
    installBtn.disabled = false;
  }
});

loadConfig();
