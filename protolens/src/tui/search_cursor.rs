// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0274 S4-S7: the document read as one string.
//!
//! Spec 0222 gave every arena slot its own text, so there is no
//! contiguous haystack for a `&str` matcher to search. [`DocCursor`]
//! supplies one anyway, as a `regex_cursor::Cursor` whose chunks are
//! those texts in document order with a `\n` between them — which is
//! exactly the string [`decode::document_lines`] would join, without
//! ever building it.
//!
//! What it walks is a **segment** (S4), not the whole document: a
//! maximal run of rows that are contiguous in the *finished* document.
//! A bake stop has a header and a footer and nothing between (spec 0249
//! S1), so the rows on either side of it are not neighbors and a match
//! must not be allowed to join them.

use super::search::RowBound;
use super::structure::Structure;
use super::*;
use regex_cursor::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;

/// One chunk's worth of the document: a node's own text entry, or the
/// closing brace a bracketed node derives rather than stores (0222 S2).
///
/// Deliberately coarser than a row. A packed run's text holds all its
/// rows joined already, and asking for row *k* of one costs O(k) —
/// spec 0272's quadratic sweep. Walking entries pays that never.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Place {
    pub(super) node: usize,
    /// The node's closing brace rather than its own text.
    pub(super) close: bool,
}

impl Place {
    #[inline]
    pub(super) fn own(node: usize) -> Self {
        Place { node, close: false }
    }

    #[inline]
    pub(super) fn close(node: usize) -> Self {
        Place { node, close: true }
    }

    /// The last chunk `node`'s subtree contributes.
    #[inline]
    fn last_of(node: usize, st: &Structure<'_>) -> Self {
        if st.is_bracketed(node) {
            Place::close(node)
        } else {
            Place::own(node)
        }
    }

    /// The chunk after this one, `None` at the end of the document.
    ///
    /// The coarse twin of `next_line`, and the same three cases: descend
    /// into a bracketed node, else step sideways, else close the parent.
    fn next(self, st: &Structure<'_>) -> Option<Self> {
        if !self.close && st.is_bracketed(self.node) {
            return Some(match st.first_child(self.node) {
                Some(child) => Place::own(child),
                None => Place::close(self.node),
            });
        }
        if let Some(sibling) = st.next_sibling(self.node) {
            return Some(Place::own(sibling));
        }
        st.parent(self.node).map(Place::close)
    }

    /// The chunk before this one, `None` at the start of the document.
    /// The exact inverse of [`Place::next`], which is what lets
    /// `backtrack` undo `advance` step for step.
    fn prev(self, st: &Structure<'_>) -> Option<Self> {
        if self.close {
            return Some(match st.last_child(self.node) {
                Some(child) => Place::last_of(child, st),
                None => Place::own(self.node),
            });
        }
        if let Some(sibling) = st.prev_sibling(self.node) {
            return Some(Place::last_of(sibling, st));
        }
        st.parent(self.node).map(Place::own)
    }
}

/// A run of rows contiguous in the finished document (S4), given by its
/// first and last chunk, both inclusive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Segment {
    pub(super) start: Place,
    pub(super) end: Place,
}

/// The bytes of one [`Segment`], chunk by chunk.
pub(super) struct DocCursor<'a> {
    st: Structure<'a>,
    text: &'a [Option<Box<str>>],
    start: Place,
    end: Place,
    at: Place,
    /// The current chunk is the `\n` joining `at`'s text to what comes
    /// before it, rather than that text itself. False at `start`, which
    /// nothing precedes.
    on_sep: bool,
    /// Bytes from the segment's first row to the current chunk (S6) —
    /// from the segment's, not the document's.
    offset: usize,
    /// Somewhere for a derived closing brace to live, since `chunk`
    /// hands out a borrow and 0222 stores that text nowhere.
    close: String,
    /// Spec 0343 B11: scratch for `node_text + "; shadowed_scalar"` on
    /// a marked own-text chunk, filled by `reload_mark`. Empty when the
    /// current place is not a marked non-close node. `bytes()` returns
    /// this instead of the raw text when non-empty.
    mark: String,
    /// The shadow bitset (spec 0343 B7). `None` before the filter runs
    /// or when no slots are marked.
    shadowed: Option<Arc<Vec<AtomicU64>>>,
    /// Mirror of `App::annotations`: the mark suffix is only visible
    /// when annotations are on, so the cursor must ask the same question
    /// or the haystack and the display disagree (spec 0343 B11).
    annotations: bool,
    /// Spec 0274 S7: the epoch cell and the value it must still hold.
    /// The cursor is the abort point, so a search that has been
    /// superseded ends the same way one that ran out of data does.
    abort: Option<(&'a AtomicU64, u64)>,
}

