// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0244 S1: one vertical viewport, shared by the three pannable
//! panes (main, override, manage).

use std::ops::Range;
use std::time::{Duration, Instant};

use super::{App, DisplayRow};

/// How tall each content row is, in terminal rows (spec 0268 S4).
///
/// Every row is one terminal row except those in `tall`, which are two —
/// the wire row (spec 0225) is drawn under them. Spec 0268 N1's
/// single-span rule is what keeps this three numbers instead of a set:
/// the map from a content row to the terminal row it starts on is
/// arithmetic, and invertible in the same form.
///
/// The side panes have no wire rows, so they pass `RowHeights::default()`
/// and every method below degenerates to the identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct RowHeights {
    /// The content rows drawn two terminal rows tall. Empty means none.
    tall: Range<usize>,
}

impl RowHeights {
    /// The rows in `tall` are two terminal rows; everything else is one.
    pub(super) fn new(tall: Range<usize>) -> Self {
        Self {
            tall: if tall.is_empty() { 0..0 } else { tall },
        }
    }

    /// The terminal row that content row `row` starts on, counted from
    /// the top of content row 0.
    pub(super) fn offset(&self, row: usize) -> usize {
        row + (row.clamp(self.tall.start, self.tall.end) - self.tall.start)
    }

    /// How many terminal rows content row `row` occupies.
    pub(super) fn height(&self, row: usize) -> usize {
        if self.tall.contains(&row) {
            2
        } else {
            1
        }
    }

    /// `offset`'s inverse: the content row containing terminal row
    /// `offset`, and how far into it that row is.
    pub(super) fn row_at(&self, offset: usize) -> (usize, usize) {
        if offset < self.tall.start {
            return (offset, 0);
        }
        let past = offset - self.tall.start;
        let span = self.tall.len() * 2;
        if past < span {
            (self.tall.start + past / 2, past % 2)
        } else {
            (self.tall.end + (past - span), 0)
        }
    }

    /// How many content rows starting at `start` it takes to *cover*
    /// `terminal_rows` — a partial row at the end counts, because it is
    /// still drawn.
    pub(super) fn rows_to_cover(&self, start: usize, terminal_rows: usize) -> usize {
        let (row, part) = self.row_at(self.offset(start) + terminal_rows);
        (row + usize::from(part > 0)).saturating_sub(start)
    }

    /// How many content rows starting at `start` fit *whole* within
    /// `terminal_rows`.
    pub(super) fn rows_fitting(&self, start: usize, terminal_rows: usize) -> usize {
        self.row_at(self.offset(start) + terminal_rows)
            .0
            .saturating_sub(start)
    }
}

/// A pane's vertical viewport: the first content row it draws, plus the
/// signed remainder in terminal rows.
///
/// Spec 0230 introduced the pair for the main pane, where a content row
/// is a *document line* and in wire mode is two terminal rows thick —
/// too coarse a unit to hold a row still across a `w` toggle. `skip` is
/// the remainder: positive means that many terminal rows of the row at
/// `index` are cut off the top of the pane, negative means that many
/// blank rows are drawn above the content's first row.
///
/// The pair is always normalized so that the two together name the same
/// terminal row (`top`), which leaves `skip` in `0..row_height` except at
/// the very top, where there is no whole row left to borrow from and it
/// goes negative.
///
/// The side panes' rows are one terminal row tall, so for them the pair
/// degenerates to "row index, plus blank rows above" — but it is the same
/// value, with the same one writer, so the resets and the window
/// arithmetic are shared rather than mirrored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PaneScroll {
    /// The first content row drawn, in the pane's own row units.
    pub(super) index: usize,
    /// The signed remainder, in terminal rows. See the type's docs.
    pub(super) skip: isize,
}

impl PaneScroll {
    /// The top edge of the viewport, in terminal rows counted from the
    /// top of content row 0. Negative means blank rows above it.
    pub(super) fn top(&self, heights: &RowHeights) -> isize {
        heights.offset(self.index) as isize + self.skip
    }

