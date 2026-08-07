// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0210: absolute line numbers, derived rather than stored.
//!
//! Every node carries the *size* of its own subtree (`lines_total`, and
//! `lines_visible` with folds applied). Nothing carries a position. A
//! position is recovered by walking the root path and summing the counts
//! of preceding siblings — O(depth x fanout), paid only on a teleport.
//!
//! That trade is the whole point. Storing positions made a commit
//! O(nodes after the splice), because every following node's stored
//! number was wrong; storing sizes makes it O(depth), because only the
//! node's ancestors' sizes change. Queries are rare and mutations are
//! not, so this is the right way round.
//!
//! Three kinds of accessor live here:
//!
//! - **Resolution** (`absolute_start`, `node_lines`, `line_pos`,
//!   `visible_row_pos`, `visible_row_of_line`) — the O(depth x fanout)
//!   descents. Use them to enter the document, not to traverse it.
//! - **Traversal** (`next_visible`, `prev_visible`) — O(1) steps along
//!   the visible-line sequence, carrying the absolute line with them.
//!   This is what keeps the descents off the per-frame path: a viewport
//!   is one descent followed by `height` steps.
//! - **Counts** (`visible_row_count`, `total_lines`, `has_children`).
//! - **Text** (`line_text`, `line_text_at`, `line_offset`,
//!   `next_line`, `prev_line`) — spec 0222: the document's text lives in
//!   the nodes, so a line's characters are reached the same way as its
//!   number, by naming the node that owns it.

use super::*;
use std::borrow::Cow;

/// One rendered line, named by the node that owns it and by which of
/// that node's own lines it is.
///
/// Spec 0210 S1's invariant: every line belongs to exactly one node,
/// because a line belonging to nobody is an absorbing barrier for the
/// cursor. The map is *many*-to-one (spec 0216 S7), which is why the
/// second coordinate is a count rather than a flag.
///
/// For a bracketed node (`TreeNode::is_bracketed`) only two values
/// occur: `0` is the header and `lines_total - 1` is the closing brace.
/// Everything between them belongs to the subtree drawn inside, which
/// is why this indexes the node's own lines rather than a screen
/// offset — the two lines of a message are not adjacent on screen. For
/// a flat node the values run `0 .. lines_total`, which is one line for
/// an ordinary scalar and one per element for a packed record.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct LinePos {
    pub(crate) node: usize,
    pub(crate) line_in_node: u32,
}

impl LinePos {
    /// The node's first line.
    pub(crate) fn header(node: usize) -> Self {
        LinePos {
            node,
            line_in_node: 0,
        }
    }
}

impl App {
    /// Whether `idx` is drawn with a closing brace of its own — so it is
    /// foldable and carries a fold marker.
    ///
    /// The name is older than the test, and neither obvious spelling of
    /// it is right. `first_child.is_some()` misses an empty-but-
    /// bracketed message (zero populated fields, still rendered as
    /// `Name {` then `}`), which is foldable and marker-worthy with no
    /// children; `lines_total > 1` wrongly accepts a collapsed packed
    /// run, which is one node of N lines with no brace anywhere (spec
    /// 0216 S7). Ask the shape directly.
    pub(super) fn has_children(&self, idx: usize) -> bool {
        self.tree[idx].is_bracketed()
    }

    /// The document-order first node: slot 0, the whole blob seen as
    /// field 1 of a virtual encompassing message (spec 0216 S1).
    ///
    /// `None` only for an empty tree.
    fn first_root(&self) -> Option<usize> {
        (!self.tree.is_empty()).then_some(self.first_node)
    }

    /// The absolute line `idx`'s subtree begins on — its header line.
    ///
    /// Walks the root path, summing the sizes of the siblings that
    /// precede each step. Spec 0216 S23: those siblings are the slots
    /// immediately below `idx` in the arena, so this is a sequential
    /// scan of a contiguous run rather than a linked-list chase — on the
    /// reference corpus, 31 KB read forwards instead of 7 771 pointer
    /// hops scattered across a 4.7 M-slot arena.
    pub(super) fn absolute_start(&self, idx: usize) -> usize {
        let first_child = self.arena.first_child();
        let mut line = 0usize;
        let mut cur = idx;
        while let Some(parent) = self.parent(cur) {
            for sibling in first_child[parent] as usize..cur {
                line += self.tree[sibling].lines_total as usize;
            }
            // The parent's own header is the single line between it and
            // its first child.
            line += 1;
            cur = parent;
        }
        // `cur` is a root, and level order puts the roots first, so the
        // ones before it are simply the slots below it. A loaded document
        // has one root and this loop is empty; a fixture handing the
        // arena an unwrapped blob of several top-level records does not.
        for root in 0..cur {
            line += self.tree[root].lines_total as usize;
        }
        line
    }

