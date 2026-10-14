//! Block-level render memoization for the parse → render pipeline; see
//! docs/dev/performance.md.
//!
//! [`RenderCache`] keys each block's rendered rows (lines and their origins) by the block's AST
//! *value*, not by its source bytes: everything that changes rendering without changing source
//! text (table-width drag overrides, post-pass promotions) mutates the AST, so such a block
//! simply misses the cache.
//!
//! `Block::ImageBlock` is never cached — its row count depends on the image decode cache,
//! which changes out-of-band as decodes complete, and re-rendering a placeholder is free.

use std::hash::BuildHasher;

use hashbrown::HashTable;
use ratatui::text::Line;
use rustc_hash::FxBuildHasher;

use super::ast::Block;
use super::row_origin::RowOrigin;

/// The cache's block map. Keyed by whole `Block` AST values, so a lookup hashes a deep
/// structure — many small `write_*` calls — on every query. std's DoS-resistant SipHash is
/// both wasted (the keys are local document content, never adversarial network input) and slow
/// at that write pattern, which is what makes the cache a net loss on cheap-to-render blocks
/// (`lists`, `math`; see docs/dev/performance.md and issue #35). `FxHasher` is built for small
/// struct keys and measurably faster here; the AST keying — hence correctness — is unchanged.
/// (seahash, the crate's other non-crypto hasher, is for whole byte buffers and benched *slower*
/// than SipHash on these keys.)
///
/// A `hashbrown::HashTable` rather than a `HashMap` so a block is hashed once per build: the
/// caller computes [`BlockMap::hash`] and passes it to every probe and insert, where a
/// `HashMap` hit (`remove_entry` from the previous build, `insert` into this one) hashed twice.
#[derive(Debug, Default)]
pub(super) struct BlockMap {
    table: HashTable<(u64, Block, CachedRows)>,
}

impl BlockMap {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            table: HashTable::with_capacity(capacity),
        }
    }

    /// `block`'s hash, for the `*_hashed` calls.
    pub fn hash(block: &Block) -> u64 {
        FxBuildHasher.hash_one(block)
    }

    /// Remove and return `block`'s entry, `hash` being [`Self::hash`] of it.
    pub fn take(&mut self, hash: u64, block: &Block) -> Option<(Block, CachedRows)> {
        self.table
            .find_entry(hash, |(_, key, _)| key == block)
            .ok()
            .map(|entry| {
                let ((_, key, rows), _) = entry.remove();
                (key, rows)
            })
    }

    pub fn get(&self, hash: u64, block: &Block) -> Option<&CachedRows> {
        self.table
            .find(hash, |(_, key, _)| key == block)
            .map(|(_, _, rows)| rows)
    }

    /// Insert an entry for a block known to be absent: every caller has just probed for it.
    pub fn insert_new(&mut self, hash: u64, block: Block, rows: CachedRows) {
        debug_assert!(self.get(hash, &block).is_none());
        self.table
            .insert_unique(hash, (hash, block, rows), |(hash, _, _)| *hash);
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub fn clear(&mut self) {
        self.table.clear();
    }

    #[cfg(test)]
    pub fn keys(&self) -> impl Iterator<Item = &Block> {
        self.table.iter().map(|(_, key, _)| key)
    }
}

/// One block's memoized rows: its rendered lines and their origins, in lockstep.
#[derive(Debug, Clone)]
pub(super) struct CachedRows {
    pub lines: Vec<Line<'static>>,
    pub origins: Vec<RowOrigin>,
}

/// Whether a block is worth memoizing. A cache hit costs a hash of the whole `Block` plus a clone
/// of its rows; only blocks whose render is *more* expensive than that come out ahead. That
/// is `Table` (column measurement) and `CodeBlock` (syntax highlighting) — and any `List` or
/// `BlockQuote` that *contains* one, since skipping those would re-run the expensive nested render
/// on every keystroke. Cheap blocks (paragraphs, plain lists, headings, rules) render for less than
/// a lookup costs, so caching them is a net loss (#35 §2; see docs/dev/performance.md); they bypass
/// the cache and re-render each build. `ImageBlock` is handled separately by the caller — it is
/// never cached for an unrelated reason (its rows track the out-of-band decode cache).
pub(super) fn is_cache_worthy(block: &Block) -> bool {
    match block {
        Block::Table { .. } | Block::CodeBlock { .. } => true,
        Block::BlockQuote { blocks, .. } => blocks.iter().any(is_cache_worthy),
        Block::List { items, .. } => items.iter().flat_map(|it| &it.blocks).any(is_cache_worthy),
        _ => false,
    }
}

