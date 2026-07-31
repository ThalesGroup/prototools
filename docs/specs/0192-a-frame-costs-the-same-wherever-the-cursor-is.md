<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0192 — a frame costs the same wherever the cursor is

Status: implemented — but S1's stored field is gone, and for a better
        reason than it had. S1 added `sibling_ordinal: u32` to every
        node, maintained incrementally, to stop the ordinal being
        walked. Spec 0216 put the arena in level order, so a sibling's
        position is `idx - sibling_block(idx).start + 1`
        (`tui/structure.rs:133`) — arithmetic, no field, nothing to
        maintain. The goal S1 states holds; the mechanism it specifies
        does not exist. S2 to S5 stand.
Implemented in: 2026-07-27
App: protolens
Refs: docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (replaces
        S1's stored ordinal with arithmetic),
      docs/protolens/rendering-flaws.md (P2),
      docs/protolens/rendering-scaling-roadmap.md (S5),
      docs/protolens/rendering-worklist.md (W11, W12, W14),
      docs/specs/0113-protolens-tui-refinements.md (D25, positional paths),
      docs/specs/0152-protolens-heat-cue-background-scoring-thread.md,
      docs/specs/0154-protolens-heat-cue-progressive-display.md (G4, G6),
      docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md (G7, G10),
      docs/specs/0183-prune-the-override-walk.md (G3),
      docs/specs/0184-packed-records-are-the-addressable-unit.md (S2, S6),
      docs/specs/0187-highlighting-is-a-property-of-the-viewport.md,
      docs/specs/0190-the-activity-dot-reports-the-highest-live-tier.md,
      docs/specs/0191-the-read-ahead-walk-is-bounded-and-the-activity-dot-stops-flickering.md

## Background

Reported interactively: on `googleapis.desc`, press `G` to jump to the
end, then hold `PageUp`. The display does not scroll smoothly — it blocks
and stutters. The activity dot stays **blank** throughout, so the heat-cue
subsystem is idle and the cost is somewhere else entirely.

Separately, and previously unexplained: on successive `Down` presses the
activity dot lights blue for roughly a second, when the expectation is
that a one-row move finds nearly everything the previous read-ahead wave
already cached and needs at most one new score.

Both were measured. They have the same root cause, plus two aggravating
factors in the event loop.

### What was measured

A new file trace (`protolens/src/tui/trace.rs`, enabled by
`PROTOLENS_TRACE=<path>`) was added for this investigation and is
retained. The TUI owns the terminal, so a trace cannot go to stderr. With
the variable unset a `trace!` costs one `OnceLock` read and never formats
its arguments.

Driven headlessly through a pty: load `googleapis.desc` (25.6 MB, used as
both blob and `--descriptor-set`) at 200x60, then `G`, 25 `PageUp`s, 15
`Down`s. Release build.

Mean cost per frame, split by phase inside `render()`:

| phase | at top of document | after `G` (end of document) |
|---|---:|---:|
| build `window` | 14 µs | 18 µs |
| syntax highlighting | 1094 µs | 519 µs |
| heat cues (`heat_cue_for`) | 69 µs | **3 988 µs** |
| active-override check | 80 µs | **17 080 µs** |
| build `Line`s | 144 µs | 189 µs |
| **`terminal.draw` total** | **2 748 µs** | **23 221 µs** |

Two columns of the same code, differing only in where the cursor is.
**21.1 ms of the 23.2 ms is in the two rows that explode**, and those two
rows call the same thing.

Syntax highlighting is flat, and slightly *cheaper* at the end. Spec 0187
already made it a property of the viewport; it is not the problem and
this spec does not touch it.

### The cause

`sibling_position` (`navigation.rs:545`) walks the `prev_sibling` chain
one node at a time, so it is O(the node's ordinal position among its
siblings). `positional_path` (`navigation.rs:568`) calls it once per
ancestor level. Two hot paths reach it once per node:

- **Drawing.** `render` → `line_has_active_override` (`render.rs:379`) →
  `resolve_active_override` → `resolve_active_override_entry`
  (`override_apply.rs:882`) → `positional_path`. Once per drawn row.
- **Heat cues, and read-ahead.** `heat_cue_resolve` →
  `current_type_key` (`heat_cue.rs:186`) → `resolve_active_override` →
  `positional_path`. Once per unsettled drawn row, and once per row the
  read-ahead walk visits.

A `FileDescriptorSet`'s repeated `file`/`message_type`/`field` runs put
thousands to tens of thousands of siblings in that walk. At the top of
the document the ordinal is near zero and the walk is free; at the end it
is near the run length. That is the whole of the position dependence.

This is flaw **P2** in `docs/protolens/rendering-flaws.md`, catalogued
from code reading. The numbers above are the first measurement of it, and
they are larger than the flaw entry's own estimate: P2 predicted the
cliff would appear "during the progressive-display window", i.e. only for
*unsettled* rows. The override check has no settled-node short circuit at
all, so it is paid on **every** row of **every** frame, settled or not.
That is why the dot is blank while the display stutters — nothing about
this is the heat subsystem.

### Two aggravating factors in the event loop

The per-frame cost alone would be a stutter. It is multiplied by two
properties of `run_loop` that the same trace exposed.

**One completed heat request costs one full frame.** `heat_worker.rs:516`
sends `HeatWorkerProgress` on every non-`Prefetch` completion, and the
redraw gate treats any received event as a reason to draw. A newly
scrolled-to page queues about one `Visible` request per row, so a single
`PageUp` buys ~58 frames. Measured: **640 of 673 draws** in the session
were heat-notification draws.

Spec 0164 G10 saw exactly this hazard for the read-ahead tier and fixed
it by never notifying at all — "a large read-ahead burst would otherwise
mean thousands of no-op redraws". The `Visible` tier was left unbatched
because at the time a frame was cheap and a screenful was a handful of
requests.

For completeness, since spec 0191 is recent: only **2** of the 673 draws
were activity-dot draws. The sliding maximum introduced there is doing
its job and is not implicated.

**The same flood starves read-ahead completely.** `prefetch_step` runs
only when `rx.try_recv()` returns `Empty` (`mod.rs:2173`). Under the
notification flood it never does. Across 25 `PageUp`s and 15 `Down`s the
trace recorded **two** read-ahead waves, both before the first keystroke:
`prefetch_step` was called zero times for the rest of the session. Spec
0191 bounded the walk so it would finish and let both threads park; this
is the opposite failure, and it means the read-ahead the design depends
on is switched off exactly when the user is moving.

The interaction is circular: an expensive frame lets more completions
accumulate, which queues more frames, which keeps the channel non-empty,
which keeps read-ahead off, which keeps rows unsettled, which keeps
`heat_cue_for` on its expensive branch.

## Goals

- **G1.** The cost of drawing a frame does not depend on where the cursor
  is in the document. Specifically, `positional_path` becomes O(depth)
  rather than O(depth x sibling ordinal).
- **G2.** A node whose heat cue has settled costs nothing to check for an
  active override, matching the short circuit `heat_cue_resolve` already
  has.
- **G3.** A burst of completed heat requests costs a bounded number of
  frames, not one frame each.
- **G4.** Read-ahead cannot be starved to zero by the event stream. Every
  iteration of the main loop makes progress on the walk, or the walk is
  genuinely exhausted.
- **G5.** These properties are measured, in-repo, and the measurements are
  recorded here — not argued.

## Non-goals

- **N1.** Syntax highlighting. Flat in the measurement, already scoped by
  spec 0187, and untouched here.
- **N2.** Making `score_all` faster. Nothing in this spec is about the
  scorer; the flaw is that the UI thread does not reach it.
- **N3.** The three linear scans over `overrides.entries()` in
  `resolve_active_override_entry_index_by_path`. Flaw P2's second half.
  Spec 0183 G3 already replaced the scans with a `partition_point`, so
  each is O(log E + matching run), and E is small. It is not in the
  measurement and it is not fixed here.
- **N4.** The other whole-document structures (`lines`, `tree`,
  `line_to_node`, `visible_rows`) — roadmap item S8.
- **N5.** Removing the `Visible`-tier notification entirely. G3 is about
  the *rate* at which notifications turn into frames, not about whether
  the main thread learns that work finished. A cue that never repaints is
  worse than one that repaints late.

## Specification

### S1. A node's sibling position is stored, not walked

Add to `TreeNode` (`decode.rs:319`):

```rust
/// Spec 0192 S1: this node's 1-based position among its siblings,
/// counting a packed run as a single position (spec 0184 S2 — every
/// element of a packed record shares the record's positional path).
/// Precomputed so `positional_path` is O(depth) rather than
/// O(depth x sibling ordinal); see that function for why it is hot.
pub sibling_ordinal: u32,
```

`build_tree` (`decode.rs:351`) already links `prev_sibling`
incrementally at `decode.rs:395-400`, at the exact point where the
previous sibling is in hand. The ordinal is derived there in the same
statement, at no additional asymptotic cost:

- no previous sibling: `1`
- previous sibling `p`: `nodes[p].sibling_ordinal`, plus 1 unless
  `same_packed_record(&nodes[p].span, &nodes[i].span)`.

`sibling_position` becomes a field read. `positional_path` is otherwise
unchanged and keeps its exact output.

#### Why the packed-run absorption does not need a repair pass

The scaling roadmap's S5 explicitly recommends *against* this route,
because `splice_override`'s packed-run absorption removes `run_len - 1`
siblings and it assumed that shifts every following sibling's ordinal.
Flaw P2's own version of the same route proposes a bounded repair walk
for it.

**That concern does not apply, and the reason is worth stating precisely
rather than relying on.** `sibling_position` does not count siblings; it
counts *distinct packed records* (`navigation.rs:549` only increments
when `same_packed_record` is false). Every element of a run shares one
`packed_record_start`, so a run of any length already contributes exactly
1 to every following sibling's ordinal. Absorbing the run into its first
element leaves one node contributing exactly 1. The total is unchanged,
whether or not the retyped survivor keeps a `packed_record_start` of its
own.

This makes the ordinal route strictly cheaper than the roadmap assumed,
and it removes the one reason S5 gave for preferring the memoization
route. It is nonetheless an argument about the highest-risk code in the
pipeline, so it is pinned by test 2 rather than trusted.

#### The one place ordinals must actually be assigned

`splice_override` (`override_apply.rs:2381`) appends a locally-built
tree and remaps its indices. Local nodes carry ordinals from their own
`build_tree` call, which are correct *within the local tree*. The one
case that is not is the child-list merge at `override_apply.rs:2376`:
local root-level nodes (`parent == None`) and the local root's own direct
children both become children of `idx`, so two independently numbered
sequences land in one sibling list.

Renumber that merged child list once, by walking `first_child` /
`next_sibling` from `idx` after the merge. It is bounded by `idx`'s child
count, which is a splice-local quantity, not a document-sized one.

### S2. A settled node short-circuits the override check

`line_has_active_override` (`render.rs:379`) resolves an override for
every drawn row, on every frame, purely to decide whether to add
`Modifier::BOLD`. `heat_cue_resolve` already returns at
`heat_cue.rs:273` for a settled node without touching any of this.

Hoist the per-row override lookup out of the `text_lines` closure into
its own pass over `window` — the same shape and position as the heat-cue
pass (spec 0154 G6) and the highlighting pass (spec 0187 S2), and for the
same borrow reason. Within that pass, resolve at most once per distinct
node: adjacent rows of a packed run share a positional path (spec 0184
S2), and a node's header and footer rows resolve identically.

This is a smaller win than S1 and is not a substitute for it — with S1 in
place the lookup is cheap anyway. It is specified because it makes the
draw path's per-row work explicit and countable, which is what test 5
asserts against.

### S3. Completed heat requests are coalesced into a frame budget

The redraw gate in `run_loop` gains one distinction: a `HeatWorkerProgress`
event marks the frame dirty; every other reason to draw is immediate, as
today.

```rust
/// Spec 0192 S3: the shortest interval between two frames drawn
/// *solely* because background scoring completed. Keystrokes, mouse
/// events, resizes and message/splash deadlines are never delayed by
/// it — only the repaint that shows a newly arrived cue is, and that
/// repaint is a visual refinement of a frame the user is already
/// looking at.
const HEAT_REPAINT_INTERVAL: Duration = Duration::from_millis(50);
```

State handling:

- `HeatWorkerProgress` still calls `recheck_pending_heat_states` and
  `poll_pending_override_work` immediately — the *state* must stay
  current; only the frame is deferred.
- It sets a `heat_dirty` flag instead of forcing a draw.
- The gate draws for heat when `heat_dirty && now >= last_heat_frame +
  HEAT_REPAINT_INTERVAL`; drawing for any reason clears `heat_dirty` and
  stamps `last_heat_frame`.
- `heat_dirty` must contribute a deadline to the `recv_timeout`
  computation, exactly as `message_deadline` and the activity tick
  already do. Otherwise a final completion arriving during the interval
  would be shown only when the next unrelated event happens to wake the
  loop — the same class of self-deadlock spec 0191 S3 hit.

At 50 ms a screenful of arriving cues costs at most 20 frames per second
instead of one frame per cue, and the cap is a refresh rate the user
cannot distinguish from immediate for a progressive fill.

### S4. Read-ahead gets a guaranteed slice of every iteration

`mod.rs:2173` runs `prefetch_step` only in the `TryRecvError::Empty` arm,
which makes read-ahead a strict lowest priority that a steady event
stream reduces to zero.

Give it a floor instead: on each iteration of the main loop, before the
receive loop, run `prefetch_step` until it reports `Idle` or a small
fixed budget of steps is spent.

```rust
/// Spec 0192 S4: read-ahead steps guaranteed per main-loop iteration,
/// regardless of how busy the event channel is. Read-ahead is still
/// opportunistic — the `Empty` arm below spends as many steps as the
/// user's idleness allows — but it is no longer possible for a steady
/// event stream to reduce it to zero, which is what a burst of
/// `HeatWorkerProgress` events did.
const PREFETCH_STEPS_PER_ITERATION: usize = 8;
```

The budget must be small enough that the guaranteed work cannot itself
add perceptible latency to a keystroke. After S1 a step is cheap; before
S1 it is not, which is why S1 and S4 land in that order.

S3 alone would very likely restore read-ahead, since the channel drains
once frames stop being emitted per completion. S4 is specified anyway,
because "read-ahead happens to run because nothing else is queued" is not
a property — it is the absence of one, and it is precisely what failed
here.

### S5. The trace is retained

`protolens/src/tui/trace.rs` and its call sites stay. It emits, when
`PROTOLENS_TRACE` is set:

- `draw <reason> us=` — the redraw-gate clause that fired
  (`key`/`heat`/`mouse`/`term`/`deadline`/`activity`).
- `render window_us= styles_us= heat_us= ovr_us= lines_us= rows=`
- `wave <reason> rows= skipped= hits= pushes= busy_ms=` — one line per
  read-ahead wave. `busy_ms` is time spent *inside* `prefetch_step`, not
  the wall time the wave spanned: the walk is interleaved with drawing,
  so wall time measures the loop rather than read-ahead. The first draft
  of this instrument got that wrong and reported 1075 ms for a wave whose
  actual cost was a fraction of it.
- `key <code> us=`

It is retained rather than deleted because every number in this spec came
out of it, and the test plan's regression gates are stated in its terms.

## Alternatives considered

### A1. Memoize `current_type_key` per node instead of storing the ordinal

The scaling roadmap's S5, and flaw P2's second proposed correction: a
`Vec<Option<String>>` parallel to `tree`, stamped with
`(structural_version, overrides_version)` and cleared on mismatch.

Rejected as the primary fix, for three reasons.

1. **It does not cover the larger half.** The measurement puts 17.1 ms in
   the override check and 4.0 ms in heat cues. Memoizing
   `current_type_key` addresses the 4.0 ms. `line_has_active_override`
   does not go through `current_type_key` at all — it calls
   `resolve_active_override` directly — so it would need its own second
   memo, or S2's hoist, or both.
2. **It adds a cache and an invalidation obligation** — including a new
   `overrides_version` counter that must be bumped by every
   `OverrideCollection` mutation, i.e. a new obligation at every
   mutation site. The roadmap names this as the hazard this codebase has
   historically got wrong, and accepts it only because it judged the
   ordinal route riskier. S1 shows that judgement rested on a premise
   about packed-run absorption that does not hold.
3. **It keeps a document-sized structure**, one `Option<String>` per
   node, which is the residency spec 0187 spent its whole length
   removing for style hints.

The ordinal is not a cache. It is a derived structural fact stored next
to the structure it is derived from, maintained where the structure is
built, and it cannot go stale independently of `prev_sibling` itself.

A1 remains available and is not mutually exclusive: if measurement after
S1 still shows `current_type_key` hot — it should not, since it becomes
O(depth) string building — memoizing it is the next step.

**This reverses the worklist's ordering.** W12 is marked "blocked by
W11", with the instruction "do not do this item if W11 alone brings the
progressive window to an acceptable frame time". The measurement settles
that conditional: W11 alone cannot, because the 17.1 ms majority of the
frame is `line_has_active_override`, which never calls
`current_type_key`. W12 — this spec's S1 — is the item to do first, and
it may make W11 unnecessary rather than the other way round. Update both
worklist entries when this lands.

### A2. Invert the override lookup: key entries by node, not by path

`resolve_active_override_entry` exists because overrides are identified
by a positional path, so answering "does this node have one" requires
computing the node's path. The inverse index — a `HashSet<usize>` of
nodes that currently have an active override, rebuilt when the override
set changes — would make the check O(1) with no path at all.

Rejected for now. It is the largest change of the three, it is a genuine
cache with a genuine invalidation obligation against *two* independent
versions (the tree and the override set), and it duplicates the
three-tier priority resolution (`Path` > `PathField` > `FqdnField`) that
`resolve_active_override_entry_index_by_path` implements. S1 makes the
path cheap enough that the index buys little; it would be the right move
only if paths stayed expensive.

### A3. Stop notifying on `Visible` completions, like `Prefetch`

Symmetric with spec 0164 G10 and a one-line change. Rejected: a
`Prefetch` result is speculative and nobody is waiting to see it, so
never waking the main thread is correct. A `Visible` result is a cue for
a row that is on screen *now* with a `[?]` placeholder on it. Not
notifying would leave the placeholder until an unrelated event redrew the
frame — the activity tick at 250 ms in the common case, and never in
principle. S3 defers the frame by a bounded interval; A3 defers it
indefinitely.

### A4. Cap the frame rate globally

Rather than distinguishing heat repaints, apply a minimum interval to all
frames. Rejected: it adds latency to keystrokes, which is the one thing
that must stay immediate. The measurement shows the flood is entirely
worker-driven (640 heat draws against 29 key draws), so the distinction
costs nothing and preserves input latency by construction.

## Test plan

1. **`sibling_position` is a field read.** For a fixture with a long
   sibling run, assert `sibling_position(last)` equals the value the old
   walking implementation produced, for every node. Keep the walking
   version in the test as the oracle. — *Done:
   `every_stored_sibling_ordinal_equals_the_walk_it_replaced`
   (`tests/navigation.rs`). Checked over the whole live tree, before and
   after a splice. Restricted to nodes reachable in document order: a
   splice abandons the nodes it absorbs in place, and an abandoned
   node's stale `prev_sibling` no longer describes anything.*
2. **A packed-run absorption preserves following siblings' ordinals**
   (S1's argument). Build a document with a packed run followed by
   several ordinary siblings, record every following sibling's
   `positional_path`, apply an override that absorbs the run, and assert
   every one of those paths is unchanged. This is the test that stands in
   for the roadmap's proposed repair walk; if it fails, the repair walk
   is needed after all. — *Already existed, from spec 0184:
   `overriding_a_packed_run_does_not_renumber_later_siblings` and
   `packed_run_ordinals_are_stable_across_the_override_lifecycle`
   (`tests/override_apply.rs`). Both now exercise the stored ordinal
   instead of the walk and both pass, so the repair walk is not needed.*
3. **A splice's merged child list is renumbered.** After an override that
   produces both root-level local nodes and local-root children, assert
   `idx`'s children have ordinals `1..=n` with no repeats, and that each
   child's `positional_path` round-trips through
   `resolve_path_to_node`. — *Subsumed by test 1's post-splice oracle
   pass, which covers the merged child list along with everything else,
   and by the existing `resolve_path`/`positional_path` round-trip tests
   in `tests/command_line.rs`.*
4. **Frame cost is independent of cursor position** (G1). — *Done:
   `a_frame_costs_the_same_at_the_end_of_a_wide_document_as_at_the_start`
   (`tests/render.rs`), not `profiling.rs`: it is a real regression
   test, not a throwaway diagnostic, so it must run in the ordinary
   suite. 20 000 siblings, best-of-5 at each end, bound 10x. Verified to
   catch the regression by restoring the walk: 8.48 ms against 321 µs, a
   26x ratio, so the bound has a 2.6x margin on the failure side and
   the passing run is at 1.0x.*
5. **The override check runs once per record, not once per row** (S2). —
   *Done: `the_override_check_runs_once_per_record_not_once_per_row`
   (`tests/render.rs`), via `override_bold_flags`'s returned resolution
   count. Asserts both that collapsing does not change the answer and
   that it collapses something. See Q1 — this test is what showed the
   header/footer half of the draft's claim to be false.*
6. **A burst of completions costs a bounded number of frames** (G3).
   Deliver N `HeatWorkerProgress` events within one
   `HEAT_REPAINT_INTERVAL` and assert at most one heat-driven frame is
   drawn, and that the state recheck ran N times regardless. — **Not
   done, see below.**
7. **A deferred repaint is not lost** (S3's deadline requirement). Send a
   single `HeatWorkerProgress` and then nothing at all; assert a frame is
   drawn within `HEAT_REPAINT_INTERVAL`, not at the next activity tick
   and not never. This is the self-deadlock spec 0191 S3 hit, in its new
   form. — **Not done, see below.**
8. **Read-ahead is not starved** (G4). With a saturated event channel,
   assert `prefetch_step` still advances the walk on every iteration. —
   **Not done, see below.**
9. **End-to-end, on the reported workload.** — *Done; see `Measured
   outcome`. All three pass conditions met: the phase split no longer
   depends on position, heat draws are 55% of a 128-frame session
   against 95% of a 673-frame one, and there were 42 read-ahead waves
   against 43 keystrokes.*
10. **The `Down` case.** — *Done; see `Measured outcome`.*

### Tests 6-8 are deferred: `run_loop` has no test harness

All three assert on the redraw gate and the receive loop, which live in
`run_loop` — a function that owns a `Terminal`, blocks on an
`mpsc::Receiver`, and returns only on quit or channel disconnect. It has
never had a test: the gate introduced by spec 0190 S8 and the sliding
maximum introduced by spec 0191 S4 are both untested for the same
reason, and this spec adds a third condition to the same expression.

Testing it needs a harness that does not exist — a backend that counts
frames (`TestBackend` does not), and a way to drive the loop for a
bounded number of iterations. That is a self-contained piece of work
worth doing on its own terms, because it would retroactively cover 0190
S8 and 0191 S4 as well, not just this spec. It is deliberately not
smuggled into this one.

Until then the three properties rest on the end-to-end trace, which does
observe all of them: the draw-reason histogram shows heat frames capped
(test 6), the session ends with no pending unshown state (test 7), and
the wave count exceeds the keystroke count (test 8).

## Open questions

- **Q1 — is S2 worth keeping once S1 lands? Resolved: kept, but
  narrowed.** The hoist is kept: it is what makes `ovr_us` a phase the
  trace can report at all, and the measured cost of the extra
  `Vec<bool>` is inside the noise (`ovr_us` 54-61 µs for 58 rows).
  The *per-node* collapse the draft asked for turned out to be partly
  imaginary. Two claims were made for it; only one survives:
  - A packed run's element rows do collapse — N nodes, one record, one
    positional path. Implemented, and pinned by test 5.
  - A node's header and footer rows do *not* collapse under any
    scheme cheaper than the lookup itself, because they are never
    adjacent: a message's whole subtree is drawn between them. The
    first implementation used a one-entry "last node seen" memo and
    silently collapsed nothing at all on a fixture with no packed run;
    test 5 caught it. `override_bold_flags`'s doc comment now states
    the adjacency requirement rather than the false general claim.
- **Q2 — is 50 ms the right repaint interval? Resolved: yes, as a fixed
  constant.** The adaptive variant is unnecessary: the post-S1 frame is
  ~2 ms, so at 50 ms the heat repaint takes at most 4% of wall time in
  the worst case, and the measured session drew 71 heat frames where it
  had drawn 640. Revisit only if the frame cost itself moves by an order
  of magnitude.
- **Q3 — `u32` or `usize` for `sibling_ordinal`? Resolved: `u32`.** One
  cast, in `sibling_position`, which is the only reader. Everything else
  reads the ordinal through that function.

## Measured outcome

Same trace, same corpus, same script (`googleapis.desc`, 200x60,
release, pty-driven `G` + 25 `PageUp` + 15 `Down`), before and after.

Per-frame phase split, mean µs over 58 drawn rows:

| phase       | before, top | before, end | after, top | after, end |
|-------------|------------:|------------:|-----------:|-----------:|
| `window_us` |          14 |          18 |          8 |         11 |
| `styles_us` |       1 094 |         519 |        924 |        851 |
| `heat_us`   |          69 |       3 988 |         80 |         74 |
| `ovr_us`    |          80 |      17 080 |         54 |         61 |
| `lines_us`  |         144 |         189 |        110 |        107 |
| draw total  |       2 748 |      23 221 |      2 103 |      2 088 |

G1 holds: the two columns are now the same to within noise, where
before the end of the document cost 8.4x the top. The
active-override phase went from 74% of the draw to 3%, and its worst
frame in the whole session was 115 µs against 17 080 µs.

Everything else:

- **Draws: 673 → 128.** Reasons before: 640 heat, 29 key, 2 activity, 1
  deadline, 1 initial. After: 71 heat, 43 key, 12 activity, 1 deadline,
  1 initial. A keystroke now costs about one frame rather than about
  twenty-three (G3).
- **Read-ahead waves: 2 → 42**, i.e. more than one per keystroke rather
  than none after the first (G4). Cost per wave: 467 ms busy for 2 048
  rows before, 1.4 ms mean / 2.0 ms max after — 228 µs/row down to
  0.7 µs/row, which is S1 again.
- **`handle_key`: 10-24 µs, unchanged.** Navigation was never the cost.
- **Test 10, the `Down` case.** Fifteen successive `Down`s each get a
  full 2 048-row wave: `skipped` ≈ 1 130, `hits` ≈ 75, `pushes` ≈ 840,
  `busy_ms` ≈ 1.4. The user's original expectation is confirmed in the
  sense that mattered — 55% of the walked rows are already settled and
  cost nothing — and the remaining frontier work is now 1.4 ms, so
  whether it is "mostly cached" stopped being a question anyone can
  feel.

`styles_us` is now the largest single phase at ~880 µs, and the draw
total is ~2.1 ms against a phase sum of ~1.1 ms, so roughly half a frame
is ratatui's own diff and flush. Neither is in this spec's scope; both
are the next thing to look at if 2 ms per frame ever becomes a
complaint.
