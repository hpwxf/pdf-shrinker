use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::EngineChoice;
use crate::level::Level;
use crate::{PdfShrinkError, Result};

/// Persisted user preferences, shared by the CLI, the GUI and the Quick Action.
///
/// This is the single source of truth for the "default level" the Quick Action
/// uses: the GUI writes it, the CLI and the Quick Action read it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default_level: Level,
    #[serde(default)]
    pub engine: EngineChoice,
}

impl Config {
    /// Path to `config.toml` under the app's Application Support directory.
    pub fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "haveneer", "pdfshrinker")
            .ok_or_else(|| PdfShrinkError::Config("could not resolve config directory".into()))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load the config, falling back to defaults if it doesn't exist yet or is unreadable.
    pub fn load() -> Config {
        Self::try_load().unwrap_or_default()
    }

    pub fn try_load() -> Result<Config> {
        let path = Self::path()?;
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(PdfShrinkError::Config(format!("reading {path:?}: {e}"))),
        };
        toml::from_str(&text).map_err(|e| PdfShrinkError::Config(format!("parsing {path:?}: {e}")))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| PdfShrinkError::Config(format!("creating {parent:?}: {e}")))?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| PdfShrinkError::Config(format!("serializing config: {e}")))?;
        fs::write(&path, text).map_err(|e| PdfShrinkError::Config(format!("writing {path:?}: {e}")))
    }
}
