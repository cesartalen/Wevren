//! Settings persisted between sessions.

use std::{fs, path::PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// Name of the settings file inside the configuration folder.
const FILE: &str = "settings.json";

/// User choices that outlive a session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Folder the notes are kept in, unset until the user picks one.
    pub folder: Option<PathBuf>,
}

impl Settings {
    /// Returns the saved settings, or the defaults when none can be read.
    #[must_use]
    pub fn load() -> Self {
        dir()
            .and_then(|dir| fs::read_to_string(dir.join(FILE)).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the settings to the configuration folder.
    ///
    /// # Errors
    ///
    /// Fails when there is no configuration folder or it cannot be written to.
    pub fn save(&self) -> Result<()> {
        let dir = dir().context("there is no configuration folder")?;
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(FILE), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Returns the folder Wevren keeps its own files in.
fn dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("wevren"))
}
