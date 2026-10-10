use std::{
    fs::{self, File},
    io::{self, Write},
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::settings;
use serde::{Deserialize, Serialize};

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
    pub panes: [Option<PathBuf>; 2],
    pub split: bool,
    pub active: usize,
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
        let active_had_note = self.panes[self.active.min(1)].is_some();
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
        for path in &mut self.panes {
            *path = path.as_ref().and_then(|path| existing(path, &open));
        }
        if self.split {
            if self.panes[0].is_some() && self.panes[0] == self.panes[1] {
                self.panes[1] = None;
            }
            self.active = self.active.min(1);
            if active_had_note
                && self.panes[self.active].is_none()
                && self.panes[1 - self.active].is_some()
            {
                self.active = 1 - self.active;
            }
        } else {
            self.panes[1] = None;
            if self.panes[0].is_none() {
                self.panes[0] = open.first().cloned();
            }
            self.active = 0;
        }
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
