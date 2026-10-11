//! Settings persisted between sessions.

use std::{fs, path::PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::shortcuts::Bindings;

/// Name of the settings file inside the configuration folder.
const FILE: &str = "settings.json";

/// User choices that outlive a session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Folder the notes are kept in, unset until the user picks one.
    pub folder: Option<PathBuf>,
    #[serde(default)]
    pub shortcuts: Bindings,
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
pub(crate) fn dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("wevren"))
}

#[cfg(test)]
mod tests {
    use super::Settings;
    use crate::shortcuts::{Action, Bindings, Chord};

    #[test]
    fn shortcut_settings_round_trip_and_restore_preserve_folder() {
        let mut settings = Settings {
            folder: Some("notes".into()),
            shortcuts: Bindings::default()
                .changed(Action::Bold, Chord::try_from("Ctrl+J".to_owned()).unwrap())
                .unwrap(),
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("Ctrl+J"));
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), settings);
        settings.shortcuts = Bindings::default();
        assert_eq!(settings.folder, Some("notes".into()));
        let restored =
            serde_json::from_str::<Settings>(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored.shortcuts, Bindings::default());
    }

    #[test]
    fn legacy_and_invalid_shortcuts_keep_folder_and_safe_defaults() {
        for json in [
            r#"{"folder":"notes"}"#,
            r#"{"folder":"notes","shortcuts":null}"#,
            r#"{"folder":"notes","shortcuts":{"bold":"Ctrl+Alt+B"}}"#,
            r#"{"folder":"notes","shortcuts":{"bold":"Ctrl+S"}}"#,
            r#"{"folder":"notes","shortcuts":{"bold":"Ctrl+C"}}"#,
            r#"{"folder":"notes","shortcuts":{"bold":"Shift+B"}}"#,
            r#"{"folder":"notes","shortcuts":{"unknown_action":"Ctrl+J"}}"#,
            r#"{"folder":"notes","shortcuts":{"bold":42}}"#,
        ] {
            let settings = serde_json::from_str::<Settings>(json).unwrap();
            assert_eq!(settings.folder, Some("notes".into()));
            assert_eq!(
                settings.shortcuts,
                Bindings::default(),
                "unsafe settings {json}"
            );
        }
        let partial =
            serde_json::from_str::<Settings>(r#"{"folder":"notes","shortcuts":{"bold":"Ctrl+J"}}"#)
                .unwrap();
        assert_eq!(partial.shortcuts.chord(Action::Bold).to_string(), "Ctrl+J");
        assert_eq!(partial.shortcuts.chord(Action::Save).to_string(), "Ctrl+S");
    }
}
