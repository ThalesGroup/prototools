<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0245 — a burst of input costs one frame, not one frame each

Status: implemented
Implemented in: 2026-08-05
App: protolens
Refs: docs/specs/0223-highlighting-yields-to-pending-input.md (the
        monochrome-under-input frame this spec stops firing spuriously),
        docs/specs/0244-a-pan-may-run-past-either-end-of-the-content.md
        (the pan bounds a stalled pan runs into)

## Background

Wheel the mouse up while already at the top of the document. Nothing
scrolls — the pan is clamped at its bound — but the screen flickers
between colored and monochrome for as long as the wheeling lasts.

Three independent facts combine to produce it.

1. `run_loop` draws one frame per event: `event_forces` is
   `received.is_some()`, whatever the event did or did not change.
2. `App::input_pending` is re-sampled at every draw, and spec 0223 makes
   a frame monochrome whenever it is true. During a burst the counter
   oscillates between 0, 1 and 2 as the reader thread and the loop
   interleave, so consecutive frames alternate colored and gray.
3. The existing `STYLES_SETTLE_INTERVAL` debounce does not help. It
   governs only frames drawn *because* the styles are stale, so it never
   applies to a frame drawn because an event arrived.

The flicker is therefore not a symptom of the machine being behind; it
is a burst of no-op frames each deciding independently whether it has
time to colorize.

## Goals

- **G1.** A burst of queued input costs one dispatch pass and one frame,
  not one of each per event.
- **G2.** An input that changes nothing costs no frame.
- **G3.** A frame whose visible window is unchanged reuses the styles it
  already computed, instead of recomputing them or dropping them.
- **G4.** Monochrome remains reachable — it is the escape hatch for a
  machine that really is behind the input — but it stops firing for
  bursts the loop absorbs comfortably.

## Non-goals

- **N1.** No change to the order in which events are dispatched, or to
  what any one of them does. Coalescing removes frames, never events.
- **N2.** No threshold on the *number* of queued events (the obvious
  "monochrome only above X events"). See Alternatives.
- **N3.** No change to spec 0223's monochrome rendering itself, nor to
  `InputPending`'s accounting.

## Specification

### S1 — drain the queue before drawing

`run_loop` dispatches every event it can take without waiting, then
draws once. The receive step is unchanged: it still blocks, still
interleaves the search sweep and the prefetch while idle. Only after an
event has been received does the loop try to take more.

The three rules that bound the drain:

- **S1a — only what is already queued.** The drain uses `try_recv` and
  stops at `Empty`. It never waits. A drain that waited could chase a
  producer that outpaces it and never draw at all; one that only takes
  what was already there is bounded by the queue's length at the moment
  the pass began.

- **S1b — a time budget, not a count.** The drain stops when the pass
  has spent `DRAIN_BUDGET` since it started. The budget is checked
  *after* each dispatch, so a single expensive event is always allowed
  to finish and the overshoot is bounded by one event's cost.

  A count is the wrong unit because per-event dispatch cost varies by
  orders of magnitude: a clamped wheel pan is a few microseconds, an
  override re-render is milliseconds. Any count that coalesces a wheel
  burst usefully would stall the frame for tens of milliseconds on a
  queue of expensive events. What the user perceives is elapsed time
  without a frame, so that is what the bound is stated in.

  `DRAIN_BUDGET = 8ms` — under a saturated queue the loop still draws
  at better than 60 Hz, and it is two to three orders of magnitude above
  the cost of one wheel dispatch, so a real wheel burst always coalesces
  completely and never hits the budget at all.

- **S1c — stop at a control transfer.** `app.should_quit`,
  `app.should_suspend` and `app.pending_editor_open` each either leave
  the loop or hand the terminal to another program. The drain stops
  before dispatching anything further and lets `run_loop`'s existing
  handling run, with the rest of the queue left in the channel. This is
  a correctness rule, not a tuning knob: dispatching a keystroke after
  the user asked to suspend would deliver it to a screen that is no
  longer ours.

  A pane or mode change is deliberately *not* a stop condition. Dispatch
  order is unchanged, so the resulting state is identical; only frames
  the user never saw disappear. The user generated every event in the
  burst against the pre-burst frame anyway.

`HeatWorkerProgress` is drained freely — it is O(1) and forces no frame,
exactly as today.

`input_pending.note_received` is called for every event the pass takes,
so after a fully drained burst the counter is 0 and the coalesced frame
is colored. Monochrome now means the machine is genuinely behind the
input, which is what spec 0223 wanted it to mean.

### S2 — a stalled pan forces no frame

