// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0280: what a heat cue's number is made of.
//!
//! A cue prints `[3/7]` and gives no reason. The reason is already
//! computed and then discarded: `EntryScore` carries the five terms the
//! score is the weighted sum of, and protolens keeps only the sum. This
//! module asks for the terms back, one node at a time, and puts them in
//! a box.
//!
//! Deliberately *not* a ranking. The override pane answers "what else
//! could this be"; this answers "how badly does what it is now fit".
//! Two questions, two surfaces (spec 0280 N1).

use super::*;
use crate::override_pane::{inferred_breakdown, ScoreBreakdown};
use prototext_core::serialize::encode_text::annotation_start;

/// How long the pointer must hold still on a type before the box
/// appears (spec 0280 S12).
///
/// 400 ms is the shortest delay that still reads as deliberate: below
/// roughly a third of a second the box arrives while the eye is still
/// travelling and is perceived as something that happened *to* the
/// pointer rather than something the reader asked for. The desktop
/// conventions this borrows from sit at 400-600 ms (GTK, Windows), and
/// the low end of that is right here because the answer is already
/// computed (S13) and the box costs nothing to dismiss (S16).
pub(super) const HOVER_DWELL: Duration = Duration::from_millis(400);

/// What can be said about one node's current type.
///
/// Three states rather than an `Option`, because "there is no scoring
/// graph" and "this type is not something the graph ranks" are
/// different answers and a box full of zeros would be a wrong one for
/// either (spec 0280 S4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Breakdown {
    /// No scoring graph was loaded, so nothing was ever scored.
    NoGraph,
    /// The node's effective type is not a scorable root: a primitive
    /// keyword (`int32`), or a name this graph does not carry.
    Unranked,
    Scored(ScoreBreakdown),
}

/// An open score box.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct ScorePopup {
    /// The type the counts are about — shown, because the whole point
    /// is that they are about *this* typing and not the node.
    pub(super) type_key: String,
    pub(super) breakdown: Breakdown,
    /// Where it was asked for; flipped at the screen edges by
    /// `anchored_rect`, so a request rather than a position.
    pub(super) anchor: (u16, u16),
}

/// The pointer resting on a type, before the dwell has expired.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Hover {
    pub(super) node: usize,
    pub(super) anchor: (u16, u16),
}

impl ScoreBreakdown {
    /// One line per non-zero term, in `EntryScore::score`'s own
    /// coefficient order — which is also its order of severity, so the
    /// box reads worst-last the way the formula is written (spec 0280
    /// S15).
    ///
    /// Zero terms are dropped: five zeros and a total is a form to
    /// read, whereas the one or two lines that are actually non-zero
    /// are the finding.
    fn lines(&self) -> Vec<String> {
        let terms: [(u64, &str, i64); 5] = [
            (self.matches, "fields matched", 1),
            (self.unknowns, "not declared by this type", -10),
            (self.out_of_range, "outside a declared range", -15),
            (self.non_canonical, "non-canonical encoding", -20),
            (self.mismatches, "required field absent", -30),
        ];
        terms
            .iter()
            .filter(|(n, _, _)| *n > 0)
            .map(|(n, label, weight)| format!("{n:>5} × {weight:<+4} {label}"))
            .collect()
    }
}

/// The byte range, within a drawn row, of the `type` token of its `#@`
/// annotation — `None` for a row that declares none (spec 0280 S10).
///
/// Read off the format rather than off the highlighter: the annotation
/// grammar (`docs/prototext/annotation-format.md`) spells a declaration
/// `[label ]type[ [packed=true]] = NUMBER`, and `push_field_decl` is
/// the only thing that writes `" = "` into an annotation — every
/// modifier spells itself `key: value`. `node_status::row_status` leans
/// on that same fact to find the same token.
///
/// An enum's type carries its value (`Color(5)`, `Color([1, 2])`); the
/// name alone is the target, because the value is a datum and not
/// something the box has anything to say about.
fn annotation_type_span(row: &str) -> Option<Range<usize>> {
    // `annotation_start` reports where the *value* ends, before the two
    // separator spaces, so the marker is still ahead of us.
    let at = annotation_start(row)?;
    let mark = at + row[at..].find("#@ ")? + "#@ ".len();

    let mut start = mark;
    for token in row[mark..].split(';') {
        let here = start;
        start += token.len() + 1;
        let Some(eq) = token.find(" = ") else {
            continue;
        };
        let decl = &token[..eq];
        let mut lo = decl.len() - decl.trim_start().len();
        for label in ["repeated ", "required "] {
            if decl[lo..].starts_with(label) {
                lo += label.len();
                break;
            }
        }
        let hi = lo + decl[lo..].find(['(', ' ']).unwrap_or(decl.len() - lo);
        return (hi > lo).then_some(here + lo..here + hi);
    }
    None
}

