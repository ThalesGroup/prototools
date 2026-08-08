<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0255 — the document finishes itself while nobody waits

Status: implemented
Implemented in: 2026-08-08
App: protolens
Refs: docs/specs/0249-a-large-document-answers-the-user-first.md (S1's
        bounded render, S3's `auto_folded`, S5's unused policy, S8's
        expand-on-arrival, and S11, which this spec implements and
        partly overrides),
      docs/specs/0254-a-changed-count-travels-up-as-a-difference.md
        (what made a bake step small enough to run here),
      docs/specs/0250-the-machine-works-on-what-the-user-waits-for.md
        (the priority argument this spec extends to a third worker),
      docs/specs/0252-a-cue-nobody-is-looking-at-is-not-worth-a-sweep.md
        (why interleaving the bake with read-ahead is worse than
        either order)

## Background

Spec 0249 built a bounded render and then could not turn it on. S5 says
so in as many words: *"What is left is the policy — which callers ask
for a budget, and how big — because until the bake exists (S11) a
bounded confirm would leave the document truncated with nothing to fill
it back in."*

So `splice_override` takes a `row_budget` that exactly one production
caller passes (`expand_auto_fold`, on a keystroke), and confirming an
override on googleapis.desc still freezes for ≈3.6 s. **Nothing a user
can observe has changed since spec 0249 was opened.** That is the gap
this spec closes.

S11 required the bake to be off the event thread, because the worst
single expansion was 766 ms. Spec 0254 showed that number *was* the
quadratic ancestor re-sum, on the widest node in the document; it is now
~22 ms at every budget, with two steps over 8 ms out of 70 797 and none
over 50. A bake step is now small enough to run between two `try_recv`
calls, which is what makes this spec small.

## Goals

- **G1.** Confirming an override on googleapis.desc returns in ≈0.4 s
  instead of ≈3.6 s, with a first screenful that is complete and
  correct — not a placeholder.
- **G2.** The document completes itself with no further keystroke, and
  the completed document is byte-identical to what the unbounded
  confirm would have produced.
- **G3.** No keystroke waits longer than one bake step.

## Non-goals

- **N1. Not off the event thread.** S11 asked for a thread; ~22 ms
  steps make one unnecessary, and an owning thread for the arena and
  `node_text` is a design spec 0249 never wrote. If the idle arm proves
  too contended this becomes a thread later, behind the same
  `bake_step` seam.
- **N2. No per-slot fresh/stale bitset (S9).** The bake queue *is*
  `auto_folded`, which `splice_override` already maintains, so a queued
  entry cannot go stale behind the bake's back — see S3.
- **N3. Not S12's `Unbaked` status rank or S13's search remainder.**
  Both describe a partly-baked document to the user; this spec makes one
  exist. They follow.
- **N4. Not the ≈0.43 s floor.** What remains after the renderer drops
  out is two O(document) invalidation passes (the vacate loop and
  `overlay_spans`), and no budget reaches them. That is S9/S10.
- **N5. No memory saving.** A baked document holds exactly what an
  unbounded one holds. The objective is latency.
- **N6. Startup is not bounded.** `App::new`'s own override pass, and
  therefore `--type` and `--load-overrides` at launch, keep rendering
  unbounded. Bounding them is the same mechanism and a separate
  decision; doing it here would silently truncate `protolens … export`,
  which has no event loop to bake in.

## Specification

- **S1. Two budgets, because they are two different questions.** The
  confirm's budget answers "how much must exist before the user can
  read the screen", and is a screenful:
  `document_pane_height().max(MIN_EXPAND_ROWS)`. The bake's answers "how
  large a slice can the event loop afford to be blocked by", and is
  `BAKE_ROW_BUDGET = 5000` rows.

  They must not be the same number. Every bounded render re-emits its
  whole right frontier as folded rows (spec 0249 S1: output size =
  budget + breadth), and those rows are rendered *again* by the step
  that expands them, so a small budget pays for the frontier over and
  over: measured over a full drain of googleapis.desc, the bake's own
  render is 3.3x the unbounded render at budget 50 and 1.6x at 5000, and
  the drain is 419 723 steps against 70 797. 5000 is where the worst
  step is still ~22 ms.

