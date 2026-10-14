//! Whether a paragraph's soft break (a source line break inside it) renders as a space or as
//! nothing, once a reflowed paragraph joins its lines into one flow.
//!
//! The segment break rule of earlier CSS Text Level 3 drafts, which the web-platform-tests check
//! and Firefox applies (the current drafts leave it to the browser): a break between two East
//! Asian Wide, Fullwidth or Halfwidth chars vanishes, since Chinese and Japanese put no spaces
//! between words; everywhere else it is a space.  Hangul keeps its space, as Korean spaces its
//! words.  The renderer
//! (`renderer::collapse_soft_breaks`), the inline column map and the row map
//! (`document::row_map`) all ask [`vanishes`] about the rendered chars on either side of a
//! break, so a dropped break drops out of the row's column mapping too.

use unicode_width::UnicodeWidthChar;

const ZERO_WIDTH_SPACE: char = '\u{200b}';

/// Whether a soft break between rendered chars `before` and `after` renders as nothing: either
/// is a zero-width space, or both are East Asian wide ([`is_east_asian_wide`]).
pub fn vanishes(before: char, after: char) -> bool {
    before == ZERO_WIDTH_SPACE
        || after == ZERO_WIDTH_SPACE
        || (is_east_asian_wide(before) && is_east_asian_wide(after))
}

/// East Asian Width Wide or Fullwidth (two cells), or Halfwidth, and not Hangul.
fn is_east_asian_wide(ch: char) -> bool {
    let halfwidth = matches!(ch, '\u{20a9}' | '\u{ff61}'..='\u{ff9f}' | '\u{ffe8}'..='\u{ffee}');
    (halfwidth || ch.width() == Some(2)) && !is_hangul(ch)
}

/// The Hangul script's blocks: jamo, compatibility jamo, enclosed Hangul, syllables, and the
/// halfwidth forms.
fn is_hangul(ch: char) -> bool {
    matches!(
        ch,
        '\u{1100}'..='\u{11ff}'
            | '\u{302e}'..='\u{302f}'
            | '\u{3130}'..='\u{318f}'
            | '\u{3200}'..='\u{321e}'
            | '\u{3260}'..='\u{327e}'
            | '\u{a960}'..='\u{a97f}'
            | '\u{ac00}'..='\u{d7ff}'
            | '\u{ffa0}'..='\u{ffdc}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_break_between_cjk_glyphs_vanishes() {
        for (before, after) in [
            ('の', 'リ'),
            ('語', '、'),
            ('。', '中'),
            ('ｱ', 'ｲ'),
            ('文', 'Ａ'),
        ] {
            assert!(vanishes(before, after), "{before:?} {after:?}");
        }
    }

    #[test]
    fn a_break_beside_latin_or_hangul_stays_a_space() {
        for (before, after) in [
            ('の', 'a'),
            ('a', 'の'),
            ('a', 'b'),
            ('어', '문'),
            ('の', '한'),
            ('ﾡ', 'ｱ'),
            ('の', ' '),
            ('の', '“'),
        ] {
            assert!(!vanishes(before, after), "{before:?} {after:?}");
        }
    }

    #[test]
    fn a_break_beside_a_zero_width_space_vanishes() {
        assert!(vanishes('a', ZERO_WIDTH_SPACE));
        assert!(vanishes(ZERO_WIDTH_SPACE, 'b'));
    }
}
