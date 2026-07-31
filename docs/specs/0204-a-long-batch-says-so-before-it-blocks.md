<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0204 — a long batch says so before it blocks

Status: draft — and contested. Two later drafts each claim to replace
        it: spec 0205 S3 (a measured 150 ms trigger instead of this
        spec's predicted one) and spec 0209 S8 (the same banner, plus a
        pulse). None of the three is implemented. Pick one before
        starting any of them.
Implemented in: —
App: protolens
Refs: docs/specs/0118-protolens-recursive-override-rendering.md (§6,
        every activation triggers the recursive pass),
      docs/specs/0160-protolens-render-overrides-batch-scaling.md (the
        batch),
      docs/specs/0190-the-activity-dot-reports-the-highest-live-tier.md
        (the activity dot, and why it cannot cover this),
      docs/specs/0202-an-override-is-refused-rather-than-fatal.md (the
        status line as a channel; itself since superseded by spec 0216),
      docs/specs/0205-the-batch-runs-off-the-input-thread.md (S3, the
        better trigger),
      docs/specs/0209-a-long-commit-keeps-a-pulse.md (S8, the banner
        that subsumes this spec)

## Background

Reported by the user: committing an override with a large scope stalls
protolens for about four seconds "without even an activity-dot
firing". Measured on `googleapis.desc` doubled, from a single batch's
trace:

```
36.2734  batch start   tree=4501014 marks=7772
36.3488  first splice
40.9445  last splice
42.0195  batch end
```

The splice loop is ~4.7 s of the ~5.7 s; `finalize_override_batch`'s
tail is the remaining ~1 s.

The absence of the dot is not a bug in the dot. `render_activity_dot`
reads `App::activity_shown`, which `run_loop` derives from
`heat_activity()` — and that reads the *heat worker's* activity byte.
The dot reports the background scoring thread and nothing else. The
override batch runs on the UI thread, inside `handle_key`, so while it
is running `run_loop` is not executing at all: it cannot sample
anything, cannot reach `terminal.draw`, and cannot service the input
reader. There is no mechanism by which the current design could show
anything. The screen is not merely un-updated — it is the frame drawn
*before* the keystroke, so it still shows the pre-override document.

Two things are wrong here, and they are separable:

1. protolens looks hung.
2. protolens *is* blocked for four seconds.

This spec addresses (1) only. It is worth doing on its own because it
is small, it carries no risk to the batch, and "it is working" is most
of what a user needs in order to wait. (2) is a much larger change and
is specified separately.

### Why the batch is not simply deferred

The obvious shape — have the key handler record "a batch is pending",
let `run_loop` draw, then run the batch at the top of the next
iteration — was designed and rejected. It reads well and it is the
scaffolding a background batch would want, but it is not safe at the
call sites as they stand.

`render_overrides` is called from sixteen places. Three of them do
work *after* the call that depends on what the call did:

- `key_dispatch.rs:186` — the `Enter` that commits an override, i.e.
  precisely the path being complained about. The batch can auto-seed
  brand-new entries (Any/MessageSet auto-expansion), which re-sorts
  the whole collection; the follow-up code then locates the entry by
  origin/type to land the management pane on it. Run before the batch,
  it finds the wrong entry or none.
- `manage_pane.rs:604` — the same re-sort hazard, called out in its own
  comment, with an index repair after the call.
- `manage_pane.rs:656` — sets `self.message` after the call, which a
  deferred batch would then overwrite.

Deferral would therefore need to carry a continuation per call site.
That is a real state machine, added for a cosmetic improvement, in the
same code path as an unbounded-arena bug that is already being fixed
under separate cover. Not worth it.

## Goals

- **G1.** Before a batch that will visibly block, the screen says
  protolens is working, so a four-second wait reads as work rather
  than as a hang.
- **G2.** No risk to the batch: no call site changes, no reordering of
  existing work, no new state carried between events.
