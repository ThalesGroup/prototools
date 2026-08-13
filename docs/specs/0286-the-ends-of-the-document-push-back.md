<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0286 — the ends of the document push back

Status: implemented
Implemented in: 2026-08-13
App: protolens
Refs: docs/specs/0244-a-pan-may-run-past-either-end-of-the-content.md
        (S5/S6 — `pan_top_bounds` and the over-pan this puts a wall in
        front of; N2, which this must not disturb)
      docs/specs/0193-the-fold-marker-lives-in-a-gutter.md (S4, the
        viewport label that becomes the cue)
      docs/specs/0245-a-burst-of-input-costs-one-frame-not-one-frame-each.md
        (S2, `event_changed_nothing` — a refused pan is the case it was
        written for)
      docs/specs/0263-the-machine-sleeps-when-nobody-is-waiting.md (why
        the forgiveness delay may not be a timer)

## Background

Spec 0244 S6 lets a pane's top edge run past either end of the content,
until one terminal row is left on screen. That is a real capability —
it is the only way to park a line anywhere but where the clamp puts it,
and `w` depends on the bounds being in terminal rows.

It is also reached by accident. `WHEEL_PAN_STEP` is one row, so a wheel
rolled toward the end of a document does not stop at the last line: it
carries straight on into blank space, and the reader who wanted to
*read the end* has to roll back. The same is true of `Ctrl-Down` at
`PAN_STEP`'s eight rows, and of a short document, where the natural
range is empty and every pan is an over-pan.

The end of the content is a place worth stopping at. Nothing marks it.

## Goals

- **G1.** A pan stops at the first or last line of content.
- **G2.** It stops *softly*: pushing on repeatedly gets through, in the
  same gesture, without reaching for another key.
- **G3.** The stop is visible. Silent resistance is indistinguishable
  from a broken wheel.
- **G4.** Pushing at the wall and then giving up costs nothing later.
- **G5.** No timed wakeup, and no parse or allocation per wheel notch.

## Non-goals

- **N1.** Over-pan is not removed. 0244 S6 bought it deliberately and
  this only puts a price on it.
- **N2.** No rubber-banding — no reduced gain past the edge, no spring
  back. See Alternatives.
- **N3.** The override and manage panes are not done here. They
  over-pan by the same bounds and want the same wall, but the main pane
  is where the complaint is; the mechanism is a type with one field per
  pane so that extending it is a field and a call site each, not a
  redesign.
- **N4.** Horizontal panning is untouched. `pan_horizontal` is bounded
  by `max_pan_offset` with no region beyond it, so there is no wall to
  push through.
- **N5.** `clamp_scroll_to_cursor` and `clamp_scroll_to_visible` are
  untouched. They are minimal nudges, not gestures, and 0244 N2 already
  says an over-pan survives them. Only an explicit pan meets the wall.
- **N6.** The two constants are not configurable. They are feel, and
  feel is decided once.

## Specification

- **S1.** Two pairs of bounds. `pan_top_bounds` keeps returning the
  **hard** pair (one content row on screen, 0244 S5). A new
  `natural_top_bounds(content_rows, pane_height)` returns
  `(0, content_rows.saturating_sub(pane_height))` — the range in which
  every drawn row is content. Like its sibling it is clamped against
  `0`, so `min <= max` always holds and `.clamp()` can never panic. A
  document shorter than the pane collapses it to `0..=0`, which is
  correct: such a document has nothing to scroll and every pan of it is
  an over-pan.

- **S2.** A pan is offered the natural bounds first. Landing on the
  natural bound having moved at all is an ordinary move — the wall is
  only met by a gesture that would have crossed it and got nothing.

