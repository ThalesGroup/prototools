<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0256 — the confirm stops paying for the document it replaces

Status: implemented
Implemented in: 2026-08-08
App: protolens
Refs: docs/specs/0222-a-node-owns-the-lines-it-draws.md (S1, the
        per-slot `Box<str>` these frees are freeing),
      docs/specs/0249-a-large-document-answers-the-user-first.md (S1/S3,
        the row budget that removed the *render* from the confirm and
        left this behind; S9, whose stale bitset this replaces with
        something smaller; open question 6, whose answer this spec
        overturns),
      docs/specs/0255-the-document-finishes-itself-while-nobody-waits.md
        (the idle arm this hands the freeing to, and rule 2's
        `bounded_confirms` flag),
      docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (the
        immutable `parent()` array the fold scrub walks)

## Background

Spec 0249's row budget took the renderer out of an override confirm —
`render_node_as` is 5 ms of a 448 ms root override on `googleapis.desc`.
What is left is not rendering at all. It is `splice_override` taking the
*previous* interpretation apart, one slot at a time, over all 2 864 189
of them.

Measured on `googleapis.desc` (25.6 MB), `/` overridden to
`google.protobuf.FileDescriptorSet`, 50-row pane, release build pinned
with `taskset -c 4-7`, timers around each phase of `splice_override`:

| phase | s | % |
|---|---|---|
| `render_node_as` (bounded) | 0.005 | 1 |
| `collect_descendants` | 0.069 | 15 |
| vacate: `unfold(d)` | 0.078 | 17 |
| vacate: `node_text[d] = None` | 0.056 | 13 |
| vacate: `tree[d] = vacant()` | 0.015 | 3 |
| vacate: `heat_states[d]` | 0.007 | 2 |
| vacate: `clear_status(d)` | 0.004 | 1 |
| **first `malloc` after the vacate loop** | **0.195** | **44** |
| everything else | 0.019 | 4 |
| **total** | **0.448** | |

### The 0.195 s is the frees, charged somewhere else

That row is two `vec![NO_NODE; 7821]` allocations at the top of
`slots_for_spans`. Seven thousand elements do not cost 195 ms; what
costs 195 ms is that they are the first allocation request after
2 864 189 `Box<str>` frees. glibc puts small frees on fastbins without
coalescing and defers `malloc_consolidate` to the next request that
cannot be served from a bin — so the whole coalescing pass is billed to
whoever allocates next.

Established by control, not by inference: replacing `node_text[d] =
None` with `std::mem::forget` — identical work otherwise — takes the
confirm from **448 ms to 189 ms** and takes that row to **0.000 s**.

So freeing the old document's text costs **0.251 s, 56% of the confirm**,
and only a fifth of it is visible at the free site.

**This overturns spec 0249 open question 6**, which measured 47 ms at
the free site, concluded the frees were not worth deferring, and
withdrew the idea. The 47 ms was right and the conclusion was wrong: it
timed the `free` calls and not the consolidation they deferred. The
0.21 s that has been attributed to `overlay_spans` ever since — spec
0255's "component that does not shrink when 5.28 M lines become
15 593" — was never `overlay_spans`' work at all. It has no fixed cost:
in the bake, `overlay_spans` runs 70 893 times for 0.44 s total.

### Why the rest is what it is

`unfold(d)` is two `HashSet::remove`s per descendant — 5.7 M hash
lookups at 13.6 ns each — to clear fold flags from a set that holds tens
of entries. The work is proportional to the document; the answer is
proportional to the fold sets.

`collect_descendants` (69 ms) and the three remaining per-slot writes
(26 ms) are irreducible without making invalidation lazy. See N2.

## Goals

- **G1.** An override confirm does not free the previous
  interpretation's text on the keystroke.
- **G2.** Clearing fold state costs the size of the fold sets, not the
  size of the document.
- **G3.** No change to what any confirm produces: the document, the
  fold sets, the status array and the exported bytes are what they are
  today, at every row budget and with no budget at all.

## Non-goals

