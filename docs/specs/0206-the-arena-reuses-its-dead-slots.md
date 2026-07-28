<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0206 — the arena reuses its dead slots

Status: draft
App: protolens
Refs: docs/specs/0111-protolens-v1-decode-navigate-extract.md (the arena
        and its post-order layout),
      docs/specs/0118-protolens-recursive-override-rendering.md (§7, a
        retyped node keeps its own index),
      docs/specs/0135-protolens-override-raw-tag-rewrap.md (G1, the
        packed sibling merge and its orphans),
      docs/specs/0152-protolens-heat-cue-background-scoring-thread.md
        (G6, `heat_states` is parallel to `tree`),
      docs/specs/0162-protolens-tree-node-reclamation.md (A2, where a
        free list was first considered and rejected),
      docs/specs/0183-prune-the-override-walk.md (the descent marks),
      docs/specs/0186-the-commit-touches-only-what-moved.md (the
        per-batch verification hook),
      docs/specs/0188-the-batch-updates-what-changed-not-what-exists.md
        (S4, `descend.len()` as the examined watermark),
      docs/specs/0202-an-override-is-refused-rather-than-fatal.md (the
        headroom guard, which stays),
      docs/specs/0203-the-override-arena-is-compacted.md (`dead`,
        `mark_dead`, `verify_arena`, and the rejection this revises)

## Background

Spec 0203 made the arena flat, and did it after the fact. A batch still
appends every node it materializes to the end of `tree`, and a
compaction pass in the following idle period walks the garbage back
out. Six batches on a doubled `googleapis.desc` now start from the same
4 499 336 nodes every time, where before they went 4 501 014 →
9 000 349 → 13 499 684 → OOM.

What it does not touch, and states as its N2, is the peak. Every batch
still climbs to 8 998 671 nodes and 3.9–4.0 GiB before the pass runs.
The arena is flat when observed between batches and doubled during one.

That doubling is not inherent. `splice_override` **frees before it
allocates**: the old subtree is marked dead at `override_apply.rs:2436`,
and the replacement is pushed from `:2555`. By the time the push loop
runs, the holes it needs already exist. Today it walks past them to the
end of the `Vec`.

So the change is to hand them back. `tree.len()` then becomes the
high-water mark of the *live* set and never exceeds it — 4 499 336
rather than 8 998 671 — and the peak disappears rather than being
reclaimed. On the measured workload a root retype frees 4.5 M nodes and
draws 4.5 M back, growing by nothing.

### Why the earlier rejections no longer hold

A free list was rejected twice, and both rejections were correct when
written.

Spec 0162's A2 rejected it on **aliasing**: reusing a slot means a
stale index now names a different live node, which "renders plausibly
and wrongly", and — the deciding clause — a free list "has no such
moment" at which the arena can be asserted correct. That was true of
the arena as it stood. It is not true now: spec 0203 built
`verify_arena`, which computes the reachable set and checks every link,
every index holder and both line maps against it. That *is* the moment,
it is continuously available rather than tied to a collection cycle,
and it already runs from spec 0186's per-batch hook on every override
test in the suite. The objection has been answered by work done since,
not argued away.

Spec 0203's own rejection had three grounds, and slot granularity
retires the first outright. It argued external fragmentation over
"contiguous extents of wildly different sizes", with a 27 000-node hole
failing to satisfy a 30 000-node request. That is an argument about
*extents*. A slot is a `TreeNode`, every one identical at 264 bytes, so
at slot granularity a hole always fits a request and external
fragmentation is not merely unlikely but impossible. The extent story
only exists because the splice's addressing is affine —
`translate = |i| i + base` — and that is self-imposed, not a property
of the arena.

The remaining ground, that reuse "fails dangerously rather than
loudly", stands untouched and is the real cost of this spec. It is
addressed in S5 rather than dismissed.

### What contiguity is actually worth here

Allocating lowest-slot-first over a bitmap is address-ordered first
fit, whose known behavior is to keep allocation dense at the bottom and
holes clustered. A replacement subtree therefore lands as a handful of
*extents* wherever the free bits already run consecutively, not as
scattered slots. Two things reinforce that: the holes a batch creates
are the old subtree, which was itself laid out as a run, and the same
batch is the one that immediately refills them — so retyping the same
node twice reuses the same extent exactly. Compaction, when it runs, is
order-preserving and hands back one extent at the top rather than a
sieve.

