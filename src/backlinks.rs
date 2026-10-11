use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{links, notes};

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub line: usize,
    pub snippet: String,
    pub range: Range<usize>,
    markup: String,
    context: String,
}

#[derive(Debug, Default)]
pub struct Results {
    pub hits: Vec<Hit>,
    pub skipped: usize,
    pub limited: bool,
}

#[derive(Debug, Default)]
pub struct Panel {
    pub root: Option<PathBuf>,
    pub target: Option<PathBuf>,
    pub results: Results,
    pub busy: bool,
    pub loaded: bool,
    pub generation: u64,
    pub cancellation: Option<Arc<AtomicBool>>,
}

impl Panel {
    pub fn accepts(&self, root: &Path, target: &Path, generation: u64) -> bool {
        self.root.as_deref() == Some(root)
            && self.target.as_deref() == Some(target)
            && self.generation == generation
    }
}

pub fn find(
    root: &Path,
    target: &Path,
    paths: &[PathBuf],
    open: &[(PathBuf, String)],
    cancelled: &AtomicBool,
) -> Results {
    let mut results = Results::default();
    for path in paths.iter().filter(|path| path.as_path() != target) {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let disk;
        let text = if let Some((_, text)) = open.iter().find(|(source, _)| source == path) {
            text.as_str()
        } else if let Ok(text) = notes::read(path) {
            disk = text;
            &disk
        } else {
            results.skipped += 1;
            continue;
        };
        for link in links::wiki_links(text) {
            if cancelled.load(Ordering::Relaxed) {
                return results;
            }
            if path
                .parent()
                .and_then(|parent| notes::find(root, parent, &text[link.target], paths))
                != Some(target)
            {
                continue;
            }
            if results.hits.len() == 200 {
                results.limited = true;
                return results;
            }
            let context = text[link.context.clone()].trim_end_matches(['\r', '\n']);
            results.hits.push(Hit {
                path: path.clone(),
                line: link.line,
                snippet: snippet(text, link.context, link.range.start),
                markup: text[link.range.clone()].to_owned(),
                context: context.to_owned(),
                range: link.range,
            });
        }
    }
    results
}

fn snippet(text: &str, line: Range<usize>, link_start: usize) -> String {
    let before = &text[line.start..link_start];
    let start = before
        .char_indices()
        .rev()
        .nth(60)
        .map_or(line.start, |(offset, _)| line.start + offset);
    let context = text[start..line.end].trim_end_matches(['\r', '\n']);
    let end = context
        .char_indices()
        .nth(180)
        .map_or(context.len(), |(offset, _)| offset);
    format!(
        "{}{}{}",
        if start > line.start { "…" } else { "" },
        &context[..end],
        if end < context.len() { "…" } else { "" }
    )
}

