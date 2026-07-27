<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0190 — the activity dot reports the highest live tier

Status: implemented
Implemented in: 2026-07-27
App: protolens
Refs: docs/specs/0138-protolens-main-pane-inference-heat-cue.md,
      docs/specs/0147-protolens-status-message-command-line-split.md,
      docs/specs/0152-protolens-heat-cue-background-scoring-thread.md,
      docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md,
      docs/specs/0189-a-superseded-request-wave-is-discarded-by-the-worker.md

## Background

protolens has one worker thread and one request queue, and neither is
visible. When a heat cue takes a while to resolve there is no way to
tell, from the screen, whether the queue is saturated, whether the
worker is mid-sweep on an expensive range, or whether nothing was ever
requested at all. Every diagnosis so far has needed either an added
`eprintln!` or a test harness.

A single cell can carry most of that signal, because the queue's
priority bands already encode *why* each request exists: `User` means a
cursor landed on something, `Visible` means a row on screen needs a cue,
`Prefetch` means read-ahead. Showing which of those is live is showing
what the machine is currently working for.

### The status bar is fully occupied

`render` splits the frame into a `Min(0)` pane area and a `Length(1)`
global command/message row (`render.rs:424-430`), bound as `cmd_row =
chunks[1]` (`render.rs:732`). That row is 100% given to the command
buffer, the rename buffer, or the status message; three separate things
downstream derive from it — `self.cmd_area` for mouse hit-testing
(`render.rs:737`), `width` for pan clamping (`render.rs:750`), and
`set_cursor_position` (`render.rs:765-766`).

### The signal must not cost a lock on the render path

`HeatRequestQueue`'s occupancy lives behind its `Mutex`
(`heat_worker.rs:61`). Reading it from `render` would put the UI thread
into contention with the worker thread on a lock the worker holds across
every pop. The render path must therefore read a lock-free snapshot,
maintained by the code that already holds the lock.

### An event-driven loop can miss the transition

`run_loop` draws unconditionally at the top of each iteration and then
blocks (`mod.rs:2008-2065`). Its wait is `rx.recv_timeout` only while a
message or splash deadline is pending; otherwise it is a bare
`rx.recv()` that blocks indefinitely (`mod.rs:2054-2060`). Not every
queue transition sends an `AppEvent`, so a dot maintained purely by
event arrival would freeze in whatever state it was last drawn in —
showing "busy" over an idle worker, which is worse than showing nothing.

## Goals

- **G1**: One cell, always at the same place, showing the
  highest-priority tier that is currently live — either queued or being
  worked on right now.
- **G2**: Nothing on the render path takes the queue's lock, and nothing
  on the queue's hot path does more than one relaxed atomic store.
- **G3**: The dot cannot go stale by more than one tick, without
  auditing every code path that drains the queue.
- **G4**: An unchanged dot costs no frame at all — the comparison
  happens before `terminal.draw`, not inside the widget tree.
- **G5**: The command/message row keeps its full behavior (pan, cursor,
  mouse hit-testing), just one column narrower.

## Non-goals

- **N1**: A general-purpose status area or a second indicator. One cell,
  one fact.
- **N2**: Reporting cache occupancy, worker throughput, or queue depth
  as a number. The tier is the useful fact; a count is not readable in
  one cell.
- **N3**: Making the tick interval or the colors configurable.
  *Trigger for revisiting*: if the tick shows up as measurable idle CPU
  on a battery-powered setup.
- **N4**: Reporting the fourth band. Per spec 0189 its entries are
  destined to be discarded rather than scored, so they are not work
  being done for the user; this spec treats `Prefetch` as one tier and
  counts only `prefetch_current`.
- **N5**: Hiding the column when the worker is disabled. The column is
  reserved unconditionally (G5) so the command row's geometry never
  changes underneath the user mid-edit.

## Specification

### S1 — `TieredBounded` reports its occupancy in O(1)

```rust
/// Bit 0: `user` non-empty. Bit 1: `visible` non-empty. Bit 2:
/// `prefetch_current` non-empty. O(1) — three `Option<usize>`
/// head-pointer tests, no traversal and no counting.
pub(super) fn band_occupancy(&self) -> u8 {
    u8::from(self.user.head.is_some())
        | (u8::from(self.visible.head.is_some()) << 1)
        | (u8::from(self.prefetch_current.head.is_some()) << 2)
}
```

`prefetch_previous` is excluded (spec 0189 S6). Its entries are
superseded requests the worker will discard rather than score, so
counting them would light the dot for a subsystem with nothing left to
do for the user — precisely the misreport this spec exists to prevent.

### S2 — the queue publishes it as a lock-free snapshot

`HeatRequestQueue` gains

