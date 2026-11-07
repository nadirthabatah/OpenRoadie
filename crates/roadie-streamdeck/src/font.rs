//! A small bitmap font, written as pictures.
//!
//! Every glyph is stored as seven rows of five characters, exactly as it
//! appears on screen. That is deliberate: font data as hex columns is
//! unreviewable — a wrong bit is invisible in `0x7E, 0x09, 0x09` and obvious
//! in a drawing of the letter. The cost is a few bytes; the benefit is that a
//! mistake in this file can be seen rather than only discovered on a device.
//!
//! The set is deliberately small — capitals, digits and common punctuation.
//! Key labels are short and read at a glance, and lowercase at five pixels
//! wide is less legible than the capital it maps to.

/// Width of every glyph, in pixels.
pub const GLYPH_WIDTH: usize = 5;

/// Height of every glyph, in pixels.
pub const GLYPH_HEIGHT: usize = 7;

/// Blank columns between adjacent glyphs.
pub const GLYPH_SPACING: usize = 1;

/// The glyph drawn for a character the font does not carry.
///
/// A hollow box rather than a blank: a missing character should be visible as
/// a missing character, not silently swallowed into whitespace.
const MISSING: [&str; GLYPH_HEIGHT] = [
    "#####", "#...#", "#...#", "#...#", "#...#", "#...#", "#####",
];

/// Every glyph this font carries, drawn as it renders.
const GLYPHS: &[(char, [&str; GLYPH_HEIGHT])] = &[
    (
        ' ',
        [
            ".....", ".....", ".....", ".....", ".....", ".....", ".....",
        ],
    ),
    (
        'A',
        [
            ".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
        ],
    ),
    (
        'B',
        [
            "####.", "#...#", "#...#", "####.", "#...#", "#...#", "####.",
        ],
    ),
    (
        'C',
        [
            ".###.", "#...#", "#....", "#....", "#....", "#...#", ".###.",
        ],
    ),
    (
        'D',
        [
            "####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####.",
        ],
    ),
    (
        'E',
        [
            "#####", "#....", "#....", "####.", "#....", "#....", "#####",
        ],
    ),
    (
        'F',
        [
            "#####", "#....", "#....", "####.", "#....", "#....", "#....",
        ],
    ),
    (
        'G',
        [
            ".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".###.",
        ],
    ),
    (
        'H',
        [
            "#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
        ],
    ),
    (
        'I',
        [
            "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "#####",
        ],
    ),
    (
        'J',
        [
            "..###", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##..",
        ],
    ),
    (
        'K',
        [
            "#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#",
        ],
    ),
    (
        'L',
        [
            "#....", "#....", "#....", "#....", "#....", "#....", "#####",
        ],
    ),
    (
        'M',
        [
            "#...#", "##.##", "#.#.#", "#...#", "#...#", "#...#", "#...#",
        ],
    ),
    (
        'N',
        [
            "#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#", "#...#",
        ],
    ),
    (
        'O',
        [
            ".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
        ],
    ),
    (
        'P',
        [
            "####.", "#...#", "#...#", "####.", "#....", "#....", "#....",
        ],
    ),
    (
        'Q',
        [
            ".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#",
        ],
    ),
    (
        'R',
        [
            "####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#",
        ],
    ),
    (
        'S',
        [
            ".####", "#....", "#....", ".###.", "....#", "....#", "####.",
        ],
    ),
    (
        'T',
        [
            "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#..",
        ],
    ),
    (
        'U',
        [
            "#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
        ],
    ),
    (
        'V',
        [
            "#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#..",
        ],
    ),
    (
        'W',
        [
            "#...#", "#...#", "#...#", "#...#", "#.#.#", "##.##", "#...#",
        ],
    ),
    (
        'X',
        [
            "#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#",
        ],
    ),
    (
        'Y',
        [
            "#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#..",
        ],
    ),
    (
        'Z',
        [
            "#####", "....#", "...#.", "..#..", ".#...", "#....", "#####",
        ],
    ),
    (
        '0',
        [
            ".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###.",
        ],
    ),
    (
        '1',
        [
            "..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###.",
        ],
    ),
    (
        '2',
        [
            ".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####",
        ],
    ),
    (
        '3',
        [
            "#####", "...#.", "..#..", "...#.", "....#", "#...#", ".###.",
        ],
    ),
    (
        '4',
        [
            "...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#.",
        ],
    ),
    (
        '5',
        [
            "#####", "#....", "####.", "....#", "....#", "#...#", ".###.",
        ],
    ),
    (
        '6',
        [
            "..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###.",
        ],
    ),
    (
        '7',
        [
            "#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#...",
        ],
    ),
    (
        '8',
        [
            ".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###.",
        ],
    ),
    (
        '9',
        [
            ".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##..",
        ],
    ),
    (
        '.',
        [
            ".....", ".....", ".....", ".....", ".....", ".##..", ".##..",
        ],
    ),
    (
        ',',
        [
            ".....", ".....", ".....", ".....", ".##..", ".##..", ".#...",
        ],
    ),
    (
        '-',
        [
            ".....", ".....", ".....", "#####", ".....", ".....", ".....",
        ],
    ),
    (
        '+',
        [
            ".....", "..#..", "..#..", "#####", "..#..", "..#..", ".....",
        ],
    ),
    (
        '/',
        [
            "....#", "....#", "...#.", "..#..", ".#...", "#....", "#....",
        ],
    ),
    (
        ':',
        [
            ".....", ".##..", ".##..", ".....", ".##..", ".##..", ".....",
        ],
    ),
    (
        '!',
        [
            "..#..", "..#..", "..#..", "..#..", "..#..", ".....", "..#..",
        ],
    ),
    (
        '?',
        [
            ".###.", "#...#", "....#", "...#.", "..#..", ".....", "..#..",
        ],
    ),
    (
        '\'',
        [
            "..#..", "..#..", "..#..", ".....", ".....", ".....", ".....",
        ],
    ),
    (
        '%',
        [
            "##..#", "##..#", "...#.", "..#..", ".#...", "#..##", "#..##",
        ],
    ),
    (
        '=',
        [
            ".....", ".....", "#####", ".....", "#####", ".....", ".....",
        ],
    ),
    (
        '*',
        [
            ".....", "#.#.#", ".###.", "#####", ".###.", "#.#.#", ".....",
        ],
    ),
];

