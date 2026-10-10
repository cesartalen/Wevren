use std::{ops::Range, path::PathBuf};

use crate::{notes, search};

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub line: usize,
    pub snippet: String,
    pub range: Range<usize>,
}

#[derive(Debug, Default)]
pub struct Results {
    pub hits: Vec<Hit>,
    pub skipped: usize,
    pub limited: bool,
}

impl Results {
    pub fn summary(&self) -> String {
        format!(
            "{} matching lines{}{}",
            self.hits.len(),
            if self.limited {
                " (first 200 shown)"
            } else {
                ""
            },
            if self.skipped > 0 {
                format!("; {} unreadable notes skipped", self.skipped)
            } else {
                String::new()
            }
        )
    }
}

pub fn find(paths: &[PathBuf], query: &str, open: &[(PathBuf, String)]) -> Results {
    let mut results = Results::default();
    if query.is_empty() {
        return results;
    }
    for path in paths {
        let text = if let Some((_, text)) = open.iter().find(|(current, _)| current == path) {
            text.clone()
        } else if let Ok(text) = notes::read(path) {
            text
        } else {
            results.skipped += 1;
            continue;
        };
        let mut offset = 0;
        for (line, text) in text.split_inclusive('\n').enumerate() {
            if let Some(range) = search::matches(text, query).into_iter().next() {
                if results.hits.len() == 200 {
                    results.limited = true;
                    return results;
                }
                results.hits.push(Hit {
                    path: path.clone(),
                    line: line + 1,
                    snippet: text.trim_end().to_owned(),
                    range: offset + range.start..offset + range.end,
                });
            }
            offset += text.len();
        }
    }
    results
}
