//! Text ranges for UI Automation's Text pattern: where the units of a text
//! begin, and how a range expands and moves by them. Pure, on UTF-16
//! offsets, which is what UI Automation counts in.
//!
//! The text has no line breaks of its own besides the ones in it, so a
//! line is a run of text up to and with its line break. A line the window
//! wraps is still one line here: the window doesn't tell its tree where it
//! wraps.

/// A unit of text, as UI Automation names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Character,
    Word,
    Line,
    Paragraph,
    /// A page, a format run and the document are all the whole text: it
    /// has one of each.
    Whole,
}

/// A range: where it starts and where it ends, in UTF-16 units. It is
/// empty when they are the same.
pub type Range = (usize, usize);

/// Which end of a range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Start,
    End,
}

fn is_break(c: u16) -> bool {
    c == 0x0A || c == 0x0D
}

fn is_blank(c: u16) -> bool {
    c == 0x20 || c == 0x09 || c == 0xA0
}

fn is_word(c: u16) -> bool {
    char::from_u32(u32::from(c)).is_none_or(|c| c.is_alphanumeric() || c == '_')
}

/// Where each unit of `unit` begins, in order, then where the text ends.
/// Empty text has the one boundary, 0.
pub fn bounds(text: &[u16], unit: Unit) -> Vec<usize> {
    let n = text.len();
    let mut at = vec![0];
    let mut i = 0;
    while i < n {
        i = match unit {
            // A surrogate pair and a CR LF are one character.
            Unit::Character => {
                let c = text[i];
                if (0xD800..0xDC00).contains(&c)
                    && i + 1 < n
                    && (0xDC00..0xE000).contains(&text[i + 1])
                    || c == 0x0D && i + 1 < n && text[i + 1] == 0x0A
                {
                    i + 2
                } else {
                    i + 1
                }
            }
            // A word is its letters or its one mark, then the blanks
            // after it and one line break: as Narrator reads "word, ".
            Unit::Word => {
                let mut j = i;
                if is_break(text[j]) {
                    // A line with nothing on it is a word of its own.
                    j = line_break_end(text, j);
                } else {
                    let word = is_word(text[j]);
                    j += 1;
                    while word && j < n && is_word(text[j]) {
                        j += 1;
                    }
                    while j < n && is_blank(text[j]) {
                        j += 1;
                    }
                    if j < n && is_break(text[j]) {
                        j = line_break_end(text, j);
                    }
                }
                j
            }
            Unit::Line | Unit::Paragraph => {
                let mut j = i;
                while j < n && !is_break(text[j]) {
                    j += 1;
                }
                if j < n {
                    j = line_break_end(text, j);
                }
                j
            }
            Unit::Whole => n,
        };
        at.push(i);
    }
    at
}

/// One line break at `i`: CR LF is one.
fn line_break_end(text: &[u16], i: usize) -> usize {
    if text[i] == 0x0D && text.get(i + 1) == Some(&0x0A) {
        i + 2
    } else {
        i + 1
    }
}

/// The index in `at` of the last boundary at or before `p`.
fn floor(at: &[usize], p: usize) -> usize {
    at.partition_point(|&b| b <= p).saturating_sub(1)
}

/// The range made the one unit that its start is in, as UI Automation
/// normalises: a smaller range is widened to it and a longer one cut to
/// it. At the very end of the text it stays empty there.
pub fn expand(text: &[u16], (s, _): Range, unit: Unit) -> Range {
    let at = bounds(text, unit);
    let a = floor(&at, s.min(text.len()));
    let b = (a + 1).min(at.len() - 1);
    (at[a], at[b])
}
/// Where `want` is in `hay`, as a unit offset: the first match, or the
/// last with `backward`. Units compare as they are; with `ignore_case` a
/// unit that is a whole character is lowercased first, and a surrogate,
/// which is half of one, is left as it is, so two different characters
/// outside the basic plane never match each other.
pub fn find_units(hay: &[u16], want: &[u16], backward: bool, ignore_case: bool) -> Option<usize> {
    let fold = |c: u16| {
        if !ignore_case {
            return c;
        }
        match char::from_u32(u32::from(c)) {
            Some(ch) => ch.to_lowercase().next().map_or(c, |l| l as u32 as u16),
            None => c,
        }
    };
    if want.is_empty() || want.len() > hay.len() {
        return None;
    }
    let want: Vec<u16> = want.iter().map(|&c| fold(c)).collect();
    let hay: Vec<u16> = hay.iter().map(|&c| fold(c)).collect();
    let mut at = (0..=hay.len() - want.len()).filter(|&i| hay[i..i + want.len()] == want[..]);
    if backward {
        at.next_back()
    } else {
        at.next()
    }
}

