/**
 * Minimal EN/FR i18n for the app UI (the CLI stays English-only).
 * Locale is auto-detected from the system on first launch, then persisted
 * (per-viewer, in the webview's localStorage) once the user picks one
 * explicitly via the language switcher.
 */
window.I18N = (() => {
  const STORAGE_KEY = "pdfshrinker.locale";

  const translations = {
    en: {
      app: {
        subtitle: "Compress PDFs while keeping them PDFs.",
      },
      drop: {
        hint: "Drag PDFs here, or",
        pick: "Choose files…",
      },
      options: {
        level: "Level",
        setDefault: "Set as default level",
      },
      level: {
        lossless: "Lossless",
        low: "Low",
        medium: "Medium",
        high: "High",
      },
      actions: {
        compress: "Compress",
        clear: "Clear list",
      },
      file: {
        pending: "pending",
        compressing: "compressing…",
        notSmaller: "already optimal, nothing to do",
        failed: "failed",
        reveal: "Show in Finder",
      },
      install: {
        button: "Install the Quick Action and command-line tool…",
        installing: "Installing…",
        quickActionLabel: "Quick Action",
        quickActionOk: "Quick Action installed",
        cliLabel: "Command-line tool",
        cliOk: "command-line tool installed (/usr/local/bin/pdfshrink)",
        itemFailed: "{label}: failed ({error})",
      },
      lang: {
        label: "Language",
      },
      size: {
        units: ["B", "KB", "MB", "GB"],
      },
      info: {
        build: "PdfShrinker {version}\ncommit {commit}{dirty}",
        dirtySuffix: " (uncommitted changes)",
      },
    },
    fr: {
      app: {
        subtitle: "Compresse des PDF en restant des PDF.",
      },
      drop: {
        hint: "Glissez des PDF ici, ou",
        pick: "Choisir des fichiers…",
      },
      options: {
        level: "Niveau",
        setDefault: "Définir comme niveau par défaut",
      },
      level: {
        lossless: "Sans perte",
        low: "Léger",
        medium: "Moyen",
        high: "Fort",
      },
      actions: {
        compress: "Compresser",
        clear: "Effacer la liste",
      },
      file: {
        pending: "en attente",
        compressing: "compression…",
        notSmaller: "déjà optimal, rien à faire",
        failed: "échec",
        reveal: "Afficher dans le Finder",
      },
      install: {
        button: "Installer l'Action rapide et l'outil en ligne de commande…",
        installing: "Installation…",
        quickActionLabel: "Action rapide",
        quickActionOk: "Action rapide installée",
        cliLabel: "Outil en ligne de commande",
        cliOk: "outil en ligne de commande installé (/usr/local/bin/pdfshrink)",
        itemFailed: "{label} : échec ({error})",
      },
      lang: {
        label: "Langue",
      },
      size: {
        units: ["o", "Ko", "Mo", "Go"],
      },
      info: {
        build: "PdfShrinker {version}\ncommit {commit}{dirty}",
        dirtySuffix: " (modifications non commitées)",
      },
    },
  };

  function detectLocale() {
    try {
      const saved = localStorage.getItem(STORAGE_KEY);
      if (saved === "en" || saved === "fr") return saved;
    } catch {
      // localStorage can throw (private mode, blocked site data); fall through.
    }
    const nav = (navigator.language || "en").toLowerCase();
    return nav.startsWith("fr") ? "fr" : "en";
  }

  let locale = detectLocale();
  const listeners = [];

  function lookup(dict, key) {
    return key.split(".").reduce((o, k) => (o && o[k] !== undefined ? o[k] : undefined), dict);
  }

  function t(key, vars) {
    let str = lookup(translations[locale], key) ?? lookup(translations.en, key) ?? key;
    if (vars) {
      for (const [k, v] of Object.entries(vars)) {
        str = str.replaceAll(`{${k}}`, v);
      }
    }
    return str;
  }

  function applyToDom(root = document) {
    root.querySelectorAll("[data-i18n]").forEach((el) => {
      el.textContent = t(el.getAttribute("data-i18n"));
    });
    root.querySelectorAll("[data-i18n-title]").forEach((el) => {
      el.title = t(el.getAttribute("data-i18n-title"));
    });
    document.documentElement.lang = locale;
    document.querySelectorAll(".lang-btn").forEach((btn) => {
      btn.classList.toggle("active", btn.dataset.lang === locale);
    });
  }

  function setLocale(next) {
    if ((next !== "en" && next !== "fr") || next === locale) return;
    locale = next;
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // Best effort only — the choice just won't survive a restart.
    }
    applyToDom();
    listeners.forEach((fn) => fn(locale));
  }

  function onChange(fn) {
    listeners.push(fn);
  }

  function sizeUnits() {
    return lookup(translations[locale], "size.units") ?? translations.en.size.units;
  }

  return { t, setLocale, getLocale: () => locale, applyToDom, onChange, sizeUnits };
})();