- **S2. Only the interactive confirm is bounded**, via a
  `bounded_confirms` flag on `App` that `run_loop` sets on entry and
  nothing else sets. `resettle_node` reads it and passes
  `Some(screenful)` or `None`.

  A flag rather than a parameter threaded through
  `render_overrides_inner`'s recursion, and a flag rather than the
  implicit `main_area.height > 0` test: the implicit form is exactly
  right in production (only `render` sets `main_area`, and `App::new`'s
  pass precedes any frame) and quietly wrong in the render tests, which
  set `main_area` directly and would start bounding splices they never
  asked to bound. N6's export path gets `false` for free.

- **S3. The bake queue is `auto_folded`, plus a `Vec` for order.**
  `splice_override` already inserts every node a bounded render stopped
  at and removes any node it renders, so the set is the exact answer to
  "what still owes a body" at all times, maintained by the same code
  that creates the debt. The bake pushes nothing and invents nothing.

  What the set cannot give is a cheap next element:
  `HashSet::iter().next()` scans buckets from the top, and the set
  drains from ~84 000 entries to zero without shrinking its capacity, so
  the last steps of a bake would each scan the whole table. A
  `bake_queue: Vec<usize>` is appended alongside the same `stopped` loop
  and popped from the front.

  **The `Vec` is a hint, not the truth.** A pop is skipped unless
  `auto_folded` still contains it, which is what makes a duplicate
  entry, a node the user expanded by hand, and a node whose ancestor was
  re-overridden underneath it all harmless without a generation counter
  or a bitset. That is N2.

  Order is FIFO. The stops of one bounded render are emitted in document
  order, so FIFO bakes downward from the viewport — the direction a
  reader scrolls — while LIFO would dive to the bottom of the first
  subtree and work back.

- **S4. One `bake_step` does at most one `expand_auto_fold`**, at
  `BAKE_ROW_BUDGET`, and returns `Progressed`/`Idle` exactly as
  `prefetch_step` and `search_sweep_step` do. It is driven from
  `run_loop`'s idle chain, which already re-checks `try_recv` between
  steps, so G3 costs nothing extra.

- **S5. The order is search, then bake, then read-ahead** — and the
  middle position is the one finding of this spec that is not obvious.

  Read-ahead is *speculative* and the bake is *owed*, which is a weak
  argument on its own. The strong one is that the two cannot be
  interleaved at all. Every bake step splices, so it bumps
  `structural_version`, and `prefetch_step_inner` restarts its wave when
  that changes. Running read-ahead between bake steps therefore means:
  a full re-walk of up to `PREFETCH_WALK_MAX_ROWS` per bake step, three
  mutex acquisitions per bake step to reset the wave, and — worst —
  heat requests issued for the rows the bake has just materialized, none
  of which anyone is looking at and every one of which the *next* bake
  step supersedes. That is precisely the waste spec 0252 removed from
  the visible band. Read-ahead would make no net progress while paying
  full price for the attempt.

  So the bake is not preferred over read-ahead; read-ahead is deferred
  until the structure stops moving, which is the only state in which it
  can make progress. The bake is a finite queue, so this is a delay and
  not starvation. The visible rows are unaffected either way: `render`
  calls `heat_cue_for` per drawn row itself and does not go through
  read-ahead.

- **S6. The bake draws no frame per step.** By spec 0249 S1's
  depth-first rule the rows on screen after a confirm are already final,
  so a bake changes nothing the user is reading — only the document's
  total height, which reaches the scrollbar thumb and the line-count
  footer. One entry can be tens of thousands of splices, so a frame per
  step is tens of thousands of frames redrawing identical rows, which is
  what spec 0245 exists to prevent.

  A `bake_dirty` flag and a `BAKE_REPAINT_INTERVAL` deadline, modeled
  exactly on spec 0192's `heat_dirty`/`HEAT_REPAINT_INTERVAL` pair
  including its deadline clause — without that clause the last frame of
  a bake waits for an unrelated event, the self-deadlock spec 0191 S3
  and spec 0192 S3 each hit in turn. The interval is 500 ms because the
  thing it repaints is a number in a footer, not the text.

