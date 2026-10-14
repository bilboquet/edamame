//! Grapheme-cluster boundary helpers over `Buffer`: convert "the next/previous grapheme from
//! this char offset" into a char offset the rope can index, so navigation and editing step over
//! user-perceived characters (ZWJ sequences, modifiers, combining marks) rather than scalar values.
//! The `str_*` helpers do the same over a short `String` (a text field's value), char-indexed
//! like the rope versions; they segment the whole value, so an index inside a cluster still
//! steps to that cluster's edges.  [`class_at`] is the one definition of a word, shared by
//! vim's word motions and objects and the mouse double-click.

use unicode_segmentation::UnicodeSegmentation;

use crate::document::Buffer;

/// Chars read on either side of the query offset before segmenting. Real clusters top out around
/// 7 chars (family emoji); 32 is headroom without materializing the whole rope.
const GRAPHEME_WINDOW: usize = 32;

/// Char offset of the grapheme-cluster boundary that follows `char_offset`.
///
/// Returns `buf.len_chars()` at end of buffer. An offset inside a grapheme yields the end of
/// that grapheme.
pub fn next_grapheme_offset(buf: &Buffer, char_offset: usize) -> usize {
    let len = buf.len_chars();
    if char_offset >= len {
        return len;
    }
    let end = (char_offset + GRAPHEME_WINDOW).min(len);
    let s = buf.rope().slice(char_offset..end).to_string();
    match s.graphemes(true).next() {
        Some(g) => char_offset + g.chars().count(),
        None => char_offset,
    }
}

/// Char offset of the grapheme-cluster boundary that precedes `char_offset`.
///
/// Returns `0` when already at the start of the buffer.
pub fn prev_grapheme_offset(buf: &Buffer, char_offset: usize) -> usize {
    if char_offset == 0 {
        return 0;
    }
    let start = char_offset.saturating_sub(GRAPHEME_WINDOW);
    let s = buf.rope().slice(start..char_offset).to_string();
    match s.graphemes(true).next_back() {
        Some(g) => char_offset - g.chars().count(),
        None => char_offset.saturating_sub(1),
    }
}

/// Char index of the grapheme-cluster boundary that follows char index `char_idx` in `s`.
///
/// Returns the char count at (or past) the end. An index inside a grapheme yields the end of
/// that grapheme.
pub fn str_next_grapheme(s: &str, char_idx: usize) -> usize {
    str_boundaries(s)
        .find(|&b| b > char_idx)
        .unwrap_or_else(|| s.chars().count())
}

/// Char index of the grapheme-cluster boundary that precedes char index `char_idx` in `s`.
///
/// Returns `0` at the start; an index past the end counts from the end. An index inside a
/// grapheme yields the start of that grapheme.
pub fn str_prev_grapheme(s: &str, char_idx: usize) -> usize {
    let char_idx = char_idx.min(s.chars().count());
    str_boundaries(s)
        .take_while(|&b| b < char_idx)
        .last()
        .unwrap_or(0)
}

/// Backspace: remove the grapheme cluster before char index `cursor` from `s` and return the
/// cursor's new char index (where that cluster began); a no-op returning `0` at the start.
pub fn str_remove_grapheme_before(s: &mut String, cursor: usize) -> usize {
    let cursor = cursor.min(s.chars().count());
    let start = str_prev_grapheme(s, cursor);
    let range = str_byte_index(s, start)..str_byte_index(s, cursor);
    s.replace_range(range, "");
    start
}

/// Delete: remove the grapheme cluster at char index `cursor` from `s`; a no-op at or past the
/// end.
pub fn str_remove_grapheme_at(s: &mut String, cursor: usize) {
    let end = str_next_grapheme(s, cursor);
    let range = str_byte_index(s, cursor)..str_byte_index(s, end);
    s.replace_range(range, "");
}

/// Byte offset of char index `char_idx` in `s`, clamped to `s.len()`.
pub fn str_byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(b, _)| b)
}

/// Char index of every grapheme-cluster boundary in `s`, ascending, `0` through the char count.
fn str_boundaries(s: &str) -> impl Iterator<Item = usize> + '_ {
    let ends = s.graphemes(true).scan(0, |chars, g| {
        *chars += g.chars().count();
        Some(*chars)
    });
    std::iter::once(0).chain(ends)
}

