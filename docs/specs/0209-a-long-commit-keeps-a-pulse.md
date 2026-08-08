<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0209 — a long commit keeps a pulse

Status: superseded by spec 0249 (2026-08-08). Since spec 0255 a confirm
        renders one screenful and the remainder is baked in the idle
        loop, so there is no long commit left to keep a pulse during.
        The pulse itself was also rejected on its own terms: spec 0249
        S13 wants the cue *steady*, because a blink is a timer-driven
        redraw every ~500 ms for the whole job and spec 0245's rule is
        that a frame is drawn only when something changed. What
        survives is that spec's dot, on the cell spec 0190 already
        established. Kept for the record; do not implement.
        Its Background says "tens of seconds" on `googleapis.desc`; a
        root override there measured about 1 s on 2026-07-31 and 0.59 s
        after spec 0255.
App: protolens
Refs: docs/specs/0151-protolens-heat-cue-cache-and-startup-progress.md
        (G8, `warm_up_heat_cues` — the existing precedent for a long
        main-thread pass that draws its own progress),
      docs/specs/0152-protolens-heat-cue-background-scoring-thread.md
        (G8/G9, the input-reader thread this spec gives a second job,
        and the shutdown ordering it already guarantees),
      docs/specs/0190-the-activity-dot-reports-the-highest-live-tier.md
        (S5/S6, the dot, its reserved cell, and `ACTIVITY_TICK`),
      docs/specs/0191-the-read-ahead-walk-is-bounded-and-the-activity-dot-stops-flickering.md
        (S2, `activity_shown` and why a draw-time probe is wrong),
      docs/specs/0204-a-long-batch-says-so-before-it-blocks.md
        (superseded by this spec — see Background),
      docs/specs/0205-the-batch-runs-off-the-input-thread.md
        (the fix this spec is *not*; see Non-goals and S10),
      docs/protolens/design/arena-and-batch.md (what a commit actually
        does, and why it takes as long as it does)

## Background

Committing an override change is synchronous. Creating an override,
rotating its origin kind, enabling it, disabling it, renaming an active
entry, deleting it — every one of these ends in a call to
`App::render_overrides`, which runs to completion on the main thread
before the next frame is drawn.

On a large document that is seconds. On `googleapis.desc` it is tens of
seconds. During the whole of it protolens does not repaint, does not
respond to input, and is indistinguishable from a hung process.

### Why the activity dot cannot help as it stands

The dot already exists (spec 0190 S5): one reserved cell at column 0 of
the bottom row, showing the highest live heat-cue tier. It is driven by
`ACTIVITY_TICK`, a 250 ms poll timeout **inside `run_loop`**.

During a commit `run_loop` is not running. No timeout fires, no frame
is drawn, and the dot is as frozen as everything else. The mechanism
that exists to say "something is happening" is disabled by precisely
the event it would be reporting.

More generally: while the commit blocks the drawing thread, *anything*
the user sees is on screen because some in-commit code point ran. There
is no way to make the display independent of in-commit instrumentation
without taking the painting off that thread.

### Two concerns, deliberately separated

**Determination** — is progress still being made? Must be accurate
(only assert progress where progress is real) and cheap enough to
assert very often.

**Display** — conveying that to the user. Must be unobtrusive, must not
depend on the determination working correctly in order to be *present*
at all. A dot that only flashes when the instrumentation is complete
would report gaps in our coverage as if they were stalls in the
program, which defeats the purpose.

This spec keeps them separate. The pulse is driven by the fact that a
commit is running — set and cleared at two call sites, not derivable
wrongly. The liveness signal is layered on top and can only change the
pulse from flashing to steady.

### Relationship to specs 0204 and 0205

