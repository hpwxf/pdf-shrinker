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
        subtitle: "Smaller is better — and still beautiful.",
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
        extreme: "Extreme",
        extremeMax: "Extreme max",
      },
      levelHint: {
        lossless: "Images untouched; duplicate objects and fonts merged. No visible change.",
        low: "Images re-encoded at high quality (JPEG up to 85), reduced to 200 dpi above 250 dpi. Fit for print.",
        medium: "Images at 150 dpi, JPEG quality tuned per image (up to 75). For screen and office printing.",
        high: "Images at 96 dpi, JPEG quality tuned per image to stay faithful. For the screen.",
        extreme: "Also caps slide images, crops images to the page and whitens scanned paper. Slower.",
        extremeMax: "Smallest file: 72 dpi, lower quality. Visibly degraded when zoomed in.",
      },
      actions: {
        compress: "Compress all",
        clear: "Clear list",
      },
      file: {
        run: "Compress this file",
        rerun: "Compress again at the selected level",
        retry: "Try again",
        queued: "Waiting",
        running: "compressing…",
        notSmaller: "already optimal",
        failed: "failed",
        reveal: "Show in Finder",
        revealExplorer: "Show in Explorer",
        remove: "Remove from the list",
      },
      install: {
        button: "Install the Finder service and command-line tool…",
        installing: "Installing…",
        serviceLabel: "Finder service",
        serviceOk: "Finder service installed — right-click a PDF › Services › PdfShrinker",
        cliLabel: "Command-line tool",
        cliOk: "command-line tool installed ({path})",
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
        subtitle: "Plus léger, et toujours aussi beau.",
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
        extreme: "Extrême",
        extremeMax: "Extrême max",
      },
      levelHint: {
        lossless: "Images intactes ; objets et polices en double fusionnés. Aucun changement visible.",
        low: "Images réencodées en haute qualité (JPEG jusqu'à 85), réduites à 200 dpi au-delà de 250 dpi. Pour l'impression.",
        medium: "Images à 150 dpi, qualité JPEG ajustée image par image (jusqu'à 75). Pour l'écran et l'impression bureautique.",
        high: "Images à 96 dpi, qualité JPEG ajustée image par image pour rester fidèle. Pour l'écran.",
        extreme: "Limite aussi les images de slides, recadre les images à la page et blanchit le papier des scans. Plus lent.",
        extremeMax: "Fichier le plus petit : 72 dpi, qualité réduite. Dégradation visible en zoomant.",
      },
      actions: {
        compress: "Tout compresser",
        clear: "Effacer la liste",
      },
      file: {
        run: "Compresser ce fichier",
        rerun: "Recompresser au niveau choisi",
        retry: "Réessayer",
        queued: "En attente",
        running: "compression…",
        notSmaller: "déjà optimal",
        failed: "échec",
        reveal: "Afficher dans le Finder",
        revealExplorer: "Afficher dans l'Explorateur",
        remove: "Retirer de la liste",
      },
      install: {
        button: "Installer le service Finder et l'outil en ligne de commande…",
        installing: "Installation…",
        serviceLabel: "Service Finder",
        serviceOk: "Service Finder installé — clic droit sur un PDF › Services › PdfShrinker",
        cliLabel: "Outil en ligne de commande",
        cliOk: "outil en ligne de commande installé ({path})",
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