The segmentation is emergent and is never represented. That is the
point: the L3 benefit of extents arrives without an extent list, a fit
policy, or a coalescing rule.

## Goals

- **G1.** `tree.len()` never exceeds the high-water mark of the live
  set. The transient double of a retype is removed, not reclaimed.
- **G2.** There is exactly one record of which slots are free. `dead`
  is it; no second structure exists that can disagree with it.
- **G3.** Allocation adds no term to a batch that the batch does not
  already pay. The scan is amortized across the batch's own
  allocations, never repeated per node.
- **G4.** Nothing user-visible changes: the rendered document, the
  cursor, the folds, the heat cues and the override entries are what
  they were.
- **G5.** `verify_arena` remains the witness, and every property it
  checks still holds at the moment spec 0186's hook runs.

## Non-goals

- **N1.** Returning memory to the operating system. This spec is a
  watermark: `capacity` is never reduced, so RSS settles at the peak
  the session reaches and stays there. That is strictly better than
  today, where the peak itself keeps rising, and it is deliberately
  where this stops.
- **N2.** The shrink policy. `compact_slice` currently ends in
  `shrink_to_fit` (`compact.rs:246-248`), which is the one place
  anything is returned. It is removed by S6 rather than tuned, and the
  hysteresis design is recorded there as future work so it is not
  rediscovered from scratch.
- **N3.** Narrowing the per-node footprint (`u32` links, interned
  `type_fqdn`). Spec 0203's N1, unchanged. Worth noting only that it
  gets *more* attractive under this spec, since a ~100 B node fits
  several to a cache line and makes contiguity worth more than it is
  at 264 B.
- **N4.** Removing spec 0202's headroom guard. It stays, and its
  arithmetic is revisited in S7 rather than deleted: a batch that
  frees before it allocates needs far less headroom than one that
  cannot.
- **N5.** Extent-granular allocation, and a summary level over the
  bitmap. Both are strictly local upgrades to the allocator that
  change nothing else, and neither is justified before the scan shows
  up in a trace. See "Alternatives considered".
- **N6.** Removing compaction. It stays, with a narrower job (S6).
- **N7.** Generation counters or any other scheme for detecting a
  stale index at use. See S5 for why the answer here is a check on the
  arena rather than a tag on the index.

## Specification

### S1. `dead` is the allocator

No free list is added. `dead: Vec<bool>` (`mod.rs:1212`) already
records precisely the set of reusable slots, and `dead_count`
(`:1218`) already tracks its size.

This is a correctness requirement and not a tidiness preference. A
separate `Vec<u32>` of free slots would be a second representation of
the same set, and `relocate_node` (`compact.rs:94`) *fills a hole*
without consulting it — it moves a live node into a dead slot and
clears the flag. The next allocation would then hand out a slot holding
a live node, and the loss would be silent. Making the list advisory
(pop, re-check `dead[i]`, discard if it lies) works, but at that point
`dead` is the truth and the list is a cache, which is N5.

Allocation is a scan over the bitmap, guided by one cursor:

- `free_cursor: usize`, with the invariant **no free slot exists below
  `free_cursor`**.
- `alloc_slot()` advances `free_cursor` to the first set flag, clears
  it, decrements `dead_count`, and returns the index. If it reaches
  `tree.len()`, it falls back to `tree.push` and the parallel arrays
  grow with it.
- `mark_dead(idx)` lowers `free_cursor` to `min(free_cursor, idx)`.

Lowering the cursor is always sound; raising it past a free slot is the
only way to break the invariant. `relocate_node` fills holes and can
therefore only leave the cursor stale-*low*, which costs a rescan and
nothing else. `compact_slice`'s truncation lowers `tree.len()` beneath
it, so the cursor is clamped there too.

The cursor persists across batches. It is not reset per splice, which
is what keeps the common case from re-walking the arena.

### S2. What a slot must be reset to

A reused slot carries the previous node's state in every array parallel
to `tree`. Allocation is responsible for all of it:

- `tree[slot]` is overwritten by assignment, which drops the previous
  `TreeNode` — and with it that node's `type_fqdn` and `rendered_as`
  heap strings. Under today's append-only scheme those strings live
  until compaction truncates them away; here they are freed as the
  batch proceeds. This should lower the batch's heap peak as well as
  its slot peak, and is stated as an expectation to be measured, not
  as a claim.
- `heat_states[slot]` must be assigned `HeatState::default()`.
  `:2625-2626` currently relies on `resize` to default the *new tail*,
  which reaches no reused slot. Inheriting a resolved cue from a
  superseded node is exactly the "renders plausibly and wrongly"
  failure.
- `dead[slot]` is cleared by `alloc_slot`.
- `descend[slot]` keeps the previous node's mark. This is safe and is
  argued in S4 rather than fixed here.

`folded`, `pending_heat_recheck`, `cursor`, `first_node` and
`override_target` need no reset, because W2 already requires that none
of them names a dead slot, and `verify_arena` checks it.

### S3. The splice allocates its slots up front

`splice_override` takes `base = self.tree.len()` at `:2555` and derives
every index from it. Four sites depend on that affine form and all four
change together:

- `let translate = |o| o.map(|i| i + base)` (`:2590`);
- `node.parent.map(|p| p + base)` (`:2599`);
- `let new_self_idx = base + local_root_idx` (`:2637`);
- `(base..base + local_len).find(...)` (`:2690`).

`base` is replaced by `slots: Vec<u32>`, of length `local_len`,
obtained by calling `alloc_slot()` `local_len` times **before** the
push loop begins. Up front rather than per node, because `build_tree`
emits a post-order local tree whose nodes name each other in both
directions: `translate` resolves forward references, so every slot must
be known before the first node is written.

Then `translate = |o| o.map(|i| slots[i] as usize)`, the parent case
takes the same map, `new_self_idx = slots[local_root_idx]`, and the
document-order scan iterates `slots` instead of a range.

`mark_fresh_subtree(base, path)` (`:1375`, called at `:1659`) takes the
slot list instead of a base, and `collect_descend_targets` is given
that list rather than the range `base..tree.len()`.

The local root's slot is allocated and then immediately marked dead at
`:2638`, as today — its span and child links are folded into the
surviving `tree[idx]` and it is never referenced again (spec 0118 §7).
Allocating a slot only to free it is deliberate: the node has to be
materialized somewhere in order to be read at `:2639` and `:2646-2647`,
and special-casing it would buy one slot per splice at the cost of a
second code path through the translation.

### S4. `descend`'s watermark survives, and why

Spec 0188 S4 gave `descend.len()` a second meaning: the length of the
arena prefix already examined for descent targets.
`compute_descend_marks` (`:1339-1352`) reads it as `scanned` and
examines only `scanned..tree.len()`. That identity assumes the arena
grows only at the end, which is exactly what this spec stops.

It nevertheless holds, for a reason worth stating rather than assuming.
`descend.len()` is kept equal to `tree.len()` by the `resize` at
`:1343` and `:1379`, so the unexamined suffix is empty whenever the
arena did not grow — which under reuse is the normal case. Fresh nodes
landing in reused slots are not covered by that range, and are instead
marked explicitly by `mark_fresh_subtree`, which S3 gives the actual
slot list. Every fresh node is therefore examined exactly once, by
name.

What remains is the stale mark a reused slot inherits. Spec 0183 L3
records the asymmetry that settles it: **over-marking costs a wasted
descent, under-marking is a silent staleness**. A reused slot can only
be over-marked, since the mark it inherits was set for a node that no
longer exists and `mark_fresh_subtree` supplies any mark the new node
needs. So the failure mode of inheriting a mark is wasted work, and it
is bounded by the size of the spliced subtree.

This is the one place where reuse degrades a spec-0188 optimization
rather than breaking it, and it is worth a test rather than a comment.

### S5. The hazard this introduces, stated plainly

Today an index into a freed subtree still resolves. The node is
abandoned but intact, so a stale index renders something wrong and
visible. Under this spec the same index names a live, unrelated node,
and renders something wrong and plausible. That is spec 0203's third
ground for rejecting a free list, it is spec 0162 A2's "serious one",
and nothing in slot granularity touches it.

Three things address it, in decreasing order of weight.