Spec 0204 proposed a static "this will take a while" message before the
commit blocks. This spec does that (S8's banner) and more, so **0204 is
superseded and should be closed** when this lands.

Spec 0205 proposes moving the commit off the input thread. That removes
the freeze rather than annotating it, and it is the better fix. It also
deletes most of this spec: with `run_loop` still turning, the dot
flashes from the ordinary draw path and needs no painter, no lock, no
mode, no handback. This spec is written as the next thing to implement
on the assumption 0205 is not imminent; S10 records what survives it.

## Goals

- **G1.** While a commit is running, the activity dot pulses, so the
  user can tell a working protolens from a wedged one.
- **G2.** The pulse's presence depends only on the commit having
  started and not yet finished — not on the coverage or correctness of
  any progress instrumentation.
- **G3.** A cheap, very frequent liveness assertion the commit can make
  from inside its own loops, tolerant of being missed or double-counted.
- **G4.** When that assertion stops arriving, the pulse stops and holds
  steady, distinguishing "slow" from "stuck".
- **G5.** No flicker, and no change to what the screen looks like when
  no commit is running.
- **G6.** Nothing in the normal-operation path pays for this: no lock
  acquired per frame, no extra allocation, no extra wakeup.

## Non-goals

- **N1.** Making the commit faster, or making it interruptible. This
  spec annotates the stall; it does not shorten it.
- **N2.** Moving the commit off the main thread. That is spec 0205.
- **N3.** Reporting progress for anything other than an override
  commit. (As written this also excused incremental arena compaction,
  spec 0203, as sliced and therefore invisible; spec 0216 has since
  deleted compaction outright.)
- **N4.** Retiring `activity_shown` or `ACTIVITY_TICK`. A painter that
  owned the dot *continuously* could subsume both (it would sample
  without frame boundaries, which is exactly what spec 0191 S2 works
  around). It would also take the dot out of ratatui's buffer, breaking
  every existing test that asserts on that cell. Out of scope; see
  Open questions.
- **N5.** Instrumenting `prototext-core`. See S7.

## Specification

### S1. A commit is one `render_overrides` call

`App::render_overrides` is the single choke point. Every trigger the
user can reach — the `type-as` command (`command_line.rs:541`), loading
overrides from YAML (`command_line.rs:852`), applying from the
override-select pane (`key_dispatch.rs:187`), renaming an active entry
(`manage_pane.rs:334`), and the activate/toggle/delete paths in the
management pane — goes through it.

The mode is therefore raised and lowered **inside `render_overrides`**,
not at its call sites. Present and future triggers are covered by
construction.

Two consequences to handle explicitly:

- `render_overrides` is also called once at startup (`mod.rs:1718`),
  before the terminal or the input thread exist. The transitions must
  be harmless when nobody is listening: raising a mode no painter reads
  is a no-op, which it is.
- Commits are not nested. A `debug_assert!` on the transition catches a
  future path that re-enters.
- Three of the sixteen callers do work *after* the call that depends on
  what the call did (`key_dispatch.rs:186` among them — the fact spec
  0205 uses to argue for moving the whole dispatch instead). That work
  is not covered by the pulse. Accepted: it is a fraction of a
  millisecond against a commit measured in seconds, and covering it
  would mean touching sixteen call sites to gain nothing observable.

### S2. The painter lives on the input thread

The input-reader thread (`event.rs:39`) already has the required shape:

```rust
while !stop.load(Relaxed) {
    if event::poll(INPUT_POLL_INTERVAL).unwrap_or(false) {   // 200 ms
        if let Ok(ev) = event::read() { tx.send(AppEvent::Term(ev)) }
    }
}
```

It wakes on key/mouse *or* on a 200 ms timeout, whichever comes first,
and it keeps waking while the main thread is blocked. Each iteration
gains one step, taken **before** the event is forwarded:

```
on wake:
    paint_step()          // new
    forward any event     // unchanged
```

`paint_step` returns immediately unless a commit is in progress, so
normal operation is unaffected beyond two atomic loads.

The painter writes to the terminal with **crossterm directly**, not
through ratatui. `Terminal::draw` is a whole-frame operation — it
resets the back buffer, diffs, and writes the difference — so a
partial draw would emit "erase everything except this cell". Rendering
a full frame is not available to the painter either, since that needs
`&App`, which the main thread owns mutably.

### S3. One lock, and the mode is read under it

Shared state, owned by `App` as an `Arc` and cloned into the painter at
spawn:

| field | type | written by | read by |
|---|---|---|---|
| `mode` + terminal ownership | `Mutex<PaintMode>` | main, at the two transitions | painter, each tick |
| `progress` | `AtomicU64`, `Relaxed` | main, from inside the commit | painter |
| `banner` | set once per commit, under the lock | main, at entry | painter |

`PaintMode` is `Idle` or `Committing { .. }` and carries what the
painter needs: the resolved dot color, the caret position to restore,
and the reserved region's geometry.

The protocol is three rules:

- painter, each tick: **take the lock, read the mode, write only if
  `Committing`, release**
- main, at commit entry: take the lock, set `Committing`, release
- main, at commit exit: take the lock, set `Idle`, release

That is the whole of the synchronization. Because the painter reads the
mode *while holding the lock*, a write after the transition is
impossible by construction — there is no handshake, no acknowledgment,
and no spinning.

**Why not a lock-free handoff.** A two-flag "exit requested / exit
acknowledged" protocol is correct — the painter would acknowledge only
between writes, never during one — but the painter's wait is
`event::poll` on the tty, which the main thread cannot wake. Main would
block up to a full 200 ms at every commit exit, waiting for the painter
to *notice*, at exactly the moment the user is waiting for the screen
to come back. The lock costs microseconds instead, because main waits
only for an in-flight write, not for an observation. It also gets the
release/acquire ordering right without hand-rolling it.

**Why this is free in normal operation.** In `Idle` the painter takes
the lock, reads, and releases without writing; the main thread never
takes it at all. There is no per-frame cost and no contention, because
the two modes are sequential in time. This is the whole reason the mode
is explicit rather than implied.

### S4. What the painter overwrites, and how it is undone

**Nothing is reserved.** No layout constraint changes, no pane is
narrowed, and no frame is laid out differently because a commit might
happen. The painter simply **overwrites** cells on the bottom row for
the duration of the commit, and main puts back what was there.

**The painter's region** is column 0 of the bottom row — the cell spec
0190 S5 already occupies with the dot — plus a fixed field of
`PAINTER_FIELD = 6` columns at the **right** end of the same row, for
the percentage (S8). The painter must never write outside these seven
cells. The field is at the right end because that is where the command
row is least likely to carry content the user is reading; overwriting
it is acceptable anyway, because the user cannot be typing while main
is blocked.

**Entry.** Main draws one ordinary frame carrying the commit banner
(S8) — laid out exactly as any other frame — and then, from that
frame's result, copies the seven cells verbatim into the mode before
raising it. It also records the caret position it just set.

Reading them back is free and exact. `Terminal::draw` returns a
`CompletedFrame` whose public `buffer` field borrows the buffer that
was just drawn, i.e. ratatui's own belief about what is on the screen
(ratatui-core 0.1.2, `terminal/render.rs:310`, `terminal/frame.rs:51`).
Main copies seven `Cell`s out of it — symbol, fg, bg, modifier. No
layout arithmetic is duplicated and nothing can drift out of step with
the renderer.

**Exit.** Main lowers the mode and restores the cells itself, in the
same critical section: take the lock, set `Idle`, write the seven saved
cells and the saved caret with raw crossterm, release. Because the
painter only ever writes while holding that lock, it cannot be
mid-write, and it cannot write again afterwards. The painter therefore
has no handback duty at all and needs no knowledge of the saved
content.

**Why an explicit write-back, rather than just drawing a frame.**
ratatui's previous-buffer belief still says the *original* content — it
never learned about the painter's writes — so the next ordinary diff
would emit nothing for those cells and the last painted glyph would
stay on the screen indefinitely. Restoring the cells makes that belief
true again, which is precisely what lets the next frame be an ordinary
diffed frame. `Terminal::clear()` would also work, but it repaints the
whole screen and reads as a flicker right after a multi-second freeze.
Seven cells is the smaller correction.

**Cost.** Seven `Cell` copies at entry, seven cell writes at exit, per
commit. Nothing is paid when no commit is running, and no frame that
does not surround a commit is laid out differently.

### S5. The flash

- Cycle: **1.2 s**, 600 ms on, 600 ms off.
- The painter decides on/off from `Instant::now()` against the commit's
  start instant, not by counting ticks — so a missed or late wake never
  accumulates drift.
- 600 ms is exactly three 200 ms poll intervals, so edges land on ticks
  and the pulse is regular. `INPUT_POLL_INTERVAL` does not change.
- 1 Hz-class blinking is well outside the 3-55 Hz photosensitivity
  band.

**Color.** The commit dot reuses `theme::heat_style(12,
HeatHue::Red, theme)` — the same resolved color the `User` tier already
uses, so no new constant and no new ANSI-16 fallback question (spec
0190 S6 already guarantees that ramp survives a 16-color terminal). The
*flashing* is what distinguishes a commit from heat activity, not the
hue. Main resolves the color once at entry and records it in the mode,
because the painter writes crossterm colors and has no access to
`self.theme`.

**A tick with nothing to do writes nothing.** With a 200 ms tick and a
1.2 s cycle there are six ticks per cycle and two transitions, so four
of six ticks are no-ops. The painter compares the glyph it is about to
write with the one it last wrote and returns early if they match. This
saves the lock acquisition and the write syscall — not terminal
traffic, which would already be empty.

### S6. Liveness: the watchdog

`progress: AtomicU64` is a monotonically increasing counter, bumped by
the commit at points where forward progress is guaranteed (S7).

The painter keeps the value it last observed. On each tick:

- value changed → **flashing**, and remember the new value
- value unchanged for longer than `STALL_TIMEOUT` (**2 s**, ten ticks)
  → **steady on**

Steady is not a latch. The painter keeps polling at its normal rate,
and if the counter moves again the pulse resumes. The displayed state
is always "what has happened in the last window", never a trapdoor.

`Relaxed` ordering is correct here, and deliberately different from the
mode: this counter conveys a hint that tolerates being stale, missed,
or double-counted, whereas the mode conveys ownership of the terminal
and must not be reordered. Two pieces of shared state, two orderings,
for a reason.

**What this can and cannot see.** A *soft* stall — the commit still
looping but not advancing — is what the counter detects. A *hard* stall
— the main thread gone into code with no instrumentation — is invisible
to the counter, but is still displayed, because the painter keeps
running and simply reports the stale value. Both end up steady-on. The
counter's value is that it distinguishes them in the trace, and that it
feeds S8's percentage.

### S7. Where the disarm points go — second pass

**Deferred. This section is to be filled before implementation
begins.**

The commit is not a flat loop. `render_overrides` has five phases, and
they are not equally instrumentable:

| phase | shape | instrumentable? |
|---|---|---|
| `compute_descend_marks` | flat walk over the arena | yes, trivially |
| `override_batch_refusal` | O(1) | not needed |
| `render_overrides_inner` | recursive, ~7 772 visits on the reference corpus | yes, per visit |
| `splice_override` → `render_node_as` | calls into `prototext-core` | **no** — opaque, no callback |
| `splice_override` → `build_tree` | O(n) over the new spans | **no** as written |
| `splice_override` → the push loop | flat | yes |
| `finalize_override_batch` | three passes over ~5.3 M line-map entries | yes |

The second pass must produce, for each phase: where the counter can be
bumped, what denominator (if any) is available there, and what it costs
to bump it.

It must also answer the uncomfortable part. On the reference corpus a
root retype spends most of its wall time inside `render_node_as` and
`build_tree`, neither of which has an instrumentable point. Left as is,
the dot pulses through the cheap phases and holds steady through the
expensive one — the inverse of what is useful. The options are: give
`prototext-core` a progress callback (crosses the crate boundary, and
`docs/prototext/` scope discipline applies — see N5), split that
stretch protolens-side, or ship knowing the slowest phase is mute and
say so in the spec.

Until this section is filled, the mechanism is implementable and
testable, but **G4 is not met in the case that matters most**.

### S8. The banner and the percentage

**The banner** is a fixed message rendered by main into the command row
in the entry frame — e.g. `Applying override…`. It is drawn once,
through ratatui, in the ordinary way. It requires no instrumentation
and is therefore correct even if S7 is never filled. This is what
subsumes spec 0204.

**The percentage** is the painter's six-column right-hand field, fed by
the counter's denominator from S7. Until S7 lands the painter leaves
those six cells untouched; they are saved and restored regardless, so
lighting them up later is additive and changes nothing else.

A percentage alone would not be enough, which is why the pulse is the
primary signal: on a large commit a number that moves 0.1% per second
is visually indistinguishable from a frozen one. The pulse carries fast
liveness; the number carries slow completion. They report different
facts at different cadences.

The denominator question — one global 0-100% across five phases with
unrelated units, versus a phase label plus a within-phase percentage —
is deferred with S7. A single global number will be non-monotonic or
badly non-linear unless the phases are weighted.

### S9. Shutdown, panic, and resize

**Shutdown** is already correct and needs no change:
`reader.shutdown()` (`mod.rs:2380`) joins the input thread *before*
`restore_terminal()` (`mod.rs:2383`). The comment there already states
that joining is what stops a thread writing to the terminal after
restore; that guarantee now covers the painter too.

**Panic.** `run` installs a hook that restores the terminal. The
painter must not write after that. It checks the stop flag under the
same lock it writes with, so a restore cannot interleave with a write
in progress.

**Resize during a commit.** The main thread cannot process the resize
event, so the recorded region geometry goes stale: the painter may
write into the wrong cell, and main's restore may put the saved cells
back in the wrong place. Both are cosmetic and self-healing — a resize
makes ratatui's next draw a full redraw (`autoresize` resizes the
buffers and clears), which overwrites the whole row regardless of what
either thread left there. Not worth handling.

