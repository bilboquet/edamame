//! The raw-reveal overlays of the cursor's table cell: which raw text the revealed cell shows,
//! and where.  The painter (`ui::rendered_view`) draws them, and the click mapper
//! (`editor::mouse_ops::coord`) maps a click on the revealed cell through the same overlay, so
//! the two agree on which raw char each screen cell shows.

use std::ops::Range;

use ratatui::text::Line;

use super::table_layout::{
    cells_of, drawn_cell_at, hard_wrap_ranges, last_cluster_start, rendered_pipe_cells, str_cells,
    unpadded_cell_shift, wrap_cell_with_indices,
};

/// The overlay of a revealed cell on a one-line row: [`compute_cell_overlay`] when the raw
/// text fits the rendered cell, else [`compute_cell_chunk_overlay`], which scrolls it
/// horizontally.  `None` when neither applies (a separator, a cell GFM drops), and the row
/// reveals whole.
pub fn single_line_cell_overlay(
    raw_row: &str,
    content_col: usize,
    rendered_line: &Line<'_>,
    cursor_col_raw: usize,
) -> Option<CellOverlay> {
    compute_cell_overlay(raw_row, content_col, rendered_line, cursor_col_raw)
        .or_else(|| compute_cell_chunk_overlay(raw_row, content_col, rendered_line, cursor_col_raw))
}

/// Metadata for overlaying a raw cell on a rendered table row.  `rendered_start..
/// rendered_end` spans the content area between the two `│` characters, exclusive, in
/// terminal cells; `raw_text` is padded/clamped to that width when painted so borders and
/// neighboring cells stay intact.
pub struct CellOverlay {
    pub rendered_start: usize,
    pub rendered_end: usize,
    pub raw_text: String,
    /// Cursor offset within `raw_text` in chars — past its end, each further index is one
    /// blank cell; `None` when it sits outside the overlay area, which means the caller takes
    /// the fallback path.
    pub cursor_in_cell: Option<usize>,
    /// Byte offset in the raw row where this cell's content starts, so the caller can
    /// align an absolute selection byte range onto `raw_text`.
    pub raw_cell_byte_start: usize,
    /// Screen cell of the cell's left `│`: a click from there up to `rendered_end` is in
    /// this cell.
    pub left_pipe: usize,
    /// Chars of the raw row holding the whole cell's content, its pad whitespace trimmed
    /// ([`content_range`]): the span a click on the revealed cell clamps into.
    pub raw_content: Range<usize>,
}

/// The chars of `cell`, a char range of the raw row whose chars are `cell_chars`, with its
/// leading and trailing whitespace trimmed; empty at the cell's end when it is all whitespace.
fn content_range(cell_chars: &[char], cell: Range<usize>) -> Range<usize> {
    let leading = cell_chars.iter().take_while(|c| c.is_whitespace()).count();
    let trailing = cell_chars[leading..]
        .iter()
        .rev()
        .take_while(|c| c.is_whitespace())
        .count();
    cell.start + leading..cell.end - trailing
}

