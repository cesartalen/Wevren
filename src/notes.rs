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
/// Characters a title cannot contain, as Windows forbids them in file names.
const FORBIDDEN: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Creates an empty note titled `title` in `folder` and returns its path.
///
/// # Errors
///
/// Fails when the title cannot be a file name, is taken, or the file cannot be created.
pub fn create(folder: &Path, title: &str) -> io::Result<PathBuf> {
    validate_title(title)?;
    let path = folder.join(format!("{title}.{EXTENSION}"));
    File::create_new(&path).map(|_| path)
}

fn validate_title(title: &str) -> io::Result<()> {
    let stem = title.split('.').next().unwrap_or_default().to_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
        });
    if title.trim().is_empty()
        || title.contains(FORBIDDEN)
        || title.chars().any(char::is_control)
        || title.ends_with(['.', ' '])
        || reserved
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("\"{title}\" cannot be the title of a note"),
        ));
    }
    Ok(())
}

pub fn rename(path: &Path, title: &str) -> io::Result<PathBuf> {
    validate_title(title)?;
    let target = path.with_file_name(format!("{title}.{EXTENSION}"));
    if target == path {
        return Ok(target);
    }
    if cfg!(windows) && target.exists() && fs::canonicalize(path)? == fs::canonicalize(&target)? {
        fs::rename(path, &target)?;
        return Ok(target);
    }
    fs::hard_link(path, &target)?;
    if let Err(error) = fs::remove_file(path) {
        let _ = fs::remove_file(&target);
        return Err(error);
    }
    Ok(target)
}

/// Creates an empty note in `folder` and returns its path.
///
/// The note is titled "Untitled", with a number added when that is taken.
///
/// # Errors
///
/// Fails when the file cannot be created.
pub fn create_untitled(folder: &Path) -> io::Result<PathBuf> {
    let mut number = 1;
    loop {
        let title = match number {
            1 => UNTITLED.to_owned(),
            _ => format!("{UNTITLED} {number}"),
        };
        match create(folder, &title) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => number += 1,
            created => return created,
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_FOLDER: AtomicUsize = AtomicUsize::new(0);

    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            let number = NEXT_FOLDER.fetch_add(1, Ordering::Relaxed);
            let folder = std::env::temp_dir()
                .join(format!("wevren-notes-test-{}-{number}", std::process::id()));
            fs::create_dir(&folder).unwrap();
            Self(folder)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rename_preserves_contents_and_updates_the_listing() {
        let folder = Folder::new();
        let source = create(&folder.0, "Untitled").unwrap();
        fs::write(&source, "# Café\n🧶").unwrap();
        let target = rename(&source, "Ideas").unwrap();
        assert!(!source.exists());
        assert_eq!(read(&target).unwrap(), "# Café\n🧶");
        assert_eq!(list(&folder.0).unwrap(), vec![target]);
    }

    #[test]
    fn rename_never_overwrites_an_existing_note() {
        let folder = Folder::new();
        let source = create(&folder.0, "First").unwrap();
        let target = create(&folder.0, "Second").unwrap();
        fs::write(&source, "first").unwrap();
        fs::write(&target, "second").unwrap();
        rename(&source, "Second").unwrap_err();
        assert_eq!(read(&source).unwrap(), "first");
        assert_eq!(read(&target).unwrap(), "second");
    }

    #[test]
    fn rename_allows_same_title_and_case_changes() {
        let folder = Folder::new();
        let source = create(&folder.0, "Ideas").unwrap();
        fs::write(&source, "body").unwrap();
        assert_eq!(rename(&source, "Ideas").unwrap(), source);
        let target = rename(&source, "ideas").unwrap();
        assert_eq!(target.file_name().unwrap(), "ideas.md");
        assert_eq!(read(&target).unwrap(), "body");
        assert_eq!(list(&folder.0).unwrap(), vec![target]);
    }

    #[test]
    fn invalid_titles_are_rejected_without_changing_the_note() {
        let folder = Folder::new();
        let source = create(&folder.0, "Original").unwrap();
        for title in [
            "", " ", "../other", "a\\b", "a:b", "a\n", "a.", "a ", "CON", "nul.txt", "COM1", "LPT9",
        ] {
            assert_eq!(
                rename(&source, title).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert!(source.exists());
        }
    }
}