    /// `idx`'s absolute line range, `start .. start + lines_total`.
    ///
    /// Unlike a stored range this cannot go stale — but it is a walk,
    /// not a field read, so a loop over many nodes wants `next_visible`
    /// or an explicit running offset instead.
    pub(super) fn node_lines(&self, idx: usize) -> Range<usize> {
        let start = self.absolute_start(idx);
        start..start + self.tree[idx].lines_total as usize
    }

    /// `idx`'s subtree just changed shape or fold state — recompute both
    /// of its counts from its children and carry the *difference* up to
    /// the root.
    ///
    /// Spec 0254 S1: only `idx` is summed, because only `idx` is known
    /// to have moved and nobody has said by how much. Every ancestor
    /// above it is adjusted by the difference instead of re-added from
    /// its own children, which is what makes the climb O(depth) rather
    /// than O(depth + the fanout of each node on the way up) — a splice
    /// under the reference corpus's root used to re-add 7 771 numbers to
    /// discover that exactly one of them had moved.
    ///
    /// It stops as soon as both differences are zero, which is the same
    /// early exit as before, computed instead of compared. That is what
    /// makes a fold toggle cheap: a fold moves only `lines_visible`, and
    /// above a *folded* ancestor even that difference is zero, because
    /// such an ancestor shows one line whatever happens beneath it (spec
    /// 0193).
    pub(super) fn refresh_line_counts(&mut self, idx: usize) {
        let (want_total, want_visible) = if self.tree[idx].is_bracketed() {
            let mut total = 0u32;
            let mut visible = 0u32;
            let mut child = self.first_child(idx);
            while let Some(c) = child {
                total += self.tree[c].lines_total;
                visible += self.tree[c].lines_visible;
                child = self.next_sibling(c);
            }
            // Header and footer, one line each, whatever is between.
            let shown = if self.is_folded(idx) { 1 } else { visible + 2 };
            (total + 2, shown)
        } else {
            // A flat node's rows are its own — a scalar's single line or
            // a packed record's elements. Nothing below it can move
            // them, and it cannot be folded, so the walk ends here.
            (self.tree[idx].lines_total, self.tree[idx].lines_total)
        };
        let d_total = i64::from(want_total) - i64::from(self.tree[idx].lines_total);
        let mut d_visible = i64::from(want_visible) - i64::from(self.tree[idx].lines_visible);
        self.tree[idx].lines_total = want_total;
        self.tree[idx].lines_visible = want_visible;

        let mut cur = self.parent(idx);
        while let Some(n) = cur {
            if d_total == 0 && d_visible == 0 {
                return;
            }
            self.tree[n].lines_total = adjust(self.tree[n].lines_total, d_total);
            if self.is_folded(n) {
                // One row whatever is beneath it, so its own visible
                // count did not move and nothing above it can have.
                d_visible = 0;
            } else {
                self.tree[n].lines_visible = adjust(self.tree[n].lines_visible, d_visible);
            }
            cur = self.parent(n);
        }
    }

    /// The node owning absolute line `line`, and which of that node's
    /// own lines it is. `None` past the end of the document.
    ///
    /// Descends from the root, at each level handing off to the child
    /// whose extent contains `line`. A materialized line-to-node vector
    /// is the rejected alternative: it cost 85 MB and had to be repaired
    /// over the whole tail of the document on every commit.
    ///
    /// Spec 0222 S3 removed the window-sized cache that used to sit in
    /// front of this. It existed because a drawn row was asked for its
    /// owner four or five times over, having thrown the walk's own
    /// answer away; now the row carries that answer, so the per-frame
    /// path does not come here at all.
    pub(super) fn line_pos(&self, line: usize) -> Option<LinePos> {
        self.descend_line_pos(line)
    }