/// The range moved by `count` units, and how many it did move, which is
/// fewer at either end of the text. A range that isn't whole units is
/// widened to them first; an empty one stays empty, at the unit's start.
pub fn move_range(text: &[u16], range: Range, unit: Unit, count: i32) -> (Range, i32) {
    let at = bounds(text, unit);
    let units = at.len() - 1;
    if units == 0 {
        return (range, 0);
    }
    let (s, e) = range;
    let from = floor(&at, s.min(text.len())).min(units - 1);
    let to = (from as i64 + i64::from(count)).clamp(0, units as i64 - 1) as usize;
    let moved = to as i64 - from as i64;
    let start = at[to];
    let end = if s == e { start } else { at[to + 1] };
    ((start, end), moved as i32)
}

/// One end of the range moved by `count` unit boundaries, and how many it
/// did move. When it passes the other end, that one comes along: the range
/// is then empty.
pub fn move_end(text: &[u16], (s, e): Range, which: End, unit: Unit, count: i32) -> (Range, i32) {
    let at = bounds(text, unit);
    let last = at.len() - 1;
    let p = match which {
        End::Start => s,
        End::End => e,
    };
    // Where it is among the boundaries: on one, or inside the unit that
    // starts at the one before.
    let k = floor(&at, p.min(text.len()));
    let inside = at[k] != p;
    let mut to = k as i64;
    let mut moved = 0;
    if count > 0 {
        let room = last as i64 - k as i64;
        let step = i64::from(count).min(room);
        to = k as i64 + step;
        moved = step as i32;
    } else if count < 0 {
        // Backwards from inside a unit, its own start is the first step.
        let room = k as i64 + i64::from(inside);
        let step = (-i64::from(count)).min(room);
        to = k as i64 + i64::from(inside) - step;
        moved = -(step as i32);
    }
    let q = at[to.clamp(0, last as i64) as usize];
    let range = match which {
        End::Start => (q, e.max(q)),
        End::End => (s.min(q), q),
    };
    (range, moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn pieces(s: &str, unit: Unit) -> Vec<String> {
        let t = u(s);
        let at = bounds(&t, unit);
        at.windows(2)
            .map(|w| String::from_utf16_lossy(&t[w[0]..w[1]]))
            .collect()
    }

    #[test]
    fn a_line_is_its_text_and_its_break() {
        assert_eq!(
            pieces("one\r\ntwo\n\nthree", Unit::Line),
            ["one\r\n", "two\n", "\n", "three"]
        );
        assert_eq!(pieces("", Unit::Line), Vec::<String>::new());
        assert_eq!(bounds(&[], Unit::Line), [0]);
        // A paragraph is a line here.
        assert_eq!(pieces("a\nb", Unit::Paragraph), ["a\n", "b"]);
    }

    #[test]
    fn a_word_keeps_the_blanks_after_it() {
        assert_eq!(
            pieces("error: file not found.\nretry", Unit::Word),
            ["error", ": ", "file ", "not ", "found", ".\n", "retry"]
        );
        // A line with nothing on it is a word of its own.
        assert_eq!(pieces("a\n\nb", Unit::Word), ["a\n", "\n", "b"]);
        // Underscores and digits stay in a word.
        assert_eq!(pieces("ffmpeg_v2 -i", Unit::Word), ["ffmpeg_v2 ", "-", "i"]);
    }

    #[test]
    fn a_character_is_a_pair_or_a_line_break_whole() {
        assert_eq!(
            pieces("a\u{1F600}b\r\nc", Unit::Character),
            ["a", "\u{1F600}", "b", "\r\n", "c"]
        );
    }

    #[test]
    fn a_range_expands_to_the_unit_around_it() {
        let t = u("one two\nthree");
        assert_eq!(expand(&t, (5, 5), Unit::Word), (4, 8));
        assert_eq!(expand(&t, (5, 5), Unit::Line), (0, 8));
        assert_eq!(expand(&t, (9, 10), Unit::Line), (8, 13));
        // A longer range is cut to its first unit.
        assert_eq!(expand(&t, (2, 6), Unit::Word), (0, 4));
        assert_eq!(expand(&t, (1, 1), Unit::Whole), (0, 13));
        // At the end it stays empty there.
        assert_eq!(expand(&t, (13, 13), Unit::Line), (13, 13));
        // Whole units stay as they are.
        assert_eq!(expand(&t, (4, 8), Unit::Word), (4, 8));
    }

    #[test]
    fn a_range_moves_by_units_and_says_how_many_it_could() {
        let t = u("one two\nthree\nfour");
        assert_eq!(move_range(&t, (0, 8), Unit::Line, 1), ((8, 14), 1));
        assert_eq!(move_range(&t, (0, 8), Unit::Line, 5), ((14, 18), 2));
        assert_eq!(move_range(&t, (14, 18), Unit::Line, -1), ((8, 14), -1));
        assert_eq!(move_range(&t, (8, 14), Unit::Line, -4), ((0, 8), -1));
        // An empty range moves as an empty range.
        assert_eq!(move_range(&t, (9, 9), Unit::Word, 1), ((14, 14), 1));
        // A range mid-unit moves from the unit it starts in.
        assert_eq!(move_range(&t, (1, 2), Unit::Word, 1), ((4, 8), 1));
        assert_eq!(move_range(&[], (0, 0), Unit::Word, 1), ((0, 0), 0));
    }

    #[test]
    fn an_end_moves_by_units_and_drags_the_other_when_it_passes() {
        let t = u("one two\nthree");
        assert_eq!(move_end(&t, (0, 4), End::End, Unit::Word, 1), ((0, 8), 1));
        assert_eq!(move_end(&t, (0, 4), End::End, Unit::Word, 9), ((0, 13), 2));
        assert_eq!(
            move_end(&t, (4, 8), End::Start, Unit::Word, -1),
            ((0, 8), -1)
        );
        // The end back past the start brings the start with it.
        assert_eq!(move_end(&t, (4, 8), End::End, Unit::Word, -2), ((0, 0), -2));
        // From inside a unit, backwards is to its start first.
        assert_eq!(move_end(&t, (0, 6), End::End, Unit::Word, -1), ((0, 4), -1));
        // The start forward past the end brings the end along.
        assert_eq!(move_end(&t, (0, 4), End::Start, Unit::Word, 2), ((8, 8), 2));
        assert_eq!(move_end(&t, (0, 4), End::Start, Unit::Word, 0), ((0, 4), 0));
    }

    #[test]
    fn a_move_by_the_most_negative_count_goes_to_the_start_without_overflowing() {
        let t = u("one two\nthree");
        assert_eq!(
            move_end(&t, (4, 8), End::End, Unit::Word, i32::MIN),
            ((0, 0), -2)
        );
        assert_eq!(
            move_end(&t, (4, 8), End::Start, Unit::Word, i32::MIN),
            ((0, 8), -1)
        );
    }

    #[test]
    fn a_find_matches_the_text_and_not_just_any_emoji() {
        let t = u("a \u{1F600} b \u{1F680} c \u{1F680}");
        let rocket = u("\u{1F680}");
        assert_eq!(find_units(&t, &rocket, false, false), Some(7));
        assert_eq!(find_units(&t, &rocket, true, false), Some(12));
        assert_eq!(find_units(&t, &rocket, false, true), Some(7));
        assert_eq!(find_units(&t, &u("\u{1F4A9}"), false, false), None);
        assert_eq!(find_units(&t, &u("\u{1F4A9}"), false, true), None);
    }

    #[test]
    fn a_find_ignoring_case_folds_letters_and_keeps_the_rest() {
        let t = u("One TWO one");
        assert_eq!(find_units(&t, &u("two"), false, true), Some(4));
        assert_eq!(find_units(&t, &u("two"), false, false), None);
        assert_eq!(find_units(&t, &u("ONE"), true, true), Some(8));
        assert_eq!(find_units(&t, &[], false, true), None);
        assert_eq!(find_units(&u("a"), &u("ab"), false, true), None);
    }
}
