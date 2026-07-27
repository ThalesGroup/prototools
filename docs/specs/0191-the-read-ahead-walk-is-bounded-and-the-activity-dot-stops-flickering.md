<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0191 — the read-ahead walk is bounded, and the activity dot stops flickering

Status: implemented
Implemented in: 2026-07-27
App: protolens
Refs: docs/specs/0151-protolens-heat-cue-cache-and-startup-progress.md,
      docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md,
      docs/specs/0189-a-superseded-request-wave-is-discarded-by-the-worker.md,
      docs/specs/0190-the-activity-dot-reports-the-highest-live-tier.md

## Background

Two symptoms observed interactively on a large `FileDescriptorSet`
(`googleapis.desc`, 25.6 MB, 49 255 roots): protolens keeps two cores
busy indefinitely after a single `G`, and the activity dot flickers
rather than reporting a steady state. They have different causes and
different fixes, but they are the same complaint — the machine should be
visibly and actually quiet when there is nothing left to do.

### The read-ahead walk has no bound

`App::prefetch_step` (`mod.rs:1572-1639`) advances a zigzag walk
outward from the cursor's display row, pushing one `Tier::Prefetch`
request per call. Its only stopping condition is
`PrefetchWalk::next_row(self.visible_rows.len())` (`mod.rs:1608`)
returning `None`, which happens only when *both* ends have run off the
ends of the document. So one cursor move starts a sweep over every
expanded row there is.

Two consequences.

**Both threads spin.** `run_loop`'s receive loop
(`mod.rs:2087-2110`) is

```rust
Err(mpsc::TryRecvError::Empty) => match app.prefetch_step() {
    PrefetchStep::Progressed => continue,
    PrefetchStep::Idle => {
        let timeout = deadline.saturating_duration_since(Instant::now());
        break rx.recv_timeout(timeout).ok();
    }
},
```

While `prefetch_step` returns `Progressed` the main thread never
blocks: it loops, pushing one row per iteration, and redraws whenever
the activity byte flips. Only `Idle` reaches the deadline-aware
receive. Meanwhile the worker runs `score_all` back to back. Two busy
cores, no user input, for tens of thousands of rows.

**Most of that work is discarded before anyone can read it.** The
result caches are `TieredBounded` at `HEAT_CACHE_MAX_ENTRIES = 8192`
(`heat_worker.rs:296,302,313-314`; `heat_cue.rs:129`), and `evict_one`
takes `prefetch_current`'s **tail** — the same end `upsert` inserts at
(`tiered.rs:309-317`). Once the cache's prefetch band is full, every
new prefetch result evicts *itself*: the worker pays a whole sweep and
the answer is gone on insert. So `HEAT_CACHE_MAX_ENTRIES` is already a
hard ceiling past which the walk is provably useless. Nothing enforces
it.

### The dot's blank frames are a sampling artifact

`run_loop` reads the activity byte one statement *before* the draw
(`mod.rs:2047-2050`):

```rust
if redraw {
    last_drawn_activity = app.heat_activity();   // read here
    terminal.draw(|frame| app.render(frame))?;   // render pushes requests here
}
```

and `render` is itself a producer: `render.rs:560` calls
`heat_cue_for(line_idx)` for every visible row, which pushes a
`Tier::Visible` request for any row not yet settled. So the dot is
structurally incapable of reporting the requests its own frame creates.

The visible consequence is one dark frame per completed request:
worker drains → activity 0 → redraw with a **blank** dot → that same
draw refills the queue → next frame lit again. The trough is real but
it exists only because the reader samples before the writer.

This one is self-limiting. `heat_cue_resolve` returns early on
`settled()` before pushing anything (`heat_cue.rs:273-275`), and a row
settles after at most two round-trips (`settled()` needs both `best`
and `current` — `heat_cue.rs:101-106`). So render stops producing once
the screenful resolves, and the flicker is a short burst. The
*indefinite* flicker on a large document is the unbounded walk, not
this.

