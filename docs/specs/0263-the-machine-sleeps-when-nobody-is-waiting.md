<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0263 — the machine sleeps when nobody is waiting

Status: implemented
Implemented in: 2026-08-09
App: protolens
Refs: docs/specs/0152-protolens-heat-cue-background-scoring-thread.md
        (G8: the input-reader thread and the one channel `run_loop`
        waits on),
        docs/specs/0190-the-activity-dot-reports-the-highest-live-tier.md
        (S7: the activity byte and the 250 ms tick that re-examines it —
        this spec makes that tick conditional),
        docs/specs/0191-the-read-ahead-walk-is-bounded-and-the-activity-dot-stops-flickering.md
        (S3: the two-window sliding maximum, which is where this spec's
        hysteresis already lives),
        docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md
        (G10: a `Prefetch` completion writes its cache entry and wakes
        nobody — the reason the sleep condition cannot be "the queue is
        empty")

## Background

Left alone with nothing to do, protolens wakes **nine times a second,
forever**, from two threads.

**Four of them are `run_loop`'s.** `ACTIVITY_TICK` (250 ms,
`terminal.rs:44`) is an unconditional candidate deadline, so the receive
at the foot of the loop is always a `recv_timeout` and never a bare
`rx.recv()`. The code already says so at `terminal.rs:687`:

> Because it is always present the receive below is always a
> `recv_timeout` and never a bare `rx.recv()`: the loop wakes four times
> a second forever rather than ever being genuinely idle.

**Five of them are the input reader's.** `event.rs:96` loops on
`event::poll(INPUT_POLL_INTERVAL)` with a 200 ms interval. The interval
catches no keystrokes — a real keypress wakes `poll` immediately — it
exists solely so the thread periodically re-reads its `stop` flag and
stays reachable.

Everything else already sleeps properly, and this spec does not touch
any of it: the heat workers block on a `Condvar` (`heat_worker.rs:685`);
`search_sweep_step` / `discard_step` / `bake_step` / `prefetch_step` all
report `Idle` and are driven only from `run_loop`; the message and
splash deadlines are transient by construction.

The cost is not the work done per wake — two relaxed atomic loads and a
comparison on one thread, one `poll` syscall on the other. It is that a
CPU woken ten times a second never reaches its deep idle states, which
is where the power actually goes.

## Goals

- **G1.** After a short settle, an unattended protolens performs **zero
  timed wakeups on any thread**. Every thread is blocked on something a
  human or the terminal has to produce.
- **G2.** Nothing on screen is left stale by the sleep. Any state that
  can still change either has an event that announces it, or the loop
  does not sleep.
- **G3.** The input reader's shutdown latency does not get worse, and
  the Neovim handoff still gets the terminal entirely to itself.

## Non-goals

- **N1.** Not merely lowering the tick rates. A longer interval reduces
  wakeups without ever reaching zero, and every millisecond added to
  `INPUT_POLL_INTERVAL` is added to the wait in the Neovim handoff.
- **N2.** Not Windows. The reader half is `cfg(unix)`; other platforms
  keep today's 200 ms poll. protolens' neighbouring terminal handling
  (suspend, editor handoff) is already unix-only.
- **N3.** No user-visible setting. There is no case for choosing to burn
  power while idle.
- **N4.** Nothing about the heat workers, which already block correctly.

## Specification

### The main loop

- **S1.** The choice between a timed and an untimed receive is made at
  the **one place the loop genuinely sleeps** (`terminal.rs:816-819`),
  not where `deadline` is computed. That point is reached only after all
  four background steps have reported `Idle` in the same pass, so the
  decision is local and cannot disturb any other path:

  ```rust
  if may_sleep_indefinitely { break rx.recv().ok(); }
  break rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).ok();
  ```

  `deadline` itself, and the `Instant::now() >= deadline` checks inside
  the progressing arms above, are untouched.

- **S2.** `may_sleep_indefinitely` is a free function over six inputs,
  so it can be tested without a loop:

  ```
  ui_deadline.is_none() && !heat_dirty && !styles_stale && !bake_dirty
      && !bake_visible && activity_settled
  ```

  with `activity_settled` = `activity_prev_window`, `activity_window`,
  `app.activity_shown` and `app.heat_activity()` all `None`.