- **N1.** Making the freeing cheaper. It is deferred, not avoided —
  roughly the same 0.25 s is paid by the idle loop, in bounded chunks,
  where nobody is waiting for it. Total work is conserved on purpose:
  an attempt to *reduce* it means changing spec 0222's one-allocation-
  per-slot text layout, which is a different spec.
- **N2.** Removing `collect_descendants` and the per-slot writes
  (95 ms combined). That needs invalidation to become lazy — an epoch
  stamped on each slot, with every reader of `tree`, `node_text`,
  `heat_states` and the status array checking it. Spec 0249 S9 proposed
  a version of this for the bake queue and spec 0255 S3 disposed of it
  there (`auto_folded` is the truth); the remaining case is this one,
  and 95 ms does not pay for putting a version check in front of four
  hot arrays. Recorded with its number so the next reader can weigh it
  rather than re-derive it.
- **N3.** The bake's own profile. The same instrumentation shows the
  5.5 s drain spending 1.97 s in `refresh_line_counts` — which carries
  a *delta* up the ancestors (spec 0254) but still re-sums the first
  node's own children by walking its sibling chain, and the stops
  directly under a 7 771-child root each pay that walk. Background
  time, a different fix, its own spec.
- **N4.** Anything about `--load-overrides` in a headless `export`.
  There is no idle loop there, so there is nothing to defer to; see S3.

## Specification

### S1. The old text is moved aside, not dropped

`App` gains `discarded_text: Vec<Box<str>>`. The vacate loop in
`splice_override` moves each descendant's text into it instead of
dropping it:

```rust
if let Some(t) = self.node_text[d].take() {
    self.discarded_text.push(t);
}
```

Every invariant that holds today still holds: `node_text[d]` is `None`
for a vacant slot, and nothing else can reach the moved box. The cost is
a pointer copy and an amortized push.

### S2. `run_loop` drains it, ahead of the bake

A fourth step in the idle arm, before `bake_step`:
`discarded_text.truncate(len - min(len, DISCARD_CHUNK))`.

- **Before the bake**, because the bake is what grows the new document.
  Draining afterwards would hold the old 180 MB of text alive next to
  the new 232 MB and raise peak memory; draining first keeps peak where
  it is today. The 0.25 s it costs the bake is 5% of a 5.5 s drain.
- **It draws no frame.** Nothing on screen depends on it. Unlike the
  bake it does not even owe a deferred repaint, so it needs no
  `*_forces` term (spec 0255's trap does not apply — there is nothing
  to draw).
- `DISCARD_CHUNK` is sized so a step is comfortably under the ~22 ms
  worst bake step; its measured basis goes in a doc comment next to it.

### S3. Deferral is conditional on there being a loop to defer to

The move is gated on `bounded_confirms` — spec 0255's flag for "an
event loop is running". A headless `export` frees inline as it does
today, because nothing would ever drain the vector and it would grow to
hold the whole previous document. This is the same failure mode spec
0255 rule 2 records for the row budget, and it reuses the same flag
deliberately: one switch, one meaning, one thing to get right.

### S4. The fold scrub walks the smaller of the two sides

`unfold(d)` per descendant asks the *document* a question about a set
that usually holds tens of entries. The alternative asks each fold set
instead:

```rust
self.folded.retain(|&f| !self.descends_from(f, idx));
self.auto_folded.retain(|&f| !self.descends_from(f, idx));
```

- The ancestry test walks `Arena::parent()`, which is immutable
  (spec 0216) and therefore valid before, during and after the splice —
  unlike the rendered tree, which is being taken apart. It is strict, so
  `idx` itself is never a candidate: spec 0118 §7 keeps a node's own
  fold across its own retype, and `splice_override` removes `idx` from
  `auto_folded` separately.
- Both spellings clear the same flags. The descendant list is the
  rendered subtree, and a fold flag can only stand on a node some
  rendering showed.

