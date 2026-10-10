use std::ops::Range;

pub fn fuzzy_score(title: &str, query: &str) -> Option<i64> {
    let title = title.to_lowercase();
    let characters: Vec<_> = title.chars().collect();
    let mut score = 0;
    for token in query.to_lowercase().split_whitespace() {
        let mut from = 0;
        let mut previous = None;
        for wanted in token.chars() {
            let index = from
                + characters[from..]
                    .iter()
                    .position(|&character| character == wanted)?;
            score += 10;
            if index == 0 || !characters[index - 1].is_alphanumeric() {
                score += 15;
            }
            if previous.is_some_and(|previous| previous + 1 == index) {
                score += 20;
            }
            score -= i64::try_from(index - from).unwrap_or(i64::MAX);
            previous = Some(index);
            from = index + 1;
        }
    }
    if title == query.trim().to_lowercase() {
        score += 100;
    }
    Some(score)
}

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