    /// Puts the viewport's top edge at `top`, splitting it back into the
    /// whole row and the remainder. The only writer of the pair.
    pub(super) fn set_top(&mut self, top: isize, heights: &RowHeights) {
        if top < 0 {
            self.index = 0;
            self.skip = top;
        } else {
            let (index, skip) = heights.row_at(top as usize);
            self.index = index;
            self.skip = skip as isize;
        }
    }

    /// What a renderer draws: how many blank rows to prepend, and which
    /// content rows to draw after them.
    ///
    /// `pane_height` is in terminal rows. The range is netted against
    /// `skip` — a viewport that starts half way down a row needs one more
    /// row to fill the same pane, and one that starts above the content
    /// needs fewer — and rounded *up*, because a partial row at either
    /// end is still drawn.
    pub(super) fn window(
        &self,
        pane_height: usize,
        heights: &RowHeights,
        total: usize,
    ) -> (usize, Range<usize>) {
        let terminal_rows = (pane_height as isize + self.skip).max(0) as usize;
        let rows = heights.rows_to_cover(self.index, terminal_rows);
        let start = self.index.min(total);
        let end = (self.index + rows).min(total);
        (self.skip.min(0).unsigned_abs(), start..end)
    }
}

/// Every content row one terminal row tall: the side panes always, and
/// the main pane whenever no bytes are shown.
pub(super) const FLAT_ROWS: RowHeights = RowHeights { tall: 0..0 };

/// Spec 0286 S3+S5: the wall's one time constant. How long the end of
/// the content must be pushed against before it lets a pan through —
/// and, the same number, how long a gap between two pushes may be
/// before the run behind them is forgiven.
///
/// Measured in *time*, not in pans. A pan is not a fixed quantity of
/// intent: `PAN_STEP` is 8 rows and `WHEEL_PAN_STEP` is 1, and — the
/// reason a pan count is the wrong unit even between two presses of the
/// same key — a held `Down` arrives at the terminal's autorepeat rate
/// while a tapped one arrives at the reader's. A count tuned for the
/// tapping is instant under autorepeat; a count tuned for autorepeat is
/// unreachable by tapping. A moment of pushing is a moment of pushing
/// either way.
///
/// One constant and not two, because the two roles are one statement
/// read from either side: pushing for this long is only pushing if the
/// pushes are no further apart than this. Two numbers can contradict
/// each other, and the first tuning did — a forgiveness *shorter* than
/// the hold sets a rate floor above the hold's own, so no deliberate
/// tapping ever gets through while a held key sails past. Equal, the
/// floor is the hold itself, which is the only rate the wall ever meant
/// to ask for.
///
/// No timer is involved (spec 0263): the clock is only ever read on a
/// pan, so the wall yields to the first push that arrives after the
/// delay, not at the delay itself. That is the same thing for a reader
/// who is still pushing, and nothing at all for one who stopped.
pub(super) const EDGE_HOLD: Duration = Duration::from_millis(400);

/// Spec 0286 S3: the fewest pans that can be a gesture, whatever the
/// clock says.
///
/// [`EDGE_HOLD`] alone would let two pans that merely *span* it through
/// — a stray notch at the bottom of a document and another one a second
/// later. Three is the smallest count a wheel's own jitter does not
/// reach: a notch either side of a stop is common, three in the same
/// direction are not.
pub(super) const EDGE_PUSHES: u8 = 3;

