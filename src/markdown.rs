use std::ops::Range;

#[derive(Clone, Debug)]
pub struct Emphasis {
    pub range: Range<usize>,
    pub bold: bool,
}

#[derive(Debug, Default)]
pub struct LineStyle {
    pub code: bool,
    pub heading: Option<u8>,
    pub emphasis: Vec<Emphasis>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct OutlineHeading {
    pub offset: usize,
    pub level: u8,
    pub title: String,
}

#[derive(Debug)]
struct Delimiter {
    start: usize,
    length: usize,
    marker: u8,
    opens: bool,
    closes: bool,
}

#[derive(Clone, Copy, Debug)]
struct Fence {
    marker: u8,
    length: usize,
}

fn unindented(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    (line.len() - trimmed.len() <= 3).then_some(trimmed)
}

fn fence(line: &str) -> Option<(Fence, &str)> {
    let line = unindented(line)?;
    let marker = *line.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let length = line.bytes().take_while(|&byte| byte == marker).count();
    (length >= 3).then_some((Fence { marker, length }, &line[length..]))
}

fn heading(line: &str) -> Option<u8> {
    let line = unindented(line)?;
    let level = line.bytes().take_while(|&byte| byte == b'#').count();
    if !(1..=6).contains(&level) || !matches!(line.as_bytes().get(level), None | Some(b' ' | b'\t'))
    {
        return None;
    }
    u8::try_from(level).ok()
}

pub fn styles(text: &str) -> Vec<LineStyle> {
    let mut open: Option<Fence> = None;
    text.split('\n')
        .map(|line| {
            if let Some(current) = open {
                if let Some((closing, rest)) = fence(line)
                    && closing.marker == current.marker
                    && closing.length >= current.length
                    && rest.trim_matches([' ', '\t', '\r']).is_empty()
                {
                    open = None;
                }
                return LineStyle {
                    code: true,
                    ..LineStyle::default()
                };
            }
            if let Some((opening, rest)) = fence(line)
                && (opening.marker != b'`' || !rest.contains('`'))
            {
                open = Some(opening);
                return LineStyle {
                    code: true,
                    ..LineStyle::default()
                };
            }
            if line.starts_with("    ") || line.starts_with('\t') {
                return LineStyle {
                    code: true,
                    ..LineStyle::default()
                };
            }
            LineStyle {
                code: false,
                heading: heading(line),
                emphasis: emphasis(line),
            }
        })
        .collect()
}

pub fn outline(text: &str) -> Vec<OutlineHeading> {
    let mut offset = 0;
    text.split_inclusive('\n')
        .zip(styles(text))
        .filter_map(|(line, style)| {
            let start = offset;
            offset += line.len();
            let level = style.heading?;
            let title = line.trim_start_matches(' ')[usize::from(level)..].trim();
            let without_hashes = title.trim_end_matches('#');
            let title = if without_hashes.is_empty() || without_hashes.ends_with([' ', '\t']) {
                without_hashes.trim_end()
            } else {
                title
            };
            Some(OutlineHeading {
                offset: start,
                level,
                title: title.to_owned(),
            })
        })
        .collect()
}

fn punctuation(character: char) -> bool {
    !character.is_alphanumeric() && !character.is_whitespace()
}

fn delimiters(text: &str) -> Vec<Delimiter> {
    let mut delimiters = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let byte = text.as_bytes()[index];
        if byte == b'\\' {
            index += 1;
            if let Some(character) = text[index..].chars().next()
                && character.is_ascii_punctuation()
            {
                index += character.len_utf8();
            }
            continue;
        }
        if byte == b'`' {
            let length = text[index..]
                .bytes()
                .take_while(|&byte| byte == b'`')
                .count();
            let mut end = index + length;
            let mut matched = false;
            while let Some(next) = text[end..].find('`') {
                end += next;
                let closing = text[end..].bytes().take_while(|&byte| byte == b'`').count();
                end += closing;
                if closing == length {
                    matched = true;
                    break;
                }
            }
            index = if matched { end } else { index + length };
            continue;
        }
        if !matches!(byte, b'*' | b'_') {
            index += text[index..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        let length = text[index..]
            .bytes()
            .take_while(|&other| other == byte)
            .count();
        let before = text[..index].chars().next_back();
        let after = text[index + length..].chars().next();
        let left = after.is_some_and(|c| !c.is_whitespace())
            && (!after.is_some_and(punctuation)
                || before.is_none_or(|c| c.is_whitespace() || punctuation(c)));
        let right = before.is_some_and(|c| !c.is_whitespace())
            && (!before.is_some_and(punctuation)
                || after.is_none_or(|c| c.is_whitespace() || punctuation(c)));
        delimiters.push(Delimiter {
            start: index,
            length,
            marker: byte,
            opens: left && (byte != b'_' || !right || before.is_some_and(punctuation)),
            closes: right && (byte != b'_' || !left || after.is_some_and(punctuation)),
        });
        index += length;
    }
    delimiters
}

fn emphasis(text: &str) -> Vec<Emphasis> {
    let mut delimiters = delimiters(text);
    let mut spans = Vec::new();
    for closing in 0..delimiters.len() {
        while delimiters[closing].closes && delimiters[closing].length > 0 {
            let end = &delimiters[closing];
            let opening = (0..closing).rev().find(|&index| {
                let start = &delimiters[index];
                start.opens
                    && start.length > 0
                    && start.marker == end.marker
                    && !((start.closes || end.opens)
                        && (start.length + end.length).is_multiple_of(3)
                        && (!start.length.is_multiple_of(3) || !end.length.is_multiple_of(3)))
            });
            let Some(opening) = opening else {
                break;
            };
            let count = if delimiters[opening].length >= 2 && delimiters[closing].length >= 2 {
                2
            } else {
                1
            };
            spans.push(Emphasis {
                range: delimiters[opening].start + delimiters[opening].length
                    ..delimiters[closing].start,
                bold: count == 2,
            });
            delimiters[opening].length -= count;
            delimiters[closing].start += count;
            delimiters[closing].length -= count;
            for delimiter in &mut delimiters[opening + 1..closing] {
                delimiter.opens = false;
                delimiter.closes = false;
            }
        }
    }
    spans
}

pub fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 1.75,
        2 => 1.5,
        3 => 1.25,
        4 => 1.125,
        5 => 1.0625,
        _ => 1.,
    }
}

#[cfg(test)]
mod tests {
    use super::{OutlineHeading, outline};

    #[test]
    fn outline_keeps_utf8_byte_offsets_and_heading_levels() {
        let text = "é📝\r\n# Café ###\r\n  ### 日本語#\n######\n";
        assert_eq!(
            outline(text),
            vec![
                OutlineHeading {
                    offset: text.find("# Café").unwrap(),
                    level: 1,
                    title: "Café".to_owned(),
                },
                OutlineHeading {
                    offset: text.find("  ###").unwrap(),
                    level: 3,
                    title: "日本語#".to_owned(),
                },
                OutlineHeading {
                    offset: text.find("######").unwrap(),
                    level: 6,
                    title: String::new(),
                },
            ]
        );
    }

    #[test]
    fn outline_ignores_code_and_invalid_heading_markers() {
        let text = "```md\n# fenced\n```\n~~~\n## fenced too\n~~~\n    # indented\n\t# tabbed\n#nospace\n####### too many\n## Visible";
        let headings = outline(text);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].title, "Visible");
        assert_eq!(headings[0].offset, text.find("## Visible").unwrap());
        assert!(outline("").is_empty());
        assert!(outline("Plain text\n").is_empty());
    }
}
