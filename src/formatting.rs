use std::ops::Range;

use crate::markdown;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Bold,
    Italic,
    Heading(u8),
    InlineCode,
    FencedCode,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
}

pub fn transform(text: &str, selection: Range<usize>, command: Command) -> Option<Edit> {
    if selection.start > selection.end
        || selection.end > text.len()
        || !text.is_char_boundary(selection.start)
        || !text.is_char_boundary(selection.end)
    {
        return None;
    }
    match command {
        Command::Bold => Some(emphasis(text, selection, 2)),
        Command::Italic => Some(emphasis(text, selection, 1)),
        Command::Heading(level) if (1..=6).contains(&level) => heading(text, selection, level),
        Command::Heading(_) => None,
        Command::InlineCode => Some(inline_code(text, selection)),
        Command::FencedCode => Some(fenced_code(text, selection)),
    }
}

fn wrap(text: &str, range: Range<usize>, prefix: &str, suffix: &str) -> Edit {
    let start = range.start + prefix.len();
    let end = start + range.len();
    Edit {
        text: format!("{prefix}{}{suffix}", &text[range.clone()]),
        range,
        selection: start..end,
    }
}

fn unwrap(text: &str, range: Range<usize>, prefix: usize, suffix: usize) -> Edit {
    let content = &text[range.start + prefix..range.end - suffix];
    let start = range.start;
    Edit {
        range,
        text: content.to_owned(),
        selection: start..start + content.len(),
    }
}

fn run_start(text: &str, marker: char) -> usize {
    text.bytes()
        .take_while(|&byte| byte == marker as u8)
        .count()
}

fn run_end(text: &str, marker: char) -> usize {
    text.bytes()
        .rev()
        .take_while(|&byte| byte == marker as u8)
        .count()
}

fn emphasis(text: &str, selection: Range<usize>, width: usize) -> Edit {
    let selected = &text[selection.clone()];
    let matches = |count: usize| count >= width && (width == 2 || count % 2 == 1);
    for marker in ['*', '_'] {
        let before = run_end(&text[..selection.start], marker);
        let after = run_start(&text[selection.end..], marker);
        if matches(before) && matches(after) {
            return Edit {
                range: selection.start - width..selection.end + width,
                text: selected.to_owned(),
                selection: selection.start - width..selection.end - width,
            };
        }
        let start = run_start(selected, marker);
        let end = run_end(selected, marker);
        if selected.len() >= width * 2
            && ((matches(start) && matches(end))
                || selected == marker.to_string().repeat(width * 2))
        {
            return unwrap(text, selection, width, width);
        }
    }
    let leading = selected.len() - selected.trim_start().len();
    let trailing = selected.len() - selected.trim_end().len();
    let range = if leading + trailing < selected.len() {
        selection.start + leading..selection.end - trailing
    } else {
        selection
    };
    let delimiter = "*".repeat(width);
    wrap(text, range, &delimiter, &delimiter)
}

fn lines(text: &str, selection: &Range<usize>) -> Range<usize> {
    let start = text[..selection.start].rfind('\n').map_or(0, |at| at + 1);
    let last = if selection.end > selection.start && text[..selection.end].ends_with('\n') {
        selection.end - 1
    } else {
        selection.end
    };
    let end = text[last..].find('\n').map_or(text.len(), |at| last + at);
    start..end
}

