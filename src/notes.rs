//! Notes stored as Markdown files in a folder.

use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// File extension that marks a file as a note.
const EXTENSION: &str = "md";
/// Title a note carries until it is given one.
const UNTITLED: &str = "Untitled";
/// Characters a title cannot contain, as Windows forbids them in file names.
const FORBIDDEN: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

pub fn disk_text(path: &Path) -> io::Result<Option<String>> {
    match read(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn write_if_unchanged(path: &Path, expected: Option<&str>, text: &str) -> io::Result<bool> {
    if disk_text(path)?.as_deref() != expected {
        return Ok(false);
    }
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "the note has no parent folder")
    })?;
    let (temporary, mut file) = loop {
        let number = NEXT_WRITE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".wevren-{}-{number}.tmp", std::process::id()));
        match File::create_new(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    };
    let written = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if disk_text(path)?.as_deref() != expected {
            return Ok(false);
        }
        fs::rename(&temporary, path)?;
        Ok(true)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(temporary);
    }
    written
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub directory: bool,
}

pub fn scan(folder: &Path) -> io::Result<Vec<Entry>> {
    let mut children = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        let directory = kind.is_dir();
        if kind.is_symlink()
            || (directory
                && matches!(
                    entry.file_name().to_str(),
                    Some(".git" | ".wevren" | ".trash")
                ))
        {
            continue;
        }
        if directory
            || (kind.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(EXTENSION)))
        {
            children.push(Entry { path, directory });
        }
    }
    children.sort_by_cached_key(|entry| {
        (
            !entry.directory,
            entry
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
        )
    });
    let mut entries = Vec::new();
    for entry in children {
        let nested = if entry.directory {
            scan(&entry.path)?
        } else {
            Vec::new()
        };
        entries.push(entry);
        entries.extend(nested);
    }
    Ok(entries)
}

pub fn create_folder(parent: &Path, name: &str) -> io::Result<PathBuf> {
    validate_title(name)?;
    let path = parent.join(name);
    fs::create_dir(&path)?;
    Ok(path)
}

pub fn link_title(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .with_extension("")
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub fn resolve(root: &Path, current: &Path, title: &str, notes: &[PathBuf]) -> io::Result<PathBuf> {
    if let Some(path) = find(root, current, title, notes) {
        return Ok(path.to_path_buf());
    }
    let normalized = title.replace('\\', "/");
    if normalized.contains('/') {
        let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
        let mut parts: Vec<_> = normalized.split('/').collect();
        let name = parts.pop().unwrap_or_default();
        validate_title(name)?;
        let mut parent = root.to_path_buf();
        for part in parts {
            validate_title(part)?;
            parent.push(part);
        }
        if !fs::canonicalize(&parent)?.starts_with(fs::canonicalize(root)?) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the link must stay in the notes folder",
            ));
        }
        return create(&parent, name);
    }
    let matching: Vec<_> = notes
        .iter()
        .filter(|path| self::title(path).to_lowercase() == title.to_lowercase())
        .collect();
    if let Some(local) = matching.iter().find(|path| path.parent() == Some(current)) {
        return Ok((*local).clone());
    }
    match matching.as_slice() {
        [] => create(current, title),
        [path] => Ok((*path).clone()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "more than one note has that title; use a folder/Note link",
        )),
    }
}

pub fn find<'a>(
    root: &Path,
    current: &Path,
    target: &str,
    notes: &'a [PathBuf],
) -> Option<&'a Path> {
    let normalized = target.replace('\\', "/");
    if normalized.contains('/') {
        let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
        return notes
            .iter()
            .find(|path| link_title(path, root).eq_ignore_ascii_case(normalized))
            .map(PathBuf::as_path);
    }
    let mut matching = notes
        .iter()
        .filter(|path| title(path).to_lowercase() == target.to_lowercase());
    if let Some(local) = matching.clone().find(|path| path.parent() == Some(current)) {
        return Some(local);
    }
    let first = matching.next()?;
    matching.next().is_none().then_some(first.as_path())
}

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
    rename_to(path, &target)?;
    Ok(target)
}

pub fn rename_to(path: &Path, target: &Path) -> io::Result<()> {
    if target == path {
        return Ok(());
    }
    if cfg!(windows) && target.exists() && fs::canonicalize(path)? == fs::canonicalize(target)? {
        fs::rename(path, target)?;
        return Ok(());
    }
    fs::hard_link(path, target)?;
    if let Err(error) = fs::remove_file(path) {
        let _ = fs::remove_file(target);
        return Err(error);
    }
    Ok(())
}

/// Creates an empty note in `folder` and returns its path.
///
/// The note is titled "Untitled", with a number added when that is taken.
///
/// # Errors
///
/// Fails when the file cannot be created.
pub fn create_untitled(folder: &Path) -> io::Result<PathBuf> {
    create_unique(folder, UNTITLED)
}

pub fn create_unique(folder: &Path, base: &str) -> io::Result<PathBuf> {
    let mut number = 1;
    loop {
        let title = match number {
            1 => base.to_owned(),
            _ => format!("{base} {number}"),
        };
        match create(folder, &title) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => number += 1,
            created => return created,
        }
    }
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