impl App {
    /// The score terms for a node's current typing (spec 0280 S4).
    ///
    /// Keyed on `(heat_scored_range(idx).start, current_type_key(idx))`
    /// — the same pair `read_heat_state` keys the cue's own caches on,
    /// so the box and the cue above it can never turn out to be about
    /// different things.
    ///
    /// Synchronous, on this thread, and that is a measured choice
    /// rather than a shortcut (spec 0280 N4): this is one type against
    /// one range where the node's cue is 58 777 of them against the
    /// same range, and `heat_cue_resolve`'s no-worker arm already calls
    /// the sibling `inferred_score` right here.
    pub(super) fn score_breakdown(&mut self, idx: usize) -> Breakdown {
        let range = self.heat_scored_range(idx);
        let key = match self.current_type_key(idx) {
            Some(key) => key,
            // A node with no effective type at all — nothing was
            // scored for it, which is `Unranked` rather than a graph
            // that is missing.
            None => return Breakdown::Unranked,
        };

        // Spec 0280 S5: one entry, not a cache. Only one box can be
        // open, so a keyed map would be a cache with a single reader
        // and an eviction policy nobody needs; re-entering the name the
        // pointer just left is what this is for.
        if let Some((memo_key, breakdown)) = &self.breakdown_memo {
            if memo_key.0 == range.start && memo_key.1 == key {
                return *breakdown;
            }
        }

        // Cloned rather than borrowed, for the reason `heat_cue_resolve`
        // clones it (spec 0180 S2): the `Arc` clone leaves `self` free
        // to be borrowed mutably for the memo write below.
        let breakdown = match self.ctx.graph.clone() {
            None => Breakdown::NoGraph,
            Some(graph) => match inferred_breakdown(&self.blob[range.clone()], &key, graph.graph())
            {
                None => Breakdown::Unranked,
                Some(b) => Breakdown::Scored(b),
            },
        };
        self.breakdown_memo = Some(((range.start, key), breakdown));
        breakdown
    }

    /// Opens the box for `idx` at `anchor`, unless something outranks
    /// it (spec 0280 S17).
    pub(super) fn open_score_popup(&mut self, idx: usize, anchor: (u16, u16)) {
        if self.menu.is_some() || idx >= self.tree.len() {
            return;
        }
        let breakdown = self.score_breakdown(idx);
        let type_key = self
            .current_type_key(idx)
            .unwrap_or_else(|| "<no type>".to_string());
        self.score_popup = Some(ScorePopup {
            type_key,
            breakdown,
            anchor,
        });
    }

    /// `s` (spec 0280 S18): the same box for the node the caret is on,
    /// at the caret, for the terminals that keep the right button and
    /// for anyone who never reaches for the mouse.
    ///
    /// Anchored through C9's `menu_anchor`, which derives the caret's
    /// cell rather than remembering it — so this is right on the first
    /// keystroke after a resize, before any frame has been drawn.
    pub(super) fn open_score_popup_at_caret(&mut self) {
        if self.override_target.is_some() && !self.manage_focus {
            self.message = OVERRIDE_FOCUS_LOCK_MESSAGE.to_string();
            return;
        }
        let anchor = self.menu_anchor();
        self.open_score_popup(self.cursor, anchor);
    }

    /// The node whose type name is drawn at this point, if any (spec
    /// 0280 S10).
    ///
    /// One span of one row: the `type` token of the `#@` annotation.
    /// That is the thing the box is *about* — the reader points at the
    /// name and is told how well the bytes fit it — and it is the only
    /// place that name is written down.
    ///
    /// Deliberately no second target. "Hover anything to learn about
    /// it" has no edge a reader can hold; "hover the type you are
    /// asking about" has exactly one.
    ///
    /// A row in wire mode is two terminal rows thick and both name it,
    /// exactly as spec 0225 S8 already has a click anywhere in the pair
    /// select the line: the row is taller, not two targets.
    pub(super) fn type_annotation_at(&mut self, column: u16, row: u16) -> Option<usize> {
        let line_idx = self.main_pane_line_idx(column, row)?;
        let pos = self.line_pos(line_idx)?;
        let content = self.row_content(self.committed_row_at(line_idx, pos));
        let span = annotation_type_span(&content)?;

        // The same mapping `set_caret_from_click` computes: pane
        // column, less the reserved glyph gutter (spec 0138 N1), plus
        // the pan already taken off the content. A column past the
        // row's drawn end lands beyond every span and so names nothing.
        let index =
            (column.saturating_sub(self.main_area.x) as usize).checked_sub(1)? + self.pan_offset;
        let lo = content[..span.start].chars().count();
        let hi = lo + content[span].chars().count();
        (lo..hi).contains(&index).then_some(pos.node)
    }