    fn descend_line_pos(&self, line: usize) -> Option<LinePos> {
        let mut cur = self.first_root()?;
        let mut start = 0usize;
        // Which root (exactly one, outside the test fixtures).
        loop {
            let total = self.tree[cur].lines_total as usize;
            if line < start + total {
                break;
            }
            start += total;
            cur = self.next_sibling(cur)?;
        }
        loop {
            let total = self.tree[cur].lines_total as usize;
            // A flat node owns a run of consecutive lines outright,
            // with nothing nested inside to hand off to.
            if !self.tree[cur].is_bracketed() {
                return Some(LinePos {
                    node: cur,
                    line_in_node: (line - start) as u32,
                });
            }
            if line == start {
                return Some(LinePos::header(cur));
            }
            if line == start + total - 1 {
                return Some(LinePos {
                    node: cur,
                    line_in_node: total as u32 - 1,
                });
            }
            // Strictly inside the body, so some child owns it. The `?`
            // is unreachable while the counts are consistent: a body
            // line with no child to claim it is exactly the corruption
            // spec 0210's invariant rules out.
            let mut child = self.first_child(cur)?;
            let mut child_start = start + 1;
            loop {
                let child_total = self.tree[child].lines_total as usize;
                if line < child_start + child_total {
                    break;
                }
                child_start += child_total;
                child = self.next_sibling(child)?;
            }
            cur = child;
            start = child_start;
        }
    }

    /// How many rows the main pane's committed content currently has,
    /// folds applied.
    pub(super) fn visible_row_count(&self) -> usize {
        let mut total = 0usize;
        let mut cur = self.first_root();
        while let Some(n) = cur {
            total += self.tree[n].lines_visible as usize;
            cur = self.next_sibling(n);
        }
        total
    }

    /// The line drawn at visible row `row`: which node owns it, and its
    /// absolute index into `lines`.
    ///
    /// The same descent as `line_pos`, run on `lines_visible` instead.
    /// Deriving visibility here is what spares a materialized
    /// visible-row list, and with it a full rebuild on every fold
    /// toggle.
    pub(super) fn visible_row_pos(&self, row: usize) -> Option<(LinePos, usize)> {
        let mut cur = self.first_root()?;
        let mut start = 0usize;
        let mut row_base = 0usize;
        loop {
            let visible = self.tree[cur].lines_visible as usize;
            if row < row_base + visible {
                break;
            }
            row_base += visible;
            start += self.tree[cur].lines_total as usize;
            cur = self.next_sibling(cur)?;
        }
        loop {
            // A flat node is never folded, so its visible rows and its
            // absolute lines advance together.
            if !self.tree[cur].is_bracketed() {
                let k = row - row_base;
                return Some((
                    LinePos {
                        node: cur,
                        line_in_node: k as u32,
                    },
                    start + k,
                ));
            }
            if row == row_base {
                return Some((LinePos::header(cur), start));
            }
            // Unreachable for a folded node: its `lines_visible` is 1,
            // so the enclosing bound `row < row_base + visible` has
            // already forced the header branch above.
            let visible = self.tree[cur].lines_visible as usize;
            let total = self.tree[cur].lines_total as usize;
            if row == row_base + visible - 1 {
                return Some((
                    LinePos {
                        node: cur,
                        line_in_node: total as u32 - 1,
                    },
                    start + total - 1,
                ));
            }
            let mut child = self.first_child(cur)?;
            let mut child_row = row_base + 1;
            let mut child_start = start + 1;
            loop {
                let child_visible = self.tree[child].lines_visible as usize;
                if row < child_row + child_visible {
                    break;
                }
                child_row += child_visible;
                child_start += self.tree[child].lines_total as usize;
                child = self.next_sibling(child)?;
            }
            cur = child;
            row_base = child_row;
            start = child_start;
        }
    }