impl<'a> DocCursor<'a> {
    pub(super) fn new(
        st: Structure<'a>,
        text: &'a [Option<Box<str>>],
        seg: Segment,
        abort: Option<(&'a AtomicU64, u64)>,
    ) -> Self {
        Self::with_marks(st, text, None, false, seg, abort)
    }

    /// Like [`Self::new`] but with the shadow bitset and annotations
    /// flag so the cursor includes `; shadowed_scalar` suffixes (B11).
    pub(super) fn with_marks(
        st: Structure<'a>,
        text: &'a [Option<Box<str>>],
        shadowed: Option<Arc<Vec<AtomicU64>>>,
        annotations: bool,
        seg: Segment,
        abort: Option<(&'a AtomicU64, u64)>,
    ) -> Self {
        let mut cur = DocCursor {
            st,
            text,
            start: seg.start,
            end: seg.end,
            at: seg.start,
            on_sep: false,
            offset: 0,
            close: String::new(),
            mark: String::new(),
            shadowed,
            annotations,
            abort,
        };
        cur.reload_close();
        cur.reload_mark();
        // An empty first row is a real row: the haystack opens with the
        // `\n` that follows it. `advance` is what puts the cursor there
        // without moving `offset` off zero.
        if cur.chunk().is_empty() {
            cur.advance();
        }
        cur
    }

    /// The text `at` names, empty for a slot this interpretation does
    /// not render.
    #[inline]
    fn bytes(&self) -> &[u8] {
        if self.at.close {
            return self.close.as_bytes();
        }
        // Spec 0343 B11: return the combined scratch when the node is
        // marked and annotations are on — `reload_mark` filled it.
        if !self.mark.is_empty() {
            return self.mark.as_bytes();
        }
        match self.text[self.at.node].as_deref() {
            Some(text) => text.as_bytes(),
            None => b"",
        }
    }

    fn reload_close(&mut self) {
        self.close.clear();
        if self.at.close {
            if let Some(header) = self.text[self.at.node].as_deref() {
                decode::write_derived_close(header, &mut self.close);
            }
        }
    }

    /// Spec 0343 B11: fill `mark` with `node_text + "; shadowed_scalar"`
    /// when the current place is a marked own-text node and annotations
    /// are on.  Called at the same four points as `reload_close`.
    fn reload_mark(&mut self) {
        self.mark.clear();
        if self.at.close || !self.annotations {
            return;
        }
        let node = self.at.node;
        let Some(bitset) = &self.shadowed else { return };
        let word = bitset
            .get(node / 64)
            .map_or(0, |w| w.load(std::sync::atomic::Ordering::Relaxed));
        if word & (1 << (node % 64)) == 0 {
            return;
        }
        // Node is marked: build the combined scratch.
        if let Some(text) = self.text[node].as_deref() {
            self.mark.push_str(text);
        }
        self.mark.push_str("; shadowed_scalar");
    }

    fn restore(&mut self, saved: (Place, bool, usize)) {
        (self.at, self.on_sep, self.offset) = saved;
        self.reload_close();
        self.reload_mark();
    }

    #[inline]
    fn aborted(&self) -> bool {
        self.abort
            .is_some_and(|(epoch, held)| epoch.load(Ordering::Relaxed) != held)
    }
}

impl regex_cursor::Cursor for DocCursor<'_> {
    #[inline]
    fn chunk(&self) -> &[u8] {
        if self.on_sep {
            return b"\n";
        }
        self.bytes()
    }

    /// True: a chunk is a whole text entry or a whole derived brace, so
    /// no boundary can fall inside a codepoint (S6).
    #[inline]
    fn utf8_aware(&self) -> bool {
        true
    }

    fn advance(&mut self) -> bool {
        if self.aborted() {
            return false;
        }
        let saved = (self.at, self.on_sep, self.offset);
        loop {
            let len = self.chunk().len();
            if self.on_sep {
                self.on_sep = false;
            } else if self.at == self.end {
                self.restore(saved);
                return false;
            } else if let Some(next) = self.at.next(&self.st) {
                self.at = next;
                self.reload_close();
                self.reload_mark();
                self.on_sep = true;
            } else {
                self.restore(saved);
                return false;
            }
            self.offset += len;
            // A slot that renders nothing still ends a row, so its
            // separator stands and only its own zero bytes are skipped:
            // `chunk` may not answer with an empty slice.
            if !self.chunk().is_empty() {
                return true;
            }
        }
    }

    fn backtrack(&mut self) -> bool {
        let saved = (self.at, self.on_sep, self.offset);
        loop {
            if self.on_sep {
                // `on_sep` is only ever set on a place `advance` reached
                // from `start`, so the step back cannot leave the
                // segment.
                match self.at.prev(&self.st) {
                    Some(prev) => {
                        self.at = prev;
                        self.reload_close();
                        self.reload_mark();
                        self.on_sep = false;
                    }
                    None => {
                        self.restore(saved);
                        return false;
                    }
                }
            } else if self.at == self.start {
                self.restore(saved);
                return false;
            } else {
                self.on_sep = true;
            }
            self.offset -= self.chunk().len();
            if !self.chunk().is_empty() {
                return true;
            }
        }
    }

    /// `None`: the segment's length is not known without walking it, and
    /// `Input` reads that as `usize::MAX` and clamps once `advance`
    /// first answers false (S6).
    #[inline]
    fn total_bytes(&self) -> Option<usize> {
        None
    }

    #[inline]
    fn offset(&self) -> usize {
        self.offset
    }
}

impl App {
    /// The document's segments, in document order (S4).
    ///
    /// One more than there are bake stops: every stop closes a segment
    /// after its header and opens the next at its footer, because that
    /// is where the rows it still owes belong. A finished bake leaves
    /// exactly one segment, which is the whole document.
    ///
    /// Spec 0274 S9 freezes this list for the length of a search, so it
    /// is taken once per sweep rather than per step.
    ///
    /// The stops are returned with it, in the same order, because
    /// [`App::segment_index_of`] is a binary search over them.
    pub(super) fn search_segments(&self) -> (Vec<Segment>, Vec<usize>) {
        let st = self.structure();
        let roots = st.sibling_block(0);
        if roots.is_empty() || !self.tree[roots.start].is_rendered() {
            return (Vec::new(), Vec::new());
        }
        // `auto_folded` is in slot order — level order, since spec 0216 —
        // and the stops have to be visited in the order the reader would
        // meet them, which is not the same thing: level order groups by
        // depth. Byte order is the reader's order: the arena asserts a
        // parent starts before its children,
        // and siblings are laid out as the bytes were, so `raw_start`
        // sorts a pre-order walk. Stops never nest — one has no rendered
        // body to hold another — so there are no ties to break.
        let raw_start = self.arena.raw_start();
        let mut stops: Vec<usize> = self
            .auto_folded
            .iter()
            .filter(|&n| self.tree[n].is_rendered())
            .collect();
        stops.sort_unstable_by_key(|&n| raw_start[n]);

        let mut out = Vec::with_capacity(stops.len() + 1);
        let mut start = Place::own(roots.start);
        for &stop in &stops {
            out.push(Segment {
                start,
                end: Place::own(stop),
            });
            start = Place::close(stop);
        }
        out.push(Segment {
            start,
            end: Place::last_of(roots.end - 1, &st),
        });
        (out, stops)
    }