/// A cell-scoped overlay for the cursor's active cell.
///
/// The cell is [`drawn_cell_at`]'s, from `content_col`.  `None` when the row isn't a row of
/// cells (the alignment row renders as `├─┼─┤`) or the cursor is in a cell GFM drops, both of
/// which reveal the whole line, or when the raw cell text is wider (in cells) than the rendered
/// area — the caller then falls back to the horizontally scrolling chunk overlay.
fn compute_cell_overlay(
    raw_row: &str,
    content_col: usize,
    rendered_line: &Line<'_>,
    cursor_col: usize,
) -> Option<CellOverlay> {
    let rendered_pipes = rendered_pipe_cells(rendered_line);
    let drawn = rendered_pipes.len().saturating_sub(1);
    let (cell_idx, raw_cell) = drawn_cell_at(raw_row, content_col, drawn, cursor_col)?;

    let raw_cell_start = raw_cell.start;
    let raw_cell_end = raw_cell.end;
    let raw_text: String = raw_row
        .chars()
        .skip(raw_cell_start)
        .take(raw_cell_end - raw_cell_start)
        .collect();

    let raw_cell_byte_start = raw_row
        .char_indices()
        .nth(raw_cell_start)
        .map(|(b, _)| b)
        .unwrap_or(raw_row.len());

    let rendered_start = rendered_pipes[cell_idx] + 1 + unpadded_cell_shift(&raw_text);
    let rendered_end = rendered_pipes[cell_idx + 1];
    let rendered_width = rendered_end.saturating_sub(rendered_start);

    if str_cells(&raw_text) > rendered_width {
        return None;
    }

    // The cell the cursor lands on; past the text each char index is one blank cell.
    let cursor_offset = cursor_col.saturating_sub(raw_cell_start);
    let raw_chars: Vec<char> = raw_text.chars().collect();
    let cursor_cell = cells_of(&raw_chars[..cursor_offset.min(raw_chars.len())])
        + cursor_offset.saturating_sub(raw_chars.len());
    let cursor_in_cell = if cursor_cell < rendered_width {
        Some(cursor_offset)
    } else if cursor_cell == rendered_width {
        // On the closing pipe of a full cell: show it on the last glyph.
        Some(if cursor_offset <= raw_chars.len() {
            last_cluster_start(&raw_chars[..cursor_offset])
        } else {
            cursor_offset - 1
        })
    } else {
        None
    };

    Some(CellOverlay {
        rendered_start,
        rendered_end,
        raw_text,
        cursor_in_cell,
        raw_cell_byte_start,
        left_pipe: rendered_pipes[cell_idx],
        raw_content: content_range(&raw_chars, raw_cell),
    })
}

/// Overlay for a cell whose raw markdown is wider than its rendered cell: hard-wraps the
/// source into chunks of at most `cell_width` cells and returns the chunk under the cursor, so
/// the cell scrolls horizontally as the user types (Raw mode shows the whole cell).
///
/// Hard-wrap ([`hard_wrap_ranges`]) rather than word-wrap so chunk boundaries depend only on
/// glyph widths and the cursor never jumps chunks mid-word.  The cell is [`drawn_cell_at`]'s,
/// from `content_col`.
fn compute_cell_chunk_overlay(
    raw_row: &str,
    content_col: usize,
    rendered_line: &Line<'_>,
    cursor_col_raw: usize,
) -> Option<CellOverlay> {
    let rendered_pipes = rendered_pipe_cells(rendered_line);
    let drawn = rendered_pipes.len().saturating_sub(1);
    let (cell_idx, raw_cell) = drawn_cell_at(raw_row, content_col, drawn, cursor_col_raw)?;

    let raw_cell_start = raw_cell.start;
    let raw_cell_end = raw_cell.end;
    let raw_cell_text: String = raw_row
        .chars()
        .skip(raw_cell_start)
        .take(raw_cell_end - raw_cell_start)
        .collect();
    let rendered_start = rendered_pipes[cell_idx] + 1 + unpadded_cell_shift(&raw_cell_text);
    let rendered_end = rendered_pipes[cell_idx + 1];
    let cell_width = rendered_end.saturating_sub(rendered_start);
    if cell_width == 0 {
        return None;
    }

    let raw_chars: Vec<char> = raw_cell_text.chars().collect();
    if str_cells(&raw_cell_text) <= cell_width {
        // Fits — `compute_cell_overlay` should have been chosen; let the caller fall through.
        return None;
    }

    let chunks = hard_wrap_ranges(&raw_chars, cell_width);
    let cursor_in_cell = cursor_col_raw.saturating_sub(raw_cell_start);
    let chunk = chunks
        .iter()
        .rfind(|r| r.start <= cursor_in_cell)
        .unwrap_or(&chunks[0]);
    let chunk_start_chars = chunk.start;
    let chunk_chars = &raw_chars[chunk.clone()];
    let chunk: String = chunk_chars.iter().collect();
    // A cursor past a full chunk's last cell shows on its last glyph.
    let mut col_in_chunk = cursor_in_cell - chunk_start_chars;
    if cells_of(&chunk_chars[..col_in_chunk.min(chunk_chars.len())]) >= cell_width {
        col_in_chunk = last_cluster_start(chunk_chars);
    }

    // Byte offset of the chunk's first char inside `raw_row`, for selection mapping.
    let chunk_byte_start = raw_row
        .char_indices()
        .nth(raw_cell_start + chunk_start_chars)
        .map(|(b, _)| b)
        .unwrap_or(raw_row.len());

    Some(CellOverlay {
        rendered_start,
        rendered_end,
        raw_text: chunk,
        cursor_in_cell: Some(col_in_chunk),
        raw_cell_byte_start: chunk_byte_start,
        left_pipe: rendered_pipes[cell_idx],
        raw_content: content_range(&raw_chars, raw_cell),
    })
}

