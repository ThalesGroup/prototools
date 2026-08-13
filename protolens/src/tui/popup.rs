// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The box a resting pointer earns: the dwell, the anchor, the
//! dismissal, and the chrome the two contents share.
//!
//! Spec 0280 built the first content. A cue prints `[3/7]` and gives no
//! reason; the reason is already computed and then discarded, since
//! `EntryScore` carries the five terms the score is the weighted sum of
//! and protolens keeps only the sum. This module asks for the terms
//! back, one node at a time, and puts them in a box.
//!
//! Deliberately *not* a ranking. The override pane answers "what else
//! could this be"; this answers "how badly does what it is now fit".
//! Two questions, two surfaces (spec 0280 N1).
//!
//! Spec 0282 added the second content, in `popup_wire.rs`: the same
//! box over one part of a wire row. One mechanism, two bodies — every
//! timing and teardown rule below is shared, and nothing here knows
//! which body it is holding.

use super::popup_doc::{annotation_type_span, DocHit};
use super::wire::WireHit;
use super::*;
use crate::override_pane::{inferred_breakdown, ScoreBreakdown};

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

/// The same, for an explanation rather than a datum (spec 0285 S7).
///
/// [`HOVER_DWELL`] is short because the answer is about the reader's
/// own bytes and they asked about one specific node. This is the other
/// case: a pointer crossing a dense `#@` annotation on its way
/// somewhere passes over five explainable tokens, and at 400 ms each
/// crossing would open five boxes in turn. 900 ms is past the top of
/// the 400-600 ms desktop range — long enough to be the pause of
/// someone who has stopped to read rather than someone travelling.
pub(super) const EXPLAIN_DWELL: Duration = Duration::from_millis(900);

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

/// What an open box is about — one mechanism, two contents
/// (spec 0282 S14).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum PopupBody {
    /// Spec 0280: how badly a node's bytes fit the type it is read as.
    Score {
        /// The type the counts are about — shown, because the whole
        /// point is that they are about *this* typing and not the node.
        type_key: String,
        breakdown: Breakdown,
    },
    /// Spec 0282: what one part of one wire row says.
    Wire(WireBox),
    /// Spec 0285: what one token of one document row means. A fixed
    /// two to four lines, so it needs nothing of 0282 S10's `fit`.
    Doc(Vec<BoxLine>),
}

/// One line of a box.
///
/// Spec 0283 S3: the mark travels with the line rather than in a table
/// keyed by line index, because `fit` drops lines and moves the flaws
/// past an ellipsis — an index-keyed table would have to be remapped at
/// every one of those steps.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct BoxLine {
    pub(super) text: String,
    /// Characters of `text` the hovered byte produced (spec 0283 S4).
    pub(super) mark: Option<Range<usize>>,
}

impl BoxLine {
    /// A line with nothing to point at, which is most of them.
    pub(super) fn plain(text: String) -> Self {
        Self { text, mark: None }
    }
}

/// The wire box's text, kept in three groups because the terminal may
/// not have room for all of it and the groups do not rank equally
/// (spec 0282 S10).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(super) struct WireBox {
    /// The answer to the question that was asked — the declared
    /// reading, the field, the mark. Last to go.
    pub(super) head: Vec<BoxLine>,
    /// The other readings of the same bytes (S9). First to go: a sixth
    /// spelling of the same number is not why the reader stopped here.
    pub(super) alts: Vec<BoxLine>,
    /// The part's flaws (S13). They outrank the alternatives.
    pub(super) flaws: Vec<BoxLine>,
}

impl WireBox {
    /// The body, built against the height it has rather than built
    /// whole and clipped (spec 0282 S10).
    ///
    /// Reserved in order: the head, then the flaws, then as many
    /// alternatives as are left over. Anything dropped leaves a single
    /// `…` where the list stops, immediately *before* the flaws — so
    /// the `…` says "there are more readings" and the lines after it
    /// say the thing worth saying.
    fn fit(&self, avail: usize) -> Vec<BoxLine> {
        let whole = self.head.len() + self.alts.len() + self.flaws.len();
        if whole <= avail {
            let mut out = self.head.clone();
            out.extend(self.alts.iter().cloned());
            out.extend(self.flaws.iter().cloned());
            return out;
        }

        // One line is spent on the `…` itself, since something is
        // certainly being dropped.
        let mut out = self.head.clone();
        let mut flaws = self.flaws.clone();
        while out.len() + flaws.len() + 1 > avail && !flaws.is_empty() {
            // Cut from the bottom, and the `…` stands for these too.
            flaws.pop();
        }
        while out.len() + 1 > avail && out.len() > 1 {
            out.pop();
        }
        let room = avail.saturating_sub(out.len() + flaws.len() + 1);
        out.extend(self.alts.iter().take(room).cloned());
        out.push(BoxLine::plain("…".to_string()));
        out.extend(flaws);
        out
    }
}