- **S3.** The toll is **repeated intent, held**. Stated as the reader
  experiences it:

  > Repeating the *same* pan within `EDGE_HOLD` is one sustained
  > intent. An intent sustained for longer than `EDGE_HOLD` goes past
  > the wall.

  In terms of the mechanism: a **push** is a pan event that moved zero
  rows because the natural bound refused it. Consecutive pushes of the
  same `(step, up)` gesture, no two more than `EDGE_HOLD` apart, are one
  **run**. A push lands at the hard bound instead when its run has
  reached `EDGE_PUSHES` pushes *and* has lasted `EDGE_HOLD`.

  `EDGE_HOLD` is the wall's **one** time constant, serving both roles.
  They are one statement read from either side: pushing for this long is
  only pushing if the pushes are no further apart than this. See S5 for
  what two numbers cost.

  **Why the run is keyed on the gesture.** A wheel notch after a run of
  `Alt`-arrows is a change of mind, not the fourth of anything, so it
  starts its own run. Input that is not a pan at all does not need this
  test: S6 has already ended the run.

  **Why the duration, and not a pan count alone.** Rows fail on step
  size: `PAN_STEP` is 8 and `WHEEL_PAN_STEP` is 1, so a row-counted
  wall would fall to one `Alt-Down` while costing the wheel eight
  notches. A pan count fails on *rate*, and fails between two presses
  of the same key: held, a key arrives at the terminal's autorepeat
  rate; tapped, at the reader's. A count tuned for tapping is
  instantaneous under autorepeat; a count tuned for autorepeat cannot
  be reached by tapping at all. A moment of pushing is a moment of
  pushing either way. This was got wrong once — see S5.

  **Why `EDGE_PUSHES`, and not the duration alone.** Two pans that
  merely *span* the delay are not a delay's worth of pushing. A floor of
  three is what stops a stray notch at the bottom of a document and
  another one a moment later from adding up to an over-pan.

  No timer is involved. The clock is only read on a pan event, so the
  wall yields to the first push *after* the delay rather than at it —
  the same thing for a reader who is still pushing, and nothing at all
  for one who stopped.

- **S4.** Being outside the natural range **is** the latch. Once
  through, the top edge is beyond a natural bound, so subsequent pans
  are clamped to the hard bounds alone and move freely; the wall
  re-arms by itself when the edge comes back inside. No `yielded`
  flag — a second representation of a fact the position already
  carries is a second thing to get wrong.

- **S5.** The forgiveness is **lazy**, and is measured from the **last**
  push. The run is forgotten when more than `EDGE_HOLD` has passed
  since that push, evaluated at the next pan event rather than on a
  timer. Every push re-arms it, so the delay bounds the gap *between*
  two pans and never the length of the run — a gesture may be held as
  long as the reader likes. Nothing on screen changes when a run is
  forgotten, so no frame is owed and no wakeup is needed, which is what
  keeps spec 0263's zero-wakeup idle intact. The clock is a parameter of
  the one entry point, so tests age it without sleeping.

  The forgiveness and the hold are the **same** constant, and an earlier
  draft had them as two — with the forgiveness the *shorter* of the two,
  which is what a separate number makes it easy to do. Shorter, the gap
  silently sets a *rate* floor above the hold's own: a reader tapping
  more slowly than the gap has every push forgiven before the next
  arrives, so no amount of deliberate tapping ever gets through while a
  held key sails past. That is precisely the reported symptom. Equal,
  the floor is the hold itself — the only rate the wall ever meant to
  ask for — and there is no second number to contradict the first.
  `EDGE_PUSHES` (S3), not the gap, is what stops two lazy pans adding
  up.

  The visible consequence, accepted: the accent of S6 lingers until the
  next pan event, one gesture past the state it reports. The cue is a
  three-character label nobody is watching while idle, and the
  alternative is a timer, which G5 forbids.

- **S6.** While pushes are pending the viewport label (`Top`, `Bot`,
  `All`, or the percentage) is drawn in an accent color. The
  statusline is `REVERSED`, so the span's `.fg()` is what paints its
  visible background — the label becomes a colored block in the bar.
  The color is a named ANSI color, theme-independent as
  `theme::focus_style` already is, so it survives a 16-color terminal
  and needs no `supports_rgb()` dispatch.

  It is applied by splitting the composed statusline on the label as a
  **suffix** — the label is the last thing in the ruler, which is the
  last thing in the right half, in every branch and after every
  truncation `statusline_text` can apply. If the suffix is not there
  the label was truncated away and there is nothing to color.

  The cue reports a gesture *in progress*, so **any input event that is
  not a pan against the wall ends the gesture**: the color goes out and
  the pressure behind it goes with it. This is asked of the wall — "was
  `land` called while dispatching this event?" — rather than of the key
  that arrived, so that no second list of "the panning keys" has to be
  kept in step with the binding table. Only key and mouse events count;
  a heat result or a resize is not the reader doing something else.

- **S7.** A push that turns the accent on owes a frame; one that only
  adds to the run behind it does not. `event_changed_nothing`
  (0245 S2) therefore reads "the top edge did not move *and* the cue
  did not change", which keeps a held wheel at the bottom of a document
  as cheap as it is today after the first notch. S6's release is
  evaluated after the handler has already answered, so putting a lit cue
  out takes that answer back.

