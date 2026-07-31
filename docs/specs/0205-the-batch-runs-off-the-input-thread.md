<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0205 — the batch runs off the input thread

Status: draft — and contested. Specs 0204, 0205 and 0209 are three
        proposals against the same stall; none is implemented. Pick one
        before starting any of them. Parts of the Background and S7
        below rest on premises spec 0216 has since removed — the
        annotations mark which.
Implemented in: —
App: protolens
Refs: docs/specs/0152-protolens-heat-cue-background-scoring-thread.md
        (G8, the input-reader thread and the event channel this
        reuses),
      docs/specs/0160-protolens-render-overrides-batch-scaling.md (the
        batch),
      docs/specs/0167-protolens-render-overrides-deferred-line-splice.md
        (why `lines` is untouched until the batch ends),
      docs/specs/0202-an-override-is-refused-rather-than-fatal.md (the
        guard, which must stay on the input thread — since deleted,
        see S7),
      docs/specs/0203-the-override-arena-is-compacted.md (the
        compaction pass, which rides along on the worker — since
        deleted, see S7),
      docs/specs/0204-a-long-batch-says-so-before-it-blocks.md (the
        stop-gap this supersedes),
      docs/specs/0209-a-long-commit-keeps-a-pulse.md (the third
        proposal; its N2 defers to this spec, its S10 records what of
        it survives this one),
      docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (which
        removed the memory pressure this spec's Background argues
        from)

## Background

A document-wide override batch blocks protolens for ~4.8 s. Spec 0204
puts a static banner on the screen before it starts, which is enough
to stop it reading as a hang but is otherwise inert: no progress, no
resize, no way out.

The reason nothing can be shown from inside the batch is structural.
`render_overrides` is called from `handle_key`, which `run_loop` calls
directly, so for the batch's whole duration the event loop is not
executing: it cannot draw, cannot sample, cannot service the input
reader. This spec moves the work off that thread.

### What this can and cannot buy

This needs stating up front, because the obvious expectation —
"protolens stays usable while the override applies" — is not
achievable and is not what this spec delivers.

A batch mutates essentially the entire document model: `tree`,
`lines`, `descend`, `heat_states`, `folded`, `cursor`, and `overrides`
itself (auto-seeding Any/MessageSet entries re-sorts the collection).
There is no meaningful subset the UI could read concurrently, so:

- a lock would be held for the batch's entire duration, making the
  reader wait exactly as long as it waits today;
- a snapshot means copying the model — 2 GiB on the reported document.

(As written this list also named both line-index maps and
`visible_rows`; spec 0210 has since deleted all three. The argument is
unaffected — what remains is still the whole model — but do not read
the list as a current inventory.)

So the document is unavailable while the batch runs, under any design.
What moving the work off-thread does buy is that the *process* stays
alive and controllable:

- a progress indicator that actually moves;
- terminal resize handled while the batch runs;
- quit honored immediately instead of after ~5 s;
- keystrokes buffered rather than dropped.

That is a genuine improvement over spec 0204's static banner, and it
is also considerably less than "keep working during the batch". Judge
the spec against the former.

### Why the unit of work is the whole dispatch

The natural framing — "move `render_overrides` to a worker" — does not
survive contact with the call sites, for exactly the reason spec 0204
recorded: three of the sixteen callers do work *after* the call that
depends on what the call did, `key_dispatch.rs:186` (the `Enter` that
commits an override) among them.

Moving the whole `handle_key` call instead makes that a non-issue.
The post-batch work runs on the worker too, in its original order,
with no call site touched at all. `render_overrides` and its sixteen
callers are not modified by this spec.

`App: Send` is a precondition of that, and was verified by compiling a
`assert_send::<App>()` probe against the current tree — it holds
today, with no change required. Note that the terminal is owned by
`run_loop`, not by `App`, which is what makes the handoff possible.

### Why not chunk it on one thread instead

A cooperative version — run the batch in slices, returning to the
event loop between them — needs no threads and was considered first.
It is the more dangerous option, not the safer one.