    /// Which segment `place` falls in, given [`App::search_segments`]'
    /// stops.
    ///
    /// A stop `s` closes its segment *after* its own header, so the
    /// question is how many stops start before `place` does — with the
    /// end of a subtree standing at `raw_end` rather than `raw_start`,
    /// which is what puts a bracketed node's footer after the stops
    /// inside it.
    pub(super) fn segment_index_of(&self, place: Place, stops: &[usize]) -> usize {
        let raw_start = self.arena.raw_start();
        let limit = if place.close {
            self.arena.raw_end()[place.node]
        } else {
            raw_start[place.node]
        };
        stops.partition_point(|&s| raw_start[s] < limit)
    }

    /// The place a line is drawn by — its node's own text, or the
    /// derived footer of a bracketed one.
    pub(super) fn place_of(&self, pos: LinePos) -> Place {
        if self.is_footer(pos) {
            Place::close(pos.node)
        } else {
            Place::own(pos.node)
        }
    }

    /// Bytes from `seg`'s first row to the start of `place`'s chunk, or
    /// `None` if `place` is not in `seg`.
    ///
    /// One add per node and no text touched (S14): a chunk's length is
    /// its entry's, and a footer's is its indent plus one.
    pub(super) fn segment_byte_of(&self, seg: Segment, place: Place) -> Option<usize> {
        let st = self.structure();
        let mut at = seg.start;
        let mut base = 0usize;
        loop {
            if at == place {
                return Some(base);
            }
            if at == seg.end {
                return None;
            }
            base += self.chunk_len(at) + 1;
            at = at.next(&st)?;
        }
    }

