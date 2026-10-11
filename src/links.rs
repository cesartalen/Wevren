use std::{
    io,
    ops::Range,
    path::{Path, PathBuf},
};

use crate::{markdown, notes};

#[derive(Debug, PartialEq, Eq)]
pub struct WikiLink {
    pub range: Range<usize>,
    pub target: Range<usize>,
    pub line: usize,
    pub context: Range<usize>,
}

pub fn wiki_links(text: &str) -> Vec<WikiLink> {
    let mut links = Vec::new();
    let mut offset = 0;
    let mut skip_until = 0;
    let mut lines: Vec<_> = text
        .split_inclusive('\n')
        .zip(markdown::styles(text))
        .map(|(line, style)| {
            let start = offset;
            offset += line.len();
            (start, line, style, text.len())
        })
        .collect();
    let mut block_end = text.len();
    for (start, line, style, span_end) in lines.iter_mut().rev() {
        if style.code || style.heading.is_some() || line.trim().is_empty() {
            *span_end = *start + line.len();
            block_end = *start;
        } else {
            *span_end = block_end;
        }
    }
    for (number, (start, line, style, span_end)) in lines.into_iter().enumerate() {
        offset = start + line.len();
        if style.code {
            continue;
        }
        let mut index = start.max(skip_until);
        while index < offset {
            let rest = &text[index..offset];
            if rest.starts_with('\\') {
                index += 1;
                if let Some(next) = text[index..offset].chars().next() {
                    index += next.len_utf8();
                }
            } else if rest.starts_with('`') {
                let count = rest.bytes().take_while(|&byte| byte == b'`').count();
                index += count;
                if let Some(close) = code_end(&text[index..span_end], count) {
                    index += close;
                    skip_until = index;
                }
            } else if rest.starts_with("[[")
                && let Some(close) = link_end(rest)
                && !rest[2..close].contains(['[', '\n', '\r'])
            {
                let target = rest[2..close].split(['|', '#']).next().unwrap_or_default();
                let trimmed = target.trim();
                let target_start = index + 2 + target.len() - target.trim_start().len();
                links.push(WikiLink {
                    range: index..index + close + 2,
                    target: target_start..target_start + trimmed.len(),
                    line: number + 1,
                    context: start..offset,
                });
                index += close + 2;
            } else {
                index += rest.chars().next().map_or(1, char::len_utf8);
            }
        }
    }
    links
}

fn link_end(text: &str) -> Option<usize> {
    let mut offset = 2;
    while let Some(close) = text[offset..].find("]]") {
        let close = offset + close;
        let escaped = text[..close]
            .bytes()
            .rev()
            .take_while(|&byte| byte == b'\\')
            .count()
            % 2
            == 1;
        if !escaped {
            return Some(close);
        }
        offset = close + 2;
    }
    None
}

struct Update {
    path: PathBuf,
    before: String,
    after: String,
}

struct Relocation<'a> {
    root: &'a Path,
    old: &'a Path,
    new: &'a Path,
    before: &'a [PathBuf],
    after: &'a [PathBuf],
}

impl Relocation<'_> {
    fn target(&self, source: &Path, target: &str) -> Option<String> {
        let parent = source.parent()?;
        let resolved = notes::find(self.root, parent, target, self.before)?;
        let destination = if resolved == self.old {
            self.new
        } else {
            resolved
        };
        let new_parent = if source == self.old {
            self.new.parent()?
        } else {
            parent
        };
        if resolved != self.old
            && notes::find(self.root, new_parent, target, self.after) == Some(destination)
        {
            return None;
        }
        let title = notes::title(destination);
        if !target.contains(['/', '\\'])
            && notes::find(self.root, new_parent, &title, self.after) == Some(destination)
        {
            return Some(title);
        }
        let qualified = notes::link_title(destination, self.root);
        Some(if qualified.contains('/') {
            qualified
        } else {
            format!("./{qualified}")
        })
    }

    fn rewrite(&self, source: &Path, text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut copied = 0;
        for link in wiki_links(text) {
            if let Some(replacement) = self.target(source, &text[link.target.clone()]) {
                result.push_str(&text[copied..link.target.start]);
                result.push_str(&replacement);
                copied = link.target.end;
            }
        }
        result.push_str(&text[copied..]);
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
    let new = notes::rename_target(old, title)?;
    relocate(root, old, &new, baseline)
}

pub fn move_note(
    root: &Path,
    old: &Path,
    folder: &Path,
    baseline: &str,
) -> io::Result<(PathBuf, String)> {
    if !folder.is_dir() || !std::fs::canonicalize(folder)?.starts_with(std::fs::canonicalize(root)?)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Choose a folder inside this workspace.",
        ));
    }
    let name = old
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "The note has no file name."))?;
    relocate(root, old, &folder.join(name), baseline)
}

