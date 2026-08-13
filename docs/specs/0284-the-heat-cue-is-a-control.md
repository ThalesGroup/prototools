<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0284 — the heat cue is a control

Status: implemented
Implemented in: 2026-08-13
App: protolens
Refs: docs/specs/0154-a-node-whose-bytes-fit-another-type-says-so.md
        (the cue itself, and the four shapes its suffix takes)
      docs/specs/0194-the-cursor-is-a-caret.md (S1 — the caret track,
        whose right-hand zone *is* the heat suffix)
      docs/specs/0199-the-caret-declares-what-it-meant.md (S10 — a
        click forfeits the anchor, which this does not change)
      docs/specs/0138-the-heat-glyph-has-a-reserved-column.md (N1 —
        column 0 is reserved gutter, never text)
      docs/specs/0242-a-selection-is-a-span-of-characters.md (S11 — the
        heat suffix is in no selection)
      docs/specs/0114-a-node-can-be-read-as-another-type.md (§1/§2 —
        `t` and the override selection pane)
      docs/protolens/input-bindings-review.md (the binding backlog N6
        defers to; this is not one of its items)

## Background

A heat cue says *some other type fits these bytes better than the one
this node has* — `[3/7]`, or `[2@7]` when candidates tie. It is the app
volunteering a question. The answer is the override selection pane, and
`t` opens it.

There is no mouse route from the question to the answer. Double-clicking
the cue selects the whole row instead, which is doubly wrong: the row's
text is not what was pointed at, and spec 0242 S11 already declares the
heat suffix to be "gutter furniture that is in no selection". The
gesture lands on a control and is spent on the text beside it.

Separately, column 0 — the reserved heat gutter (0138 N1) — has no
click behavior of its own. `set_caret_from_click` (`mouse.rs:547`) folds
it onto column 1 with a `saturating_sub(1)`, and `clamp_caret_column`
then raises the result to `caret_bounds().0`. Unpanned that lands on the
line's first non-blank character, which is the useful thing; with
`pan_offset > 0` the same click lands on the leftmost *visible*
character instead. The gutter does not scroll, so a click in it should
not mean two different things depending on how far the view has
scrolled.

## Goals

- **G1.** The cue answers, by mouse, the question it raises.
- **G2.** A click in column 0 means one thing at any pan.
- **G3.** No new key, no new modifier, no new pane.
- **G4.** Nothing costs a frame that did not already cost one.

## Non-goals

- **N1.** The margin glyph is not a double-click target. It is one
  column wide, and `handle_click`'s fold-marker comment already records
  what that costs: "a one-column target is too small to click
  repeatedly: a fast run of clicks drifts by a cell and one of its
  presses silently becomes a caret placement instead". The numeric
  suffix is five or more columns and has no such problem.
- **N2.** A single click on the suffix is unchanged — it puts the caret
  on the cue. Spec 0194 S1 deliberately made the suffix the caret
  track's right-hand zone, "since the suffix is a rendered fact about
  the node", so `$` already lands there. Snapping the caret back to the
  text's last character would make a click and `$` disagree at the one
  place 0194 went out of its way to make them agree.
- **N3.** Neither click arms a caret anchor. Spec 0199 S10 forfeits it
  at every click site, on a rule stated rather than derived — *a click
  expresses where, never why*. An anchored caret changes what the next
  `h`/`l` does; a click must not decide that. So S5 below moves the
  caret to the home *column* and leaves `CaretAnchor::Free`.
- **N4.** Not `Enter`'s smart proxy (`open_smart_override_or_manage`).
  The cue is specifically about the node's *type*, and `t` is the
  binding for that. The proxy's other half opens the management pane,
  which the cue says nothing about.
- **N5.** No context-menu row for this. The menu (review C9) replays
  keys at the caret and `t` is already among them; a second route to
  the same binding is not what the cue was missing.
- **N6.** `Ctrl-w` (review C7) is not in this spec. It is a different
  feature that happens to be outstanding at the same time.

## Specification

- **S1.** A row's heat suffix is measurable from outside `render`.
  `heat_chrome` already exists to be "called twice — once to draw, once
  to measure" (its own doc, `render.rs:1046`); the hit test is a third
  caller and needs no new formatting function. The `HeatDisplay` for an
  arbitrary line comes from `heat_cue_for(line_idx)`.

- **S2.** `heat_cue_at_point(col, row) -> Option<usize>` names the line
  whose drawn suffix covers the point. The suffix begins at pane column
  `1 + (row_content(row).chars().count() - pan_offset)` — one column for
  the reserved gutter, then the row's text as the pan left it — and runs
  for the suffix's own character count. The whole of it is in *pane
  columns*: **no `pan_offset` is added back**, unlike the fold marker's
  hit test, because `render` pushes the suffix span *after* `pan_spans`
  and so the suffix is never one of the characters the pan scrolls off.
  It rides the text's right edge instead.

- **S3.** Every drawn suffix is a target, including the pending `[?/7]`
  and `[?]`. A reader who double-clicks a cue that is still resolving
  wants the same pane, and the pane resolves its own candidates on open.
  A uniform rule also means the target does not appear and disappear
  under the pointer as a background sweep lands.