Both are worth fixing. Only the first is a bug.

## Goals

- **G1**: A read-ahead wave visits a bounded number of rows, so it
  terminates, so both threads reach a genuine idle state with no user
  input.
- **G2**: That bound is never above the point at which a prefetch
  result would evict itself on insert — enforced at compile time, not
  by comment.
- **G3**: The activity dot reports a level that only drops after a full
  inter-frame interval with nothing live. Rises are immediate.
- **G4**: The dot's value accounts for the requests the previous draw
  itself queued, so the one-frame producer/reader lag stops producing
  blank frames.
- **G5**: No new lock, no new thread, no new atomic. G3/G4 are a change
  of *when* the existing atomics are sampled, not of what they hold.

## Non-goals

- **N1**: Deriving the walk bound from
  `HEAT_REQUEST_QUEUE_MAX_ENTRIES`. The two constants coincide at 2048
  today and that is a coincidence. The queue cap bounds *outstanding
  requests* — how far ahead of the worker the walk may run before it is
  parked (`UpsertOutcome::Rejected` → `PrefetchStep::Idle`,
  `mod.rs:1634-1637`). The walk bound caps *rows visited per wave* —
  how far read-ahead is still worth computing. Raising one to smooth
  out stalls must not silently double the other's reach. The invariant
  that *is* real is against the cache (G2).
- **N2**: Making either bound configurable, or adapting it to terminal
  height. A fixed row budget is predictable and does not change
  underneath the user on a window resize; a screenful-derived bound
  would.
- **N3**: Resuming an exhausted walk on a timer, or extending it when
  the user scrolls without moving the cursor. A cursor move already
  restarts the walk from the new origin (`mod.rs:1577-1605`), which is
  the case that matters.
- **N4**: An explicit confirmation counter for the dot. S3's two-window
  sliding maximum already *is* the two-frames rule — a level survives one
  quiet window and drops on the second — and it needs no separate state
  to stay consistent with the value being displayed.
- **N5**: Removing the `ACTIVITY_TICK` wakeup. It remains spec 0190 N3's
  business.

## Specification

### S1 — the walk carries a row budget

```rust
/// Spec 0191 G1: how many rows one read-ahead wave may visit before
/// it reports `Idle` and lets both threads park. Not a reach limit
/// per side — a budget across both ends, so a cursor near the top of
/// the document still gets its full allowance downward.
///
/// Deliberately *not* derived from `HEAT_REQUEST_QUEUE_MAX_ENTRIES`,
/// which happens to hold the same number: that one bounds requests
/// outstanding against the worker, this one bounds rows visited per
/// wave (spec 0191 N1).
const PREFETCH_WALK_MAX_ROWS: usize = 2048;

/// Spec 0191 G2: past the cache's capacity a prefetch result evicts
/// itself on insert (`evict_one` takes `prefetch_current`'s tail, the
/// end `upsert` inserts at), so the worker would pay a full
/// `score_all` for an answer nobody can ever read.
const _: () = assert!(PREFETCH_WALK_MAX_ROWS <= heat_cue::HEAT_CACHE_MAX_ENTRIES);
```

`PrefetchWalk::next_row` (`mod.rs:1513-1544`) gains one guard at the
top of its loop:

```rust
if self.above + self.below >= PREFETCH_WALK_MAX_ROWS {
    self.above_done = true;
    self.below_done = true;
    return None;
}
```

`above` and `below` already count steps taken on each end, so their sum
is exactly the number of rows the walk has returned. Setting both
`_done` flags rather than returning `None` unconditionally keeps the
struct's "exhausted" state single-valued, so a later call takes the
existing `above_done && below_done` early-out without re-deriving the
budget.

The budget bounds rows *visited*, not requests *pushed* — the walk
skips non-header, non-overridable and already-settled rows with
`continue` (`mod.rs:1612-1617`) without pushing. Bounding the visited
count is the stronger statement and the one G1 needs: it is what caps
the loop at `mod.rs:1607`, which is where the main thread spins.