/// Spec 0286: the resistance at either end of the content.
///
/// Spec 0244 S6 lets the viewport run past the content, which is a real
/// capability and is also reached by accident on the way to the last
/// line. This puts a wall at the last line that sustained pushing gets
/// through.
///
/// Two fields and no more. In particular there is no "already yielded"
/// flag: once through, the top edge is *outside* the natural range, and
/// [`Self::land`] reads that position as the latch (S4). A second
/// representation of a fact the position already carries is a second
/// thing to get wrong.
///
/// One of these per pannable pane. Only the main pane has one today
/// (spec 0286 N3); the side panes want the same wall and need a field
/// and a call site each, not a redesign.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct EdgeResistance {
    /// Pans refused at this end since the run began, for
    /// [`EDGE_PUSHES`]. Zero is "nothing is pushing".
    pushes: u8,
    /// When the run began, for [`EDGE_HOLD`].
    first_push: Option<Instant>,
    /// When the last pan of it was refused, for [`EDGE_HOLD`] in its
    /// forgiveness role. Re-armed by every push, so the gap that
    /// forgives is the gap between two pans and not the age of the run.
    last_push: Option<Instant>,
    /// Whether [`Self::land`] has been called since the last
    /// [`Self::settle`] — that is, whether the input event being
    /// dispatched is a pan. Not state about the wall; state about the
    /// event, and cleared by every one of them.
    touched: bool,
    /// Which pan the run is made of: `(step, up)`, which is as much as
    /// tells a wheel notch from an `Alt`-arrow and either from its
    /// opposite. A run is a repeat of the *same* push, so changing the
    /// gesture starts a new one. Input that is not a pan at all never
    /// gets this far — [`Self::settle`] has already ended the run.
    gesture: (usize, bool),
}

impl EdgeResistance {
    /// Whether the wall is currently being pushed, which is what the
    /// viewport label's accent reports (spec 0286 S6).
    ///
    /// Deliberately not decayed here. The forgiveness is evaluated in
    /// [`Self::land`] because that is where a decision depends on it;
    /// applying it in the reader as well would make the accent honest in
    /// the data model and no sooner on screen, since nothing draws a
    /// frame while the pane sits idle (S5).
    pub(super) fn pushing(&self) -> bool {
        self.pushes > 0
    }

    /// Called once after every dispatched input event: any input that
    /// was not itself a pan against the wall ends the gesture, dropping
    /// the pressure and putting the cue out (spec 0286 S6).
    ///
    /// Asked of the wall rather than of the key that arrived, so that
    /// the binding table does not have to be kept in step with a second
    /// list of "these are the panning keys".
    ///
    /// Returns whether that put a *lit* cue out, which is a frame owed
    /// (S7): the caller has already decided the event changed nothing.
    pub(super) fn settle(&mut self) -> bool {
        if std::mem::take(&mut self.touched) {
            return false;
        }
        let was_lit = self.pushing();
        self.forget();
        was_lit
    }

    /// Where a pan lands: `gesture` is `(step, up)` — what the pan *is*,
    /// so that a repeat can be told from a change of mind — `from` is
    /// the viewport's top edge, `want` where the gesture would put it,
    /// `natural` the bounds within which every drawn row is content,
    /// `hard` spec 0244 S5's over-pan bounds.
    ///
    /// `now` is a parameter rather than read here so that a test can age
    /// the clock without sleeping.
    ///
    /// Both bound pairs must be ordered, which their two constructors
    /// guarantee by clamping against `0`; `clamp` would panic otherwise.
    pub(super) fn land(
        &mut self,
        gesture: (usize, bool),
        from: isize,
        want: isize,
        natural: (isize, isize),
        hard: (isize, isize),
        now: Instant,
    ) -> isize {
        self.touched = true;
        // S5, lazily: a push older than the delay never happened.
        if self
            .last_push
            .is_some_and(|at| now.saturating_duration_since(at) > EDGE_HOLD)
        {
            self.forget();
        }
        // S4: already past a natural bound, so the wall has been paid
        // for. Move freely, and re-arm on the way back in.
        if from < natural.0 || from > natural.1 {
            let landed = want.clamp(hard.0, hard.1);
            if landed >= natural.0 && landed <= natural.1 {
                self.forget();
            }
            return landed;
        }
        // S2: the natural bounds get the first say, and a gesture that
        // moves at all is an ordinary move however short it was stopped.
        let landed = want.clamp(natural.0, natural.1);
        if landed != from {
            self.forget();
            return landed;
        }
        // S3: refused, and nothing moved. That is a push — but only a
        // push of the *same* pan is a repeat of the same intent, so a
        // different one starts its own run rather than inheriting.
        if self.gesture != gesture {
            self.forget();
        }
        self.gesture = gesture;
        // Two floors, for two different accidents: long enough to be a
        // gesture, and more pans than a jittering wheel delivers by
        // itself.
        let began = *self.first_push.get_or_insert(now);
        self.pushes = self.pushes.saturating_add(1);
        self.last_push = Some(now);
        if self.pushes >= EDGE_PUSHES && now.saturating_duration_since(began) >= EDGE_HOLD {
            self.forget();
            return want.clamp(hard.0, hard.1);
        }
        from
    }