- **S4.** Double-clicking a suffix runs `toggle_override()` and selects
  nothing. The first click of the pair has already put the caret on that
  line through `handle_click`, and `toggle_override` acts on the cursor,
  so the gesture needs no separate positioning step. Its close arm is
  unreachable from here: while the override pane is open
  `main_interactive` is false and a main-pane click is refused with the
  focus-lock message, so from the cue `t` is only ever an open.

- **S5.** A click whose pane column is 0 puts the caret at the row's
  first non-blank character, at any pan — `set_caret_from_click` gains a
  case ahead of its two zones rather than letting the saturation decide.
  Setting the column to 0 is enough: the `clamp_caret_column` already at
  the end of that function raises it to `caret_bounds().0`, which is
  that character by definition.

- **S6.** A double-click pair must agree on its zone. `last_click`'s key
  becomes `(line, zone)` rather than the bare line, so a click on the
  text followed by a click on the cue is two single clicks and not a
  pair. `pending_double_click` records the zone the pair formed in, so
  the `Up` arm acts on what `Down` decided instead of hit-testing the
  release position — a release that moved is a `Drag`, and re-deriving
  the zone there would let one gesture start on a control and finish
  somewhere else.

- **S7.** The fold marker's rule is *not* extended to the cue. A marker
  clears `last_click` because it acts on every click and four fast
  clicks must be four toggles; the cue acts only on the second of a
  pair, so it pairs normally.

- **S8.** The help text's mouse section gains both: double-click on a
  `[…]` cue opens the override pane, and a click in the left margin puts
  the caret at the start of the line's text.

## Alternatives considered

### Single-click on the cue opens the pane

Rejected: a single click is how the caret is placed, and 0194 S1 put the
cue inside the caret track on purpose. A control that fires on the same
gesture that places the caret cannot be pointed at without being
triggered.

### Single-click on the cue snaps the caret to the text's last character

Proposed, then dropped once `caret_bounds()` was read: it returns
`len - 1 + caret_suffix_len`, so the caret is *already* allowed to rest
on the cue and `$` already goes there. The snap would have created the
one disagreement between a click and `$`.

### A click in column 0 declares `CaretAnchor::Home`

The stronger version — the caret would then stick to home across
vertical moves, as `0`/`^` makes it. Rejected under N3: 0199 S10 makes
every click forfeit the anchor, and that rule was stated deliberately
rather than derived. The pan-independence is the actual defect and S5
fixes it without touching the anchor.

### Re-hit-testing the zone in the `Up` handler

Cheaper than S6's zoned key, and wrong for the same reason the existing
double-click does not consult the pointer's position at release: the
gesture was decided at the second `Down`. Two `Down`s in different zones
would also still pair, so the zoned key is needed regardless.

## Test plan

1. `a_double_click_on_a_numeric_cue_opens_the_override_pane` — S4, for
   both `[c/b]` and `[n@s]`, and it selects nothing.
2. `a_double_click_on_a_cue_leaves_the_row_unselected` — the row
   selection a double-click on text would have made does not happen.
3. `a_double_click_on_the_text_still_selects_the_row` — S6 does not
   disturb the existing gesture.
4. `a_click_on_the_text_then_on_the_cue_is_not_a_pair` — S6.
5. `a_pending_cue_is_a_target_too` — S3, over `[?/7]`.
6. `a_row_with_no_cue_has_no_target` — `heat_cue_at_point` is `None`
   where nothing is drawn, and the double-click selects the row.
7. `the_cue_target_follows_the_drawn_suffix_at_any_pan` — S2.
8. `a_click_in_the_left_margin_lands_on_the_first_non_blank` — S5, at
   pan 0 and at a pan that would otherwise move it.
9. `a_click_in_the_left_margin_forfeits_the_anchor` — N3.
10. `the_help_text_documents_both_gestures` — S8.

## Measured outcome

Implemented 2026-08-13, in 10 new `tui::tests::mouse` tests and one in
`tui::tests::help_text`; 1004 protolens tests pass.

S5's fix was proved to bite by restoring the old arithmetic:
`a_click_in_the_left_margin_lands_on_the_first_non_blank` then reports
column 3 where the row's first non-blank is 2, at `pan_offset` 5 — the
defect the spec describes, and not one the test could have passed over.
The anchor half (test 9) passes on the old code too; it guards N3
rather than fixing anything.

Deviations from the text above:

- **S2's original last clause said the suffix "does not scroll", and it
  does.** `render` panning shortens the text the suffix is appended to,
  so the suffix slides left by `pan_offset` along with the text's right
  edge. What is true — and what the hit test needs — is that the suffix
  is never one of the characters the pan removes, so the arithmetic
  stays in pane columns and adds no `pan_offset` back. The sentence and
  test-plan item 7's name were both corrected.

- **Only the document row of a wire pair is a target.** S2 says
  *drawn*, and the wire row draws no suffix. Spec 0225 S8's "the row is
  taller, not two targets" is about a click naming a *line*; a control
  is where it is painted, so `heat_cue_at_point` goes through
  `main_pane_line_part` and refuses part 1.

- **The `Cue` arm of `Up` clears the selection before opening.** S4's
  "selects nothing" has to include the anchor the `Down` armed —
  otherwise the gesture leaves a selection half-started, which a plain
  click does not.

- `pending_double_click` became `Option<ClickZone>` rather than a bool
  beside a second field: there is no such thing as a pair without a
  zone, and the ladder in the `Up` arm is then exhaustive.