    /// The visible row absolute `line` is *represented* at — its own row
    /// if it is drawn, and otherwise the row of the folded node standing
    /// in for it. `None` only past the end of the document.
    ///
    /// The inverse of `visible_row_pos`, and the one descent that cannot
    /// be replaced by a traversal — it is what puts the cursor's own
    /// line on screen.
    ///
    /// Folding to the ancestor rather than reporting "hidden" is what
    /// the callers want: a cursor or an overlay anchor on a folded-away
    /// line still has to be drawn *somewhere*, and the fold's own header
    /// is the row the user sees it at.
    pub(super) fn visible_row_of_line(&self, line: usize) -> Option<usize> {
        let mut cur = self.first_root()?;
        let mut start = 0usize;
        let mut row_base = 0usize;
        loop {
            let total = self.tree[cur].lines_total as usize;
            if line < start + total {
                break;
            }
            start += total;
            row_base += self.tree[cur].lines_visible as usize;
            cur = self.next_sibling(cur)?;
        }
        loop {
            if !self.tree[cur].is_bracketed() {
                return Some(row_base + (line - start));
            }
            if line == start {
                return Some(row_base);
            }
            let visible = self.tree[cur].lines_visible as usize;
            // A single visible row and `line` is not the header: the
            // node is folded and `line` is inside the body it hides, so
            // this fold's own header row is where it shows up.
            if visible == 1 {
                return Some(row_base);
            }
            let total = self.tree[cur].lines_total as usize;
            if line == start + total - 1 {
                return Some(row_base + visible - 1);
            }
            let mut child = self.first_child(cur)?;
            let mut child_start = start + 1;
            let mut child_row = row_base + 1;
            loop {
                let child_total = self.tree[child].lines_total as usize;
                if line < child_start + child_total {
                    break;
                }
                child_start += child_total;
                child_row += self.tree[child].lines_visible as usize;
                child = self.next_sibling(child)?;
            }
            cur = child;
            start = child_start;
            row_base = child_row;
        }
    }

    /// `count` consecutive visible rows starting at `from`, as
    /// `(absolute line, owner)` pairs.
    ///
    /// One descent and then a walk — spec 0210 S3's whole point. Drawing
    /// a frame by resolving each of its rows separately would be
    /// `height` descents, and on the reference corpus each of those
    /// crosses the root's 7 771 children.
    pub(super) fn visible_window(&self, from: usize, count: usize) -> Vec<(usize, LinePos)> {
        let mut out = Vec::with_capacity(count);
        let Some((mut pos, mut line)) = self.visible_row_pos(from) else {
            return out;
        };
        loop {
            out.push((line, pos));
            if out.len() == count {
                return out;
            }
            let Some((next, delta)) = self.next_visible(pos) else {
                return out;
            };
            pos = next;
            line += delta;
        }
    }

    /// The visible line after `pos`, and how many absolute lines forward
    /// it lies. `None` at the end of the document.
    ///
    /// O(1). Together with one `line_pos`/`visible_row_pos` descent to
    /// enter the document, this is how a viewport is drawn — spec 0210
    /// S3's "the frame is a walk, the index is not in the per-frame
    /// path".
    pub(super) fn next_visible(&self, pos: LinePos) -> Option<(LinePos, usize)> {
        let total = self.tree[pos.node].lines_total;
        if self.tree[pos.node].is_bracketed() {
            if pos.line_in_node == 0 && !self.is_folded(pos.node) {
                if let Some(child) = self.first_child(pos.node) {
                    return Some((LinePos::header(child), 1));
                }
                // An empty-but-bracketed message: no children, but its
                // own closing brace is still a line of its own.
                return Some((
                    LinePos {
                        node: pos.node,
                        line_in_node: total - 1,
                    },
                    total as usize - 1,
                ));
            }
        } else if pos.line_in_node + 1 < total {
            // The next element of a packed run.
            return Some((
                LinePos {
                    node: pos.node,
                    line_in_node: pos.line_in_node + 1,
                },
                1,
            ));
        }
        // Everything `pos.node` has to show is behind us, so the next
        // line is the one just past its extent — which from its last
        // line is one step, and from a folded node's header is the whole
        // of it. Both are `lines_total - line_in_node`.
        let delta = (total - pos.line_in_node) as usize;
        if let Some(sibling) = self.next_sibling(pos.node) {
            return Some((LinePos::header(sibling), delta));
        }
        // Last child, so the next line is the parent's own closing
        // brace, which sits exactly at the end of its last child's
        // extent — no further offset accrues while climbing. The parent
        // cannot be folded, or we would not be inside it.
        self.parent(pos.node).map(|parent| {
            (
                LinePos {
                    node: parent,
                    line_in_node: self.tree[parent].lines_total - 1,
                },
                delta,
            )
        })
    }