// ── Word class ──────────────────────────────────────────────────────────────────

/// Character class for word boundaries: vim's `w`/`e`/`b` and a double-click separate word
/// characters (alphanumeric + `_`) from punctuation, while `W`/`E`/`B` (`big`) treat every
/// non-blank as one class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Blank,
    Word,
    Punct,
}

fn class(c: char, big: bool) -> Class {
    if c.is_whitespace() {
        Class::Blank
    } else if big || c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// The [`Class`] of the char at `i`: the class of the first char of its glyph, so a glyph
/// never straddles a word boundary (a decomposed `cafe\u{301}` is one word).  `get_char`
/// indexes a rope or a rendered row alike.
///
/// The glyph start is found pairwise ([`joins_previous`]), so a mark after a space is blank,
/// as in vim, while one after a control char classes by itself.  The walk back is capped at
/// [`GRAPHEME_WINDOW`] to keep scans over a long run of marks linear; past that a mark
/// classes by itself, splitting "zalgo" text where the grapheme helpers split it too.
pub(crate) fn class_at(i: usize, big: bool, get_char: impl Fn(usize) -> char) -> Class {
    let floor = i.saturating_sub(GRAPHEME_WINDOW);
    let mut base = i;
    let mut c = get_char(i);
    while base > floor {
        let prev = get_char(base - 1);
        if !joins_previous(prev, c) {
            break;
        }
        base -= 1;
        c = prev;
    }
    class(c, big)
}

/// [`class_at`] over `buf`'s rope.
pub(crate) fn class_of(buf: &Buffer, offset: usize, big: bool) -> Class {
    let rope = buf.rope();
    class_at(offset, big, |i| rope.char(i))
}

/// Do `prev` and `c` form one grapheme cluster on their own?  Pairwise, so the rules that
/// need longer context (a ZWJ emoji sequence, regional-indicator pairs) can answer "no"
/// inside one glyph — harmless here, since every char of such a glyph is punctuation.
fn joins_previous(prev: char, c: char) -> bool {
    if prev.is_ascii() && c.is_ascii() {
        return prev == '\r' && c == '\n';
    }
    let mut buf = [0u8; 8];
    let n = prev.encode_utf8(&mut buf).len();
    let n = n + c.encode_utf8(&mut buf[n..]).len();
    let pair = std::str::from_utf8(&buf[..n]).expect("two encoded chars are valid UTF-8");
    pair.graphemes(true).nth(1).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Buffer {
        Buffer::from_str(s)
    }

    #[test]
    fn next_steps_over_ascii_one_char_at_a_time() {
        let buf = b("hi");
        assert_eq!(next_grapheme_offset(&buf, 0), 1);
        assert_eq!(next_grapheme_offset(&buf, 1), 2);
        assert_eq!(next_grapheme_offset(&buf, 2), 2); // EOF
    }

    #[test]
    fn prev_steps_over_ascii_one_char_at_a_time() {
        let buf = b("hi");
        assert_eq!(prev_grapheme_offset(&buf, 2), 1);
        assert_eq!(prev_grapheme_offset(&buf, 1), 0);
        assert_eq!(prev_grapheme_offset(&buf, 0), 0); // BOF
    }

    #[test]
    fn next_treats_single_codepoint_emoji_as_one_step() {
        let buf = b("🥇");
        assert_eq!(next_grapheme_offset(&buf, 0), 1);
        assert_eq!(prev_grapheme_offset(&buf, 1), 0);
    }

    #[test]
    fn next_treats_zwj_family_as_one_grapheme() {
        // 👨‍👩‍👧‍👦 = 7 chars (man + ZWJ + woman + ZWJ + girl + ZWJ + boy).
        let buf = b("👨\u{200D}👩\u{200D}👧\u{200D}👦x");
        assert_eq!(buf.len_chars(), 8);
        assert_eq!(next_grapheme_offset(&buf, 0), 7);
        assert_eq!(prev_grapheme_offset(&buf, 7), 0);
        assert_eq!(next_grapheme_offset(&buf, 7), 8);
    }

    #[test]
    fn next_treats_combining_mark_as_single_grapheme() {
        let buf = b("e\u{0301}!");
        assert_eq!(buf.len_chars(), 3);
        assert_eq!(next_grapheme_offset(&buf, 0), 2);
        assert_eq!(prev_grapheme_offset(&buf, 2), 0);
    }

    #[test]
    fn next_treats_skin_tone_modifier_as_one_grapheme() {
        let buf = b("👍\u{1F3FD}!");
        assert_eq!(next_grapheme_offset(&buf, 0), 2);
        assert_eq!(prev_grapheme_offset(&buf, 2), 0);
    }

    #[test]
    fn str_steps_over_whole_clusters() {
        // e + U+0301, the family (7 chars), then `x`.
        let s = "e\u{0301}👨\u{200D}👩\u{200D}👧\u{200D}👦x";
        assert_eq!(str_next_grapheme(s, 0), 2);
        assert_eq!(str_next_grapheme(s, 2), 9);
        assert_eq!(str_next_grapheme(s, 9), 10);
        assert_eq!(str_next_grapheme(s, 10), 10, "end");
        assert_eq!(str_prev_grapheme(s, 10), 9);
        assert_eq!(str_prev_grapheme(s, 9), 2);
        assert_eq!(str_prev_grapheme(s, 2), 0);
        assert_eq!(str_prev_grapheme(s, 0), 0, "start");
    }

    #[test]
    fn str_index_inside_a_cluster_steps_to_its_edges() {
        let s = "a👍\u{1F3FD}b";
        assert_eq!(str_next_grapheme(s, 2), 3, "inside the thumbs-up: its end");
        assert_eq!(
            str_prev_grapheme(s, 2),
            1,
            "inside the thumbs-up: its start"
        );
        assert_eq!(str_next_grapheme(s, 99), 4, "past the end clamps");
        assert_eq!(
            str_prev_grapheme(s, 99),
            3,
            "past the end counts from the end"
        );
    }

    /// Starting mid-flag, the segmentation must not pair the flag's second half with the next
    /// flag: the boundary is found in the whole value, not in the slice after the cursor.
    #[test]
    fn str_index_inside_a_flag_steps_to_its_edges() {
        let s = "🇺🇸🇬🇧";
        assert_eq!(str_next_grapheme(s, 1), 2);
        assert_eq!(str_prev_grapheme(s, 3), 2);
    }

    #[test]
    fn str_backspace_and_delete_remove_a_whole_cluster() {
        let mut s = "a❤\u{FE0F}e\u{0301}👨\u{200D}👩b".to_owned();
        // Delete at `❤️` (char 1) takes the variation selector with it.
        str_remove_grapheme_at(&mut s, 1);
        assert_eq!(s, "ae\u{0301}👨\u{200D}👩b");
        // Backspace from after the family (char 6) takes all three of its chars.
        assert_eq!(str_remove_grapheme_before(&mut s, 6), 3);
        assert_eq!(s, "ae\u{0301}b");
        // Backspace from after `é` takes the base and the accent.
        assert_eq!(str_remove_grapheme_before(&mut s, 3), 1);
        assert_eq!(s, "ab");
        // No-ops at the edges.
        assert_eq!(str_remove_grapheme_before(&mut s, 0), 0);
        str_remove_grapheme_at(&mut s, 2);
        assert_eq!(s, "ab");
    }

    mod prop {
        use proptest::prelude::*;

        use super::super::*;

        /// Values built from pieces that segment unusually: combining marks, ZWJ joins, skin
        /// tones, variation selectors, regional indicators, Hangul jamo, CRLF, and wide CJK.
        fn value() -> impl Strategy<Value = String> {
            let piece = prop::sample::select(vec![
                "a",
                "é",
                "\u{0301}",
                "\u{200D}",
                "👨",
                "👩",
                "\u{1F3FD}",
                "❤",
                "\u{FE0F}",
                "🇺",
                "🇸",
                "\u{1100}",
                "\u{1161}",
                "\r",
                "\n",
                "日",
                " ",
            ]);
            prop::collection::vec(piece, 0..12).prop_map(|ps| ps.concat())
        }

        /// The cluster boundaries as char indices, derived independently from byte offsets.
        fn boundaries(s: &str) -> Vec<usize> {
            s.grapheme_indices(true)
                .map(|(b, _)| s[..b].chars().count())
                .chain(std::iter::once(s.chars().count()))
                .collect()
        }

        proptest! {
            /// From every index, including ones inside a cluster and past the end, each step
            /// lands on the nearest boundary in its direction.
            #[test]
            fn steps_land_on_the_nearest_boundary(s in value()) {
                let bs = boundaries(&s);
                let n = s.chars().count();
                for i in 0..=n + 1 {
                    let next = bs.iter().copied().find(|&b| b > i).unwrap_or(n);
                    let prev = bs.iter().copied().rfind(|&b| b < i.min(n)).unwrap_or(0);
                    prop_assert_eq!(str_next_grapheme(&s, i), next, "next from {}", i);
                    prop_assert_eq!(str_prev_grapheme(&s, i), prev, "prev from {}", i);
                }
            }

            /// Backspace and Delete remove exactly the chars between the cursor and the
            /// neighboring boundary, and Backspace returns where the cursor lands.
            #[test]
            fn removals_cut_exactly_one_step(s in value()) {
                let chars: Vec<char> = s.chars().collect();
                let n = chars.len();
                for i in 0..=n {
                    let prev = str_prev_grapheme(&s, i);
                    let mut back = s.clone();
                    prop_assert_eq!(str_remove_grapheme_before(&mut back, i), prev);
                    let expected: String = chars[..prev].iter().chain(&chars[i..]).collect();
                    prop_assert_eq!(back, expected);

                    let next = str_next_grapheme(&s, i);
                    let mut del = s.clone();
                    str_remove_grapheme_at(&mut del, i);
                    let expected: String = chars[..i].iter().chain(&chars[next..]).collect();
                    prop_assert_eq!(del, expected);
                }
            }
        }
    }

    // ── class_at ──────────────────────────────────────────────────────────────

    fn class_in(text: &str, i: usize) -> Class {
        let chars: Vec<char> = text.chars().collect();
        class_at(i, false, |j| chars[j])
    }

    #[test]
    fn class_at_gives_a_mark_the_class_of_the_char_it_marks() {
        assert_eq!(class_in("e\u{301}", 1), Class::Word);
        assert_eq!(class_in("❤\u{FE0F}", 1), Class::Punct);
        assert_eq!(class_in("e\u{301}\u{302}", 2), Class::Word, "stacked marks");
        // After a space the mark joins the space's glyph; after a control char it stands alone.
        assert_eq!(class_in(" \u{301}", 1), Class::Blank);
        assert_eq!(class_in("a\n\u{301}", 2), Class::Punct);
        assert_eq!(class_in("a\t\u{301}", 2), Class::Punct);
    }

    #[test]
    fn class_at_treats_zero_width_joiners_as_extending() {
        assert_eq!(class_in("a\u{200D}", 1), Class::Word);
        assert_eq!(class_in("a\u{200C}", 1), Class::Word);
        assert_eq!(class_in("👨\u{200D}👩", 1), Class::Punct);
    }

    #[test]
    fn class_at_gives_every_char_of_a_glyph_its_first_chars_class() {
        // A skin tone (Sk) after a letter, and a digit after a Prepend number sign, join the
        // glyph without being marks.
        assert_eq!(class_in("a\u{1F3FD}", 1), Class::Word);
        assert_eq!(class_in("\u{600}1", 1), Class::Punct);
        assert_eq!(class_in("\r\n", 1), Class::Blank);
        assert_eq!(class_in("ab", 1), Class::Word, "ASCII never joins");
    }

    #[test]
    fn class_at_walk_back_is_bounded_over_a_long_run_of_marks() {
        // Untrusted "zalgo" text: classing every char of the run used to re-walk it to the
        // base each time, quadratic in its length (seconds for a few thousand marks).
        let n = 10_000;
        let chars: Vec<char> = std::iter::once('e')
            .chain(std::iter::repeat_n('\u{301}', n))
            .collect();
        let reads = std::cell::Cell::new(0usize);
        for i in 0..chars.len() {
            class_at(i, false, |j| {
                reads.set(reads.get() + 1);
                chars[j]
            });
        }
        assert!(reads.get() <= chars.len() * (2 * GRAPHEME_WINDOW + 2));
    }
}