    fn forget(&mut self) {
        self.pushes = 0;
        self.first_push = None;
        self.last_push = None;
    }

    /// Pretends the run standing against the wall began `by` earlier, so
    /// that a test can reach the yield without sleeping through
    /// [`EDGE_HOLD`] — and without a second, fake clock to get wrong.
    #[cfg(test)]
    pub(super) fn backdate(&mut self, by: Duration) {
        if let Some(first) = self.first_push.as_mut() {
            *first = first.checked_sub(by).unwrap_or(*first);
        }
    }
}

/// Which of a node's own rows an anchor names, in a form that survives
/// the node's row count changing under it (spec 0259 S2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AnchorLine {
    /// Counted from the node's header: a bracketed node's header, or any
    /// row of a flat node, whose count a splice does not change.
    FromStart(u32),
    /// A bracketed node's closing brace. It lives at `lines_total - 1`,
    /// so its `line_in_node` is stale the moment the body it closes is
    /// baked in — stored as "the last one" instead, and resolved against
    /// whatever the count has become.
    Footer,
}

/// Spec 0268 S1: the one run of rows currently showing their bytes.
///
/// Held as a pair of nodes rather than a pair of line numbers for the
/// same reason [`ScrollAnchor`] is: a splice renumbers every line below
/// it, so a span kept as numbers slides off the rows it was pointing at.
/// [`AnchorLine::Footer`] does the other half — it means "the node's
/// last line, whatever the count has become", which is what lets a `W`
/// at the root keep covering the lines a bake has not produced yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WireSpan {
    pub(super) first: WireAnchor,
    pub(super) last: WireAnchor,
}

/// One end of a [`WireSpan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WireAnchor {
    pub(super) node: usize,
    pub(super) line: AnchorLine,
}

/// A [`WireSpan`] resolved to visible rows, and the structure it was
/// resolved against.
///
/// Resolving costs two descents and the geometry asks for the answer
/// several times a frame, so `App` keeps the last one. The two questions
/// this answers are deliberately separate fields rather than nested
/// `Option`s: whether an answer exists at all is the *cache's* business,
/// while `rows` being `None` is a real answer — the span's ends are
/// currently folded away, and no row shows its bytes.
#[derive(Clone, Debug)]
pub(super) struct WireRowCache {
    pub(super) version: u64,
    pub(super) rows: Option<Range<usize>>,
}

/// The main pane's viewport, expressed against the document's structure
/// rather than against its row numbering (spec 0259 S1).
///
/// A splice renumbers every row below it, so a viewport held as a row
/// number moves the content out from under the reader. Held as a node it
/// does not: the arena is immutable (spec 0216), so a slot index stays
/// valid for the life of the document and nothing has to invalidate this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScrollAnchor {
    node: usize,
    line: AnchorLine,
    /// `PaneScroll.skip` as it was, so a wire-mode viewport that starts
    /// half way down a row lands back half way down it.
    skip: isize,
}

impl App {
    /// Spec 0259 S1: remember which node the pane's top row belongs to,
    /// so that the next splice can put that row back where it was.
    ///
    /// Read off the window the frame has already built — `build_window`
    /// resolves every row to its owning node (spec 0222 S3), so the top
    /// one is in hand and this costs no descent.
    ///
    /// Spec 0259 S5: not while a preview overlay is held. With an
    /// overlay a display row and a committed visible row are different
    /// numbers, and the restore answers in the second. No anchor leaves
    /// the pre-spec behavior, which is what the overlay had anyway.
    pub(super) fn capture_scroll_anchor(&mut self, window: &[DisplayRow]) {
        self.scroll_anchor = None;
        if self.preview_overlay.is_some() {
            return;
        }
        let Some(&DisplayRow::Committed(c)) = window.first() else {
            return;
        };
        self.scroll_anchor = Some(ScrollAnchor {
            node: c.pos.node,
            line: if self.is_footer(c.pos) {
                AnchorLine::Footer
            } else {
                AnchorLine::FromStart(c.pos.line_in_node)
            },
            skip: self.scroll.skip,
        });
    }