pub fn locate(
    root: &Path,
    target: &Path,
    paths: &[PathBuf],
    hit: &Hit,
    text: &str,
) -> Option<Range<usize>> {
    let parent = hit.path.parent()?;
    let candidates: Vec<_> = links::wiki_links(text)
        .into_iter()
        .filter(|link| {
            text[link.range.clone()] == hit.markup
                && notes::find(root, parent, &text[link.target.clone()], paths) == Some(target)
        })
        .collect();
    if let Some(link) = candidates.iter().find(|link| {
        link.range == hit.range
            && text[link.context.clone()].trim_end_matches(['\r', '\n']) == hit.context
    }) {
        return Some(link.range.clone());
    }
    if candidates.len() == 1 {
        return Some(candidates[0].range.clone());
    }
    let mut matching = candidates
        .iter()
        .filter(|link| text[link.context.clone()].trim_end_matches(['\r', '\n']) == hit.context);
    let first = matching.next()?;
    matching.next().is_none().then(|| first.range.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_local_qualified_ambiguous_alias_and_fragment_links() {
        let root = Path::new("workspace");
        let target = root.join("a/Target.md");
        let duplicate = root.join("b/Target.md");
        let local = root.join("a/Source.md");
        let other = root.join("b/Source.md");
        let ambiguous = root.join("c/Source.md");
        let paths = vec![
            target.clone(),
            duplicate,
            local.clone(),
            other.clone(),
            ambiguous.clone(),
        ];
        let open = vec![
            (target.clone(), "[[Target]]".to_owned()),
            (paths[1].clone(), String::new()),
            (
                local.clone(),
                "[[target|label]] [[Target#heading]]".to_owned(),
            ),
            (
                other.clone(),
                "[[Target]] [[A\\TARGET#heading|alias]]".to_owned(),
            ),
            (ambiguous.clone(), "[[Target]] [[a/Target]]".to_owned()),
        ];
        let hits = find(root, &target, &paths, &open, &AtomicBool::new(false)).hits;
        assert_eq!(hits.len(), 4);
        assert_eq!(
            hits.iter().map(|hit| &hit.path).collect::<Vec<_>>(),
            vec![&local, &local, &other, &ambiguous]
        );
    }

    #[test]
    fn uses_unsaved_snapshots_reports_unreadable_and_relocates_changed_text_safely() {
        let root = Path::new("missing-backlinks-workspace");
        let target = root.join("Target.md");
        let source = root.join("Source.md");
        let paths = vec![target.clone(), source.clone(), root.join("Unreadable.md")];
        let original = "é before [[Target|label]] after\r\n";
        let open = vec![(source, original.to_owned())];
        let results = find(root, &target, &paths, &open, &AtomicBool::new(false));
        assert_eq!(results.skipped, 1);
        let hit = &results.hits[0];
        assert_eq!(&original[hit.range.clone()], "[[Target|label]]");
        assert_eq!(hit.snippet, "é before [[Target|label]] after");
        let changed = format!("new first line\r\n{original}");
        let range = locate(root, &target, &paths, hit, &changed).unwrap();
        assert_eq!(&changed[range], "[[Target|label]]");
        assert!(locate(root, &target, &paths, hit, "`[[Target|label]]`").is_none());
        assert!(
            locate(
                root,
                &target,
                &paths,
                hit,
                "one [[Target|label]]\ntwo [[Target|label]]"
            )
            .is_none()
        );
        assert!(
            find(root, &target, &paths, &open, &AtomicBool::new(true))
                .hits
                .is_empty()
        );
    }

    #[test]
    fn rejects_results_for_previous_target_root_or_generation() {
        let panel = Panel {
            root: Some("workspace".into()),
            target: Some("workspace/Target.md".into()),
            generation: 2,
            ..Panel::default()
        };
        assert!(panel.accepts(Path::new("workspace"), Path::new("workspace/Target.md"), 2));
        assert!(!panel.accepts(Path::new("workspace"), Path::new("workspace/Other.md"), 2));
        assert!(!panel.accepts(Path::new("other"), Path::new("workspace/Target.md"), 2));
        assert!(!panel.accepts(Path::new("workspace"), Path::new("workspace/Target.md"), 1));
    }

    #[test]
    fn unsaved_snapshots_override_existing_disk_content_and_long_snippets_are_utf8_safe() {
        let root = std::env::temp_dir().join(format!(
            "wevren-backlinks-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let target = root.join("Target.md");
        let source = root.join("Source.md");
        std::fs::write(&source, "No links on disk").unwrap();
        let paths = vec![target.clone(), source.clone()];
        assert!(
            find(&root, &target, &paths, &[], &AtomicBool::new(false))
                .hits
                .is_empty()
        );
        let text = format!("{}[[Target]]{}\r\n", "é".repeat(300), "ø".repeat(300));
        let results = find(
            &root,
            &target,
            &paths,
            &[(source.clone(), text)],
            &AtomicBool::new(false),
        );
        std::fs::remove_file(&source).unwrap();
        std::fs::remove_dir(&root).unwrap();
        assert_eq!(results.hits.len(), 1);
        assert!(results.hits[0].snippet.contains("[[Target]]"));
        assert!(results.hits[0].snippet.chars().count() <= 182);
        assert!(results.hits[0].snippet.starts_with('…'));
        assert!(results.hits[0].snippet.ends_with('…'));
    }
}