    /// The row `byte` falls on, with that row's first and last byte
    /// offsets inside `seg`.
    ///
    /// A byte one past a row's last is that row's too — it is the `\n`
    /// the row ends with, and a match may start on one.
    pub(super) fn locate_in_segment(
        &self,
        seg: Segment,
        byte: usize,
    ) -> Option<(LinePos, Range<usize>)> {
        let st = self.structure();
        let mut at = seg.start;
        let mut base = 0usize;
        loop {
            let len = self.chunk_len(at);
            if byte <= base + len {
                return Some(self.locate_in_chunk(at, base, byte - base));
            }
            if at == seg.end {
                return None;
            }
            base += len + 1;
            at = at.next(&st)?;
        }
    }

    /// [`App::locate_in_segment`] once the chunk is known: `local` is an
    /// offset into it and `base` is where it starts in the segment.
    fn locate_in_chunk(&self, at: Place, base: usize, local: usize) -> (LinePos, Range<usize>) {
        if at.close {
            let pos = LinePos {
                node: at.node,
                line_in_node: self.tree[at.node].lines_total - 1,
            };
            return (pos, base..base + self.chunk_len(at));
        }
        let text = self.node_text[at.node].as_deref().unwrap_or("");
        // Spec 0343 B11: a match landing inside `; shadowed_scalar` has
        // no real byte to name, so clamp to the row's end.  The clamp
        // must precede the two slices below, which would panic otherwise.
        let local = local.min(text.len());
        // A packed run is one node holding many rows (spec 0216 S22), so
        // the row is however many newlines precede `local`.
        let before = &text[..local];
        let line_start = before.rfind('\n').map_or(0, |at| at + 1);
        let line_end = text[local..].find('\n').map_or(text.len(), |at| local + at);
        let pos = LinePos {
            node: at.node,
            line_in_node: before.matches('\n').count() as u32,
        };
        (pos, base + line_start..base + line_end)
    }