/// An open box.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Popup {
    pub(super) body: PopupBody,
    /// Where it was asked for; flipped at the screen edges by
    /// `anchored_rect`, so a request rather than a position.
    pub(super) anchor: (u16, u16),
}

/// What the pointer is resting on (spec 0282 S1).
///
/// A target enum rather than a second field on `Hover`, so that
/// "still on the same thing, do not restart the dwell" stays one
/// comparison and cannot be asked of two targets at once.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum HoverTarget {
    /// Spec 0280: the type name in a `#@` annotation, and the node it
    /// types.
    Type(usize),
    /// Spec 0282: one part of one wire row.
    Wire(WireHit),
    /// Spec 0285: one token of one document row.
    Doc(DocHit),
}

impl HoverTarget {
    /// Whether two targets are the same thing asked about differently
    /// (spec 0283 S11) — only a wire hit has such a thing, since only it
    /// resolves finer than the thing it names.
    fn same_part(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Wire(a), Self::Wire(b)) => a.same_part(b),
            _ => false,
        }
    }

    /// How long the pointer must hold still to earn this box
    /// (spec 0285 S7). One deadline field either way: which constant it
    /// is set from is a property of what is being asked, not of the
    /// timer.
    fn dwell(&self) -> Duration {
        match self {
            Self::Doc(_) => EXPLAIN_DWELL,
            _ => HOVER_DWELL,
        }
    }
}