fn heading(text: &str, selection: Range<usize>, level: u8) -> Option<Edit> {
    let range = lines(text, &selection);
    let first_line = text[..range.start]
        .bytes()
        .filter(|&byte| byte == b'\n')
        .count();
    let styles = markdown::styles(text);
    let affected: Vec<_> = text[range.clone()]
        .split('\n')
        .enumerate()
        .map(|(index, line)| (line, &styles[first_line + index]))
        .collect();
    let remove = affected
        .iter()
        .filter(|(_, style)| !style.code)
        .all(|(_, style)| style.heading == Some(level));
    let mut output = String::new();
    let mut offset = range.start;
    let mut changes = Vec::new();
    for (index, (line, style)) in affected.into_iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        if style.code {
            output.push_str(line);
        } else {
            let indent = line.len() - line.trim_start_matches(' ').len();
            let indent = if indent <= 3 { indent } else { 0 };
            let existing = style.heading.map_or(0, |heading| {
                let hashes = usize::from(heading);
                hashes
                    + usize::from(matches!(
                        line.as_bytes().get(indent + hashes),
                        Some(b' ' | b'\t')
                    ))
            });
            let prefix = if remove {
                String::new()
            } else {
                format!("{} ", "#".repeat(usize::from(level)))
            };
            changes.push((offset + indent..offset + indent + existing, prefix.len()));
            output.push_str(&line[..indent]);
            output.push_str(&prefix);
            output.push_str(&line[indent + existing..]);
        }
        offset += line.len() + 1;
    }
    if output == text[range.clone()] {
        return None;
    }
    let map = |position: usize| {
        let mut mapped = position;
        for (removed, inserted) in &changes {
            if position >= removed.end {
                mapped = mapped + inserted - removed.len();
            } else if position >= removed.start {
                mapped = mapped + inserted - (position - removed.start);
            }
        }
        mapped
    };
    Some(Edit {
        range,
        text: output,
        selection: map(selection.start)..map(selection.end),
    })
}

fn longest_backticks(text: &str) -> usize {
    text.split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0)
}

fn inline_code(text: &str, selection: Range<usize>) -> Edit {
    let selected = &text[selection.clone()];
    let before = &text[..selection.start];
    let after = &text[selection.end..];
    let padded = (selected.starts_with('`')
        || selected.ends_with('`')
        || (selected.starts_with(' ') && selected.ends_with(' ') && !selected.trim().is_empty()))
        && before.ends_with(' ')
        && after.starts_with(' ');
    let padding = usize::from(padded);
    let left = run_end(&before[..before.len() - padding], '`');
    let right = run_start(&after[padding..], '`');
    if left > 0 && left == right && longest_backticks(selected) < left {
        return Edit {
            range: selection.start - left - padding..selection.end + right + padding,
            text: selected.to_owned(),
            selection: selection.start - left - padding..selection.end - left - padding,
        };
    }
    let left = run_start(selected, '`');
    let right = run_end(selected, '`');
    if left == selected.len() && left > 0 && left.is_multiple_of(2) {
        return unwrap(text, selection, left / 2, left / 2);
    }
    if left > 0
        && left == right
        && selected.len() >= left * 2
        && longest_backticks(&selected[left..selected.len() - right]) < left
    {
        let content = &selected[left..selected.len() - right];
        let padding = usize::from(
            content.starts_with(' ') && content.ends_with(' ') && !content.trim().is_empty(),
        );
        return unwrap(text, selection, left + padding, right + padding);
    }
    let delimiter = "`".repeat(longest_backticks(selected) + 1);
    let pad = if selected.starts_with('`')
        || selected.ends_with('`')
        || (selected.starts_with(' ') && selected.ends_with(' ') && !selected.trim().is_empty())
    {
        " "
    } else {
        ""
    };
    wrap(
        text,
        selection,
        &format!("{delimiter}{pad}"),
        &format!("{pad}{delimiter}"),
    )
}