/// The rows of the glyph for `character`.
///
/// Lowercase maps to its capital. A character the font does not carry becomes
/// a hollow box, so it is visibly missing rather than silently blank.
#[must_use]
pub fn glyph(character: char) -> [&'static str; GLYPH_HEIGHT] {
    let wanted = character.to_ascii_uppercase();
    GLYPHS
        .iter()
        .find(|(candidate, _)| *candidate == wanted)
        .map_or(MISSING, |(_, rows)| *rows)
}

/// Whether the font carries `character` (after mapping to uppercase).
#[must_use]
pub fn carries(character: char) -> bool {
    let wanted = character.to_ascii_uppercase();
    GLYPHS.iter().any(|(candidate, _)| *candidate == wanted)
}

/// The characters in `text` this font cannot draw, in the order they appear.
///
/// Each is drawn as a hollow box, which is visible on the key but says nothing
/// about what went wrong — and says nothing at all to someone who cannot see
/// the key. So the caller is given the list and can tell them.
///
/// Distinct, because a label of eight unsupported characters is one problem
/// with the label rather than eight.
#[must_use]
pub fn missing_from(text: &str) -> Vec<char> {
    let mut missing: Vec<char> = Vec::new();
    for character in text.chars() {
        if !carries(character) && !missing.contains(&character) {
            missing.push(character);
        }
    }
    missing
}

/// Width in pixels of `text` at scale 1, spacing included.
#[must_use]
pub fn text_width(text: &str) -> usize {
    let count = text.chars().count();
    if count == 0 {
        return 0;
    }
    count * GLYPH_WIDTH + (count - 1) * GLYPH_SPACING
}

#[cfg(test)]
mod tests {
    use super::{
        GLYPH_HEIGHT, GLYPH_WIDTH, GLYPHS, MISSING, carries, glyph, missing_from, text_width,
    };