```rust
/// Lock-free mirror of `state.mru.band_occupancy()` (spec 0190 G2),
/// so `render` can read what the queue is holding without taking the
/// `Mutex` the worker thread holds across every pop.
///
/// `Relaxed` is sufficient *and exact*: every store happens while
/// the `Mutex` is held, so the stores are already totally ordered
/// with respect to each other and each one reflects the state the
/// storing thread just established. The reader tolerates seeing a
/// slightly old value by construction — it redraws on the next tick
/// (G3).
queued: AtomicU8,
```

refreshed at the tail of every mutating operation, before the lock is
released: `push` (`heat_worker.rs:101-118`), `pop_blocking`
(`:128-139`), and `drop_current_wave` (spec 0189 S2). One helper called
from all three, taking `&mut HeatRequestQueueState`, keeps the
"published under the lock" invariant in one place.

### S3 — the in-flight tier is published too

Reporting only what is *queued* would make the `User` tier practically
invisible: a `User` push wakes the worker through the condvar
(`heat_worker.rs:116`), which pops it immediately, so the bit is set and
cleared within microseconds and almost never survives to a draw. Yet a
`User`-tier sweep is precisely the one the user is waiting on.

`HeatRequestQueue` therefore also carries

```rust
/// The tier of the request the worker is executing right now, or 0
/// for "idle" (spec 0190 S3). Written by the worker thread only,
/// outside the `Mutex`: `Relaxed` on both ends, because it feeds a
/// cosmetic indicator that is re-read every tick anyway.
in_flight: AtomicU8,
```

`heat_worker_loop` stores `req.tier as u8 + 1` right after
`pop_blocking` returns, and stores `0` once that request's scoring
finishes — including on the early-out paths where the cache already
covers the request by the time the worker re-checks it.

Two separate atomics rather than one packed byte: the queue side and the
worker side write on different threads at different moments, so
separating them means each writer does a plain `store` and never a
read-modify-write, and there is no window where one writer's update is
half-applied from the other's point of view.

### S4 — the two are combined at read time

```rust
/// The highest-priority tier that is live — queued or in flight —
/// or `None` when the worker has nothing to do at all. Read from
/// `render`; two relaxed loads and some bit twiddling, no lock.
pub(super) fn activity(&self) -> Option<Tier>
```

`Tier::User` wins over `Tier::Visible` wins over `Tier::Prefetch`,
across both sources.

### S5 — the column is reserved unconditionally

`chunks[1]` splits horizontally into `[Length(1), Min(0)]`. The first
cell is the dot; the second becomes `cmd_row`. Because `cmd_area`,
`width` and `set_cursor_position` all already derive from `cmd_row`
(`render.rs:737/750/765`), the pan logic, the mouse hit-testing and the
cursor placement follow the narrower row without further change — that
is the whole reason to re-bind `cmd_row` rather than to inset at each
use site.

**First cell, not last.** The last cell of the last row is the terminal's
auto-wrap/scroll hazard cell; writing it can scroll the screen on some
terminals. Column 0 also lines up with the main pane's heat-cue column
(`render.rs:608-611`), which is the right visual association: the dot
says something about the same subsystem those cues come from.

### S6 — the glyph and the ramp are the existing ones

The glyph is `heat_cue::HEAT_GLYPH` (`'●'`). This is not aesthetic
preference: `●` and its plausible alternatives are East-Asian Ambiguous
width, so in a CJK-configured terminal they render double-width and
would overflow a one-cell slot. The app already depends on how this
particular glyph behaves everywhere else, so reusing it means the dot
cannot break in a configuration the heat cues survive.

Colors come from `theme::heat_style(level, hue, theme)`
(`theme.rs:508-529`), which already provides light/dark-aware 12-stop
red and blue ramps:

| tier | hue | level |
|---|---|---|
| `User` | `Red` | 12 |
| `Visible` | `Red` | 5 |
| `Prefetch` | `Blue` | 5 |
| idle | — | render a space |

Every level is ≥ 4, which matters: `heat_style` returns `None` for
`level <= 3` on the ANSI-16 fallback (`theme.rs:516`), and a dot that
silently vanishes on a 16-color terminal would be a worse diagnostic
than no dot. On that fallback the three levels collapse to `LightRed`,
`Red` and `Blue` — still three distinguishable states, still darkening
along `User` → `Visible` → `Prefetch`.

### S7 — a heartbeat bounds staleness

```rust
/// How long protolens may sit unattended before re-examining the
/// activity byte (spec 0190 G3). Bounds the dot's staleness without
/// requiring every queue-draining path to emit an `AppEvent`.
const ACTIVITY_TICK: Duration = Duration::from_millis(250);
```

folded into `run_loop`'s existing deadline computation
(`mod.rs:2024-2029`) as a third candidate alongside `message_deadline`
and `splash_deadline`. Since it is always present, `deadline` is always
`Some` and the `None => rx.recv()` arm (`mod.rs:2059`) is removed.