fn fenced_code(text: &str, selection: Range<usize>) -> Edit {
    let range = lines(text, &selection);
    let mut opening: Option<(usize, usize, markdown::Fence)> = None;
    let mut offset = 0;
    for line in text.split('\n') {
        if let Some((start, content_start, current)) = opening {
            if let Some((closing, rest)) = markdown::fence(line)
                && closing.marker == current.marker
                && closing.length >= current.length
                && rest.trim_matches([' ', '\t', '\r']).is_empty()
            {
                let end = offset + line.len();
                if range.start >= start && range.end <= end {
                    let content_end = offset.saturating_sub(1).max(content_start);
                    let content = &text[content_start..content_end];
                    let map = |position: usize| {
                        start + position.clamp(content_start, content_end) - content_start
                    };
                    let mapped = if !selection.is_empty()
                        && (selection.start < content_start || selection.end > content_end)
                    {
                        start..start + content.len()
                    } else {
                        map(selection.start)..map(selection.end)
                    };
                    return Edit {
                        range: start..end,
                        text: content.to_owned(),
                        selection: mapped,
                    };
                }
                opening = None;
            }
        } else if let Some((fence, rest)) = markdown::fence(line)
            && (fence.marker != b'`' || !rest.contains('`'))
        {
            opening = Some((offset, offset + line.len() + 1, fence));
        }
        offset += line.len() + 1;
    }
    if let Some((start, content_start, _)) = opening
        && range.start >= start
    {
        let content_start = content_start.min(text.len());
        let content = &text[content_start..];
        let map = |position: usize| start + position.max(content_start) - content_start;
        return Edit {
            range: start..text.len(),
            text: content.to_owned(),
            selection: map(selection.start)..map(selection.end),
        };
    }
    let delimiter = "`".repeat((longest_backticks(&text[range.clone()]) + 1).max(3));
    let prefix = format!("{delimiter}\n");
    let mut edit = wrap(text, range, &prefix, &format!("\n{delimiter}"));
    edit.selection = selection.start + prefix.len()..selection.end + prefix.len();
    edit
}

#[cfg(test)]
mod tests {
    use super::{Command, transform};
    use std::ops::Range;

    fn apply(text: &str, selection: Range<usize>, command: Command) -> (String, Range<usize>) {
        let edit = transform(text, selection, command).unwrap();
        let mut output = text.to_owned();
        output.replace_range(edit.range, &edit.text);
        assert!(output.is_char_boundary(edit.selection.start));
        assert!(output.is_char_boundary(edit.selection.end));
        assert!(edit.selection.start <= edit.selection.end);
        (output, edit.selection)
    }

    #[test]
    fn emphasis_toggles_utf8_content_and_inclusive_markers() {
        for command in [Command::Bold, Command::Italic] {
            let original = "prefix café日本語 suffix";
            let range = 7..original.find(" suffix").unwrap();
            let (formatted, selection) = apply(original, range.clone(), command);
            assert_eq!(&formatted[selection.clone()], "café日本語");
            let (plain, selection) = apply(&formatted, selection, command);
            assert_eq!((plain, selection), (original.to_owned(), range));
        }
        assert_eq!(
            apply("**café**", 0..9, Command::Bold),
            ("café".to_owned(), 0..5)
        );
        assert_eq!(
            apply("_word_", 0..6, Command::Italic),
            ("word".to_owned(), 0..4)
        );
        assert_eq!(apply("***word***", 3..7, Command::Italic).0, "**word**");
        assert_eq!(apply("***word***", 3..7, Command::Bold).0, "*word*");
        assert_eq!(apply("**word**", 2..6, Command::Italic).0, "***word***");
    }

    #[test]
    fn empty_pairs_keep_the_caret_inside_and_toggle_off() {
        for (command, expected, caret) in [
            (Command::Bold, "before ****after", 9),
            (Command::Italic, "before **after", 8),
            (Command::InlineCode, "before ``after", 8),
            (Command::FencedCode, "```\nbefore after\n```", 11),
        ] {
            let (formatted, selection) = apply("before after", 7..7, command);
            assert_eq!(formatted, expected);
            assert_eq!(selection, caret..caret);
            assert_eq!(
                apply(&formatted, selection, command),
                ("before after".to_owned(), 7..7)
            );
        }
        assert_eq!(
            apply("", 0..0, Command::FencedCode),
            ("```\n\n```".to_owned(), 4..4)
        );
        assert_eq!(
            apply("```\n\n```", 4..4, Command::FencedCode),
            (String::new(), 0..0)
        );
        assert_eq!(
            apply("``", 0..2, Command::InlineCode),
            (String::new(), 0..0)
        );
    }

