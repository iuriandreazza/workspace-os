//! Hard wrapping by display width: nothing is cut off, because the root panel
//! must always show the whole exact action (ADR 0007).

use ratatui::text::Span;

pub fn cell_width(c: char) -> usize {
    // Printable ASCII is one cell: the common case, without an allocation.
    if c.is_ascii_graphic() || c == ' ' {
        return 1;
    }
    Span::raw(c.to_string()).width()
}

pub fn text_width(text: &str) -> usize {
    text.chars().map(cell_width).sum()
}

/// `text` split into lines of at most `width` cells. Existing line breaks are
/// kept, an empty line stays one, and a tab is four spaces.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for source in text.split('\n') {
        let mut line = String::new();
        let mut used = 0;
        for c in source.replace('\t', "    ").chars() {
            let cells = cell_width(c);
            while used + cells > width && !line.is_empty() {
                // Break after the last space when there is one, so words stay
                // whole; the space stays on the line, so no character is lost.
                let cut = line.rfind(' ').map(|i| i + 1).filter(|&i| i < line.len());
                let rest = cut.map(|i| line.split_off(i)).unwrap_or_default();
                lines.push(std::mem::take(&mut line));
                line = rest;
                used = text_width(&line);
            }
            line.push(c);
            used += cells;
        }
        lines.push(line);
    }
    lines
}

/// The last `width` cells of `text`: what fits on an input line whose end is
/// where the cursor is.
pub fn tail(text: &str, width: usize) -> String {
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0;
    for c in text.chars().rev() {
        let cells = cell_width(c);
        if used + cells > width {
            break;
        }
        used += cells;
        kept.push(c);
    }
    kept.into_iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::{tail, text_width, wrap};

    #[test]
    fn long_lines_are_split_and_short_ones_kept() {
        assert_eq!(wrap("abcdef", 4), ["abcd", "ef"]);
        assert_eq!(wrap("ab\n\ncd", 4), ["ab", "", "cd"]);
        assert_eq!(wrap("", 4), [""]);
    }

    #[test]
    fn words_stay_whole_and_no_character_is_lost() {
        assert_eq!(wrap("one two three", 8), ["one two ", "three"]);
        let text = "a  path with  two spaces/and-a-very-long-name.iso";
        for width in 1..30 {
            assert_eq!(wrap(text, width).concat(), text, "width {width}");
        }
        let mixed = " abcdefghi漢 字 x漢漢 yz";
        for width in 2..30 {
            for line in wrap(mixed, width) {
                assert!(text_width(&line) <= width, "{line:?} at {width}");
            }
            assert_eq!(wrap(mixed, width).concat(), mixed);
        }
    }

    #[test]
    fn wide_characters_count_two_cells() {
        assert_eq!(wrap("漢字漢", 4), ["漢字", "漢"]);
    }

    #[test]
    fn a_width_of_zero_still_makes_progress() {
        assert_eq!(wrap("ab", 0), ["a", "b"]);
    }

    #[test]
    fn the_tail_is_the_end_of_the_text() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 3), "ab");
    }
}