`render_overrides_inner` is a recursive walk carrying `path`, depth,
the inherited shift correction and `patch_scope` down with it.
Suspending it means rewriting that recursion as an explicit resumable
state machine, in the most delicate code in the application, in the
same release as an arena rewrite.

And it would not deliver interactivity anyway. Between slices the
model is mid-batch: `pending_line_patches` is partial, `pending_shift`
is partway accumulated, and some subtrees carry new spans while their
siblings still carry old ones. The line buffer itself happens to stay
coherent — spec 0167 defers every write to `lines` to the end of the
batch, so it still holds the pre-batch document throughout — but
anything resolved through `tree` (cursor, folds, heat cues) would be
reading a half-updated arena. So a slice-based design still has to
refuse input and still has to be modal, having paid for a state
machine to get there.

## Goals

- **G1.** The event loop keeps running for the duration of a batch:
  it draws, it services the input reader, and it handles resize.
- **G2.** A batch shows progress that visibly advances.
- **G3.** Quit is honored while a batch is running.
- **G4.** No `render_overrides` call site changes, and no reordering
  of any existing work.
- **G5.** A batch that is fast enough not to matter is
  indistinguishable from today — no flicker, no overlay, no extra
  frame.

## Non-goals

- **N1.** Using the document during a batch. See above; it is not
  achievable without either a full-duration lock or a full copy.
- **N2.** Cancelling a batch in flight. A splice mutates the arena in
  place and there is no undo; aborting midway would leave a document
  that is neither the old one nor the new one. The batch is
  interruptible for *display* purposes only.
- **N3.** Making the batch faster, or reducing what it splices.
- **N4.** Moving any other work off-thread. The heat worker already
  has its own thread (spec 0152); nothing else here changes.
- **N5.** Concurrency between batches. Exactly one dispatch is in
  flight at a time, by construction — the worker holds `App`.

## Specification

### S1. `App` is lent, not shared

A single long-lived worker thread and two channels. `App` lives on the
input thread as it does today and is *moved* to the worker for the
duration of one dispatch, then moved back. Ownership is the whole
synchronization mechanism: no lock, no `Arc<Mutex<App>>`, no
possibility of the two threads touching the model at once.

```
input thread                      worker thread
  send (App, event)        ──►      app.handle_key(event)
  ... draw, poll input ...          ...
  recv App                 ◄──      send App back
```

While the worker holds `App`, the input thread has no access to it —
enforced by the compiler, not by discipline.

Every event goes through this path, not just the ones that might be
slow. A uniform path is the point: there is no predicate to keep in
step with the dispatcher, which is what spec 0204's S2/S3 had to
introduce and what this spec deletes. The per-event cost is two
channel sends on an already-warm thread, microseconds against the
16 ms frame budget.

### S2. What the screen shows meanwhile

Before handing `App` over, the input thread keeps a clone of the last
drawn `Buffer` (a few tens of thousands of cells — trivial next to the
model). While waiting, each progress tick redraws that snapshot with
the progress overlay composited on top.

The user therefore keeps seeing the document exactly as it was when
they pressed the key — which is what they see today — with a live
overlay added, rather than a blank or a modal panel. Re-emitting the
snapshot in full is required, not optional: ratatui resets the back
buffer every frame, so a cell that is not re-emitted is erased, not
preserved (the same constraint spec 0190 S8 documents).

### S3. The overlay appears only when the wait is real

The input thread waits with `recv_timeout`. The overlay is drawn only
once the dispatch has been outstanding longer than `OVERLAY_DELAY`
(150 ms); below that the App comes back and the loop proceeds as
today, having drawn nothing extra.

This is the part that supersedes spec 0204 outright. The trigger is
"this has actually been slow", measured, rather than "this key might
be slow", predicted from a table that mirrors the dispatcher and can
drift out of step with it. It needs no knowledge of what any key does,
and it cannot be wrong.

### S4. Progress is approximate, and says so