The tick is deliberately *not* armed per queue event: queue events are
far too frequent, and each one would reschedule the timer. A flat
interval is a fixed cost independent of queue traffic.

### S8 — an unchanged dot skips the frame

`terminal.draw` currently runs unconditionally at the top of each loop
iteration (`mod.rs:2009`). It becomes conditional on a `redraw` flag,
initialized `true` and recomputed after each receive:

- a received event → redraw;
- no event, but a message or splash deadline has actually elapsed →
  redraw (this is the existing reason for the timeout to exist);
- no event and only the activity tick elapsed → redraw **only if**
  `activity()` differs from the value last drawn.

`App` carries the last-drawn value so the comparison is two bytes. This
is the point of the design: on an idle-but-working protolens the tick
fires four times a second and costs two atomic loads and a comparison,
not a frame. Skipping *inside* `render` would not work — ratatui resets
the back buffer every frame, so a span that is not re-emitted is erased
rather than preserved; the gate has to be above `draw`, not below it.

### S9 — the heat cue's per-frame allocation

`render.rs:610` builds the main-pane cue with
`Span::styled(heat_cue::HEAT_GLYPH.to_string(), style)` — a heap
allocation per cue per frame, on every visible row. `HEAT_GLYPH` becomes
a `&'static str` so both this call site and the new dot can use it
without allocating. Included here because the dot is a third consumer of
the same constant and would otherwise copy the same mistake.

## Feasibility

The tick means the process never blocks indefinitely again: it wakes
four times a second forever, where today an idle protolens with no
pending message sleeps until the user presses a key. Each wake is two
relaxed loads, a comparison, and going back to sleep — but it is a real
change from "genuinely idle" to "polling slowly", and it is stated here
rather than buried.

Everything else is contained: `TieredBounded`, `HeatRequestQueue` and
`HeatWorkerHandle` are `pub(super)` within `protolens::tui`, and the
render change re-binds one existing local.

## Test plan

protolens is a bin-only crate — `cargo test --release --bin protolens`.

1. `tiered.rs` — `band_occupancy` returns the expected bits for each
   combination of populated bands, including an empty structure
   returning 0 and a superseded wave clearing bit 2.
2. `heat_worker.rs` — a push at each tier sets the corresponding
   `queued` bit under the lock; `pop_blocking` clears it; a
   `start_new_wave` clears bit 2 without disturbing bits 0-1.
3. `heat_worker.rs` — with a real worker and the existing tiny in-memory
   graph, `in_flight` is non-zero while a sweep runs and returns to 0
   afterward, including on the "cache already covers it" early-out.
4. `activity()` precedence: `User` over `Visible` over `Prefetch`,
   and in-flight counted equally with queued.
5. Render — a `TestBackend` frame with a seeded queue shows `HEAT_GLYPH`
   at column 0 of the last row, and the command text starting at column
   1; with an empty queue, column 0 is a space.
6. Render — with a command buffer active, the reported cursor position
   is one column right of what it was before this spec, and pan
   clamping uses the narrowed width.
7. Interactive on `/tmp/pdb.desc`: the dot is blue during read-ahead,
   goes bright red on a cursor move onto an unscored node, and clears
   once the document settles.

## Open questions

- None.

## Implementation notes

`App::heat_activity` is `pub(in crate::tui)`, not `pub(super)`: `App`
is `pub(crate)`, so `pub(super)` in `tui/mod.rs` means `pub(in crate)`
and the compiler rejects it for exposing the more-private `Tier`.

`set_in_flight` encodes the tier into the same bitmask layout as
`band_occupancy`, which makes `activity()` a plain
`queued | in_flight` followed by a priority cascade. The first attempt
derived the bit arithmetically from the tier's discriminant; it was
correct but unreadable, and would have broken silently if `Tier` ever
gained a variant.

The idle-state test could not simply assert a blank dot after a draw:
the first render's own heat-cue lookups queue `Visible` requests, and
`heat_cues_hidden` does not suppress them (by design — it still primes
the cache; see `heat_cue_for_still_pushes_a_request_when_heat_cues_
hidden`). The test therefore asserts blankness with *no worker
installed*, then installs the stub and uses the render's own `Visible`
pushes as the baseline against which a `User` push must differ. That
turned out to exercise the real path rather than a contrived one.

`HEAT_GLYPH` became a `&str` (S9) so both it and `ACTIVITY_GLYPH` can
be used directly as `Span` content. They are separate constants sharing
a character on purpose: the shared fact is narrow (`●` is East-Asian
Ambiguous width, so it is the glyph already known to survive every
terminal this app runs in), and the two meanings must be free to
diverge.
