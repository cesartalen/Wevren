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

pub fn headings(text: &str) -> Vec<Option<u8>> {
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
                return None;
            }
            if let Some((opening, rest)) = fence(line)
                && (opening.marker != b'`' || !rest.contains('`'))
            {
                open = Some(opening);
                return None;
            }
            heading(line)
        })
        .collect()
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
    use super::{heading_scale, headings};

    #[test]
    fn recognizes_six_levels_and_retains_empty_lines() {
        assert_eq!(
            headings("# One\n## Two\n### Three\n#### Four\n##### Five\n###### Six\n"),
            vec![Some(1), Some(2), Some(3), Some(4), Some(5), Some(6), None]
        );
        assert_eq!(
            headings("#\n##\tCafé 🧶\n   ### Indented"),
            vec![Some(1), Some(2), Some(3)]
        );
    }

    #[test]
    fn leaves_hashtags_escaped_markers_and_indented_code_alone() {
        assert_eq!(
            headings("#tag\n####### Too many\n\\# Escaped\n    # Code\n\t# Code\nordinary # text"),
            vec![None; 6]
        );
    }

    #[test]
    fn skips_fenced_code_and_respects_the_closing_fence() {
        assert_eq!(
            headings(
                "# Before\n````rust\n# Code\n```\n## Still code\n```` text\n### Still code\n`````\n## After"
            ),
            vec![Some(1), None, None, None, None, None, None, None, Some(2)]
        );
        assert_eq!(
            headings("~~~\n# Code\n```\n## Code\n~~~\n# After"),
            vec![None, None, None, None, None, Some(1)]
        );
        assert_eq!(headings("```\n# Unclosed"), vec![None, None]);
    }

    #[test]
    fn heading_sizes_follow_the_hierarchy() {
        for level in 1..6 {
            assert!(heading_scale(level) > heading_scale(level + 1));
        }
        assert!((heading_scale(6) - 1.).abs() < f32::EPSILON);
    }
}
