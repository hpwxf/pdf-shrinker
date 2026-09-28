# PdfShrinker

Compresse des PDF en macOS — comme iLovePDF ou UPDF — en réduisant le volume des fichiers tout en
restant au format PDF (pas de zip). Écrit en Rust, Apple Silicon uniquement.

Trois façons de l'utiliser :

- **En ligne de commandes** (`pdfshrink`)
- **Une app macOS** (`PdfShrinker.app`, installable via `.dmg`), qui apparaît dans Finder ›
  clic droit sur un PDF › **Ouvrir avec**
- **Une Action rapide** Finder (clic droit sur un PDF › **Actions rapides** › PdfShrinker), qui
  compresse directement avec le niveau par défaut réglé dans l'app

Quatre niveaux de compression : **Sans perte** (nettoyage structurel uniquement), **Léger**, **Moyen**
(par défaut), **Fort**. Le fichier compressé est écrit à côté de l'original (`nom-compressed.pdf`) ;
l'original n'est jamais modifié, et si le résultat n'est pas plus petit, rien n'est écrit.

## Installation

Télécharger le `.dmg` (voir [Build](#build) pour le produire soi-même), l'ouvrir, glisser
`PdfShrinker.app` dans `Applications`. L'app n'étant pas signée par un compte développeur Apple
(signature ad-hoc), macOS Gatekeeper affichera un avertissement au premier lancement — clic droit sur
l'app › **Ouvrir**.

Au premier lancement, le bouton **« Installer l'Action rapide et l'outil en ligne de commande »**
dans l'app :

- installe l'Action rapide Finder (`~/Library/Services`) ;
- crée le lien `/usr/local/bin/pdfshrink` vers la CLI embarquée dans l'app.

(Équivalent en ligne de commandes : `pdfshrink install --quick-action --cli-link`.)

## Utilisation

### En ligne de commandes

```bash
pdfshrink fichier.pdf                       # niveau et moteur par défaut (configurables)
pdfshrink -l high -e best fichier.pdf        # niveau et moteur explicites
pdfshrink -l medium *.pdf                    # plusieurs fichiers, en parallèle
pdfshrink config get level                   # lire un réglage par défaut
pdfshrink config set level low               # changer un réglage par défaut
```

Niveaux (`-l`) : `lossless`, `low`, `medium`, `high`.
Moteurs (`-e`) : `rust` (intégré, toujours disponible), `gs` (Ghostscript, si installé via Homebrew),
`best` (essaie les deux, garde le plus petit).

Codes de sortie : `0` succès, `1` erreur, `2` au moins un fichier déjà optimal (rien écrit pour
celui-ci).

### App

Glisser des PDF dans la fenêtre (ou clic droit sur un PDF › **Ouvrir avec** › PdfShrinker), choisir un
niveau, **Compresser**. La case **« Définir comme niveau par défaut »** change aussi le comportement
de l'Action rapide, puisque les deux partagent le même réglage.

### Action rapide

Clic droit sur un ou plusieurs PDF dans le Finder › **Actions rapides** › **PdfShrinker**. Compresse
immédiatement avec le niveau par défaut réglé dans l'app, puis affiche une notification macOS.

## Build

Prérequis : Rust (`rustup target add aarch64-apple-darwin`), Xcode Command Line Tools, `cargo-tauri`
(`cargo install tauri-cli --version "^2"`). Ghostscript est optionnel, à l'exécution seulement
(`brew install ghostscript`) — jamais embarqué (licence AGPL incompatible avec une distribution
fermée).

```bash
# CLI seule
cargo build --release -p pdfshrink-cli

# App + .dmg (installateur complet, signature ad-hoc)
./scripts/build-dmg.sh
```

Voir [`CLAUDE.md`](CLAUDE.md) pour l'architecture détaillée (moteurs de compression, calcul du DPI
effectif des images, structure de l'app Tauri, etc.), les commandes de développement (`cargo test`,
`cargo tauri dev`, génération de PDF de test…) et l'état des vérifications.

## Licence

MIT.
