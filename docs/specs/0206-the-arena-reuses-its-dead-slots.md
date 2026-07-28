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
        `mark_dead`, `verify_arena`, and the rejection this revises),
      docs/specs/0207-where-the-override-memory-work-stands.md (the
        wrap-up: the other two transients, and the open questions this
        spec depends on)

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
rather than 8 998 671 — and the arena's peak is prevented rather than
reclaimed. On the measured workload a root retype frees 4.5 M nodes and
draws 4.5 M back, growing by nothing.

### What this does not fix, and how much of the peak it leaves

The first draft of this spec said the process peak disappears. It does
not, and the correction matters enough to state before the goals.

A root retype has three large transients live at once, and the arena is
only one of them:

| transient | on a 4 499 336-node root retype |
|---|---|
| the arena's second copy | ~1.37 GB, at ~305 B/node — **what this spec removes** |
| `local_tree` (`override_apply.rs:2566`) | 4.5 M × 264 B = 1.19 GB |
| `render_cache`'s clone of the render | 4.5 M × `size_of::<NodeSpan>()` = 432 MB, plus a second `Vec<String>` of every rendered line |

`local_tree` is `build_tree`'s output, built in full before the push
loop at `:2567` consumes it, and `Vec` does not release its buffer
incrementally — so all 1.19 GB stays resident until the loop ends,
which is precisely when the arena is also at its widest. The render
cache's copy comes from `:2908`'s `value.clone()` on a miss and
`render_cache.rs:77`'s clone on a hit, so it is paid on every path.

The arithmetic, stated so it can be checked rather than believed. The
arena at 0203's peak is 8 998 671 × ~305 B = 2.74 GB; plus
`local_tree`'s 1.19 GB that is 3.93 GB, against a measured 3.9–4.0
GiB. The two largest transients therefore account for the whole
observed figure on their own — which means the render cache's 432 MB
is *not* visible in it, and one of three things is true: the
~305 B/node estimate is generous, the sampling window missed the
insert, or the cache had been evicted. That discrepancy is not
resolved and should be resolved by the measurement this spec produces,
rather than reasoned about now.

Removing the arena's garbage half leaves 1.37 GB of arena plus
`local_tree`'s 1.19 GB, so the expected peak under this spec is around
2.6 GB — not the size of the live set.

The other two are separate work, deliberately not folded in here: they
are changes to the *splice*, whereas this spec is a change to the
*allocator*, and the in-place variant of `local_tree` depends on this
spec having landed first (a node can only be built in place once its
destination slot is known). Both are carried in spec 0207.

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
  set. The arena's transient double on a retype is removed, not
  reclaimed. This is a goal about the arena; the process peak is
  addressed only in proportion (see "What this does not fix").
- **G2.** There is exactly one record of which slots are free. `dead`
  is it; no second structure exists that can disagree with it.
- **G3.** In the expected case allocation adds no term to a batch that
  the batch does not already pay: the cursor is monotone, so the scan
  is amortized across the batch's own allocations rather than repeated
  per node. This is not a worst-case claim. A batch that frees little,
  allocates little, and meets an arena whose holes are all high pays
  an `O(tree.len())` scan for a handful of slots. That shape is
  admitted rather than prevented, and the summary level that would
  bound it is N5.
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
- `mark_dead(idx)` lowers `free_cursor` to `min(free_cursor, idx)`.
- `alloc_slots(n) -> Vec<u32>` returns `n` slot indices. It advances
  `free_cursor` to each successive set flag, clearing it and
  decrementing `dead_count`; when the cursor reaches `tree.len()` the
  remaining `k` slots are the range `tree.len()..tree.len() + k`,
  reserved by growing `tree`, `heat_states`, `dead` and `descend`
  together.

