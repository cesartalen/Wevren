use std::ops::Range;

pub fn matches(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut folded = String::new();
    let mut offsets = Vec::new();
    for (start, character) in text.char_indices() {
        let end = start + character.len_utf8();
        for lower in character.to_lowercase() {
            folded.push(lower);
            offsets.extend(std::iter::repeat_n(start..end, lower.len_utf8()));
        }
    }
    let query = query.to_lowercase();
    let mut matches: Vec<_> = folded
        .match_indices(&query)
        .map(|(start, matched)| offsets[start].start..offsets[start + matched.len() - 1].end)
        .collect();
    matches.dedup();
    matches
}