### S10. What survives spec 0205

Spec 0205 moves the whole `handle_key` dispatch to a worker thread and
keeps the event loop turning throughout. Under that design the thread
that draws is never blocked, so every mechanism here that exists
*because* it is blocked disappears:

- **S2** — no painter, and no reason for one; the overlay is drawn by
  the ordinary `Terminal::draw` path.
- **S3** — no mode lock; 0205's ownership handoff is the whole
  synchronization.
- **S4** — no crossterm bypass, no seven-cell save and restore, no
  region; a normal frame composites the overlay onto 0205 S2's buffer
  snapshot.
- **S9** — no cross-thread terminal-ownership dance.

These should be **deleted**, not kept as a second path.

What survives verbatim is the determination half plus the visual
vocabulary: the `progress` counter and its disarm points (S6, S7) —
the type may change, the sites do not — the flash cadence and color
(S5), the banner and the percentage (S8), and the separation of
determination from display that motivates all of it. 0205 S4 already
assumes something of this shape: it pairs a fraction with a spinner
"so it keeps moving even when the fraction does not", which is this
spec's pulse under another name, and its `BatchProgress` atomics are
this spec's counter.

Implement S7 in a way that does not assume the reader is on the same
thread.

**These are alternatives, not a sequence.** 0209 is the cheap one: no
`App: Send` handoff, no `run_loop` test harness, no quit-during-commit
semantics. 0205 is strictly more capable — it also buys resize, quit,
and buffered input — and is a much larger change. Doing both means
writing S2/S3/S4/S9 and then removing them. Three specs now claim to
subsume 0204 (0205 S3, and this spec's S8); pick one before starting
any of them.

## Test plan

1. **Mode transitions.** `render_overrides` raises the mode on entry
   and lowers it on exit, including on the early-return path when
   `override_batch_refusal` refuses the batch (spec 0202). A commit
   that panics must not leave the mode raised.
2. **Startup is harmless.** The `mod.rs:1718` call with no painter
   spawned raises and lowers the mode without touching a terminal.
3. **No nesting.** The `debug_assert!` fires if a transition is raised
   while already raised.
4. **The flash is a pure function.** `paint_step`'s decision —
   (mode, start instant, now, counter value, last observed value,
   last written glyph) → (glyph to write, or nothing) — is a free
   function with no I/O, tested directly: on/off at 600 ms boundaries,
   drift-free across a skipped tick, steady after `STALL_TIMEOUT`, and
   resuming when the counter moves again.
5. **Four of six ticks write nothing.** Stepping the pure function
   across one 1.2 s cycle at 200 ms yields exactly two writes.
6. **Save and restore round-trip.** The seven cells copied out of the
   entry `CompletedFrame` are exactly what main writes back at exit,
   after an arbitrary number of intervening painter writes. Asserted
   against a fake writer and a `TestBackend`, not a real terminal.
7. **Normal operation is untouched.** Every existing bottom-row and
   activity-dot buffer assertion (`heat_cue.rs` and friends) still
   passes unmodified, and no `Layout` constraint changes — this is the
   check that N4 and "nothing is reserved" are both being honored.
8. **No lock in the frame path.** A test that `run_loop`'s draw path
   never acquires the paint lock while `Idle`. If that is awkward to
   assert directly, a counting wrapper around the mutex is acceptable.
9. **End-to-end, manual.** The pty driver from
   `docs/protolens/design/arena-and-batch.md`: open `googleapis.desc`
   against itself, retype the root, and observe the pulse for the
   duration. Record which phases pulse and which hold steady — this
   doubles as the input to S7's second pass.

## Open questions

1. **Does steady-on need its own color?** Once flashing stops, the dot
   is visually identical to a `User`-tier heat dot. The banner
   disambiguates it, but a distinct hue may be worth the extra
   constant.
2. **Should the painter own the dot continuously?** It would retire
   `activity_shown` and `ACTIVITY_TICK` and fix spec 0191 S2's
   sampling artifact at the source. It would also take the dot out of
   ratatui's buffer, which no existing test could then assert on. N4
   defers this; the trade should be revisited once S7 is settled.
3. **Is `STALL_TIMEOUT = 2 s` right?** It must exceed the longest gap
   between two disarm points, which S7 will measure. Two seconds is a
   placeholder.
4. **Six columns for the percentage** assumes a form like ` 100%`. If
   S7 produces a phase label instead, the reservation grows and the
   entry frame's layout narrowing grows with it.