## Alternatives considered

### Bound the confirm and let the user expand what they scroll to

Spec 0249 S8 already does this, and it is why `expand_auto_fold` exists.
Rejected as the *whole* answer: a document that is 15 593 lines until
you touch it and 5.28 M lines afterwards makes the line-count footer,
the scrollbar, `G`, and every search a lie. The bake is what makes the
bounded confirm honest.

### Run the bake on its own thread (spec 0249 S11 as written)

Rejected for now, per N1. The arena, `node_text` and the whole tree are
owned by `App` on the event thread; moving the bake off it means either
a lock around every structural read on the draw path or a snapshot of a
1.6 GiB arena. The measured reason to accept that cost — a 766 ms step
that cannot be interrupted — no longer exists.

### Give the bake its own bounded time slice instead of a step count

Rejected: `run_loop`'s inner loop already re-checks `try_recv` before
every step and breaks on its own deadline, so a second timer inside the
bake would bound the same thing twice, with two constants to keep
consistent.

## Test plan

1. `a_bounded_confirm_leaves_stops_and_the_bake_drains_them` — confirm
   an override with a small budget on a fixture deep enough to stop,
   then drive `bake_step` to `Idle`, and assert `auto_folded` is empty.
2. `a_baked_document_is_the_unbounded_document` — the same override
   applied two ways (bounded + drained, and unbounded) produces
   identical `document_lines()` and identical per-node line counts. This
   is G2, at fixture scale; the 232 MB corpus version is a measurement,
   not a test.
3. `bake_step_is_idle_with_nothing_queued` — the resting state, since
   this runs on every idle iteration forever.
4. `a_stale_bake_queue_entry_is_skipped` — queue a node, expand it by
   hand (S8's path), and assert the next `bake_step` neither panics on
   `expand_auto_fold`'s debug assertion nor re-splices it.
5. `the_bake_does_not_bound_an_export` — `bounded_confirms` defaults
   false, so a headless pass leaves `auto_folded` empty. N6.
6. Whole existing suite. Every splice the bake performs runs
   `assert_line_counts_are_exact`, which is spec 0254's guard and
   applies unchanged here.

## Measured outcome

googleapis.desc (25.6 MB), the root override, `taskset -c 4-7`, release
build. The confirm is timed over the whole of `load_overrides`, which is
more than the render, and the same timer is used on both sides.

| | confirm | bake | worst step |
|---|---|---|---|
| unbounded (before) | **4.55 s** | — | — |
| bounded, 50-row pane | **0.59 s** | 6.55 s | 25 ms |

**G1 holds**: 7.7x, and the 0.59 s screenful is the real document, not a
placeholder. **G3 holds**: over 70 894 steps the worst is 25 ms, two are
over 8 ms and none over 50.

**G2 holds at corpus scale.** The drained export is byte-identical
(`cmp`-clean, 232 892 696 B) to the unbounded one at pane heights of 50,
500 and 5000 — three different confirms, one document.

### The bake's cost is not all the bake's

The drain was 6.55 s at a 50-row pane, 15.53 s at 500 and 102.24 s at
5000, for a step count that never moves (70 894 / 70 893 / 70 797) and
identical queue pushes (63 123) and materialized lines (5.26 M). The
same work, three wall times.

It is linear in the *pane height*: 19.96 ms per pane row across the
50→500 interval and 19.27 ms across 500→5000, extrapolating to 5.55 s at
a pane of zero. The cause is `finalize_override_batch`'s
`clamp_pan_offset` (`override_apply.rs:651`), which every splice runs and
which resolves the whole visible window through `max_visible_line_len`
against a window cache the `structural_version` bump has just emptied.

So the bake proper is 5.55 s and the pan clamp is 1.0 s of the 6.55 —
15% on a normal terminal, and 94% on an absurd one. It is a per-splice
cost that predates this spec and is paid by every fold and every commit
today; the bake only makes it visible by doing 70 894 splices in a row.
Not fixed here: it is one clamp per *batch*, and a bake step that draws
no frame (S6) has nothing to clamp for until the next one is drawn.