The plural form is not a convenience. A single `alloc_slot()` cannot
"fall back to `tree.push`", because at the moment a slot is reserved
there is no `TreeNode` to push: `TreeNode` derives only `Debug`
(`decode.rs:479`) — no `Default`, no `Clone` — and S3 requires every
slot to be known *before* the first node is written. Reservation and
writing are therefore two steps, and the tail has to be reserved by
resizing arrays that can be resized (`heat_states`, `dead`, `descend`)
while `tree` itself is grown by the push loop that follows, which fills
exactly those indices in order. Whichever way the implementation
arranges that, it must state the arrangement: this is the one place
where "reserved" and "initialized" come apart, and a panic between the
two would leave the arena short.

An alternative that avoids the split is to give `TreeNode` a cheap
`Default` (all links `None`, an empty span) and reserve the tail with
`resize_with`. It costs one write per fresh node that the push loop
immediately overwrites. Worth taking if the two-step version turns out
to need a guard.

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
obtained from one `alloc_slots(local_len)` call **before** the push
loop begins. Up front rather than per node, because `build_tree` emits
a post-order local tree whose nodes name each other in both directions:
`translate` resolves forward references, so every slot must be known
before the first node is written.

Then `translate = |o| o.map(|i| slots[i] as usize)`, the parent case
takes the same map, `new_self_idx = slots[local_root_idx]`, and the
document-order scan at `:2690` iterates `slots` instead of a range.

`slots` is itself a transient the arena did not previously carry: 4 B
per local node, 18 MB on a root retype, and `translate` gathers from it
at random on the batch's hot path. Small against the 1.19 GB
`local_tree` beside it, and it disappears entirely if spec 0207's
in-place construction lands (`build_tree` would emit global indices
directly and there would be nothing to translate) — but it is a real
term and it is what the "Alternatives considered" extent argument turns
on, so it is stated here rather than left implicit.

`mark_fresh_subtree(base, path)` (`:1375`, called at `:1659`) takes the
slot list instead of a base, and `collect_descend_targets` is given
that list rather than the range `base..tree.len()`. Its
`if self.tree.len() <= base { return; }` guard (`:1376`) goes with the
base: under reuse `tree.len()` does not grow, so the guard as written
would fire on the common case and mark nothing at all. See S4.

The local root's slot is allocated and then immediately marked dead at
`:2638`, as today — its span and child links are folded into the
surviving `tree[idx]` (spec 0118 §7). Allocating a slot only to free it
is deliberate: the node has to be materialized somewhere in order to be
read, and special-casing it would buy one slot per splice at the cost
of a second code path through the translation.

That slot is read *after* it is freed, which under reuse is no longer
a free action. `:2639` (`span.clone()`), `:2646-2647` (the child links)
and `:2689` (`self.tree[new_self_idx].doc_next`, fifty lines later)
all dereference a slot that `dead` already advertises as available.
Nothing allocates in between today, so it is correct today; it is
correct only by accident of ordering, and S5 states the constraint.

### S4. `descend`'s watermark survives, and why

Spec 0188 S4 gave `descend.len()` a second meaning: the length of the
arena prefix already examined for descent targets.
`compute_descend_marks` (`:1339-1352`) reads it as `scanned` and
examines only `scanned..tree.len()`. That identity assumes the arena
grows only at the end, which is exactly what this spec stops.

It nevertheless holds, for a reason worth stating rather than assuming
— and the reason is not the one an earlier draft of this section gave.

`descend.len() == tree.len()` is **not** a standing invariant.
`descend` is `Vec::new()` at construction (`mod.rs:1564`) while the
arena is already full, so before the first batch the watermark is 0 and
the whole arena is unexamined, which is exactly what the watermark is
for. What is true is narrower: the `resize` calls at `:1343` and
`:1379` raise `descend.len()` to `tree.len()` whenever they run, so
*after* a batch the two agree. Under reuse `tree.len()` does not grow,
so both resizes become no-ops, `scanned == tree.len()`, and
`collect_descend_targets(scanned, tree.len())` scans an empty range.
That is the intended outcome, not a degradation.