    /// The visible line before `pos`, and how many absolute lines back
    /// it lies. `None` at the start of the document. The mirror of
    /// `next_visible`, and likewise O(1).
    pub(super) fn prev_visible(&self, pos: LinePos) -> Option<(LinePos, usize)> {
        if pos.line_in_node > 0 {
            if !self.tree[pos.node].is_bracketed() {
                return Some((
                    LinePos {
                        node: pos.node,
                        line_in_node: pos.line_in_node - 1,
                    },
                    1,
                ));
            }
            return Some(match self.last_child(pos.node) {
                Some(child) => self.last_visible_of(child),
                // Empty-but-bracketed: its own header is the line above
                // its brace.
                None => (
                    LinePos::header(pos.node),
                    self.tree[pos.node].lines_total as usize - 1,
                ),
            });
        }
        if let Some(sibling) = self.prev_sibling(pos.node) {
            return Some(self.last_visible_of(sibling));
        }
        // First child: the line above is the parent's own header.
        self.parent(pos.node)
            .map(|parent| (LinePos::header(parent), 1))
    }

    /// `idx`'s last *visible* line, paired with how far back from the
    /// line just past `idx`'s extent it sits — which is what
    /// `prev_visible`'s two backward steps both need.
    ///
    /// A folded node shows only its header, so the step back over it is
    /// the whole of its extent rather than one line.
    fn last_visible_of(&self, idx: usize) -> (LinePos, usize) {
        if self.is_folded(idx) {
            return (LinePos::header(idx), self.tree[idx].lines_total as usize);
        }
        (
            LinePos {
                node: idx,
                line_in_node: self.tree[idx].lines_total - 1,
            },
            1,
        )
    }

    /// How many lines the committed document has, folds ignored.
    ///
    /// Derived from the roots' own counters, the same way
    /// `visible_row_count` is, so a splice cannot leave it stale.
    pub(super) fn total_lines(&self) -> usize {
        let mut total = 0usize;
        let mut cur = self.first_root();
        while let Some(n) = cur {
            total += self.tree[n].lines_total as usize;
            cur = self.next_sibling(n);
        }
        total
    }