/// The pointer resting on something, before the dwell has expired.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Hover {
    pub(super) target: HoverTarget,
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
        self.popup = Some(Popup {
            body: PopupBody::Score {
                type_key,
                breakdown,
            },
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
            // Spec 0282 S2: which target a point names is decided by
            // which terminal row of the pair it is in, not by what is
            // drawn there. Part 0 is the document row and keeps 0280's
            // target; part 1 is the wire row and takes 0282's.
            match self.main_pane_line_part(column, row) {
                Some((_, part)) if part > 0 => {
                    self.wire_part_at(column, row).map(HoverTarget::Wire)
                }
                // Spec 0285 S5: the type name first, so it keeps 0280's
                // box and its shorter dwell; every other token of the
                // row falls through to the explanation.
                Some(_) => match self.type_annotation_at(column, row) {
                    Some(node) => Some(HoverTarget::Type(node)),
                    None => self.doc_element_at_point(column, row).map(HoverTarget::Doc),
                },
                None => None,
            }
            .map(|target| Hover {
                target,
                anchor: (column, row),
            })
        };

        // Still on the same thing: the dwell keeps running from where it
        // started, so sliding along the name — or along a payload run —
        // does not postpone the answer forever.
        //
        // Spec 0283 S11: two bytes of one payload are two targets, since
        // the hit names the byte. They are not two *questions*, so the
        // dwell is not restarted either; and if the box is already open
        // it re-marks itself at once rather than after another dwell,
        // because the delay is there to decide whether the reader wants
        // a box at all and one is already in front of them.
        let continued = match (&self.hover, &target) {
            (Some(old), Some(new)) if old.target == new.target => return false,
            (Some(old), Some(new)) => old.target.same_part(&new.target).then_some(old.anchor),
            _ => None,
        };
        if let (Some(anchor), Some(hover)) = (continued, target.as_ref()) {
            let target = hover.target.clone();
            let open = self.popup.is_some();
            self.hover = Some(Hover {
                target: target.clone(),
                anchor,
            });
            // The box does not move: it is the same question, so it
            // stays anchored where the gesture that asked it began.
            match (open, &target) {
                (true, HoverTarget::Wire(hit)) => {
                    self.open_wire_popup(hit, anchor);
                    return true;
                }
                _ => return false,
            }
        }

        self.hover = target;
        self.hover_deadline = self
            .hover
            .as_ref()
            .map(|hover| Instant::now() + hover.target.dwell());
        // Spec 0280 S13: the query goes out on arrival, so the box is
        // full the moment it appears rather than a frame or a walk
        // later. A wire target has nothing to send: `wire_part_at` has
        // already done the whole of its work.
        if let Some(Hover {
            target: HoverTarget::Type(node),
            ..
        }) = self.hover
        {
            self.score_breakdown(node);
        }
        // A box left on screen by the pointer that has now left it must
        // be erased, and that is the one hover event owed a frame.
        self.popup.take().is_some()
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
        let Some(hover) = self.hover.clone() else {
            return;
        };
        match hover.target {
            HoverTarget::Type(node) => self.open_score_popup(node, hover.anchor),
            HoverTarget::Wire(hit) => self.open_wire_popup(&hit, hover.anchor),
            HoverTarget::Doc(hit) => self.open_doc_popup(&hit, hover.anchor),
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
    pub(super) fn dismiss_popup(&mut self) {
        self.popup = None;
        self.hover = None;
        self.hover_deadline = None;
    }

    /// The box itself (spec 0280 S14).
    ///
    /// Sized to its content and placed by the same `anchored_rect` the
    /// context menu uses, then `Clear`ed — it stands over the pane
    /// rather than beside it, so without the clear the border would
    /// enclose stale cells.
    pub(super) fn render_popup(&mut self, frame: &mut Frame, area: Rect) {
        let Some(popup) = &self.popup else {
            return;
        };
        // Spec 0282 S10: the body is built against the height it has,
        // because the chrome's own clamping would cut from the bottom
        // and so drop the flaws first — exactly backwards.
        let lines = Self::popup_lines(popup, area.height.saturating_sub(2).max(1) as usize);
        let inner_width = lines
            .iter()
            .map(|l| l.text.chars().count())
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
        // Spec 0283 S8: a mark reaching past the box's own edge is
        // dropped rather than clamped. Half of `\377` marked says the
        // byte spells three characters, which is the one thing this is
        // for getting right.
        let shown = width.saturating_sub(2) as usize;
        let theme = self.theme;
        let text: Vec<Line> = lines
            .into_iter()
            .map(|line| match line.mark {
                Some(mark) if mark.end <= shown => marked_line(&line.text, mark, theme),
                _ => Line::from(line.text),
            })
            .collect();
        frame.render_widget(Paragraph::new(text), inner);
    }

    /// The box's text, in at most `avail` lines.
    ///
    /// Only the wire body is built against the height (spec 0282 S10);
    /// a score box is five lines at its very largest and has no ranking
    /// to apply.
    pub(super) fn popup_lines(popup: &Popup, avail: usize) -> Vec<BoxLine> {
        let (type_key, breakdown) = match &popup.body {
            PopupBody::Wire(body) => return body.fit(avail),
            PopupBody::Doc(lines) => return lines.clone(),
            PopupBody::Score {
                type_key,
                breakdown,
            } => (type_key, breakdown),
        };
        let mut lines = vec![type_key.clone()];
        match breakdown {
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
        // A score box points at nothing: it is about a node, and the
        // pointer was on a type name rather than on a byte.
        lines.into_iter().map(BoxLine::plain).collect()
    }
}

/// One box line split around its mark (spec 0283 S9).
///
/// The only place a mark becomes a `Style`; everything above it is
/// ranges.
fn marked_line(text: &str, mark: Range<usize>, theme: ThemeKind) -> Line<'static> {
    let chars: Vec<char> = text.chars().collect();
    let piece = |range: Range<usize>| chars[range].iter().collect::<String>();
    let mut spans = Vec::with_capacity(3);
    if mark.start > 0 {
        spans.push(Span::raw(piece(0..mark.start)));
    }
    spans.push(Span::styled(
        piece(mark.clone()),
        // Spec 0283 S7: the style a search gives the match it landed on.
        // Not the muted one, which means "another occurrence, context
        // rather than the answer" — this *is* the answer.
        theme::search_current_style(theme),
    ));
    if mark.end < chars.len() {
        spans.push(Span::raw(piece(mark.end..chars.len())));
    }
    Line::from(spans)
}