    /// Spec 0259 S3: put the anchored row back on the terminal row it was
    /// drawn on.
    ///
    /// Called at the top of `finalize_override_batch`, before the pan
    /// clamp — with the viewport already restored the clamp finds the
    /// caret inside the pane and moves nothing, so the two compose
    /// instead of fighting. The other order undoes this and leaves the
    /// caret off screen.
    pub(super) fn restore_scroll_anchor(&mut self) {
        let Some(anchor) = self.scroll_anchor else {
            return;
        };
        // Spec 0259 S4: an `FqdnField` override can flatten this node's
        // parent to `bytes`, leaving the node with no row of its own.
        // The root is always rendered, so the climb terminates.
        let mut node = anchor.node;
        let mut climbed = false;
        while self.tree[node].lines_total == 0 {
            match self.parent(node) {
                Some(p) => {
                    node = p;
                    climbed = true;
                }
                None => return,
            }
        }
        // A climb lands on the node that swallowed the anchor, and the
        // row within it that the anchor named is gone with the rendering
        // it was part of — so the ancestor's own header is what is left
        // to aim at.
        let line_in_node = match anchor.line {
            _ if climbed => 0,
            AnchorLine::Footer => self.tree[node].lines_total.saturating_sub(1),
            AnchorLine::FromStart(k) => k.min(self.tree[node].lines_total.saturating_sub(1)),
        };
        let line = self.absolute_start(node) + line_in_node as usize;
        let Some(row) = self.visible_row_of_line(line) else {
            return;
        };
        let top = self.row_heights().offset(row) as isize + anchor.skip;
        self.set_scroll_top(top);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No wire rows anywhere: every content row is one terminal row.
    fn flat() -> RowHeights {
        RowHeights::default()
    }

    /// Wire rows under the whole document, which is what the pre-0268
    /// `row_height() == 2` meant.
    fn all_tall() -> RowHeights {
        RowHeights::new(0..1000)
    }

    /// A 100-row document in a 20-row pane: the natural range is
    /// `0..=80`, the hard one `-19..=99` (spec 0244 S5).
    const NATURAL: (isize, isize) = (0, 80);
    const HARD: (isize, isize) = (-19, 99);

    /// A tapping rate the forgiveness tolerates — under `EDGE_HOLD`,
    /// which is the whole of what it asks. Three taps this far apart
    /// span the hold, which is the cheapest way through the wall there
    /// is.
    const TAP: Duration = Duration::from_millis(300);

    /// Two gestures that are not each other: a wheel notch down and an
    /// `Alt`-arrow down, which is `WHEEL_PAN_STEP` against `PAN_STEP`.
    const NOTCH: (usize, bool) = (1, false);
    const ARROW: (usize, bool) = (8, false);

    /// Pushes the wall at `rate` with a `step`-row pan until one gets
    /// through: how many pans that took, and how long they took.
    fn pans_to_break(step: isize, rate: Duration) -> (u32, Duration) {
        let start = Instant::now();
        let mut wall = EdgeResistance::default();
        let mut at = start;
        for pans in 1..100 {
            if wall.land(NOTCH, 80, 80 + step, NATURAL, HARD, at) != 80 {
                assert!(!wall.pushing(), "getting through spends the pressure");
                return (pans, at.saturating_duration_since(start));
            }
            assert!(wall.pushing(), "a refused pan lights the cue");
            at += rate;
        }
        panic!("step {step} at {rate:?} never got through");
    }

    /// Spec 0286 test plan 1 / S2: a pan that would cross the end of the
    /// content lands on it instead, at either end and however far it
    /// over-reached.
    #[test]
    fn a_pan_stops_at_the_last_line_rather_than_past_it() {
        let now = Instant::now();
        for (from, want, stop) in [
            (78isize, 86isize, 80isize),
            (80, 88, 80),
            (2, -6, 0),
            (0, -8, 0),
        ] {
            let mut wall = EdgeResistance::default();
            assert_eq!(
                wall.land(NOTCH, from, want, NATURAL, HARD, now),
                stop,
                "{from} -> {want}"
            );
        }
    }

    /// Spec 0286 test plan 2 / S3: the toll is `EDGE_HOLD` of pushing,
    /// whatever the pushing is made of. A `PAN_STEP` of 8 and a
    /// `WHEEL_PAN_STEP` of 1 buy the same progress through it, and so do
    /// a held key's autorepeat and a reader's own tapping — which is the
    /// whole argument for measuring time rather than counting pans.
    #[test]
    fn the_wall_yields_to_a_sustained_push() {
        let held = Duration::from_millis(30);
        let tapped = TAP;
        for step in [1isize, 8] {
            for rate in [held, tapped] {
                let (_, took) = pans_to_break(step, rate);
                assert!(
                    took >= EDGE_HOLD && took < EDGE_HOLD + rate,
                    "step {step} at {rate:?} got through after {took:?}"
                );
            }
        }
        // The same delay is a great many pans of a held key and a
        // handful of tapped ones. No single count could have served both.
        assert!(
            pans_to_break(1, held).0 > pans_to_break(1, tapped).0 + 5,
            "the two rates must not agree on a pan count"
        );
    }

    /// Spec 0286 S3: `EDGE_PUSHES` is the floor `EDGE_HOLD` cannot
    /// supply. Two pans that merely *span* the delay are not a delay's
    /// worth of pushing — a stray notch at the bottom of a document and
    /// another one a moment later must not over-pan.
    ///
    /// One constant serves both roles, so exactly `EDGE_HOLD` apart is
    /// the only spacing at which two pans could have managed it: any
    /// wider and the first is forgiven before the second lands.
    #[test]
    fn two_pans_far_apart_do_not_add_up_to_a_hold() {
        let now = Instant::now();
        let mut wall = EdgeResistance::default();
        assert_eq!(wall.land(NOTCH, 80, 81, NATURAL, HARD, now), 80);
        assert_eq!(
            wall.land(NOTCH, 80, 81, NATURAL, HARD, now + EDGE_HOLD),
            80,
            "long enough, but two pans are not a gesture"
        );
    }

    /// Spec 0286 S5: the gap that forgives is measured from the *last*
    /// push, so every push re-arms it and a run held longer than
    /// `EDGE_HOLD` is still one run. Without that, the delay would
    /// double as a ceiling on how long a gesture may last.
    #[test]
    fn each_push_re_arms_the_forgiveness() {
        let now = Instant::now();
        let mut wall = EdgeResistance::default();
        // Pans a whole delay apart, which is as slow as a run may go.
        // It outlives the delay twice over and is never forgiven.
        for n in 0..2 {
            assert_eq!(
                wall.land(NOTCH, 80, 81, NATURAL, HARD, now + EDGE_HOLD * n),
                80
            );
        }
        assert_eq!(
            wall.land(NOTCH, 80, 81, NATURAL, HARD, now + EDGE_HOLD * 2),
            81,
            "still the same run, and long past the hold"
        );
    }

    /// Spec 0286 S3: a run is a repeat of the *same* push. Reaching for
    /// a different pan is a different intent and starts the count over,
    /// however much pressure the previous one had built. Input that is
    /// not a pan at all never reaches here — `settle` ends the run
    /// first.
    #[test]
    fn a_different_gesture_starts_its_own_run() {
        let now = Instant::now();
        let mut wall = EdgeResistance::default();
        for n in 0..2 {
            assert_eq!(wall.land(NOTCH, 80, 81, NATURAL, HARD, now + TAP * n), 80);
        }
        // One more notch would have been through. An arrow is not one
        // more notch.
        assert_eq!(wall.land(ARROW, 80, 88, NATURAL, HARD, now + TAP * 2), 80);
        assert_eq!(wall.land(ARROW, 80, 88, NATURAL, HARD, now + TAP * 3), 80);
        assert_eq!(
            wall.land(ARROW, 80, 88, NATURAL, HARD, now + TAP * 4),
            88,
            "the arrow paid its own toll from the beginning"
        );
    }

    /// Spec 0286 test plan 3 / S4: past the wall there is no second toll
    /// — the position is the latch — and coming back inside re-arms it.
    #[test]
    fn an_over_panned_pane_moves_freely_and_re_arms_on_return() {
        let now = Instant::now();
        let mut wall = EdgeResistance::default();
        for n in 0..3 {
            wall.land(NOTCH, 80, 81, NATURAL, HARD, now + TAP * n);
        }
        // Out in the over-pan region: every step is granted whole, up to
        // spec 0244 S5's own bound and no further.
        assert_eq!(wall.land(NOTCH, 81, 89, NATURAL, HARD, now), 89);
        assert_eq!(wall.land(NOTCH, 89, 97, NATURAL, HARD, now), 97);
        assert_eq!(
            wall.land(NOTCH, 97, 105, NATURAL, HARD, now),
            99,
            "0244's bound"
        );
        // Back inside, and the wall is standing again.
        assert_eq!(wall.land(NOTCH, 99, 91, NATURAL, HARD, now), 91);
        assert_eq!(wall.land(NOTCH, 84, 76, NATURAL, HARD, now), 76);
        assert_eq!(wall.land(NOTCH, 76, 84, NATURAL, HARD, now), 80, "re-armed");
    }

    /// Spec 0286 test plan 4 / S5: the forgiveness is a lazy comparison
    /// against the clock the caller passes, so a test ages it rather than
    /// sleeping through it — and nothing here needs a timer to fire.
    #[test]
    fn a_pause_forgives_the_pushes() {
        let start = Instant::now();
        let mut wall = EdgeResistance::default();
        // Two taps in. One more at this rate would have gone through.
        for n in 0..2 {
            wall.land(NOTCH, 80, 81, NATURAL, HARD, start + TAP * n);
        }
        // But the reader stopped instead.
        let later = start + TAP + EDGE_HOLD + Duration::from_millis(1);
        assert_eq!(
            wall.land(NOTCH, 80, 81, NATURAL, HARD, later),
            80,
            "the pause forgave the run, so this pan starts a new one"
        );
        assert_eq!(wall.land(NOTCH, 80, 81, NATURAL, HARD, later + TAP), 80);
        assert_eq!(
            wall.land(NOTCH, 80, 81, NATURAL, HARD, later + TAP * 2),
            81,
            "and the hold was measured from here, not from `start`"
        );
    }

    /// Spec 0286 test plan 5 / S3: a pan that moves the viewport at all
    /// is an ordinary pan, however short its own step was cut. Only a pan
    /// that got nothing is a push.
    #[test]
    fn a_step_that_moves_is_not_a_push() {
        let now = Instant::now();
        let mut wall = EdgeResistance::default();
        wall.land(NOTCH, 80, 88, NATURAL, HARD, now);
        wall.land(NOTCH, 80, 88, NATURAL, HARD, now + TAP);
        assert!(wall.pushing(), "two refusals so far");
        // Arriving at the bound from two rows short moves two rows.
        assert_eq!(wall.land(NOTCH, 78, 86, NATURAL, HARD, now + TAP * 2), 80);
        assert!(
            !wall.pushing(),
            "which is a move, so the pressure behind it is spent"
        );
        // So the pan that would have been the third is a first again.
        assert_eq!(
            wall.land(NOTCH, 80, 88, NATURAL, HARD, now + TAP * 2),
            80,
            "full toll again"
        );
    }

    /// Spec 0286 test plan 8 / S1: a document that fits has an empty
    /// natural range, so it does not scroll at all until the wall is
    /// pushed — and then it over-pans like any other.
    #[test]
    fn a_document_shorter_than_the_pane_holds_still_until_pushed() {
        let now = Instant::now();
        let natural = (0, 0);
        let hard = (-19, 2);
        let mut wall = EdgeResistance::default();
        for n in 0..2 {
            assert_eq!(wall.land(NOTCH, 0, 1, natural, hard, now + TAP * n), 0);
        }
        assert_eq!(wall.land(NOTCH, 0, 1, natural, hard, now + TAP * 2), 1);
    }

    /// `set_top` and `top` are inverses at every offset, in both row
    /// heights, on both sides of zero — the invariant every other method
    /// here assumes.
    #[test]
    fn set_top_and_top_round_trip() {
        for heights in [flat(), all_tall(), RowHeights::new(3..6)] {
            for top in -8isize..=16 {
                let mut scroll = PaneScroll::default();
                scroll.set_top(top, &heights);
                assert_eq!(scroll.top(&heights), top, "{heights:?}, top {top}");
                if top >= 0 {
                    assert!(scroll.skip >= 0, "only a negative top borrows: {scroll:?}");
                    assert!(
                        scroll.skip < heights.height(scroll.index) as isize,
                        "skip must stay under its own row: {scroll:?} in {heights:?}",
                    );
                } else {
                    assert_eq!(scroll.index, 0, "nothing above row 0 to name: {scroll:?}");
                }
            }
        }
    }

    /// Spec 0268 S4: the map is a bijection on terminal rows at every
    /// span position, the empty span included.
    #[test]
    fn offset_and_row_at_round_trip() {
        let spans = [
            0..0,
            0..1,
            0..12,
            3..4,
            3..7,
            7..8,
            11..12,
            11..40,
            39..40,
            40..41,
        ];
        for span in spans {
            let heights = RowHeights::new(span.clone());
            let mut expected = 0;
            for row in 0..24 {
                assert_eq!(heights.offset(row), expected, "{span:?} row {row}");
                for part in 0..heights.height(row) {
                    assert_eq!(
                        heights.row_at(expected + part),
                        (row, part),
                        "{span:?} row {row} part {part}",
                    );
                }
                expected += heights.height(row);
            }
        }
    }

    /// A viewport sitting exactly on a row draws no blank rows and
    /// exactly as many content rows as fit.
    #[test]
    fn window_on_a_whole_row() {
        let scroll = PaneScroll { index: 3, skip: 0 };
        assert_eq!(scroll.window(5, &flat(), 100), (0, 3..8));
        assert_eq!(scroll.window(6, &all_tall(), 100), (0, 3..6));
    }

    /// A positive skip cuts the top row in half, so one more content row
    /// is needed to fill the same pane.
    #[test]
    fn window_with_a_partial_row_at_the_top() {
        let scroll = PaneScroll { index: 3, skip: 1 };
        assert_eq!(scroll.window(6, &all_tall(), 100), (0, 3..7));
    }

    /// A negative skip is blank rows above the content, and that many
    /// fewer content rows drawn.
    #[test]
    fn window_with_blank_rows_above() {
        let scroll = PaneScroll { index: 0, skip: -3 };
        assert_eq!(scroll.window(5, &flat(), 100), (3, 0..2));
        assert_eq!(scroll.window(5, &all_tall(), 100), (3, 0..1));
    }

    /// Spec 0268 G3: only the rows in the span cost two terminal rows,
    /// so a pane whose span is over by row 5 keeps drawing one row per
    /// row after it.
    #[test]
    fn window_spends_two_rows_only_inside_the_span() {
        let scroll = PaneScroll { index: 2, skip: 0 };
        // Rows 2 and 3 are two rows each, then 4..8 are one: 4 + 4 = 8.
        assert_eq!(scroll.window(8, &RowHeights::new(2..4), 100), (0, 2..8));
        // Ahead of the span it is flat until row 6, which is cut in half.
        assert_eq!(scroll.window(5, &RowHeights::new(6..9), 100), (0, 2..7));
    }

    /// Content shorter than the pane, and a pane with no room at all,
    /// both yield an empty-or-short range rather than an out-of-bounds
    /// one — every caller slices with it.
    #[test]
    fn window_is_clamped_to_the_content() {
        let scroll = PaneScroll { index: 4, skip: 0 };
        assert_eq!(scroll.window(10, &flat(), 6), (0, 4..6));
        assert_eq!(scroll.window(10, &flat(), 2), (0, 2..2));
        assert_eq!(scroll.window(0, &flat(), 6), (0, 4..4));

        let scroll = PaneScroll { index: 0, skip: -9 };
        assert_eq!(scroll.window(5, &flat(), 6), (9, 0..0));
    }
}