    /// The characters of the line `pos` names.
    ///
    /// Owned only for a bracketed node's closing brace, which spec 0222
    /// S2 derives rather than stores; every other line is borrowed
    /// straight out of its owner's text.
    pub(super) fn line_text(&self, pos: LinePos) -> Cow<'_, str> {
        self.line_text_at(pos, self.line_offset(pos))
    }

    /// `line_text` for a caller that already knows where in its owner's
    /// text the line starts — spec 0222 S4's byte cursor, which is what
    /// keeps a frame drawn inside a long packed run linear rather than
    /// quadratic.
    pub(super) fn line_text_at(&self, pos: LinePos, offset: usize) -> Cow<'_, str> {
        let Some(text) = self.node_text[pos.node].as_deref() else {
            return Cow::Borrowed("");
        };
        if self.tree[pos.node].is_bracketed() && pos.line_in_node > 0 {
            return Cow::Owned(decode::derived_close(text));
        }
        Cow::Borrowed(line_at(text, offset))
    }

    /// Where in `node_text[pos.node]` the line `pos` names begins.
    ///
    /// Zero for anything but a later line of a packed run, and O(that
    /// line's index) for one of those — hence `line_text_at` for the
    /// callers that walk.
    pub(super) fn line_offset(&self, pos: LinePos) -> usize {
        if pos.line_in_node == 0 || self.tree[pos.node].is_bracketed() {
            return 0;
        }
        match self.node_text[pos.node].as_deref() {
            Some(text) => offset_of_line(text, pos.line_in_node as usize),
            None => 0,
        }
    }

    /// The line after `pos` in document order, folds ignored. `None` at
    /// the end of the document.
    ///
    /// The fold-blind twin of `next_visible`, and O(1) for the same
    /// reasons. Search wants this rather than a node-level `doc_next`
    /// step: a match can sit on a closing brace, and pre-order's answer
    /// to "the node after this one" descends back into the node's own
    /// children, which would visit lines out of the order the user reads
    /// them in.
    pub(super) fn next_line(&self, pos: LinePos) -> Option<LinePos> {
        let total = self.tree[pos.node].lines_total;
        if self.tree[pos.node].is_bracketed() {
            if pos.line_in_node == 0 {
                if let Some(child) = self.first_child(pos.node) {
                    return Some(LinePos::header(child));
                }
                return Some(LinePos {
                    node: pos.node,
                    line_in_node: total - 1,
                });
            }
        } else if pos.line_in_node + 1 < total {
            return Some(LinePos {
                node: pos.node,
                line_in_node: pos.line_in_node + 1,
            });
        }
        if let Some(sibling) = self.next_sibling(pos.node) {
            return Some(LinePos::header(sibling));
        }
        self.parent(pos.node).map(|parent| LinePos {
            node: parent,
            line_in_node: self.tree[parent].lines_total - 1,
        })
    }

    /// The line before `pos` in document order, folds ignored. `None` at
    /// the start of the document. The mirror of `next_line`.
    pub(super) fn prev_line(&self, pos: LinePos) -> Option<LinePos> {
        if pos.line_in_node > 0 {
            if !self.tree[pos.node].is_bracketed() {
                return Some(LinePos {
                    node: pos.node,
                    line_in_node: pos.line_in_node - 1,
                });
            }
            return Some(match self.last_child(pos.node) {
                Some(child) => self.last_line_of(child),
                None => LinePos::header(pos.node),
            });
        }
        if let Some(sibling) = self.prev_sibling(pos.node) {
            return Some(self.last_line_of(sibling));
        }
        self.parent(pos.node).map(LinePos::header)
    }

    /// `idx`'s own last line — its closing brace, or the last element of
    /// a packed run.
    fn last_line_of(&self, idx: usize) -> LinePos {
        LinePos {
            node: idx,
            line_in_node: self.tree[idx].lines_total - 1,
        }
    }

    /// The document's first line, `None` for an empty tree.
    pub(super) fn first_line(&self) -> Option<LinePos> {
        self.first_root().map(LinePos::header)
    }

    /// The document's last line, `None` for an empty tree.
    ///
    /// Resolved on demand rather than kept as an endpoint, so that a
    /// backward search pays for it once and only if it wraps — spec 0195
    /// S1's hazard.
    pub(super) fn last_line(&self) -> Option<LinePos> {
        let mut cur = self.first_root()?;
        while let Some(next) = self.next_sibling(cur) {
            cur = next;
        }
        Some(self.last_line_of(cur))
    }

    /// `idx`'s subtree as freshly built lines, in document order.
    ///
    /// The one place that materializes text the way `App::lines` used
    /// to, for the callers that hand a whole subtree to something
    /// outside the TUI — export and the clipboard. Bounded by the
    /// subtree, not by the document.
    pub(super) fn subtree_lines(&self, idx: usize) -> Vec<String> {
        decode::subtree_lines(&self.tree, &self.node_text, &self.arena, idx)
    }

    /// The whole document as freshly built lines.
    ///
    /// Test-only. Production code reads one line at a time, through the
    /// node that owns it; a test that is checking *what the document
    /// says* is far clearer written against the text than against a
    /// walk, so it gets the old `App::lines` back for the length of one
    /// assertion.
    #[cfg(test)]
    pub(super) fn document_lines(&self) -> Vec<String> {
        decode::document_lines(&self.tree, &self.node_text, &self.arena)
    }
}

/// A line count moved by a signed difference — negative when the
/// subtree beneath it shrank.
///
/// Spec 0254 S4: a difference that would take a count below zero is a
/// corrupted tree, not a case to handle. `assert_line_counts_are_exact`
/// runs over the whole document after every splice in the test suite
/// and is what would catch it first; this is only the backstop that
/// keeps a release build from wrapping to four billion lines.
fn adjust(count: u32, delta: i64) -> u32 {
    let moved = i64::from(count) + delta;
    debug_assert!(moved >= 0, "line count {count} moved by {delta} to {moved}");
    moved.clamp(0, i64::from(u32::MAX)) as u32
}

/// The line beginning at `offset`, up to but not including its newline.
fn line_at(text: &str, offset: usize) -> &str {
    let rest = &text[offset..];
    match rest.find('\n') {
        Some(end) => &rest[..end],
        None => rest,
    }
}

/// Where the `k`-th line of `text` begins.
fn offset_of_line(text: &str, k: usize) -> usize {
    let mut offset = 0usize;
    for _ in 0..k {
        match text[offset..].find('\n') {
            Some(at) => offset += at + 1,
            None => return text.len(),
        }
    }
    offset
}