- **S3.** The last of those four terms is `heat_activity()` and **not
  "the request queue is empty"**. A `Prefetch` completion deliberately
  sends no event (`heat_worker.rs:1213`, spec 0164 G10), so a part still
  in flight can write a cue into the cache with nothing to announce it;
  sleeping on an empty queue would strand that cue until the user
  touched something. `activity()` folds `queued | in_flight` in two
  relaxed loads and is exactly the predicate G2 needs.

- **S4.** **The hysteresis is already there and needs no new constant.**
  `activity_shown` falls to `None` only after two consecutive quiet
  windows (spec 0191 S3), each window one iteration bounded by the
  250 ms tick. So the loop cannot reach the sleeping state until roughly
  half a second after the last work finished, and the frame that shows
  the activity dot going dark is drawn *before* the sleep rather than
  being owed after it.

### The input reader

- **S5.** Per iteration, the reader (a) drains everything crossterm has
  already parsed with `event::poll(Duration::ZERO)`, then (b) blocks in
  `poll(2)` with **no timeout** on three descriptors, and (c) loops when
  woken by the first two, exits on the third or on an error.

  The three are the terminal's, a `SIGWINCH` pipe, and a stop pipe.

- **S5a.** **The `SIGWINCH` pipe is not optional.** crossterm's unix
  source waits on two descriptors, not one (`mio.rs:46-49`): the tty,
  and a `signal_hook` pipe it uses to synthesize `Event::Resize`. A
  resize makes *that* descriptor readable and leaves the tty alone, so a
  reader polling only the tty would sleep through every resize until the
  next keystroke. `EINTR` does not save it: `SIGWINCH` is
  process-directed and the kernel may deliver it to any thread.

  protolens therefore registers **its own** `signal_hook` pipe for
  `SIGWINCH`. Registrations for one signal chain, so crossterm's
  notification still fires, and the drain at the top of the next pass is
  what turns it into the `Resize` event. Installing a raw `sigaction`
  handler instead would *replace* crossterm's and stop resize events
  reaching the app at all.

- **S6.** `shutdown()` writes one byte to the write end, then joins.
  Shutdown becomes immediate instead of up to 200 ms late, which is a
  direct improvement to G3.

- **S7.** **The drain in S5(a) is required, not an optimization.**
  crossterm reads up to 1 KiB from the terminal in one go, parses every
  event it can out of that chunk into an internal queue, and returns one
  (`mio.rs:69`, `Parser::advance`). After such a read the descriptor is
  *not* readable while events remain buffered. Blocking in `poll(2)`
  without draining first strands them until the next keystroke — which
  for a paste or a wheel burst means visibly lost input.

- **S8.** **Poll the descriptor crossterm reads, not fd 0.** crossterm
  uses stdin when `isatty(STDIN_FILENO)` and opens `/dev/tty` otherwise
  (`file_descriptor.rs:124`). protolens must make the same choice with
  the same test, or it will sleep on a descriptor nobody writes to —
  protolens can be handed its blob on stdin, so this is reachable.

- **S9.** `SIGTTIN` is unaffected. The hazard documented at
  `terminal.rs:981` is that a *backgrounded* process **reading** its
  controlling terminal draws `SIGTTIN`, whose default disposition stops
  every thread in the process. `poll(2)` does not read and does not
  raise it; only `event::read()` does, and S5(a) runs it only while the
  reader is alive. The handoff still shuts the reader down before
  yielding the terminal, and S6 makes that shutdown prompt rather than
  up to a poll interval late.

- **S10.** `#[cfg(unix)]` for the pipe/poll body, `#[cfg(not(unix))]`
  for today's timed one. `nix` gains its `poll` feature (already a unix
  dependency, `protolens/Cargo.toml:42`) and `signal-hook` becomes a
  direct unix dependency — it is already in the lockfile at 0.3.18 via
  crossterm, so this pulls in no new crate.

- **S11.** Setup failure (fd exhaustion, no controlling terminal) falls
  back to the timed loop rather than aborting. A session that wakes five
  times a second is a power problem; a reader that cannot be stopped is
  a broken Neovim handoff.

## Alternatives considered

**Blocking `event::read()`, detaching the thread instead of stopping
it.** One line. Ruled out by the Neovim handoff (`terminal.rs:977`):
the outgoing reader is still blocked on the terminal when the respawned
one starts, so keystrokes split between two threads at random. It trades
a power problem for a correctness bug.

**Lengthening `INPUT_POLL_INTERVAL`.** Never reaches zero — the thread
still wakes, just less often — and pushes shutdown latency straight into
the handoff, which is the one path that must not stall.