/// Fingerprint of every `Renderer` input besides the block itself; any change clears the whole
/// cache.  The theme is identified by address — themes are `&'static` and the editor already
/// treats pointer identity as theme identity (`EditorState::set_theme` uses `ptr::eq`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RenderSettings {
    pub theme_addr: usize,
    pub viewport_width: usize,
    pub code_wrap: bool,
    pub image_max_height: usize,
    pub row_striping: bool,
    pub big_h1: bool,
    pub syntax_highlighting: bool,
    /// Reflow prose paragraphs (soft breaks → spaces, wrap to viewport).  A render-output knob, so
    /// it belongs in the fingerprint: toggling it (e.g. a mode switch into Raw) must clear the cache.
    pub reflow_paragraphs: bool,
    /// `highlight::warm_generation()` at build time, or 0 when highlighting is off.  Warming a
    /// grammar changes a code block's rendered lines without changing its `Block` value, so
    /// without this the block would stay uncolored for the life of the document.
    pub highlight_generation: u64,
    /// `highlight::retry_epoch()` at build time, or 0 when highlighting is off.  Covers the case
    /// the counter above cannot: a grammar the burst budget turned away.  A retry is granted
    /// exactly when no grammar warmed, so without this the retry's reparse hits every cached
    /// block, never reaches `render_code_block`, and never re-asks for the slot it was granted.
    pub highlight_retry_epoch: u64,
}

/// Memoized rendered rows per top-level block, owned by `EditorState` and threaded into
/// `ParsedDoc::build_with_overrides` on every reparse.  Eviction is by document membership:
/// each build moves the entries it hits into a fresh map and drops the old one.
#[derive(Debug, Default)]
pub struct RenderCache {
    pub(super) settings: Option<RenderSettings>,
    pub(super) entries: BlockMap,
    /// Rows the last build rendered: the next build, which mostly repeats it, sizes its
    /// [`RowSink`](super::row_origin::RowSink) from this instead of growing it by doubling.
    pub(super) last_row_count: usize,
}

impl RenderCache {
    /// Reset to the given settings, clearing all entries when they differ from the previous
    /// build's.  Returns the previous entry map; the caller moves hits out of it into the fresh
    /// [`entries`](Self::entries) map and lets the remainder drop.
    pub(super) fn begin_build(&mut self, settings: RenderSettings) -> BlockMap {
        if self.settings.as_ref() != Some(&settings) {
            self.entries.clear();
            self.settings = Some(settings);
        }
        // Sized for the last build's blocks, which this one mostly repeats: growing from empty
        // would rehash — and so re-hash every `Block` key — at each doubling.
        let capacity = self.entries.len();
        std::mem::replace(&mut self.entries, BlockMap::with_capacity(capacity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> RenderSettings {
        RenderSettings {
            theme_addr: 0,
            viewport_width: 80,
            code_wrap: false,
            image_max_height: 20,
            row_striping: false,
            big_h1: false,
            syntax_highlighting: true,
            reflow_paragraphs: false,
            highlight_generation: 0,
            highlight_retry_epoch: 0,
        }
    }

    /// The retry epoch's absence is invisible: it moves exactly when `highlight_generation` does
    /// *not*, so a cache ignoring it keeps serving the plain render of a budget-refused block.
    #[test]
    fn both_highlight_counters_clear_the_cache() {
        for bump in [
            |s: &mut RenderSettings| s.highlight_generation += 1,
            |s: &mut RenderSettings| s.highlight_retry_epoch += 1,
        ] {
            let mut cache = RenderCache::default();
            cache.begin_build(settings());
            let block = Block::HorizontalRule {
                src: Default::default(),
            };
            cache.entries.insert_new(
                BlockMap::hash(&block),
                block,
                CachedRows {
                    lines: vec![Line::from("x")],
                    origins: vec![RowOrigin::chrome(None)],
                },
            );

            let prev = cache.begin_build(settings());
            assert_eq!(prev.len(), 1, "an unchanged fingerprint must not clear");
            cache.entries = prev;

            let mut changed = settings();
            bump(&mut changed);
            let prev = cache.begin_build(changed);
            assert!(prev.is_empty(), "a moved counter must clear the cache");
        }
    }
}