`App` carries an `Arc<BatchProgress>` (two atomics: visited, total),
cloned by the input thread before the handoff so it remains readable
while the worker owns `App`. `compute_descend_marks` publishes the
mark count as the total; `render_overrides_inner` bumps visited once
per visit.

The fraction is a genuine approximation and the overlay must not
imply otherwise. Marked nodes are counted, but cost is proportional to
nodes *spliced*, and those are wildly uneven — on the reported
document a single splice at `/5767` is 260 944 nodes out of 4 499 335,
so the bar sits still for a noticeable fraction of the batch while one
subtree re-decodes. The overlay shows a spinner alongside the fraction
for that reason: the spinner is driven by the tick, so it keeps moving
even when the fraction does not, which is precisely the information
the user needs (still working, not wedged).

Counting the marks is `O(arena)` and is only done when a total is
actually wanted; a batch that finishes before `OVERLAY_DELAY` never
pays for it.

### S5. Quit, resize, and buffered input

- **Quit** (`q`, `Ctrl-C`): honored immediately. The input thread
  restores the terminal and exits the process, abandoning the worker.
  Abandoning it is safe precisely because there is nothing to save —
  the model is being discarded anyway — and it avoids waiting ~5 s to
  quit an application that looks stuck.
- **Resize**: handled by the input thread. The snapshot is stale at
  the new size, so a resize during a batch clears the screen and draws
  the overlay alone until `App` returns; a resize is a rare event
  during a 5 s window and a correct-but-plain frame beats a
  misaligned one.
- **Other input**: left queued in the existing channel (spec 0152 G8's
  input reader keeps running throughout) and dispatched in order once
  `App` is back. Keystrokes are buffered, not dropped — same as today,
  where the OS buffers them for a thread that is not reading.

### S6. Panics

Today a panic inside `handle_key` unwinds through `run_loop` and the
terminal guard restores the screen. With the dispatch on a worker, the
panic kills the worker and `App` is lost with it — the receive fails
rather than returning a model.

That case must restore the terminal and exit with the panic reported,
which is the same *observable* outcome as today. What must not happen
is the input thread blocking forever on a channel whose sender has
been dropped: the receive has to treat disconnection as a fatal,
reported condition, not as a timeout to retry.

### S7. What stays on the input thread

- **Read-ahead** (`prefetch_step`) and the heat-cue rechecks. These
  need `App`, so they simply do not run while a batch is in flight —
  a 5 s pause in read-ahead during a batch that is about to
  invalidate the walk anyway costs nothing.

This section originally also had to place spec 0202's memory guard
(before the handoff, to keep a refusal instantaneous) and spec 0203's
compaction pass (at the end of `render_overrides`, so on the worker).
Spec 0216 deleted both, so neither needs placing.

## Test plan

`run_loop` still has no test harness, and this spec is largely about
`run_loop`. Building one is a prerequisite, not an optional extra, and
is the main cost of this change beyond the handoff itself.

1. `the_model_survives_a_round_trip` — `App` handed to the worker and
   back is unchanged for a dispatch that changes nothing, and shows
   the expected change for one that does.
2. `a_fast_dispatch_draws_no_overlay` — below `OVERLAY_DELAY`, the
   frame count for a keystroke is what it is today.
3. `a_slow_dispatch_draws_the_overlay_and_keeps_ticking` — a fixture
   batch held artificially long produces multiple frames with an
   advancing spinner.
4. `quit_during_a_batch_exits_without_waiting`.
5. `a_panicking_dispatch_restores_the_terminal` — the worker's death
   is reported, not hung on.
6. `progress_totals_are_published_before_the_first_visit` — the
   overlay never divides by a zero total.

## Open question

Which of the three drafts to implement. Spec 0204 is the smallest and
can ship first, but S3 here replaces its predicted trigger with a
measured one and deletes its key table, so doing both means writing and
testing 0204's predicate in order to remove it. Spec 0209 is the middle
option — it keeps the commit on the input thread and paints a pulse
from a second one, and its own S10 says most of it is deleted by this
spec. Decide before starting any of them.

## Measured outcome

(to be filled in)
