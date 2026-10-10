use std::{
    io,
    path::{Path, PathBuf},
};

use crate::{markdown, notes};

struct Update {
    path: PathBuf,
    before: String,
    after: String,
}

struct Rename<'a> {
    root: &'a Path,
    old: &'a Path,
    new: &'a Path,
    before: &'a [PathBuf],
    after: &'a [PathBuf],
}

impl Rename<'_> {
    fn target(&self, source: &Path, target: &str) -> Option<String> {
        let parent = source.parent()?;
        if notes::find(self.root, parent, target, self.before) != Some(self.old) {
            return None;
        }
        let title = notes::title(self.new);
        if !target.contains(['/', '\\'])
            && notes::find(self.root, parent, &title, self.after) == Some(self.new)
        {
            return Some(title);
        }
        let qualified = notes::link_title(self.new, self.root);
        Some(if qualified.contains('/') {
            qualified
        } else {
            format!("./{qualified}")
        })
    }

    fn rewrite(&self, source: &Path, text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        for (line, style) in text.split_inclusive('\n').zip(markdown::styles(text)) {
            if style.code {
                result.push_str(line);
            } else {
                result.push_str(&self.rewrite_line(source, line));
            }
        }
        result
    }

    fn rewrite_line(&self, source: &Path, line: &str) -> String {
        let mut result = String::new();
        let mut copied = 0;
        let mut index = 0;
        while index < line.len() {
            let rest = &line[index..];
            if rest.starts_with('\\') {
                index += 1;
                if let Some(next) = line[index..].chars().next() {
                    index += next.len_utf8();
                }
                continue;
            }
            if rest.starts_with('`') {
                let count = rest.bytes().take_while(|&byte| byte == b'`').count();
                index += count;
                if let Some(close) = code_end(&line[index..], count) {
                    index += close;
                }
                continue;
            }
            if rest.starts_with("[[")
                && let Some(close) = rest.find("]]")
                && !rest[2..close].contains(['[', '\n'])
            {
                let body = &rest[2..close];
                let target = body.split(['|', '#']).next().unwrap_or_default();
                let trimmed = target.trim();
                if let Some(replacement) = self.target(source, trimmed) {
                    let start = index + 2 + target.len() - target.trim_start().len();
                    let end = start + trimmed.len();
                    result.push_str(&line[copied..start]);
                    result.push_str(&replacement);
                    copied = end;
                }
                index += close + 2;
            } else {
                index += rest.chars().next().map_or(1, char::len_utf8);
            }
        }
        result.push_str(&line[copied..]);
        result
    }
}

fn code_end(text: &str, length: usize) -> Option<usize> {
    let mut index = 0;
    while let Some(offset) = text[index..].find('`') {
        index += offset;
        let count = text[index..]
            .bytes()
            .take_while(|&byte| byte == b'`')
            .count();
        index += count;
        if count == length {
            return Some(index);
        }
    }
    None
}

pub fn rename(
    root: &Path,
    old: &Path,
    title: &str,
    baseline: &str,
) -> io::Result<(PathBuf, String)> {
    let before: Vec<_> = notes::scan(root)?
        .into_iter()
        .filter(|entry| !entry.directory)
        .map(|entry| entry.path)
        .collect();
    if notes::read(old)? != baseline {
        return Err(io::Error::other(
            "The note changed outside Wevren. Resolve the conflict before renaming.",
        ));
    }
    let new = old.with_file_name(format!("{title}.md"));
    if new == old {
        return Ok((new, baseline.to_owned()));
    }
    let after: Vec<_> = before
        .iter()
        .map(|path| {
            if path == old {
                new.clone()
            } else {
                path.clone()
            }
        })
        .collect();
    let rename = Rename {
        root,
        old,
        new: &new,
        before: &before,
        after: &after,
    };
    let mut updates = Vec::new();
    let mut renamed_text = baseline.to_owned();
    for path in &before {
        let text = notes::read(path)?;
        if path == old && text != baseline {
            return Err(io::Error::other(
                "The note changed outside Wevren. Resolve the conflict before renaming.",
            ));
        }
        let rewritten = rename.rewrite(path, &text);
        if path == old {
            renamed_text.clone_from(&rewritten);
        }
        if rewritten != text || path == old {
            updates.push(Update {
                path: if path == old {
                    new.clone()
                } else {
                    path.clone()
                },
                before: text,
                after: rewritten,
            });
        }
    }
    for update in &updates {
        let source = if update.path == new {
            old
        } else {
            &update.path
        };
        if notes::disk_text(source)?.as_deref() != Some(&update.before) {
            return Err(io::Error::other(format!(
                "{} changed outside Wevren; rename cancelled",
                source.display()
            )));
        }
    }
    let renamed = notes::rename(old, title)?;
    if let Err((applied, error)) = apply(&updates) {
        return Err(rollback(&updates[..applied], &renamed, old, &error));
    }
    Ok((renamed, renamed_text))
}

fn apply(updates: &[Update]) -> Result<(), (usize, io::Error)> {
    for (index, update) in updates.iter().enumerate() {
        if !notes::write_if_unchanged(&update.path, Some(&update.before), &update.after)
            .map_err(|error| (index, error))?
        {
            return Err((
                index,
                io::Error::other(format!("{} changed outside Wevren", update.path.display())),
            ));
        }
    }
    Ok(())
}

fn rollback(updates: &[Update], renamed: &Path, old: &Path, error: &io::Error) -> io::Error {
    let mut failures = Vec::new();
    for update in updates.iter().rev() {
        match notes::disk_text(&update.path) {
            Ok(Some(text)) if text == update.before => {}
            Ok(Some(text)) if text == update.after => {
                if !matches!(
                    notes::write_if_unchanged(&update.path, Some(&update.after), &update.before),
                    Ok(true)
                ) {
                    failures.push(update.path.display().to_string());
                }
            }
            _ => failures.push(update.path.display().to_string()),
        }
    }
    if let Err(rollback) = notes::rename_to(renamed, old) {
        failures.push(rollback.to_string());
    }
    io::Error::other(if failures.is_empty() {
        format!("Rename cancelled; original links restored. {error}")
    } else {
        format!(
            "Rename interrupted: {error}. Some files changed again and could not be restored: {}",
            failures.join(", ")
        )
    })
}