- **G3.** The banner never appears for work that would have been
  imperceptible anyway.

## Non-goals

- **N1.** Removing the stall. Moving the batch off the UI thread is
  what actually does that, and it is the follow-on spec. This one
  removes the *silence*, not the wait.
- **N2.** Progress *within* a batch — a percentage, a count, a moving
  dot. The batch owns the thread for its whole duration, so no frame
  can be drawn from inside it. Requires N1 first.
- **N3.** Cancelling a batch in flight.
- **N4.** Making the batch faster.
- **N5.** Covering the startup batch. There is no event loop and no
  terminal frame yet at `App::new` time; startup already has its own
  splash.

## Specification

### S1. One frame, drawn before the handler runs

`run_loop` is the only place holding the terminal, so this lives
entirely in its `Key` dispatch arm and nowhere else — which is what
buys G2.

Before `app.handle_key(key)`, when S2 says the keystroke could start a
perceptible batch: save any message currently showing, set
`app.message` to the banner, `terminal.draw`, then restore the saved
message before dispatching.

Clearing the banner *before* the handler runs is the point, not an
oversight. The frame already went to the terminal, so the banner stays
visible for exactly as long as the batch blocks; and because
`app.message` no longer holds it, the frame drawn after the batch
shows whatever the batch itself has to say — including spec 0202's
refusal, which must not be clobbered.

The banner does not interact with `message_deadline`: it is never live
in `app.message` at the point where the loop evaluates that deadline.

### S2. When it fires

Two conditions, both O(1):

**The document is big enough to stall.** `tree.len() >= 250_000`. This
is a proxy and is labeled as one: a batch's cost is proportional to
what it splices, not to the arena, so no arena-derived number can
predict it (spec 0202 S2 covers why predicting it properly is not
available). The threshold's job is only to keep the banner off the
small documents where a batch completes in a frame and the banner
would be a flicker. It is calibrated between the two measurements on
hand — 20 ms of batch work at 382 k nodes (spec 0188), 4.8 s at 4.5 M
— and rounded down, because being early is free.

**The keystroke can change the active override set**, given which pane
has focus:

| focus | keys |
|---|---|
| override selection pane | `Enter` |
| management pane | `Space`, `Shift+Space`, `a`, `d`, `x`, `Enter` |
| command line | `Enter` |

### S3. The predicate is a hint and must stay one

Being over-inclusive costs one frame — a draw was measured at ~2 ms,
against a batch of thousands of milliseconds. Being under-inclusive
costs nothing at all: the result is today's behavior for that key.

So this table is allowed to drift out of step with the dispatch it
mirrors, and nothing may come to depend on it. It must not gate any
work, short-circuit any handler, or be consulted for anything but
whether to draw one extra frame. Stated here because a cheap, mostly
accurate predicate sitting next to a dispatcher is an inviting thing
to reuse, and the moment it is load-bearing its inaccuracy stops being
free.

### S4. Mouse

The management pane's entry-marker click (`manage_pane.rs:791`) runs a
batch too, and gets the same treatment in the `Mouse` arm. Other mouse
paths are not covered — deciding what an arbitrary click will do means
resolving hit-testing before dispatch, which is the kind of
duplication S3 just forbade. A limitation, recorded rather than
hidden; N1 removes the need for any of this.

## Test plan

`run_loop` has no test harness, so the frame itself cannot be asserted
end to end. What is testable is the decision and the message handling:

1. `the_banner_is_silent_on_a_small_arena` — the predicate is false
   below the threshold for every key in the table.
2. `the_banner_covers_every_key_that_can_start_a_batch` — a table test
   per focus, mirroring S2, so that a binding change that adds an
   override-activating key fails here rather than silently regressing
   to a hang-looking stall.
3. `the_banner_does_not_outlive_the_frame_it_was_drawn_for` — an
   existing message is restored, and an absent one stays absent, so
   the post-batch frame is free for the batch's own message.

## Measured outcome

(to be filled in)