**Making the activity dot event-driven**, so the worker announces its
own fall to idle and `ACTIVITY_TICK` could be deleted outright. Rejected:
it puts one more event per completed sweep on the channel that keystrokes
queue behind, in order to remove a deadline that S1's condition removes
for free. Spec 0190's reason for a flat tick — that queue events are far
too frequent to arm a timer per event — applies here unchanged.

**A dedicated idle-settle constant.** Rejected by S4: the settle already
exists, is load-bearing for the dot, and a second one would only have to
be kept consistent with it.

## Test plan

1. `may_sleep_indefinitely_only_when_nothing_is_owed` — table over S2's
   six inputs; each one alone must veto the untimed receive.
2. `a_worker_still_in_flight_keeps_the_loop_on_a_timer` — S3, driven
   through a real `HeatWorkerHandle` with a request in flight and an
   empty queue, which is the state the "queue is empty" predicate would
   have got wrong.
3. `an_expiring_message_still_dismisses_itself_with_no_events` — G2, and
   the regression this spec most plausibly causes.
4. `an_event_wakes_the_sleeping_loop` — settled loop, one event sent
   from another thread, one frame drawn.
5. `the_reader_stops_at_once_when_asked` — tighten the existing
   `spawn_and_shutdown_round_trip_within_a_bounded_timeout` from three
   poll intervals to a few milliseconds. It is the direct evidence for
   S6.
6. `the_reader_delivers_every_event_of_a_chunk` — S7, over a pty,
   writing a multi-event burst in one `write` and asserting every event
   arrives without a following keystroke to flush them.

Measurement for G1: `voluntary_ctxt_switches` and
`nonvoluntary_ctxt_switches` from `/proc/<pid>/status`, sampled ten
seconds apart on an untouched session, before and after. Expected before
≈ 90 across the two threads per 10 s (4/s + 5/s); expected after 0.

## Measured outcome

**G1 met exactly.** `descriptor.pb`, 200x50 pty, five threads, sampled
ten seconds apart after a twelve-second settle, pinned to `-c 4-7`:

| | voluntary | non-voluntary |
|---|---|---|
| before (`edfe932`) | **90** / 10 s | 0 |
| after | **0** / 10 s | 0 |

90 is 9/s to the switch — the 4/s tick and the 5/s poll the Background
section predicted, with nothing else hiding behind them. Zero after
means no thread in the process performs any timed wakeup at all: not
the loop, not the reader, not the heat workers.

The two behaviors a sleeping app could plausibly have lost were checked
end to end on a fully settled session (two seconds of provably zero
output first, so the app really was asleep): a resize repaints (5128
bytes, S5a) and a keystroke repaints (730 bytes). Both without any
other input to wake it.

### What shipped differently

- **The drain's failure path is a back-off, not a fall-through** (an
  addition to S5/S7). If `event::poll` cannot say what is on the
  terminal, the descriptor stays readable *because* those events were
  not collected — so blocking in `poll(2)` would return immediately and
  forever. That is a spin, which is the exact opposite of this spec.
  The reader instead sleeps one `INPUT_POLL_INTERVAL`, which costs the
  five wakeups a second the timed loop always cost, lets a transient
  failure clear, and cannot peg a core.

- **Test 4 is not a test of its own.** `a_progressing_bake_forces_a_repaint`
  already runs an idle loop as its control and asserts it draws exactly
  one frame before returning on a quit sent from another thread — which
  *is* "an event wakes the sleeping loop". A second test would have
  asserted the same thing over a different fixture; the control's
  comment now names this spec instead.

- **Test 5 is conditional on which wait it got.** The fallback (S11) is
  reachable in ordinary test environments — a build sandbox has a
  `/dev/tty` with no controlling terminal behind it — and a reader that
  fell back is entitled to its old, slower answer. The handle therefore
  reports which wait it has, so the fast bound is an assertion rather
  than a coin flip.

- **Test 6 re-execs the test binary.** The terminal a process reads is
  a property of the process, not of a thread, so the reader under test
  runs in a child with its stdin bound to a pty. That makes it also the
  one test that certainly exercises the untimed path (S8: stdin is a
  tty), so it asserts on that too. It passes in the Nix sandbox as well
  as in a terminal.

- **`bake_visible` cannot in fact be set at the one call site** — the
  arm that raises it leaves the receive loop immediately, and the frame
  it forces clears it again. It is a term in `may_sleep_indefinitely`
  anyway: the argument for leaving it out is a property of a control
  flow three screens away, and the cost of keeping it is one `&&`.