`App` gains `event_changed_nothing: bool`. `run_loop` clears it before
*every* dispatch, not once per pass, and an event forces a frame only
if it left the flag false — so one no-op event inside a burst does not
excuse the pass from drawing for the others.

The flag is set by the pan functions, and only by them: a pan whose
clamped destination equals its origin sets it. Every other handler
leaves it false, so the default is "redraw", exactly as today. The
covered entry points are `pan_horizontal` and `pan_vertical`
(`navigation.rs`, ten main-pane bindings between them),
`override_pan_vertical`/`override_pan_horizontal` and
`manage_pan_vertical`/`manage_pan_horizontal`.

A pan is the right place to stop, and a general "did anything change?"
check on `App` is the wrong one: a pan is the one input the user
generates in long unbroken runs against a hard bound, and its effect is
a single number that is trivially comparable. Diffing the whole of
`App` would be both expensive and fragile.

### S3 — reuse the window styles when the window is unchanged

`window_styles_for(text, indent_size)` is a pure function of exactly
those two arguments, so they are the complete validity key. `App`
caches the `Vec<String>` and the `indent_size` that produced the current
`window_styles`. Each frame:

- key unchanged → keep `window_styles` as they are, and do not run
  tree-sitter;
- key changed and `input_pending` → clear, as spec 0223 does today;
- key changed and not pending → recompute and store the new key.

This subsumes the flicker case: during a burst the window does not move
(the pan is clamped), so the styles survive whatever `input_pending`
says. It also removes the tree-sitter parse from every heat repaint,
activity tick and settle frame that redraws an unmoved window.

Spec 0223 clears rather than keeps because stale hints would paint one
viewport's colors onto another viewport's rows. That objection is
exactly the "key changed" branch; when the key is unchanged the hints
are not stale, they are current.

## Alternatives considered

**Monochrome only above X queued events.** The user's first suggestion,
and the direct read of the symptom. It treats a queue depth of 2 as
noise and 3 as an emergency, but the queue depth during a burst is set
by thread scheduling, not by load — so the threshold would be crossed
and re-crossed by the same jitter that causes the flicker today, just
less often. It also leaves the underlying waste in place: X no-op frames
per burst instead of all of them.

**Hysteresis on `input_pending`** (enter monochrome above a high-water
mark, leave below a low one). Fixes the alternation without fixing the
waste, and adds a second piece of state to reason about at every draw.
Coalescing makes it unnecessary: a drained burst leaves the counter at
0, so there is nothing to oscillate.

**A general "the frame would be identical" check.** Would catch every
no-op input, not just pans. Rejected: computing it means either diffing
`App` or hashing the rendered frame, and hashing means rendering, which
is the cost the check exists to avoid.

## Test plan

1. `a_burst_of_queued_events_is_one_dispatch_pass_and_one_frame` — eight
   queued events cost exactly one frame. The count is exact rather than
   a bound, which is what makes it a regression test.
2. `a_drain_stops_at_a_control_transfer` — a quit mid burst leaves the
   events behind it unconsumed.
3. `a_pan_that_hit_its_bound_asks_for_no_frame` — main pane and side
   pane, vertical and horizontal; and a pan that *does* move clears the
   flag again.
4. `an_unchanged_window_keeps_its_styles_while_input_is_pending`.
5. `stale_styles_are_cleared_not_left` — 0223's clear, now with the
   viewport moved between the two frames, which is the only case it
   applies to.

Tests 1 and 2 drive the real `run_loop` over a pre-filled channel with a
frame-counting `Backend` wrapper (`tui/tests/event_loop.rs`); the frame
count is not otherwise observable.

## Measured outcome

Sixty wheel-up reports written to the pty in one `write`, against
`prototext-core/fixtures/descriptor.pb` with the 25.6 MB
`googleapis.desc`, traced through `PROTOLENS_TRACE`. Baseline is
`fd79249` built from a worktree.

| burst | before | after |
| --- | --- | --- |
| 60 events that still pan (from the top into 0244's blank rows) | 60 frames, 58 of them monochrome | **1 frame**, in color |
| the next 60, entirely against the bound | 60 frames, 58 of them monochrome | **0 frames** |

The flicker is 58 gray frames in a 49 ms window — the alternation the
user saw. The whole coalesced pass dispatches its 60 events in 2.3 ms,
so `DRAIN_BUDGET` is never approached by a real wheel burst; it exists
for queues of expensive events, and this run does not exercise it.

S3's reuse does not show in this trace: the one frame each burst draws
is the one whose window *did* move, so it recomputes (209 µs) as it
should. Its win is on the frames a burst no longer draws at all, and on
heat repaints and activity ticks over an unmoved window.