fn relocate(root: &Path, old: &Path, new: &Path, baseline: &str) -> io::Result<(PathBuf, String)> {
    let before: Vec<_> = notes::scan(root)?
        .into_iter()
        .filter(|entry| !entry.directory)
        .map(|entry| entry.path)
        .collect();
    if notes::read(old)? != baseline {
        return Err(io::Error::other(
            "The note changed outside Wevren. Resolve the conflict before moving or renaming it.",
        ));
    }
    if new == old {
        return Ok((new.to_path_buf(), baseline.to_owned()));
    }
    let after: Vec<_> = before
        .iter()
        .map(|path| {
            if path == old {
                new.to_path_buf()
            } else {
                path.clone()
            }
        })
        .collect();
    let relocation = Relocation {
        root,
        old,
        new,
        before: &before,
        after: &after,
    };
    let mut updates = Vec::new();
    let mut updated_text = baseline.to_owned();
    for path in &before {
        let text = notes::read(path)?;
        if path == old && text != baseline {
            return Err(io::Error::other(
                "The note changed outside Wevren. Resolve the conflict before moving or renaming it.",
            ));
        }
        let rewritten = relocation.rewrite(path, &text);
        if path == old {
            updated_text.clone_from(&rewritten);
        }
        if rewritten != text || path == old {
            updates.push(Update {
                path: if path == old {
                    new.to_path_buf()
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
                "{} changed outside Wevren; operation cancelled",
                source.display()
            )));
        }
    }
    notes::rename_to(old, new)?;
    if let Err((applied, error)) = apply(&updates) {
        return Err(rollback(&updates[..applied], new, old, &error));
    }
    Ok((new.to_path_buf(), updated_text))
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
        format!("Note change cancelled; original links restored. {error}")
    } else {
        format!(
            "Note change interrupted: {error}. Some files changed again and could not be restored: {}",
            failures.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_links_exclude_code_and_escapes_and_preserve_utf8_crlf_ranges() {
        let text = "é [[ Target#heading|alias ]]\r\n\\[[escaped]] \\é [[visible]]\r\n`[[inline]]` ``[[inline too]]``\r\n~~~rust\r\n[[fenced]]\r\n~~~\r\n    [[indented]]\r\n\t[[tab]]\r\n`multiline\r\n[[hidden]]` [[last]]\r\n";
        let links = wiki_links(text);
        assert_eq!(
            links
                .iter()
                .map(|link| &text[link.target.clone()])
                .collect::<Vec<_>>(),
            vec!["Target", "visible", "last"]
        );
        assert_eq!(links[0].range, 3..29);
        assert_eq!(links[0].line, 1);
        assert_eq!(&text[links[0].range.clone()], "[[ Target#heading|alias ]]");
        assert_eq!(links[2].line, 10);
    }

    #[test]
    fn unmatched_backticks_do_not_hide_links_across_paragraph_or_code_boundaries() {
        let text = "literal ` tick\n\n[[Target]] later ` tick\n\nmore ` text\n~~~rust\n` [[hidden]]\n~~~\n[[visible]]\n\nlast ` literal\n    ` [[indented]]\n[[after indent]]\n";
        assert_eq!(
            wiki_links(text)
                .iter()
                .map(|link| &text[link.target.clone()])
                .collect::<Vec<_>>(),
            vec!["Target", "visible", "after indent"]
        );
        assert_eq!(
            wiki_links("`same\n[[hidden]]` [[visible]]")
                .iter()
                .map(|link| &"`same\n[[hidden]]` [[visible]]"[link.target.clone()])
                .collect::<Vec<_>>(),
            vec!["visible"]
        );
    }

    #[test]
    fn escaped_opening_and_closing_markers_are_not_links() {
        let text = "\\[[Target]] [[Target|label\\]]\n\\\\[[Target]] [[Target|label\\\\]]";
        assert_eq!(
            wiki_links(text)
                .iter()
                .map(|link| &text[link.range.clone()])
                .collect::<Vec<_>>(),
            vec!["[[Target]]", "[[Target|label\\\\]]"]
        );
    }

    #[test]
    fn relocation_uses_the_same_link_parser_and_preserves_aliases_fragments() {
        let root = Path::new("workspace");
        let old = root.join("Target.md");
        let new = root.join("Renamed.md");
        let source = root.join("Source.md");
        let before = vec![old.clone(), source.clone()];
        let after = vec![new.clone(), source.clone()];
        let relocation = Relocation {
            root,
            old: &old,
            new: &new,
            before: &before,
            after: &after,
        };
        assert_eq!(
            relocation.rewrite(
                &source,
                "[[ Target#h|label ]] `[[Target]]`\n    [[Target]]\n\\[[Target]]"
            ),
            "[[ Renamed#h|label ]] `[[Target]]`\n    [[Target]]\n\\[[Target]]"
        );
    }
}