Neither spelling is right for every call, so the smaller side is walked:
`retain` when the two sets' combined capacity is below the descendant
count, the per-descendant `unfold` otherwise, and neither when both sets
are empty. This is not hedging. `HashSet::retain` is O(capacity), and
`auto_folded`'s capacity peaks around 84 000 entries mid-bake without
shrinking — a cost the *bake's* 70 893 splices, a handful of descendants
each, would pay 70 893 times over. Measured: retaining unconditionally
takes the confirm to 0.153 s as intended and the drain from 5.4 s to
**17.5 s**. The confirm and the bake want different sides of the same
question, exactly as spec 0255 rule 4 found they want different budgets.

## Alternatives considered

**Leave the box in `node_text[d]` and sweep vacant slots later.** No
extra vector, and `overlay_spans` would drop most of them for free as
the bake re-renders the slots. Rejected: it breaks "a vacant slot holds
no text", which is asserted at the packed-run branch of `overlay_spans`
and assumed by the status and export walks. Buying 23 MB of pointers to
keep an invariant that four places rely on is the right trade.

**Free on a worker thread.** No: `Box<str>` is owned by `App`, so the
boxes would have to be moved to the thread anyway (S1's vector, plus a
channel), and glibc's consolidation contends on the same arena lock the
main thread allocates from. The idle loop already exists and already
has the right shape.

**Swap the global allocator.** A bump or arena allocator for node text
would make the frees free. Out of scope, and it changes the memory
profile of everything else in the process to fix one phase of one
operation.

**Keep `unfold(d)` but only skip it when both sets are empty.** Cheaper
to write, and it does nothing in the case that matters: `auto_folded`
holds 7 770 entries the moment a bounded confirm has run once, which is
exactly when the next confirm happens. Kept as one of S4's two arms
rather than as the whole answer.

## Test plan

1. `a_confirm_moves_the_old_text_aside_instead_of_dropping_it` — after
   a splice with `bounded_confirms`, `discarded_text.len()` is the
   descendant count and `node_text` is `None` at every vacated slot.
2. `a_headless_confirm_frees_inline` — with `bounded_confirms` false,
   `discarded_text` is empty after the same splice. This is the S3
   guard, and the only thing standing between a scripted `export` and
   an unbounded vector.
3. `the_idle_loop_empties_the_discarded_text` — `run_loop` with nothing
   else owed leaves `discarded_text` empty, in more than one step for a
   fixture larger than `DISCARD_CHUNK`.
4. `a_fold_under_a_retyped_node_is_scrubbed` — a user fold and an
   auto-fold, both under `idx`, are gone after the splice; a fold on a
   sibling subtree and `idx`'s own fold survive. This is the mutation
   guard for S4: dropping the `retain` entirely must fail here.
5. `a_baked_document_is_the_unbounded_document` (existing, spec 0255) —
   re-run at three budgets; G3 is what makes this spec safe, and it is
   the assertion that carries it. From `MIN_EXPAND_ROWS` up: a budget of
   1 buys the header and nothing else, so the node stops at itself and
   the walk cannot move down, which is why `confirm_row_budget` clamps
   it away before any caller sees it.

## Measured outcome

Same corpus, same recipe, same pinning as the Background table, and an
A/B on a *single* binary — one switch selecting the inline free and the
per-descendant `unfold`, so the two columns differ in nothing else.
Three pairs, alternating:

| | before | after |
|---|---|---|
| confirm | 0.428 / 0.445 / 0.456 s | **0.153 / 0.159 / 0.160 s** |
| drain | 5.38 / 5.73 / 5.48 s | 5.74 / 5.67 / 5.99 s |
| exported bytes | 232 892 696 | 232 892 696, `cmp`-identical |

**The confirm is 2.8x shorter — 0.44 s to 0.16 s.** Of the 0.28 s, about
0.20 s is the deferred freeing (S1/S2) and 0.078 s is the fold scrub
(S4), which on a first confirm skips entirely because both sets are
empty.

The drain absorbs roughly +0.3 s, ≈5%, which is N1's conserved work
landing where nobody waits for it. It is inside the run-to-run spread of
the before column, which is the honest way to report it.

What is left in the confirm is `collect_descendants` and the three
remaining per-slot writes — N2's 95 ms, plus the render and the splice
itself.