**The window is one splice long, and auditable.** `mark_dead` runs at
`:2436` and the first `alloc_slot` at `:2555`. Every index computed
before the free and read after the allocation is a candidate. The audit
is over that span of one function, not over the program: `after`,
`packed_next_sibling_of_run` and `packed_run_is_last_child` are the
indices that cross it, and each must be shown to lie outside the freed
set (`after` is the document-order successor of the old subtree;
`packed_next_sibling_of_run` is past the absorbed run). This audit is
part of the implementation, and its conclusion belongs in a comment at
`:2436` where the free happens.

**The invariant is checked continuously.** W2 — every link field and
every index holder names a live node — is precisely the property that
fails when a stale index survives, and `verify_arena` decides it
against the reachable set. It runs from spec 0186's hook after every
batch in every override test, including
`randomized_override_sequences_keep_every_span_consistent`. Under
append-only semantics a violation was survivable and therefore easy to
miss; the change here is that the same check now has teeth.

**The verifier must become reachable from the shipping binary.** Spec
0203 left this as an aside; here it is required. The documents that
would expose a violation are the multi-million-node ones no fixture
reaches, and they exist only under the real binary. `verify_arena`
becomes available outside `cfg(test)` behind an environment variable —
the same shape `trace.rs` already uses, and reporting through the same
channel, since the process owns the terminal and cannot simply print.
It stays off by default and out of the event loop; it is a thing a user
or a bug report can be asked to turn on.

Generation counters (N7) are the textbook alternative and are not taken.
They cost a word per node and a check per dereference to detect a
condition that W2 already decides globally, and they detect it at the
point of *use* rather than at the point where the arena became wrong.

### S6. Compaction keeps its mechanism and loses most of its job

With reuse, `dead_count` no longer accumulates: a batch's garbage is
consumed by the same batch. `COMPACT_MIN_DEAD_SHARE`'s 1/8 gate will
therefore be closed almost always, which is correct and is the intended
outcome. Compaction becomes the answer to a narrower question — the
live set genuinely shrank, because the user discarded a large subtree
without replacing it — rather than routine housekeeping.

Two changes:

- The three `shrink_to_fit` calls (`compact.rs:246-248`) are removed.
  `truncate` alone lowers `len` without touching `capacity`, which is
  N1's watermark, and it costs no reallocation. Keeping them would be
  worse than the status quo, not better: `shrink_to_fit` sets capacity
  to exactly the live count, so the next allocation past it triggers a
  `Vec` doubling that reallocates and copies the whole arena — 1.2 GB
  — to gain one slot. Shrink-to-exact next to an allocator that reuses
  is the classic ping-pong.
- Truncation must clamp `free_cursor`, per S1.

The shrink policy is future work (N2), and the design is recorded here
so it is not rederived: release only when `live * 4 <= capacity`, and
release to `capacity / 2`. Since `Vec` grows by doubling, capacities
are powers of two and this is power-of-two reclaim by construction.
The factor-of-four dead band is what makes it amortized — live must
double before anything regrows and halve again before anything shrinks
— whereas the tempting `live * 2 <= capacity` puts the two thresholds
at the same point and is the ping-pong again. Whatever implements it
must also account for `shrink_to` itself reallocating and copying on
the main thread.

### S7. Spec 0202's guard, revisited but not removed

The refusal guard estimates a batch's arena growth at ~305 B/node
against available memory. Under this spec a batch that frees at least
as much as it allocates grows by nothing, so the estimate is wrong in
the safe direction — it refuses batches that would now succeed, and
the document-wide retype it was written for is exactly that case.

The guard stays (N4), and the arithmetic should subtract the slots the
splice is about to free. That number is known: the old subtree has been
collected before the estimate is needed. If it ever fires after this,
that is information, which was already spec 0203's N4 position.

## Test plan

Tests assert through content projections rather than indices, as spec
0203's do — which slot a node occupies is the implementation detail
under test.

1. `a_retype_reuses_the_slots_it_just_freed` — on a fixture with a
   subtree large enough to matter, `tree.len()` is unchanged across an
   apply/remove cycle, and `verify_arena` passes at each step. This is
   G1.
2. `a_reused_slot_carries_no_state_from_its_previous_occupant` — the
   heat state of a node in a reused slot is pending, not whatever the
   superseded node had resolved to. This is the S2 failure that would
   otherwise be invisible.
