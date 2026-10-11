use std::{
    fs::{self, File},
    io::{self, Write},
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{panes::Layout, settings};
use serde::{Deserialize, Deserializer, Serialize};

static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    pub selection: Range<usize>,
    pub reversed: bool,
    pub scroll: [f32; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    pub path: PathBuf,
    #[serde(default)]
    pub view: View,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub folder: PathBuf,
    pub tabs: Vec<SavedTab>,
    #[serde(skip_serializing)]
    pub panes: [Option<PathBuf>; 2],
    #[serde(skip_serializing)]
    pub split: bool,
    #[serde(skip_serializing)]
    pub active: usize,
    #[serde(deserialize_with = "layout_or_legacy")]
    pub layout: Option<Layout>,
    pub expanded: Vec<PathBuf>,
    pub selected_folder: Option<PathBuf>,
}

impl Session {
    pub fn load(root: &Path) -> io::Result<Option<Self>> {
        let Some(directory) = settings::dir() else {
            return Ok(None);
        };
        let text = match fs::read_to_string(directory.join("session.json")) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let session: Self = serde_json::from_str(&text).map_err(io::Error::other)?;
        let matches = session.folder == root
            || fs::canonicalize(&session.folder)
                .ok()
                .zip(fs::canonicalize(root).ok())
                .is_some_and(|(saved, current)| saved == current);
        Ok(matches.then_some(session))
    }

    pub fn sanitize(&mut self, notes: &[PathBuf], folders: &[PathBuf]) {
        self.tabs.retain_mut(|tab| {
            if let Some(path) = existing(&tab.path, notes) {
                tab.path = path;
                true
            } else {
                false
            }
        });
        let mut seen = std::collections::HashSet::new();
        self.tabs.retain(|tab| seen.insert(tab.path.clone()));
        let open: Vec<_> = self.tabs.iter().map(|tab| tab.path.clone()).collect();
        let legacy = self.layout.is_none();
        let mut layout = self
            .layout
            .take()
            .unwrap_or_else(|| Layout::legacy(self.panes.clone(), self.split, self.active));
        layout.sanitize(|path| existing(path, &open));
        if legacy && layout.leaves().len() == 1 && layout.path(layout.active).is_none() {
            layout.set_path(layout.active, open.first().cloned());
        }
        self.layout = Some(layout);
        self.expanded = self
            .expanded
            .iter()
            .filter_map(|path| existing(path, folders))
            .collect();
        self.selected_folder = self
            .selected_folder
            .as_ref()
            .and_then(|path| existing(path, folders));
    }

    pub fn save(&self) -> io::Result<()> {
        let directory = settings::dir().ok_or_else(|| {
            io::Error::other("There is no configuration folder for session restore.")
        })?;
        fs::create_dir_all(&directory)?;
        let temporary = directory.join(format!(
            ".session-{}-{}.tmp",
            std::process::id(),
            NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
        ));
        let text = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let mut file = File::create_new(&temporary)?;
        let written = (|| {
            file.write_all(&text)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, directory.join("session.json"))
        })();
        if temporary.exists() {
            let _ = fs::remove_file(temporary);
        }
        written
    }
}

fn layout_or_legacy<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Layout>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

fn existing(path: &Path, allowed: &[PathBuf]) -> Option<PathBuf> {
    if let Some(path) = allowed.iter().find(|candidate| candidate.as_path() == path) {
        return Some(path.clone());
    }
    let canonical = fs::canonicalize(path).ok()?;
    allowed
        .iter()
        .find(|candidate| fs::canonicalize(candidate).ok().as_ref() == Some(&canonical))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::Edge;

    #[test]
    fn legacy_session_preserves_both_notes_and_active_pane() {
        let mut saved: Session = serde_json::from_str(r#"{"folder":"notes","tabs":[{"path":"a.md"},{"path":"b.md"}],"panes":["a.md","b.md"],"split":true,"active":1}"#).unwrap();
        saved.sanitize(&["a.md".into(), "b.md".into()], &[]);
        let layout = saved.layout.as_ref().unwrap();
        assert_eq!(layout.leaves().len(), 2);
        assert_eq!(layout.path(layout.active), Some(&PathBuf::from("b.md")));
        let roundtrip: Session =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(roundtrip.layout, saved.layout);
    }

    #[test]
    fn closed_last_pane_stays_empty_without_losing_open_tabs() {
        let mut layout = Layout::default();
        layout.set_path(0, Some("a.md".into()));
        layout.close(0);
        let mut saved = Session {
            tabs: vec![SavedTab {
                path: "a.md".into(),
                view: View::default(),
            }],
            layout: Some(layout),
            ..Session::default()
        };
        saved = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        saved.sanitize(&["a.md".into()], &[]);
        assert_eq!(saved.tabs.len(), 1);
        let layout = saved.layout.unwrap();
        assert!(layout.path(layout.active).is_none());
    }

    #[test]
    fn malformed_layout_falls_back_and_missing_paths_collapse_safely() {
        let mut saved: Session = serde_json::from_str(
            r#"{"tabs":[{"path":"a.md"}],"layout":{"root":{"kind":"unknown"},"active":99}}"#,
        )
        .unwrap();
        saved.sanitize(&["a.md".into()], &[]);
        assert_eq!(saved.tabs.len(), 1);
        assert_eq!(
            saved.layout.as_ref().unwrap().path(0),
            Some(&PathBuf::from("a.md"))
        );
        let layout = saved.layout.as_mut().unwrap();
        let pane = layout.split(0, Edge::Below).unwrap();
        layout.set_path(pane, Some("missing.md".into()));
        layout.active = pane;
        saved.sanitize(&["a.md".into()], &[]);
        let layout = saved.layout.unwrap();
        assert_eq!(layout.leaves().len(), 1);
        assert_eq!(layout.path(layout.active), Some(&PathBuf::from("a.md")));
    }
}