    #[test]
    fn headings_change_whole_lines_and_exclude_a_trailing_next_line() {
        let original = "αlpha\n## café\nnext";
        let end = original.find("next").unwrap();
        let (formatted, selection) = apply(original, 0..end, Command::Heading(3));
        assert_eq!(formatted, "### αlpha\n### café\nnext");
        assert_eq!(&formatted[selection.clone()], "αlpha\n### café\n");
        let (plain, selection) = apply(&formatted, selection, Command::Heading(3));
        assert_eq!(plain, "αlpha\ncafé\nnext");
        assert_eq!(selection, 0..plain.find("next").unwrap());
        assert_eq!(
            apply("  # title", 5..5, Command::Heading(2)),
            ("  ## title".to_owned(), 6..6)
        );
        assert_eq!(
            apply("## title", 1..1, Command::Heading(2)),
            ("title".to_owned(), 0..0)
        );
        assert!(transform("```\n# code\n```", 5..5, Command::Heading(1)).is_none());
        assert_eq!(apply("#nospace", 0..0, Command::Heading(1)).0, "# #nospace");
    }

    #[test]
    fn inline_code_uses_safe_delimiters_and_padding() {
        for original in ["`literal", "a``b`c", " café ", "   ", "x\ny"] {
            let (formatted, selection) = apply(original, 0..original.len(), Command::InlineCode);
            assert_eq!(&formatted[selection.clone()], original);
            assert_eq!(
                apply(&formatted, selection, Command::InlineCode).0,
                original
            );
        }
        assert_eq!(apply("`literal`", 0..9, Command::InlineCode).0, "literal");
        let (formatted, _) = apply("`literal", 0..8, Command::InlineCode);
        assert_eq!(formatted, "`` `literal ``");
        assert_eq!(
            apply(&formatted, 0..formatted.len(), Command::InlineCode).0,
            "`literal"
        );
    }

    #[test]
    fn code_blocks_wrap_lines_with_safe_fences_and_preserve_neighbors() {
        let original = "before\nα ``` β\n~~~ literal\nafter";
        let range = original.find('α').unwrap()..original.find("after").unwrap();
        let (formatted, selection) = apply(original, range.clone(), Command::FencedCode);
        assert_eq!(formatted, "before\n````\nα ``` β\n~~~ literal\n````\nafter");
        assert_eq!(
            apply(&formatted, selection, Command::FencedCode).0,
            original
        );
        let fenced = "before\n  ~~~rust\nα\n  ~~~~\nafter";
        let caret = fenced.find('α').unwrap() + 'α'.len_utf8();
        assert_eq!(
            apply(fenced, caret..caret, Command::FencedCode),
            ("before\nα\nafter".to_owned(), 9..9)
        );
        assert_eq!(
            apply("```rust\ncode\n```", 0..16, Command::FencedCode),
            ("code".to_owned(), 0..4)
        );
        assert_eq!(
            apply("```\ncode", 8..8, Command::FencedCode),
            ("code".to_owned(), 4..4)
        );
        assert_eq!(
            apply("```", 3..3, Command::FencedCode),
            (String::new(), 0..0)
        );
        let nested = "```\ncode\n```\nplain";
        let (formatted, _) = apply(nested, 0..nested.len(), Command::FencedCode);
        assert!(formatted.starts_with("````\n```\n"));
        assert!(formatted.ends_with("\n````"));
    }

    #[test]
    fn invalid_offsets_and_heading_levels_are_rejected() {
        assert!(transform("é", 1..1, Command::Bold).is_none());
        assert!(transform("é", 0..3, Command::InlineCode).is_none());
        assert!(transform("é", 2..0, Command::Italic).is_none());
        assert!(transform("note", 0..4, Command::Heading(7)).is_none());
    }
}