### S2 — the dot renders a decided value, not a live probe

`App` gains

```rust
/// Spec 0191 G3/G4: the activity level the dot should show, decided
/// by `run_loop` from a high-water sample rather than probed at draw
/// time. `render_activity_dot` reads this instead of calling
/// `heat_activity()` directly, because the value that matters spans
/// the interval since the last frame — including the requests the
/// last frame's own `heat_cue_for` calls queued, which a probe taken
/// before `terminal.draw` can never see.
activity_shown: Option<tiered::Tier>,
```

initialized `None`, and `render_activity_dot` (`render.rs:821-835`)
reads `self.activity_shown` in place of `self.heat_activity()`. Nothing
else about the widget changes — same glyph, same ramp, same column.

This moves the dot from "live probe" to "what the loop decided to
show". That is the honest description of what it becomes, and it is
what makes hysteresis expressible at all: a probe has nowhere to hold
state.

### S3 — the loop keeps a two-window sliding maximum

`Tier` is already `Ord` with `Prefetch < Visible < User`
(`tiered.rs:16-21`), and `Option<T>` orders `None < Some(_)`, so
"highest level seen" is literally `max`.

One **window** is one iteration of `run_loop`. Each iteration
accumulates every sample it takes into `activity_window`, then closes
that window by shifting it into `activity_prev_window` and starting
clean. The dot shows `max(previous, current)`:

```rust
let mut activity_window: Option<tiered::Tier> = None;
let mut activity_prev_window: Option<tiered::Tier> = None;
loop {
    if redraw {
        app.activity_shown = activity_prev_window.max(activity_window);
        terminal.draw(|frame| app.render(frame))?;
        // The requests this draw's own `heat_cue_for` calls just
        // queued belong to the current window (G4).
        activity_window = activity_window.max(app.heat_activity());
    }
    // ... deadline computation, prefetch/receive loop ...
    activity_window = activity_window.max(app.heat_activity());
    redraw = received.is_some()
        || ui_deadline.is_some_and(|d| Instant::now() >= d)
        || activity_prev_window.max(activity_window) != app.activity_shown;
    activity_prev_window = activity_window;
    activity_window = None;
```

Rises are immediate: a push during the receive loop raises the current
window, the gate sees the maximum differ from `activity_shown`, and the
next iteration draws it. Falls need **two** consecutive quiet windows —
which during real work never happens, because at least one push lands in
every iteration, and which when genuinely idle takes two 250 ms ticks.
That is the "confirmed for two frames" rule, expressed as a sliding
maximum rather than a counter.

The post-draw sample is what fixes G4; the two-window span is what fixes
G3.

**Both windows must be reset, never merely accumulated.** A single
high-water mark reset only at a draw self-deadlocks: with the mark stuck
high and the dot already showing that level, the gate finds no
difference, so no draw happens, so the mark is never reset — and the dot
stays lit forever after a read-ahead wave finishes. This is not
hypothetical; it is what the first implementation did, and it is why the
window is closed unconditionally at the bottom of every iteration rather
than at the top of a draw.

The comparison is against `app.activity_shown` rather than a separate
`last_drawn_activity` local, because they would always hold the same
value; one field is one thing to keep consistent instead of two.

### S4 — no oscillation-driven redraws

The gate at `mod.rs:2115-2117` currently reads
`app.heat_activity() != last_drawn_activity`, so **every** trough
between two requests forces a frame. Comparing the debounced value
instead means high-frequency toggling stops forcing redraws at all.

This is not a separate mechanism — it falls out of S3 — but it is the
part with a measurable cost, and it is worth naming: it removes main-
thread frames during exactly the period when the worker wants the CPU.

## Feasibility

S1 is four lines in a module-private struct plus two constants; its
only external surface is that a wave now ends. S2 adds one `App` field
and redirects one widget read. S3 rewrites about eight lines of
`run_loop`. No new locking, no new thread, no change to the queue, the
caches or the worker.

