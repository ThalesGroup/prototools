<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0280 — a heat cue says what its score is made of

Status: implemented
Implemented in: 2026-08-13
App: protolens
Refs: docs/specs/0138-….md (the cue and its `[current/best]` suffix),
        docs/specs/0154-….md (`HeatDisplay`'s four states, which this
        reuses verbatim for the popup's pending states),
        docs/specs/0223-….md (S2 dropped `Moved` at the reader; S4's
        `event_changed_nothing` is what keeps a hover from costing a
        frame), docs/specs/0245-….md (`event_changed_nothing`),
        docs/specs/0263-….md (`ui_deadline` is the one-shot wake this
        borrows), docs/protolens/input-bindings-review.md §C9 (the
        anchored-box pattern and the derived anchor)

## Background

A heat cue prints two numbers and no reason. `[3/7]` says the node's
current type scores 3 where something else scores 7; a reader who wants
to know *why* has nowhere to go. The override pane answers a different
question — which other types fit — and answers it with a ranked list of
names and scores, which is again numbers without reasons.

The reasons are already computed. `EntryScore`
(`prototext-graph/src/score/walk.rs:48`) carries the whole
decomposition:

```rust
matches, unknowns, out_of_range, non_canonical, mismatches, vetoed
```

and `EntryScore::score()` is nothing but their weighted sum —
`matches − 10·unknowns − 15·out_of_range − 20·non_canonical −
30·mismatches`, the coefficients ordered by how damning each signal is.

protolens throws all of it away. `override_pane::inferred_score` calls
`score_one`, reads `.score()`, and drops the struct; `HeatState`
(`tui/heat_cue.rs:102`) is three scalars by deliberate design, because
there is one per arena slot and a byte here is 4.7 MB on googleapis.
So the counts exist, are cheap, and never reach the screen.

## Goals

- **G1.** A cue can be asked what its number is made of, and answers
  with the per-category counts the scorer already computed.
- **G2.** Two ways in — the pointer and a key — because motion
  reporting is not universal and `Shift` bypasses it everywhere.
- **G3.** The answer is on screen the instant the box appears. The
  query is issued when the pointer *arrives*, not when the delay
  expires; the delay gates the display only.
- **G4.** An idle protolens still performs zero timed wakeups
  (spec 0263's measured guarantee, unchanged).
- **G5.** A pointer merely crossing the pane costs no frame.

## Non-goals

- **N1. Not a candidate list.** The override pane ranks types against
  the range; this explains one type's score against it. They answer
  different questions and must not be merged — a popup that also
  ranked would be the override pane in a smaller box.
- **N2. One column, not two.** The breakdown shown is the node's
  *current* type only, never the best candidate beside it. A red cue
  already says "something fits better"; what the reader cannot get
  anywhere is "and this one fits badly *how*". A side-by-side
  comparison is a different feature and would double the query.
- **N3. Hover is only over the type name.** The `type` token of a
  row's `#@` annotation, nothing else. You hover the name you are
  asking about, which is a rule the reader can hold; "hover anything
  for information about it" is an open-ended surface with no natural
  edge.
- **N4. No new worker request kind, no cache-format change.** The
  breakdown is computed synchronously on the main thread. It is one
  type against one range — `1/58777` of the sweep the same node's cue
  already costs — and `heat_cue_resolve`'s no-worker arm already calls
  `inferred_score` synchronously on that thread today.
- **N5. No tooltip framework.** One box, one caller, no generic
  hover-target registry. There is exactly one hoverable thing.

## Specification

### The content

- **S1.** `override_pane::inferred_breakdown(range_bytes, fqdn, graph)
  -> Option<ScoreBreakdown>`, a sibling of `inferred_score` at the same
  site so the two cannot come to disagree about what `score_one`
  returns. `None` has the same meaning it has there: `fqdn` is not a
  known root type.
- **S2.** `ScoreBreakdown` is owned and `Copy` — six `u64`s and a
  `bool` — not `EntryScore<'g>`, whose `fqdn` borrows the archived
  graph. It is stored across frames, so it cannot hold that borrow.
- **S3.** A vetoed entry reports **only** that it is vetoed. Its
  counters are whatever had accumulated when the veto fired part-way
  through a field, which is not a fact about the payload. This is the
  same convention `EntryScore::termination` already documents for
  itself.
- **S4.** `App::score_breakdown(idx) -> BreakdownState`, keyed on
  `(heat_scored_range(idx).start, current_type_key(idx))` — the same
  key `read_heat_state` uses, so the popup and the cue can never
  describe different things. `BreakdownState` distinguishes *no
  scoring graph* from *not a known root type* from an answer, because
  the first two must not render as a box full of zeros.
- **S5.** The memo is **one entry**, not a cache: only one popup can
  exist at a time, so a keyed map would be a cache with a
  single reader and an eviction policy nobody needs. Re-entering the
  cue the pointer just left is free; crossing N cue rows costs N
  scores, each once.

### The input

- **S6.** `Moved` reaches `handle_mouse` again. `carries_intent`
  (`tui/event.rs:83`) stops being a *filter* and becomes the
  *counting* predicate: the reader forwards every event and calls
  `note_sent` only for those that carry intent.
- **S7.** `InputPending::note_received` must apply the same predicate.
  It currently decrements on every `AppEvent::Term(_)`; an uncounted
  `Moved` reaching it would drive the unsigned counter below zero and
  wrap it to `usize::MAX`, pinning the display monochrome forever —
  the exact failure its own doc comment warns about for
  `HeatWorkerProgress`. **Send and receive must share one predicate.**
- **S8.** `handle_mouse`'s `Moved` arm stays where it is, at the very
  top, ahead of the splash dismissal and the message clear. Those
  three side effects are the reason spec 0223 put the guard there and
  a hover must still not trigger any of them.
- **S9.** A `Moved` sets `event_changed_nothing = true` (spec 0245 S2)
  unless it dismisses a *visible* popup. Arming the dwell is not a
  visible change and the frame it eventually needs is bought by S11's
  deadline; so a pointer crossing the pane draws nothing (G5).
- **S10.** Hover target: the columns the `type` token of the row's
  `#@` annotation occupies. That token is where the type being scored
  is written down, so the reader points at the name and is told how
  well the bytes underneath it fit.

  Its span is read off the annotation *format*, not off the
  highlighter: a declaration is `[label ]type[ [packed=true]] = NUMBER`
  and `push_field_decl` is the only thing that writes `" = "` into an
  annotation, so the declaration is the one `;`-separated token
  carrying it. `node_status::row_status` already finds the same token
  the same way. For an enum the type carries its value (`Color(5)`) and
  the name alone is the target.

  A row whose annotations are hidden (`a`) has no target, because
  `row_content` is what is hit-tested and the annotation is not in it.
  A row two terminal rows thick in wire mode is one target on both of
  them, exactly as spec 0225 S8 already has a click anywhere in the
  pair select the line.

### The dwell

- **S11.** `hover_deadline: Option<Instant>` joins `message_deadline`
  and `splash_deadline` in `run_loop`'s `ui_deadline` (`terminal.rs:730`).
  Nothing else is needed: `may_sleep_indefinitely` already takes
  `ui_deadline` as its first term, and `deadline_forces` already draws
  the frame. Armed when the pointer lands on a target, cleared when it
  leaves or fires — so an untouched mouse arms nothing and G4 holds.
- **S12.** `HOVER_DWELL: Duration = 400 ms`, checked by
  `track_hover_dwell()` called from `render()` beside
  `track_message_timeout` and `track_splash_timeout`, which is the
  established shape for "a deadline whose expiry is noticed by the
  next frame".
- **S13.** The query runs on *arrival*, not on expiry (G3). By the
  time the box appears its content is already in hand, so it never
  opens empty and never has a pending state of its own.

### The surface

- **S14.** `render_score_popup`, drawn last, immediately after
  `render_menu`. The anchor/flip/clamp arithmetic in `render_menu`
  (`render.rs:2198-2220`) is extracted to `anchored_rect(anchor,
  width, height, area)` and used by both — there is exactly one right
  answer to "put a box at a point without leaving the screen".
- **S15.** The box shows the type key it scored, then the score itself
  — labeled `score`, since that is the word the rest of the interface
  uses — then one line per non-zero category, in `EntryScore::score()`'s
  own coefficient order (`matches`, `unknowns`, `out_of_range`,
  `non_canonical`, `mismatches`), each with its count. Zero categories
  are omitted: a clean node's box is one line saying so, not five zeros.

  The score comes *first* rather than as a `total` under the sum. It is
  the number the cue is colored from and the number the reader opened
  the box for; the terms are the detail, read only when the score
  surprises. The terms keep their common weight column, so they still
  visibly sum to the number above them.
- **S16.** It is dismissed by anything at all except the pointer
  holding still: any key, any button, any wheel, and a `Moved` off the
  target. There is no dismiss binding to learn, and nothing can be
  left behind by it.
- **S17.** No popup while a menu is open, and none while the override
  pane locks focus. The menu is the innermost modal (C9) and stays so.

### The key

- **S18.** `s` — free, and the mnemonic is "score". It opens the same
  box for the caret's node, anchored at the caret via C9's
  `menu_anchor`, and is refused with the usual message when the
  override pane holds focus.
- **S19.** Because it is a binding, it is also a context-menu row
  (C9: a row *is* a `KeyEvent`), offered on any node whose cue is
  showing.
- **S20.** Help text gains it in the section that documents the cue.

## Alternatives considered

**A dwell timer of its own, outside `ui_deadline`.** Rejected on
inspection: `ui_deadline` is already the one-shot "wake me at T", is
already the first term of `may_sleep_indefinitely`, and already forces
the frame. A second timer would be a second thing to keep consistent
with 0263.

**Keeping `carries_intent` as a drop and reconstructing hover from
`Drag`.** A drag is a selection gesture; hovering with a button held
is not hovering. It also gives nothing while the user is merely
looking.

**Issuing the query at dwell expiry.** Correct and simpler to reason
about, and rejected on the user's point: the delay exists to avoid
showing a box the reader did not ask for, not to avoid computing an
answer. Deferring the work moves the latency to precisely the moment
the reader is looking.

**Storing the breakdown in `HeatState`.** One per arena slot at 4.7 M
slots is what forced 0220's three-scalar encoding in the first place;
six more counters would be ~28 bytes × 4.7 M. The popup needs one
node's answer at a time.

**A `Vec<HoverTarget>` registry rebuilt each frame.** The generic
shape, and unnecessary: there is one hoverable thing, its geometry is
two spans of a row, and `heat_chrome` already reports both widths.

## Test plan

1. `hover_over_a_cue_glyph_arms_the_dwell` — a `Moved` onto column 0
   of a cue row sets `hover_deadline`; one onto an ordinary column
   does not.
2. `the_dwell_opens_the_popup_and_leaving_closes_it` — planting an
   expired `hover_deadline` and rendering opens it; a `Moved` off the
   target closes it.
3. `a_bare_move_costs_no_frame` — a `Moved` that changes no hover
   target leaves `event_changed_nothing` true, and one that dismisses
   a visible popup does not.
4. `motion_is_forwarded_but_not_counted` — `carries_intent` still
   answers false for `Moved`, and `note_sent`/`note_received` paired
   over a `Moved` leave the counter at zero rather than wrapping
   (S7's underflow, asserted directly).
5. `the_breakdown_is_the_scores_own_terms` — on a fixture with a known
   anomaly, the counts reported equal the ones `score_one` returns,
   and their weighted sum equals the score the cue shows.
6. `a_vetoed_type_reports_only_that` — no counters in the box.
7. `s_opens_the_same_box_at_the_caret` — and the row appears in the
   context menu for a node showing a cue.
8. `nothing_hovers_while_a_menu_is_open`.

## Measured outcome

Implemented 2026-08-13. `protolens/src/tui/score_popup.rs` is the whole
surface — 5 fields on `App`, one new module, and edits to five existing
files that are each one or two lines.

Three notes where the implementation is narrower or wider than the text
above:

- **S4's type is called `Breakdown`, not `BreakdownState`.** It is
  `ScorePopup`'s own field, and `breakdown: BreakdownState` reads as
  two words for one thing. Its three variants are as specified.
- **S14's extraction landed as a free function**, `render::anchored_rect`,
  rather than a method — it reads nothing off `self`, and a method would
  have implied it did.
- **S20 had no section to add to.** The cue's own `i` toggle was never
  documented either — it survived `help_text.rs`'s scan because the
  override pane's unrelated `i` (candidate sort) is in the help and that
  test is per-character, not per-binding. So the help gained a "Heat
  cues" section naming both, plus the hover.

The three quantitative claims hold as reasoned:

- **G3/S13.** `handle_hover` calls `score_breakdown` on arrival, so
  `track_hover_dwell` only reads a value already in `breakdown_memo`.
  The box has never been observed to open empty because it structurally
  cannot.
- **G4.** `hover_deadline` is `None` unless the pointer is on a cue, so
  `may_sleep_indefinitely`'s first term is unchanged for an untouched
  mouse and spec 0263's measured 0 wakeups / 10 s stands unaltered.
- **G5.** `a_bare_move_costs_no_frame` asserts both directions: a move
  over the document and a move that arms the dwell both leave
  `event_changed_nothing` true; only the move that erases a visible box
  clears it.

S7's underflow is asserted directly rather than inferred:
`motion_is_forwarded_but_not_counted` pairs `note_received` over a
`Moved` and requires the counter to stay at zero. Without the matching
predicate that line wraps to `usize::MAX` and the test fails loudly,
which is the point.

**Amended the same day: the hover target moved.** It was first built
over the cue itself — the glyph cell and the `[…]` suffix beside it —
which meant only a line that already *has* a cue could be interrogated,
and the box could say nothing about a line whose type is uncontested.
S10 and N3 now describe the target actually shipped: the `type` token of
the row's `#@` annotation, cue or no cue. Consequences worth recording:

- **The span is read off the annotation format, not off the
  highlighter.** `annotation_type_span` takes the one `;`-separated
  token containing `" = "` — the same rule `node_status::row_status`
  already relies on, and sound because `push_field_decl` is the only
  writer of `" = "` — then skips a `repeated `/`required ` label and
  stops at `(` or a space, so an enum's `Color(5)` offers `Color` and
  not its value. The tree-sitter hints were rejected: they are indexed
  into `display_row_text` (no fold margin), are cleared while
  `input_pending`, and would cost a parse per motion event.
- **Two cases needed no code at all.** With annotations hidden (`a`),
  `row_text_of` strips the annotation before `row_content` ever sees
  it, so the target simply is not there. And on a wire row,
  `main_pane_line_idx` already maps both terminal rows of the pair to
  one line — spec 0225 S8's "the row is taller, not two targets".
- `render::heat_chrome` went back to private: the hit test no longer
  needs to know how a cue is drawn.