    /// A chunk's length in bytes, without building it.
    ///
    /// Spec 0343 B11: adds `"; shadowed_scalar".len()` on a marked
    /// own-text node when annotations are on — one bit test, no text
    /// built.
    fn chunk_len(&self, at: Place) -> usize {
        let text = self.node_text[at.node].as_deref().unwrap_or("");
        if at.close {
            // `derived_close` is the header's indent and one `}`.
            return text.len() - text.trim_start_matches(' ').len() + 1;
        }
        let base = text.len();
        if self.annotations {
            let extra = self
                .shadowed
                .as_ref()
                .and_then(|b| b.get(at.node / 64))
                .map_or(0, |w| {
                    if w.load(std::sync::atomic::Ordering::Relaxed) & (1 << (at.node % 64)) != 0 {
                        "; shadowed_scalar".len()
                    } else {
                        0
                    }
                });
            base + extra
        } else {
            base
        }
    }

    /// Spec 0274 S9: hand one segment to a thread of its own.
    ///
    /// `None` when there is nowhere to report a result to — a headless
    /// export, or a test driving `search_sweep_step` by hand — and the
    /// caller then scans on this thread instead. The same fallback the
    /// heat worker has when no scoring graph was loaded.
    pub(super) fn spawn_segment_scan(
        &self,
        re: &Arc<CursorRegex>,
        seg: Segment,
        bound: RowBound,
        dir: SearchDir,
        span: (usize, usize),
    ) -> Option<SegmentScan> {
        let progress = self.search_progress.clone()?;
        // Spec 0274 S8: three refcount bumps, and the whole of what the
        // scan is allowed to touch. Nothing here is copied, and nothing
        // the main thread does can move it while the scan holds it —
        // `tree_mut` and `node_text_mut` both end the scan first.
        let tree = Arc::clone(&self.tree);
        let arena = Arc::clone(&self.arena);
        let text = Arc::clone(&self.node_text);
        let re = Arc::clone(re);
        // Spec 0343 B11: shadow bitset is `Arc`, so the clone is a
        // refcount bump — no copy of the bitset itself.
        let shadowed = self.shadowed.clone();
        let annotations = self.annotations;
        let epoch = Arc::new(AtomicU64::new(SCAN_LIVE));
        let held = Arc::clone(&epoch);
        let (tx, result) = mpsc::channel();
        let (lo, hi) = span;
        let join = thread::spawn(move || {
            // Spec 0264: a CPU mask is inherited across `clone(2)`, so
            // a thread that does not widen would run on the one core
            // the main loop reserved for drawing.
            crate::affinity::widen();
            let st = Structure {
                tree: tree.as_slice(),
                arena: &arena,
            };
            let abort = Some((&*held, SCAN_LIVE));
            let found = match dir {
                SearchDir::Forward => {
                    find_in_segment(st, &text, shadowed, annotations, &re, seg, lo, abort)
                        .filter(|r| r.start < hi)
                }
                SearchDir::Backward => {
                    find_last_in_segment(st, &text, shadowed, annotations, &re, seg, lo, hi, abort)
                }
            };
            // The answer goes out *before* the wake-up, so the main
            // thread's `try_recv` on this channel cannot see the event
            // and miss the result behind it.
            let _ = tx.send(found);
            let _ = progress.send(event::AppEvent::SearchWorkerProgress);
        });
        Some(SegmentScan {
            seg,
            bound,
            join: Some(join),
            result,
            epoch,
        })
    }