    /// One `Moved` event (spec 0280 S6-S9).
    ///
    /// Returns whether anything visible changed. Almost always it has
    /// not: arming the dwell shows nothing, and the frame that
    /// eventually shows the box is bought by `hover_deadline` through
    /// `ui_deadline` instead. Only tearing down a box already on screen
    /// owes a frame — which is what keeps a pointer crossing the pane
    /// from redrawing at motion rate (G5).
    pub(super) fn handle_hover(&mut self, column: u16, row: u16) -> bool {
        // The menu is the innermost modal and owns the pointer while it
        // is open (C9); the override pane locks focus and refuses
        // everything else in the main pane.
        let blocked = self.menu.is_some() || self.override_target.is_some() || self.splash;
        let target = if blocked {
            None
        } else {
            self.type_annotation_at(column, row).map(|node| Hover {
                node,
                anchor: (column, row),
            })
        };

        if let (Some(old), Some(new)) = (self.hover, target) {
            if old.node == new.node {
                // Still on the same node: the dwell keeps running from
                // where it started, so sliding along the name does not
                // postpone the answer forever.
                return false;
            }
        }

        self.hover = target;
        self.hover_deadline = target.map(|_| Instant::now() + HOVER_DWELL);
        // Spec 0280 S13: the query goes out on arrival, so the box is
        // full the moment it appears rather than a frame or a walk
        // later.
        if let Some(h) = target {
            self.score_breakdown(h.node);
        }
        // A box left on screen by the pointer that has now left it must
        // be erased, and that is the one hover event owed a frame.
        self.score_popup.take().is_some()
    }

    /// Spec 0280 S11/S12: the dwell has expired, so the box the
    /// pointer earned is opened. Called once per `render()`, beside
    /// `track_message_timeout` — the established shape for a deadline
    /// whose expiry is noticed by the next frame rather than by an
    /// event.
    pub(super) fn track_hover_dwell(&mut self) {
        let Some(deadline) = self.hover_deadline else {
            return;
        };
        if Instant::now() < deadline {
            return;
        }
        self.hover_deadline = None;
        if let Some(hover) = self.hover {
            self.open_score_popup(hover.node, hover.anchor);
        }
    }

    /// Spec 0280 S16: anything at all except the pointer holding still
    /// closes the box. Called from the head of `handle_key` and
    /// `handle_mouse`, so there is no dismiss binding to learn and
    /// nothing can be left behind.
    ///
    /// Clears the pending dwell too: a box that was *about* to open is
    /// as unwanted as one already open once the reader has done
    /// something else.
    pub(super) fn dismiss_score_popup(&mut self) {
        self.score_popup = None;
        self.hover = None;
        self.hover_deadline = None;
    }

    /// The box itself (spec 0280 S14).
    ///
    /// Sized to its content and placed by the same `anchored_rect` the
    /// context menu uses, then `Clear`ed — it stands over the pane
    /// rather than beside it, so without the clear the border would
    /// enclose stale cells.
    pub(super) fn render_score_popup(&mut self, frame: &mut Frame, area: Rect) {
        let Some(popup) = &self.score_popup else {
            return;
        };
        let lines = Self::score_popup_lines(popup);
        let inner_width = lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(1)
            .max(1) as u16;
        let width = (inner_width + 2).min(area.width.max(1));
        let height = (lines.len() as u16 + 2).min(area.height.max(1));
        let rect = render::anchored_rect(popup.anchor, width, height, area);

        frame.render_widget(Clear, rect);
        let block = Block::bordered().border_type(BorderType::Rounded);
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        let text: Vec<Line> = lines.into_iter().map(Line::from).collect();
        frame.render_widget(Paragraph::new(text), inner);
    }

    /// The box's text: the typing it is about, then its terms.
    pub(super) fn score_popup_lines(popup: &ScorePopup) -> Vec<String> {
        let mut lines = vec![popup.type_key.clone()];
        match popup.breakdown {
            Breakdown::NoGraph => lines.push("no scoring graph loaded".to_string()),
            Breakdown::Unranked => lines.push("not a scored type".to_string()),
            // Spec 0280 S3: a veto fires part-way through a field, so
            // the counters hold whatever had accumulated when the walk
            // stopped. Printing them would be printing where the walk
            // gave up as though it were a property of the payload.
            Breakdown::Scored(b) if b.vetoed => {
                lines.push("vetoed — the wire contradicts this type".to_string());
            }
            Breakdown::Scored(b) => {
                let terms = b.lines();
                if terms.is_empty() {
                    lines.push("nothing scored for this range".to_string());
                } else {
                    lines.extend(terms);
                    lines.push(format!("{:>5}       total", b.score()));
                }
            }
        }
        lines
    }
}
