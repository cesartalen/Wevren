//! Settings persisted between sessions.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

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
    #[serde(default)]
    pub pinned_notes: BTreeSet<PathBuf>,
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
        self.save_to(&dir.join(FILE))
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn set_pinned(&mut self, path: PathBuf, pinned: bool) {
        if pinned {
            self.pinned_notes.insert(path);
        } else {
            self.pinned_notes.remove(&path);
        }
    }

    pub fn relocate_pin(&mut self, old: &Path, new: &Path) -> bool {
        if old != new && self.pinned_notes.remove(old) {
            self.pinned_notes.insert(new.to_path_buf());
            true
        } else {
            false
        }
    }

    pub fn visible_pins<'a>(&self, root: &Path, notes: &'a [PathBuf]) -> Vec<&'a PathBuf> {
        notes
            .iter()
            .filter(|path| {
                path.starts_with(root) && self.pinned_notes.contains(*path) && path.is_file()
            })
            .collect()
    }
}

/// Returns the folder Wevren keeps its own files in.
pub(crate) fn dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("wevren"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::Settings;
    use crate::shortcuts::{Action, Bindings, Chord};

    struct TestFolder(PathBuf);

    impl TestFolder {
        fn new() -> Self {
            let folder = std::env::temp_dir().join(format!(
                "wevren-pins-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&folder).unwrap();
            Self(folder)
        }
    }

    impl Drop for TestFolder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn pins_are_explicit_idempotent_and_persist_without_changing_other_settings() {
        let folder = TestFolder::new();
        let first = folder.0.join("First.md");
        let second = folder.0.join("Second.md");
        let mut settings = Settings {
            folder: Some(folder.0.clone()),
            shortcuts: Bindings::default()
                .changed(Action::Bold, Chord::try_from("Ctrl+J".to_owned()).unwrap())
                .unwrap(),
            ..Settings::default()
        };
        settings.set_pinned(first.clone(), true);
        settings.set_pinned(first.clone(), true);
        settings.set_pinned(second.clone(), true);
        settings.set_pinned(second.clone(), false);
        settings.set_pinned(second, false);
        assert_eq!(settings.pinned_notes, [first.clone()].into());
        let file = folder.0.join("settings.json");
        settings.save_to(&file).unwrap();
        let mut restored: Settings =
            serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
        assert_eq!(restored, settings);
        restored.shortcuts = Bindings::default();
        assert_eq!(restored.folder, settings.folder);
        assert_eq!(restored.pinned_notes, [first].into());
        assert!(settings.save_to(&folder.0).is_err());
        let legacy: Settings = serde_json::from_str(r#"{"folder":"notes"}"#).unwrap();
        assert!(legacy.pinned_notes.is_empty());
    }

    #[test]
    fn visible_pins_follow_workspace_scan_and_hide_missing_files() {
        let folder = TestFolder::new();
        let root = folder.0.join("workspace");
        let nested = root.join("collapsed");
        fs::create_dir_all(&nested).unwrap();
        let pinned = nested.join("Same.md");
        let unpinned = root.join("Same.md");
        let other = folder.0.join("Other.md");
        let missing = root.join("Missing.md");
        for path in [&pinned, &unpinned, &other] {
            fs::write(path, "").unwrap();
        }
        let mut settings = Settings::default();
        for path in [&pinned, &other, &missing, &nested] {
            settings.set_pinned(path.clone(), true);
        }
        let notes = vec![pinned.clone(), unpinned, other.clone(), missing, nested];
        assert_eq!(settings.visible_pins(&root, &notes), vec![&pinned]);
        settings.folder = Some(folder.0.clone());
        assert_eq!(
            settings.visible_pins(&folder.0, &notes),
            vec![&pinned, &other]
        );
        assert!(settings.visible_pins(&root, &[]).is_empty());
        fs::remove_file(&pinned).unwrap();
        assert!(settings.visible_pins(&root, &notes).is_empty());
        assert!(settings.pinned_notes.contains(&other));
    }

    #[test]
    fn relocated_pins_follow_rename_and_move_and_deduplicate() {
        let old = PathBuf::from("workspace/Old.md");
        let renamed = PathBuf::from("workspace/New.md");
        let moved = PathBuf::from("workspace/sub/New.md");
        let mut settings = Settings::default();
        assert!(!settings.relocate_pin(&old, &renamed));
        settings.set_pinned(old.clone(), true);
        assert!(!settings.relocate_pin(&old, &old));
        assert!(settings.relocate_pin(&old, &renamed));
        settings.set_pinned(moved.clone(), true);
        assert!(settings.relocate_pin(&renamed, &moved));
        assert_eq!(settings.pinned_notes, [moved].into());
        assert!(!settings.relocate_pin(&old, &renamed));
    }

    #[test]
    fn shortcut_settings_round_trip_and_restore_preserve_folder() {
        let mut settings = Settings {
            folder: Some("notes".into()),
            shortcuts: Bindings::default()
                .changed(Action::Bold, Chord::try_from("Ctrl+J".to_owned()).unwrap())
                .unwrap(),
            ..Settings::default()
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