    /// Every glyph must be exactly the declared size. A row one character
    /// short would shift everything after it, and is easy to typo.
    #[test]
    fn every_glyph_is_the_declared_shape() {
        for (character, rows) in GLYPHS {
            assert_eq!(rows.len(), GLYPH_HEIGHT, "{character:?} has wrong height");
            for (index, row) in rows.iter().enumerate() {
                assert_eq!(
                    row.chars().count(),
                    GLYPH_WIDTH,
                    "{character:?} row {index} is {row:?}"
                );
                assert!(
                    row.chars().all(|pixel| pixel == '#' || pixel == '.'),
                    "{character:?} row {index} has something other than '#' and '.': {row:?}"
                );
            }
        }
        assert_eq!(MISSING.len(), GLYPH_HEIGHT);
    }

    #[test]
    fn no_character_is_defined_twice() {
        let mut seen: Vec<char> = GLYPHS.iter().map(|(character, _)| *character).collect();
        let total = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), total, "a character is defined more than once");
    }

    /// A blank glyph where a letter should be is the failure this catches:
    /// it renders as a wordless gap and nothing else would notice.
    #[test]
    fn every_glyph_except_space_draws_something() {
        for (character, rows) in GLYPHS {
            if *character == ' ' {
                continue;
            }
            assert!(
                rows.iter().any(|row| row.contains('#')),
                "{character:?} is blank"
            );
        }
    }

    /// Two letters drawn identically means one of them is wrong, and reading
    /// them back on a device is the only other way that surfaces.
    #[test]
    fn no_two_glyphs_are_drawn_the_same() {
        for (i, (left, left_rows)) in GLYPHS.iter().enumerate() {
            for (right, right_rows) in GLYPHS.iter().skip(i + 1) {
                assert_ne!(
                    left_rows, right_rows,
                    "{left:?} and {right:?} are drawn identically"
                );
            }
        }
    }

    #[test]
    fn the_alphabet_and_digits_are_all_present() {
        for character in ('A'..='Z').chain('0'..='9') {
            assert!(carries(character), "{character} is missing from the font");
        }
    }

    #[test]
    fn lowercase_is_drawn_as_its_capital() {
        assert_eq!(glyph('a'), glyph('A'));
        assert_eq!(glyph('z'), glyph('Z'));
    }

    #[test]
    fn an_unknown_character_is_a_visible_box_not_a_blank() {
        assert!(!carries('\u{263a}'));
        let drawn = glyph('\u{263a}');
        assert_eq!(drawn, MISSING);
        assert!(
            drawn.iter().any(|row| row.contains('#')),
            "a missing character must be visible"
        );
    }

    /// A label the font cannot draw renders as hollow boxes, which says
    /// nothing about what went wrong — and nothing at all to someone who
    /// cannot see the key. The caller needs the list so it can say.
    #[test]
    fn the_characters_a_font_cannot_draw_are_reported() {
        assert_eq!(missing_from("MUTE MIC").len(), 0);
        assert!(
            missing_from("mute mic").is_empty(),
            "lowercase is drawn as capitals"
        );
        assert_eq!(
            missing_from("\u{30df}\u{30e5}\u{30fc}\u{30c8}"),
            vec!['\u{30df}', '\u{30e5}', '\u{30fc}', '\u{30c8}']
        );
        assert_eq!(missing_from("MUTE \u{1f3a4}"), vec!['\u{1f3a4}']);
    }

    /// Eight unsupported characters are one problem with the label, not eight.
    #[test]
    fn a_character_repeated_is_reported_once() {
        assert_eq!(
            missing_from("\u{1f3a4}\u{1f3a4}\u{1f3a4}"),
            vec!['\u{1f3a4}']
        );
    }

    #[test]
    fn text_width_counts_the_gaps_between_glyphs() {
        assert_eq!(text_width(""), 0);
        assert_eq!(text_width("A"), GLYPH_WIDTH);
        // Two glyphs plus one gap.
        assert_eq!(text_width("AB"), GLYPH_WIDTH * 2 + 1);
        assert_eq!(text_width("ABC"), GLYPH_WIDTH * 3 + 2);
    }

    /// Symmetry the eye would catch immediately but a bit-array would not.
    #[test]
    fn glyphs_that_should_be_symmetric_are() {
        for character in ['A', 'H', 'I', 'M', 'O', 'T', 'U', 'V', 'W', 'X', 'Y', '8'] {
            for (index, row) in glyph(character).iter().enumerate() {
                let mirrored: String = row.chars().rev().collect();
                assert_eq!(
                    *row, mirrored,
                    "{character:?} row {index} is not left-right symmetric: {row:?}"
                );
            }
        }
    }
}