/// Cursor position inside a *wrapped* table cell (one that broke onto several rendered
/// sub-lines). `RenderedView::render` uses it to move `cursor_rendered_line` onto the right
/// sub once the cell reveals.
pub struct WrappedCellOverlay {
    /// Sub-line index in `lines` of the row's first rendered sub.
    pub row_first_line_idx: usize,
    /// One entry per rendered sub-line of the row, painted on
    /// `lines[row_first_line_idx + i]`. Trailing entries are blank when the raw
    /// text wraps to fewer chunks than the row's height, so the painter wipes the stale tail.
    pub subs: Vec<CellOverlay>,
    /// Index within `subs` that contains the cursor.
    pub cursor_sub: usize,
}

/// One `CellOverlay` per rendered sub-line of the cursor's row, mapping word-wrap chunks of the
/// raw cell text onto the sub-lines. Returns `None` for single-sub rows, which
/// `compute_cell_overlay` / `compute_cell_chunk_overlay` handle.
///
/// `row_lines` are the rows of `lines` (the document's rendered lines) showing the table row's
/// cells (its origins' [`row_map::table_row`](crate::document::row_map::table_row)), `raw_row`
/// its source line, and `content_col` the row's content column there (the hit's `raw_col`).
///
/// Raw text is wider than rendered (markers the renderer drops), so it routinely wraps to more
/// chunks than the row has sub-lines; the overlay then scrolls a `row_height`-chunk window
/// containing the cursor's chunk onto the row.
pub fn compute_wrapped_cell_overlay(
    lines: &[Line<'_>],
    row_lines: Range<usize>,
    raw_row: &str,
    content_col: usize,
    cursor_col_raw: usize,
) -> Option<WrappedCellOverlay> {
    let row_height = row_lines.len();
    let block_lines = lines.get(row_lines.clone())?;

    // Every wrap sub-line of a row has identical pipe cells (see `render_table_row`); the
    // char positions differ per sub-line once a wide glyph sits left of the cell.
    let first_line = block_lines.first()?;
    let rendered_pipes = rendered_pipe_cells(first_line);
    let drawn = rendered_pipes.len().saturating_sub(1);
    let (cell_idx, raw_cell) = drawn_cell_at(raw_row, content_col, drawn, cursor_col_raw)?;

    let raw_cell_start_char = raw_cell.start;
    let raw_cell_end_char = raw_cell.end;
    let raw_cell_text: String = raw_row
        .chars()
        .skip(raw_cell_start_char)
        .take(raw_cell_end_char - raw_cell_start_char)
        .collect();
    let cell_rendered_start = rendered_pipes[cell_idx] + 1;
    let cell_rendered_end = rendered_pipes[cell_idx + 1];
    // Minus the leading/trailing padding space the renderer emits around cell content.
    let content_width = cell_rendered_end
        .saturating_sub(cell_rendered_start)
        .saturating_sub(2);
    if content_width == 0 {
        return None;
    }

    // Wrap the *trimmed* content: the pad whitespace belongs to the rendered pad columns, so
    // the chunks start on the content the cell shows.
    let raw_chars: Vec<char> = raw_cell_text.chars().collect();
    let raw_content = content_range(&raw_chars, raw_cell);
    let raw_leading = raw_content.start - raw_cell_start_char;
    let trimmed: String = raw_chars[raw_leading..raw_content.end - raw_cell_start_char]
        .iter()
        .collect();

    // Word-wrap drops whitespace at break points, so a cursor on dropped whitespace maps to
    // the start of the next visible row.
    let wrapped = wrap_cell_with_indices(&trimmed, content_width);
    if wrapped.is_empty() {
        return None;
    }

    // Single-sub rows fall back to the horizontal-scroll overlays. Multi-sub rows must not:
    // replacing one sub-line would leave the others painted as a stale tail.
    if row_height <= 1 {
        return None;
    }

    // Offsets are relative to the trimmed content; a cursor on the leading pad clamps to the
    // first content char.
    let cursor_in_cell = cursor_col_raw.saturating_sub(raw_cell_start_char + raw_leading);
    let last_idx = wrapped.len() - 1;
    let mut cursor_sub = last_idx;
    let mut cursor_col_in_chunk = wrapped[last_idx].1.chars().count();
    for (i, (start_idx, row_text)) in wrapped.iter().enumerate() {
        let next_start = wrapped.get(i + 1).map(|(s, _)| *s).unwrap_or(usize::MAX);
        if cursor_in_cell < next_start {
            cursor_sub = i;
            let row_chars = row_text.chars().count();
            let pos_in_row = cursor_in_cell.saturating_sub(*start_idx);
            cursor_col_in_chunk = pos_in_row.min(row_chars);
            break;
        }
    }

    // Bottom-anchored minimal scroll: a chunk below `row_height` stays on its own sub-line, or
    // the text jumps upward when the reveal fires.
    let window_start = cursor_sub.saturating_sub(row_height - 1);
    let window = &wrapped[window_start..(window_start + row_height).min(wrapped.len())];
    let cursor_sub = cursor_sub - window_start;

    // Char index → byte offset, with a sentinel so indexing past the last char is safe.
    let raw_row_byte_at: Vec<usize> = raw_row
        .char_indices()
        .map(|(b, _)| b)
        .chain(std::iter::once(raw_row.len()))
        .collect();

    let mut subs: Vec<CellOverlay> = Vec::with_capacity(window.len());
    for (i, (start_in_cell, chunk_text)) in window.iter().enumerate() {
        // Chunks are trimmed, so paint one column right of the cell edge and let the
        // renderer's leading pad space show through.
        let painted_start = cell_rendered_start + 1;
        let chunk_first_char_in_row = raw_cell_start_char + raw_leading + start_in_cell;
        let raw_cell_byte_start = raw_row_byte_at
            .get(chunk_first_char_in_row)
            .copied()
            .unwrap_or(raw_row.len());
        let cursor_in_cell = if i == cursor_sub {
            Some(cursor_col_in_chunk.min(chunk_text.chars().count()))
        } else {
            None
        };
        subs.push(CellOverlay {
            rendered_start: painted_start,
            rendered_end: cell_rendered_end,
            raw_text: chunk_text.clone(),
            cursor_in_cell,
            raw_cell_byte_start,
            left_pipe: rendered_pipes[cell_idx],
            raw_content: raw_content.clone(),
        });
    }

    // Fewer chunks than sub-lines (styled and raw wraps break differently, and an edit can
    // shrink the raw text before the row height is re-parsed): pad with blank overlays so
    // `overlay_raw_cell` wipes the stale rendered tail.
    let cell_end_byte = raw_row_byte_at
        .get(raw_cell_end_char)
        .copied()
        .unwrap_or(raw_row.len());
    while subs.len() < row_height {
        subs.push(CellOverlay {
            rendered_start: cell_rendered_start,
            rendered_end: cell_rendered_end,
            raw_text: String::new(),
            cursor_in_cell: None,
            raw_cell_byte_start: cell_end_byte,
            left_pipe: rendered_pipes[cell_idx],
            raw_content: raw_content.clone(),
        });
    }

    Some(WrappedCellOverlay {
        row_first_line_idx: row_lines.start,
        subs,
        cursor_sub,
    })
}

#[cfg(test)]
mod tests {
    use ratatui::text::Span;

    use super::*;
    use crate::config::Theme;
    use crate::document::{row_map, Buffer};
    use crate::markdown::inline_col_map::RefLabels;
    use crate::markdown::table_layout::table_raw_col_to_rendered;

    fn line_with(s: &str) -> Line<'static> {
        Line::from(vec![Span::raw(s.to_owned())])
    }

    /// The rows showing the first data row's cells of the table opening the document.
    fn data_row(state: &crate::editor::EditorState) -> std::ops::Range<usize> {
        let row = row_map::row_for_line(&state.parsed, 0, 2);
        row_map::table_row(&state.parsed, 0, row).unwrap().rows
    }

    /// Chunks are cut by cells, not chars: each fits the cell's six cells, and the cursor lands
    /// in the chunk that holds it.
    #[test]
    fn chunk_overlay_cuts_wide_text_by_cells() {
        let line = Line::from("│ abcd │");
        let raw = "| 日本語日本語 |";
        // Cell text ` 日本語日本語 ` chunks as ` 日本` | `語日本` | `語 `.
        let second_go = raw
            .char_indices()
            .filter(|(_, c)| *c == '語')
            .nth(1)
            .unwrap()
            .0;
        let cursor = raw[..second_go].chars().count();
        let ov = compute_cell_chunk_overlay(raw, 0, &line, cursor).expect("wider than the cell");
        assert_eq!(ov.raw_text, "語 ");
        assert_eq!(ov.cursor_in_cell, Some(0));
        assert_eq!((ov.rendered_start, ov.rendered_end), (1, 7));
        assert_eq!(ov.raw_cell_byte_start, second_go);

        for cursor in 1..raw.chars().count() {
            let ov = compute_cell_chunk_overlay(raw, 0, &line, cursor).unwrap();
            assert!(str_cells(&ov.raw_text) <= 6, "{:?}", ov.raw_text);
            let col = ov.cursor_in_cell.unwrap();
            let before: String = ov.raw_text.chars().take(col).collect();
            assert!(str_cells(&before) < 6, "cursor cell inside the overlay");
        }
    }

    /// A cursor in a cell GFM drops gets no chunk overlay either, so the row reveals whole; the
    /// last drawn cell, too wide for its rendered cell, still scrolls.
    #[test]
    fn chunk_overlay_skips_a_dropped_cell() {
        let line = Line::from("│ ab │");
        let raw = "| abcdefgh | x |";
        assert!(compute_cell_chunk_overlay(raw, 0, &line, raw.find('x').unwrap()).is_none());
        assert!(compute_cell_chunk_overlay(raw, 0, &line, raw.find('f').unwrap()).is_some());
    }

    /// A cluster that straddles a chunk boundary moves whole into the next chunk: every chunk
    /// but one holding a single over-wide cluster fits the cell, so the cursor stays on screen.
    #[test]
    fn chunk_overlay_never_packs_a_cluster_past_the_cell() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let line = Line::from("│ abcd │");
        let raw = format!("| ab{family}xyzw |");
        let mut texts = Vec::new();
        for cursor in 1..raw.chars().count() {
            let ov =
                compute_cell_chunk_overlay(&raw, 0, &line, cursor).expect("wider than the cell");
            assert!(
                str_cells(&ov.raw_text) <= 6 || ov.raw_text == family,
                "{:?}",
                ov.raw_text
            );
            if texts.last() != Some(&ov.raw_text) {
                texts.push(ov.raw_text);
            }
        }
        assert_eq!(texts, vec![" ab", family, "xyzw "]);
    }

    /// A cursor past a full last chunk clamps onto its last glyph — the `e`, not the zero-width
    /// accent after it, which has no cell of its own.
    #[test]
    fn chunk_overlay_clamps_a_cursor_onto_the_last_cluster() {
        let line = Line::from("│ abcd │");
        // Cell text ` abcdefghije\u{301}` chunks as ` abcde` | `fghije\u{301}` (six cells each).
        let raw = "| abcdefghije\u{301}|";
        let closing = raw.chars().count() - 1;
        let ov = compute_cell_chunk_overlay(raw, 0, &line, closing).expect("wider than the cell");
        assert_eq!(ov.raw_text, "fghije\u{301}");
        assert_eq!(ov.cursor_in_cell, Some(5));
    }

    /// Raw text wider than the rendered height must take the multi-sub path; the single-sub
    /// fallback would leave the row's other wrap rows as a stale tail.
    #[test]
    fn raw_wider_than_rendered_height_scrolls_vertically() {
        let theme: &'static Theme = Box::leak(Box::new(Theme::default()));
        let src = "| a | b |\n|---|---|\n| x | `aa` `bb` `cc` `dd` `ee` |\n";
        let mut state = crate::editor::EditorState::new(Buffer::from_str(src), theme);
        state.set_viewport_width(18);

        let raw_row = "| x | `aa` `bb` `cc` `dd` `ee` |";
        let cursor_col = raw_row.find("cc").unwrap(); // ASCII: byte == char col

        let overlay = compute_wrapped_cell_overlay(
            &state.parsed.lines,
            data_row(&state),
            raw_row,
            0,
            cursor_col,
        )
        .expect("multi-sub row must use the wrapped-cell overlay, not the chunk fallback");

        assert!(overlay.subs.len() >= 2, "fixture row must wrap");
        assert!(
            overlay.subs.iter().all(|s| !s.raw_text.is_empty()),
            "raw chunks must cover every sub-line: {:?}",
            overlay.subs.iter().map(|s| &s.raw_text).collect::<Vec<_>>()
        );
        let joined: String = overlay.subs.iter().map(|s| s.raw_text.as_str()).collect();
        assert!(
            joined.contains('`'),
            "overlay must reveal raw backticks: {joined:?}"
        );
        assert!(overlay.cursor_sub < overlay.subs.len());
    }

    /// Regression: untrimmed wrapping plus a top-anchored window yanked the cursor's chunk up
    /// to the first sub-line and scrolled the span's start out of view.
    #[test]
    fn click_on_second_wrap_line_keeps_span_start_visible() {
        let theme: &'static Theme = Box::leak(Box::new(Theme::default()));
        let src = "| a | b |\n|---|---|\n| x | `tracing-appender` |\n";
        let mut state = crate::editor::EditorState::new(Buffer::from_str(src), theme);
        state.set_viewport_width(17);

        let raw_row = "| x | `tracing-appender` |";
        // 'a' of "appender" renders on the row's second wrap sub-line.
        let cursor_col = raw_row.find("appender").unwrap(); // ASCII: byte == char col

        let overlay = compute_wrapped_cell_overlay(
            &state.parsed.lines,
            data_row(&state),
            raw_row,
            0,
            cursor_col,
        )
        .expect("wrapped code-span cell must use the multi-sub overlay");

        assert_eq!(overlay.subs.len(), 2, "fixture row wraps to two sub-lines");
        assert!(
            overlay.cursor_sub > 0,
            "cursor clicked on the second sub-line must stay below the first chunk"
        );
        assert!(
            overlay.subs[0].raw_text.starts_with('`'),
            "the span's start must stay visible on the first sub-line: {:?}",
            overlay.subs.iter().map(|s| &s.raw_text).collect::<Vec<_>>()
        );
        assert!(
            overlay.subs.iter().all(|s| !s.raw_text.is_empty()),
            "no sub-line may be wasted on a pad-space chunk: {:?}",
            overlay.subs.iter().map(|s| &s.raw_text).collect::<Vec<_>>()
        );
    }

    /// A wide glyph in the cell to the left shifts every later pipe by a cell per glyph; the
    /// overlay addresses screen cells, so it must land past them.
    #[test]
    fn compute_cell_overlay_places_the_cell_in_screen_cells() {
        // Cells: │0 ␠1 日2-3 本4-5 ␠6 │7 ␠8 a9 b10 ␠11 │12
        let line = line_with("│ 日本 │ ab │");
        let raw = "| 日本 | ab |";
        let cursor = raw.chars().position(|c| c == 'a').unwrap();
        let ov = compute_cell_overlay(raw, 0, &line, cursor).expect("fits");
        assert_eq!((ov.rendered_start, ov.rendered_end), (8, 12));
        assert_eq!(ov.raw_text, " ab ");
        assert_eq!(ov.cursor_in_cell, Some(1));
    }

    #[test]
    fn compute_cell_overlay_measures_the_raw_text_in_cells() {
        // Content area of 6 cells: ` 日本 ` (6) fits, ` 日本語 ` (8) does not.
        let line = line_with("│ abcd │");
        assert!(compute_cell_overlay("| 日本 |", 0, &line, 2).is_some());
        assert!(compute_cell_overlay("| 日本語 |", 0, &line, 2).is_none());
    }

    /// On the closing pipe of a cell the raw text fills exactly, the cursor shows on the last
    /// glyph instead of past the border.
    #[test]
    fn compute_cell_overlay_clamps_a_cursor_past_a_full_cell() {
        let line = line_with("│ abcd │");
        let raw = "| 日本 |";
        let closing = raw.chars().count() - 1;
        let ov = compute_cell_overlay(raw, 0, &line, closing).expect("fits");
        assert_eq!(ov.cursor_in_cell, Some(3), "on the trailing pad char");
    }

    /// When the last char is a zero-width mark, the clamp lands on its cluster's base char:
    /// a zero-width char has no cell of its own, so a cursor on the mark would not be drawn.
    #[test]
    fn compute_cell_overlay_clamps_a_cursor_onto_the_last_cluster() {
        // ` ` ` ` a b c e U+0301: seven chars, six cells — exactly the content area.
        let line = line_with("│ abcd │");
        let raw = "|  abce\u{301}|";
        let closing = raw.chars().count() - 1;
        let ov = compute_cell_overlay(raw, 0, &line, closing).expect("fits");
        assert_eq!(ov.cursor_in_cell, Some(5), "on the `e`, not the accent");
    }

    #[test]
    fn compute_cell_overlay_none_when_raw_exceeds_rendered_width() {
        let raw = "| supercalifragilistic | b |";
        let rendered = line_with("│ a │ b │");
        assert!(compute_cell_overlay(raw, 0, &rendered, 3).is_none());
    }

    #[test]
    fn compute_cell_overlay_returns_metadata_when_fits() {
        let raw = "| a | b |";
        let rendered = line_with("│ a │ b │");
        let overlay = compute_cell_overlay(raw, 0, &rendered, 2).expect("overlay fits");
        assert_eq!(overlay.raw_text, " a ");
        assert_eq!(overlay.rendered_start, 1);
        assert_eq!(overlay.rendered_end, 4);
    }

    /// A row without edge pipes reveals cell by cell, its unpadded first cell one cell in so
    /// its text covers the rendered text, not the pad.
    #[test]
    fn compute_cell_overlay_pairs_cells_without_edge_pipes() {
        let rendered = line_with("│ a │ b │");
        let a = compute_cell_overlay("a | b", 0, &rendered, 0).expect("fits");
        assert_eq!(a.raw_text, "a ");
        assert_eq!((a.rendered_start, a.rendered_end), (2, 4));
        assert_eq!(a.cursor_in_cell, Some(0));
        let b = compute_cell_overlay("a | b", 0, &rendered, 4).expect("fits");
        assert_eq!(b.raw_text, " b");
        assert_eq!((b.rendered_start, b.rendered_end), (5, 8));
        assert_eq!(b.cursor_in_cell, Some(1));
    }

    /// A cursor past a short row's closing pipe reveals the first missing cell, empty, with the
    /// cursor on its first content cell, where the indicator before the reveal shows it too.
    #[test]
    fn a_cursor_past_a_short_rows_closing_pipe_shows_in_the_missing_cell() {
        let raw = "| 1 |";
        let rendered = line_with("│ 1 │   │   │");
        let ov = compute_cell_overlay(raw, 0, &rendered, 5).expect("fits");
        assert_eq!(ov.raw_text, "");
        assert_eq!((ov.rendered_start, ov.rendered_end), (6, 8));
        assert_eq!(ov.cursor_in_cell, Some(0));
        assert_eq!(ov.raw_cell_byte_start, 5);
        let labels = RefLabels::default();
        assert_eq!(
            table_raw_col_to_rendered(raw, 0, &[rendered], 5, &labels),
            Some((0, 6))
        );
    }

    /// A cursor in a cell GFM drops has no drawn cell: no cell overlay and no indicator, so the
    /// row reveals whole and the cursor shows on its raw text.
    #[test]
    fn a_cursor_in_a_dropped_cell_has_no_cell_overlay() {
        let raw = "| 1 | 2 | 3 | 4 | 5 |";
        let rendered = line_with("│ 1 │ 2 │ 3 │");
        let five = raw.find('5').unwrap();
        assert!(compute_cell_overlay(raw, 0, &rendered, five).is_none());
        let labels = RefLabels::default();
        assert!(
            table_raw_col_to_rendered(raw, 0, std::slice::from_ref(&rendered), five, &labels)
                .is_none()
        );
        // The last drawn cell, up to its closing pipe, still pairs.
        let three = raw.find('3').unwrap();
        assert!(compute_cell_overlay(raw, 0, &rendered, three).is_some());
    }
}