The behavioral change worth stating plainly: after this spec, read-ahead
on a document longer than 2048 expanded rows covers strictly less of it
than before. That is deliberate — the rows past the budget were being
scored into a cache that discarded them, so nothing that was previously
*readable* becomes unreadable. What changes is that the CPU stops.

## Test plan

protolens is a bin-only crate — `cargo test --release --bin protolens`.

1. `PrefetchWalk::next_row` — a walk with an origin in the middle of a
   very long document returns exactly `PREFETCH_WALK_MAX_ROWS` rows and
   then `None`, and stays `None` on further calls.
2. `PrefetchWalk::next_row` — origin at row 0, so one end is exhausted
   immediately: the walk still returns `PREFETCH_WALK_MAX_ROWS` rows
   (all downward), confirming the budget is shared across ends rather
   than applied per side.
3. `PrefetchWalk::next_row` — the existing zigzag-order and
   open-end-continuation tests still pass unchanged, confirming the
   guard does not perturb ordering below the budget.
4. `App::prefetch_step` — on a fixture with more eligible rows than the
   budget, the walk reports `Progressed` at most `PREFETCH_WALK_MAX_ROWS`
   times and then `Idle` (this is the property that lets the main thread
   reach `recv_timeout`).
5. `activity_shown` starts `None` and a render with it `None` draws a
   space at the dot's column; set to `Some(Tier::User)`, the same render
   draws `ACTIVITY_GLYPH`. This pins S2 — that the widget reads the field
   and not the atomics.
6. Interactive on `googleapis.desc`: after `G`, CPU returns to idle
   rather than staying pegged; the dot shows a steady blue during
   read-ahead and clears once, without flicker.
7. Interactive on `/tmp/pdb.desc`: the short blink burst while the first
   screenful settles is gone.

## Open questions

- 2048 is chosen, not measured: it is a quarter of the cache, which
  leaves room for roughly four waves' worth of retained results plus the
  visible set. Whether a smaller budget is indistinguishable in use is a
  question for a real scroll session, not for a bench.

## Implementation notes

The single-high-water-mark deadlock described in S3 was found
interactively, not by a test: after a cursor move the dot went blue and
never went out, while `t` (which produces a stream of events, hence a
stream of draws, hence a stream of resets) behaved correctly. The
asymmetry between "with input it recovers, without input it sticks" is
the signature of an accumulator whose only reset is on a path gated by
that same accumulator. The two-window form has no such gate.

`run_loop` itself is not unit-tested — it owns a `Terminal`, a channel
and an unbounded loop, and exercising it would mean building a harness
larger than the code under test. S3's correctness therefore rests on
test-plan items 6 and 7 (interactive) plus the structural argument
above. What *is* unit-tested is the contract that makes S3 possible at
all: that the widget renders `activity_shown` and not a live probe
(item 5), which is the assertion that fails against the pre-0191
implementation.

Test-plan items 6 and 7 were left resting on interactive observation
alone at the time. Spec 0192 S5's `PROTOLENS_TRACE` later made both
machine-checkable after the fact, and a headless 40-keystroke session on
`googleapis.desc` corroborates them: every one of the 42 `wave` lines
reports `rows=2048` — the budget is reached and enforced on each wave,
never exceeded — and only 12 of the session's 128 draws were
activity-driven, i.e. the dot changed state 12 times across 40
keystrokes rather than once per completed request. Neither is a
substitute for a `run_loop` harness, but both are now numbers rather
than impressions.

`prefetch_fixture` gained cycling field numbers (1..=15). It previously
counted field numbers up from 1 and wrote the tag as a single `as u8`,
which silently truncates past field 31 — harmless at the five-node scale
the fixture was written for, but the budget tests need thousands of
nodes. Nodes stay distinct through their `raw_range`, which is what keys
the request queue.

Test plan item 3 needed no new code: the pre-existing zigzag-order tests
use origins and documents far below the budget, so they exercise the
unbudgeted path unchanged, which is exactly what item 3 asks for.