    /// The same scan run here rather than handed out — the two
    /// non-incremental callers (`n`, `N`, a committed prompt), which
    /// have no loop to interleave with.
    pub(super) fn scan_segment_inline(
        &self,
        re: &CursorRegex,
        seg: Segment,
        dir: SearchDir,
        lo: usize,
        hi: usize,
    ) -> Option<Range<usize>> {
        let st = self.structure();
        let shadowed = self.shadowed.clone();
        let annotations = self.annotations;
        match dir {
            SearchDir::Forward => find_in_segment(
                st,
                &self.node_text,
                shadowed,
                annotations,
                re,
                seg,
                lo,
                None,
            )
            .filter(|r| r.start < hi),
            SearchDir::Backward => find_last_in_segment(
                st,
                &self.node_text,
                shadowed,
                annotations,
                re,
                seg,
                lo,
                hi,
                None,
            ),
        }
    }
}

/// Spec 0274 S7: the value a scan's epoch cell holds while its answer
/// is still wanted. Anything else ends the walk at the next chunk
/// boundary, which is how a superseded search stops without a
/// cancellation protocol.
const SCAN_LIVE: u64 = 0;
const SCAN_ABORTED: u64 = 1;

/// Spec 0274 S9: one segment being scanned on a thread of its own.
pub(super) struct SegmentScan {
    /// What was handed out, so that a scan cut short by a document
    /// write can be put back on the queue rather than silently counting
    /// as a miss.
    pub(super) seg: Segment,
    pub(super) bound: RowBound,
    /// `Option` only so that [`Drop`] can take it; alive for the whole
    /// of the scan.
    join: Option<thread::JoinHandle<()>>,
    result: mpsc::Receiver<Option<Range<usize>>>,
    epoch: Arc<AtomicU64>,
}

impl SegmentScan {
    /// The scan's answer, or `None` while it is still walking.
    ///
    /// A disconnected channel is a worker that panicked. The hook in
    /// `tui::run` has already recorded that; here it reads as a miss
    /// for this segment, which is the answer that loses the least.
    pub(super) fn collect(&self) -> Option<Option<Range<usize>>> {
        match self.result.try_recv() {
            Ok(found) => Some(found),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(None),
        }
    }
}

impl Drop for SegmentScan {
    /// Abort, then join. Spec 0274 S8's refcount argument *is* this
    /// destructor: a scan holds clones of the tree, the arena and the
    /// text, so a superseded sweep that merely dropped its handle would
    /// leave a thread running and turn the next `Arc::make_mut` into a
    /// copy of the whole document.
    fn drop(&mut self) {
        self.epoch.store(SCAN_ABORTED, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Where `re` first matches inside `seg`, in bytes from the segment's
/// first row, searching from `from`.
///
/// `None` for a miss and for an abort alike: the caller knows which it
/// asked for, and S7 makes the two indistinguishable here on purpose.
///
/// A free function over the pieces rather than a method, because the
/// worker holds those pieces through `Arc`s and has no `&App` (S8).
pub(super) fn find_in_segment(
    st: Structure<'_>,
    text: &[Option<Box<str>>],
    shadowed: Option<Arc<Vec<AtomicU64>>>,
    annotations: bool,
    re: &CursorRegex,
    seg: Segment,
    from: usize,
    abort: Option<(&AtomicU64, u64)>,
) -> Option<Range<usize>> {
    let mut cursor = DocCursor::with_marks(st, text, shadowed, annotations, seg, abort);
    let mut input = CursorInput::new(&mut cursor);
    input.set_start(from);
    re.find(input).map(|m| m.range())
}

/// The **last** match in `seg` whose start lies below `before` — a
/// backward search's stop (spec 0246 S4).
///
/// There is no reverse engine in the cursor meta API, so this reads the
/// prefix forwards and keeps the last (S14).
pub(super) fn find_last_in_segment(
    st: Structure<'_>,
    text: &[Option<Box<str>>],
    shadowed: Option<Arc<Vec<AtomicU64>>>,
    annotations: bool,
    re: &CursorRegex,
    seg: Segment,
    from: usize,
    before: usize,
    abort: Option<(&AtomicU64, u64)>,
) -> Option<Range<usize>> {
    let mut cursor = DocCursor::with_marks(st, text, shadowed, annotations, seg, abort);
    let mut input = CursorInput::new(&mut cursor);
    input.set_start(from);
    let mut last = None;
    for m in re.find_iter(input) {
        if m.start() >= before {
            break;
        }
        last = Some(m.range());
    }
    last
}