3. `allocation_and_compaction_never_hand_out_the_same_slot` — force a
   partially compacted arena (a pass at budget 1, abandoned), then
   splice, and verify. This is the specific hazard that motivates G2.
4. `the_free_cursor_never_skips_a_hole` — after an interleaving of
   frees, allocations and a compaction pass, no slot is both marked
   dead and above the cursor. Checked directly, since the invariant is
   what allocation's correctness rests on.
5. `a_fresh_node_in_a_reused_slot_is_still_a_descent_target` — the S4
   argument, exercised where it could fail: a splice into a slot whose
   previous occupant was unmarked, followed by a batch that must reach
   the new node.
6. `an_index_held_across_a_splice_still_names_its_own_node` — the S5
   audit as a test: cursor, first node, override target, folds and
   heat rechecks all name the right content after a retype that frees
   and reallocates around them.
7. The spec 0202 reproduction, extended: the same three
   `t`/`Enter`/`o`/`d` cycles on a doubled `googleapis.desc`, asserting
   the *peak* rather than the settled figure. This is the measurement
   spec 0203 could not make.

## Alternatives considered

### Extent-granular free lists

Keep a sorted list of free extents, coalesce on free, serve a request
from one extent where possible. Preserves `base`-relative addressing
outright and keeps a run in one place.

**Not taken, on cost rather than on principle.** It needs a sorted
structure, insert-with-coalesce, a fit policy and an argument that the
policy does not degrade; slots need a push and a scan and cannot
fragment at all. The benefit it buys over address-ordered first fit is
locality that first fit largely supplies anyway (see Background), and
which a 264-byte node — five cache lines, ~15 to a page — cannot
concentrate much further regardless. The arena is traversed by
pointer-chasing along `doc_next`/`parent` chains that post-order layout
already makes non-sequential, so there is less contiguity to lose than
the extent argument assumes.

The thing that would actually favor extents is `translate`'s gather:
`slots[i]` over an `m`-entry table, which on a root retype is 18 MB of
random access on the batch's hot path. That is measurable after the
fact — count the extents a real batch produces — and if it matters, the
fix does not need extents either: a run that lands in few segments can
be translated by binary search over those segments.

### A summary level over the bitmap

One bit per 64-word block, set if the block holds a free slot. Turns
the scan from amortized `O(n)` per batch into `O(n/4096)`.

**Not taken yet.** The worst case it addresses is a batch that
allocates more than it frees onto an arena whose holes are all high,
and that batch is already Ω(n) in its own right — it constructs
millions of 264-byte nodes with a heap string apiece. `compact_slice`
runs the identical scan today (`compact.rs:212`, `:222`) and has never
been what costs. Adding it now would be optimizing a scan nobody has
measured. `dead` as a `Vec<u64>` bitset is the cheaper half of the same
idea (560 KB instead of 4.5 MB, `trailing_zeros` for the position) and
is equally deferred.

### Run-granular allocation, with `malloc`-like extents outside the `Vec`

Allocate each splice's run as its own block, 264-byte aligned so that
`(addr - base) / 264` still yields a `usize` index.

**Not taken.** It introduces a second level of identity — runs, which
today are not objects at all: no header, no record, nothing points to
one, and `base` is a local variable. Freeing at run granularity then
needs a per-run live count, because no run is ever fully dead (the
local root's slot dies at birth, `:2638`) and nested overrides
(spec 0118) kill proper subsets — so a single surviving node pins its
whole run. Node granularity has none of these problems and is what the
arena's own structure already supports.

## Measured outcome

*(to be filled in on implementation)*

The figures to report, against the spec 0203 baseline of a flat
4 499 336 at batch start and 8 998 671 at batch end:

- `tree.len()` at batch start and end, expected equal;
- peak RSS during a batch, against the 3.9–4.0 GiB spec 0203 leaves
  unchanged — this is the number the spec exists for;
- settled RSS across six batches, against 2594 → 2722 MiB, which
  includes a +128 MiB drift spec 0203 could not attribute to the arena
  and which this spec does not claim to fix;
- whether spec 0202's guard fires, and whether compaction's gate ever
  opens.
