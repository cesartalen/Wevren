//! Notes stored as Markdown files in a folder.

use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

/// File extension that marks a file as a note.
const EXTENSION: &str = "md";
/// Title a note carries until it is given one.
const UNTITLED: &str = "Untitled";

/// Creates an empty note in `folder` and returns its path.
///
/// The note is titled "Untitled", with a number added when that is taken.
///
/// # Errors
///
/// Fails when the file cannot be created.
pub fn create(folder: &Path) -> io::Result<PathBuf> {
    let mut number = 1;
    loop {
        let title = match number {
            1 => UNTITLED.to_owned(),
            _ => format!("{UNTITLED} {number}"),
        };
        let path = folder.join(format!("{title}.{EXTENSION}"));
        match File::create_new(&path) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => number += 1,
            created => return created.map(|_| path),
        }
    }
}

/// Returns the notes in `folder`, sorted by title.
///
/// # Errors
///
/// Fails when the folder cannot be read.
pub fn list(folder: &Path) -> io::Result<Vec<PathBuf>> {
    let mut notes: Vec<_> = fs::read_dir(folder)?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.is_file())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(EXTENSION))
        })
        .collect();
    notes.sort_by_cached_key(|path| title(path).to_lowercase());
    Ok(notes)
}

/// Returns the title of the note at `path`: its file name without the extension.
#[must_use]
pub fn title(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Returns the text of the note at `path` with Unix line endings.
///
/// # Errors
///
/// Fails when the file cannot be read as UTF-8 text.
pub fn read(path: &Path) -> io::Result<String> {
    Ok(fs::read_to_string(path)?.replace("\r\n", "\n"))
}