- **S8.** The mechanism is a type, `EdgeResistance`, beside `PaneScroll`
  in `tui/pane_scroll.rs`, with two methods. `land` takes the gesture,
  where the edge is, where that gesture wants it, both bounds and the
  clock, and returns where it lands; `settle` is called once per
  dispatched input event and enforces S6. `App` holds one for the main
  pane. N3's panes each need one more field and one more `land` call —
  `settle` is already generic over all of them.

## Alternatives considered

### Rubber-banding, iOS style

Reduced gain past the edge and a spring back when the gesture ends.
There is no gesture end: a terminal wheel reports notches and never a
release, so there is nothing to spring back *from* — and springing back
would undo 0244 S6's over-pan every time it was used deliberately.

### Count refused rows instead of refused pushes

Rejected by S3's arithmetic: with `PAN_STEP` at 8, one `Alt-Down`
would clear any wall a wheel user has to work for.

### Count refused pans, and nothing else

The first implementation, and wrong for the reason S3 gives: a pan is
not a fixed quantity of intent, because its arrival rate is the
terminal's to decide. Three pans is a flick of the wheel and a fifth of
a second of a held key. Kept as a *floor* under the duration, which is
a different job.

### Stop dead at the natural bound, and put over-pan on another key

No timers, no counter, no cue. Rejected because it makes over-pan
undiscoverable — nobody reads a keymap to find a scroll position — and
because it breaks the gesture in two, which is precisely what G2 says
the reader should not have to do.

### Decay on a timer

A wakeup at the forgiveness deadline, as `hover_deadline` does. The
hover deadline earns its wakeup by having a frame to draw at the end of
it; this one has nothing to draw, so it would be a wakeup spent to make
a stale label honest. Spec 0263 got that count to zero.

### A `yielded` flag alongside the push count

The first draft. Deleted once it was clear the top edge's own position
answers the same question (S4).

## Test plan

1. `a_pan_stops_at_the_last_line_rather_than_past_it` — S2, both ends.
2. `the_wall_yields_to_a_sustained_push` — S3: the same `EDGE_HOLD` gets
   through at a held key's rate and at a tapped one's, with pan counts
   that differ by an order of magnitude. That divergence is the whole of
   S3's argument, so it is asserted too.
3. `an_over_panned_pane_moves_freely_and_re_arms_on_return` — S4.
4. `a_pause_forgives_the_pushes` — S5, by aging the injected clock.
5. `each_push_re_arms_the_forgiveness` — S5: a run held far longer than
   `EDGE_HOLD`, at the widest spacing a run survives, is still one run.
6. `two_pans_far_apart_do_not_add_up_to_a_hold` — S3's `EDGE_PUSHES`
   floor.
7. `a_different_gesture_starts_its_own_run` — S3: a wheel notch does not
   inherit an arrow's pressure.
8. `a_step_that_moves_is_not_a_push` — S3: a gesture landing *on* the
   bound from two rows short resets rather than counts.
9. `the_viewport_label_is_accented_while_the_wall_is_pushed` — S6, read
   off the rendered status row. Asserts a named color, never an RGB
   triple: the CI sandbox has no `COLORTERM`.
10. `any_other_input_ends_the_gesture` — S6's release, driven through
    `dispatch_event` so that a pan and a non-pan go through the same
    door.
11. `the_push_that_lights_the_cue_owes_a_frame` — S7.
12. `a_document_shorter_than_the_pane_holds_still_until_pushed` — S1's
    collapsed range.

## Measured outcome

Implemented 2026-08-13. Two constants: `EDGE_HOLD` 400 ms, `EDGE_PUSHES`
3.

Three of spec 0244's own tests had to be taught to push through the wall
before asserting the over-pan bounds, which is the change stated back:
those bounds are still reachable, and are no longer arrived at.

The first cut counted pans alone (`EDGE_PUSHES` with no clock) and was
reported unusable within the hour: breakable by a flick of the wheel,
unbreakable by a repeated key. Both halves of that report are S3's
argument, from opposite ends. The second cut added the clock but kept a
separate forgiveness delay, and set it *shorter* than the hold, which
reintroduced the same asymmetry as a hidden rate floor (S5). The third
made the two one number, which is why they can no longer be set that way.

The repeated key in that report turned out to be a caret key, not a pan:
only `Alt-Up`/`Alt-Down` pan the main pane, and N5's caret keys never
over-panned in the first place. The rate floor above was real
nonetheless, and found by reading rather than by the report.

`EdgeResistance` is five fields and no allocation; the per-pan cost is a
comparison against a bound and, at a wall, one `Instant::now()`.