Fresh nodes landing in reused slots are not covered by that range and
are instead marked explicitly by `mark_fresh_subtree`, which S3 gives
the actual slot list. Every fresh node is therefore examined exactly
once, by name. This is load-bearing rather than incidental: with the
suffix empty, `mark_fresh_subtree` becomes the *only* thing that marks
a fresh node, where today the suffix scan would have caught it anyway.
Its `tree.len() <= base` early return (`:1376`) must go with the base
for the same reason — under reuse it would fire on every splice.

The claim that spec 0188's other two per-node target sources need no
re-examination (a node's auto-expand eligibility, and `rendered_as`
going `None` → `Some`) was checked against `compute_descend_marks`'s
own reasoning only. It has *not* been checked against the
override-activation path, which is where a source could change without
the node being re-decoded. That verification is a precondition of
implementing this section.

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
`:2436` and the first allocation at `:2555`. Every index computed
before the free and read after the allocation is a candidate. The audit
is over that span of one function, not over the program: `after` and
`packed_next_sibling_of_run` are the indices that cross it, and each
must be shown to lie outside the freed set (`after` is the
document-order successor of the old subtree; `packed_next_sibling_of_
run` is past the absorbed run, captured at `:2389` *before* the free).
`packed_run_is_last_child` is a `bool`, not an index, and carries no
aliasing risk. This audit is part of the implementation, and its
conclusion belongs in a comment at `:2436` where the free happens.

**Two orderings inside the splice become load-bearing.** Both are
satisfied by the code as it stands, and neither is stated anywhere,
which is the problem: an innocuous reordering would break them
silently.

1. *Everything that reads a freed node's links must precede the first
   allocation.* `doc_next_after_subtree` (`:2479`) walks
   `doc_next` chains **through** the freed subtree to find the seam,
   and the holder repair (`:2454-2467`) tests `cursor`, `first_node`,
   `override_target` and `pending_heat_recheck` against
   `old_descendants`. Both run on slots `dead` has already released.
   They are at `:2454-2479` and allocation is at `:2555`, so the order
   holds — by fifty lines of unrelated code, not by construction.
2. *The local root's slot must not be reallocated between `:2638` and
   `:2689.`* It is marked dead at `:2638` and read at `:2639`,
   `:2646-2647` and `:2689`. No allocation intervenes within one
   splice, and a nested splice (spec 0118) is a separate call that
   cannot begin mid-function. Still true, still unstated.

The cheapest way to make both durable is to assert them rather than
comment them: a debug-only "no allocation has occurred since" counter
checked at `:2479` and `:2689` costs nothing in release and fails
loudly on the reordering that would otherwise be silent.

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

`reset_compaction` (`compact.rs:271`) becomes load-bearing, and S6 has
to say so. `splice_override` calls it at `:2362` to abandon a
half-finished pass, because a pass's two cursors were taken against an
arena that has since changed underneath them. That call is today an
optimization — an abandoned pass leaves a partially compacted but
entirely consistent arena. Under reuse it is a correctness
requirement: a live pass and the allocator are two writers of `dead`,
and `relocate_node` clears a flag the allocator may have just cleared
for a slot it is about to fill. Test-plan item 3 exists for exactly
this, and the call site deserves a comment saying which of the two
roles it is playing.

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
Item 7 is not a test. The spec 0202 reproduction — the same three
`t`/`Enter`/`o`/`d` cycles on a doubled `googleapis.desc` under the pty
driver, sampling peak RSS rather than the settled figure — is a manual
measurement, and it belongs under "Measured outcome". No fixture
reaches the scale at which the arena's peak is observable, which is
also why S5 requires the verifier to be reachable from the shipping
binary.

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
  unchanged. Expected around 2.6 GB, **not** the live-set size: this
  spec removes one of three concurrent transients, and "What this does
  not fix" itemizes the other two. A result near the live set would
  mean the accounting there is wrong, and is as interesting as a
  result near 2.6 GB;
- settled RSS across six batches, against 2594 → 2722 MiB, which
  includes a +128 MiB drift spec 0203 could not attribute to the arena
  and which this spec does not claim to fix;
- whether spec 0202's guard fires, and whether compaction's gate ever
  opens.
