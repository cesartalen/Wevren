use std::ops::Range;

#[derive(Debug)]
pub struct Completion {
    pub range: Range<usize>,
    pub items: Vec<String>,
    pub selected: usize,
    pub close: bool,
}

pub fn link(text: &str, caret: usize, titles: &[String]) -> Option<Completion> {
    let before = &text[..caret];
    let start = before.rfind("[[")? + 2;
    let query = &text[start..caret];
    if query.contains(['[', ']', '|', '#', '\n']) {
        return None;
    }
    let escaped = text[..start - 2]
        .chars()
        .rev()
        .take_while(|&c| c == '\\')
        .count();
    if !escaped.is_multiple_of(2) {
        return None;
    }
    let query = query.to_lowercase();
    let mut items: Vec<_> = titles
        .iter()
        .filter(|title| title.to_lowercase().contains(&query))
        .cloned()
        .collect();
    items.sort_by_cached_key(|title| {
        let title = title.to_lowercase();
        (!title.starts_with(&query), title)
    });
    items.truncate(7);
    if items.is_empty() {
        return None;
    }
    let suffix = &text[caret..];
    let end = suffix
        .find(['[', ']', '|', '#', '\n'])
        .filter(|&index| suffix[index..].starts_with([']', '|', '#']))
        .map_or(caret, |index| caret + index);
    let close = !text[end..].starts_with(['|', '#']);
    let end = if text[end..].starts_with("]]") {
        end + 2
    } else if text[end..].starts_with(']') {
        end + 1
    } else {
        end
    };
    Some(Completion {
        range: start..end,
        items,
        selected: 0,
        close,
    })
}
