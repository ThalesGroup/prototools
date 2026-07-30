<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0211 — the arena's links are half as wide

Status: implemented
Implemented in: 2026-07-30
App: protolens
Refs: docs/protolens/design/arena-and-batch.md (the redesign brief; its
        annex is the plan this spec executes one row of),
      docs/protolens/rendering-scaling-roadmap.md S12 and
        docs/protolens/rendering-worklist.md W25 (the earlier plan; both
        predate the annex and quote stale sizes — see Background),
      docs/specs/0181-delete-natural-annotation.md (the only row of that
        plan that has landed so far),
      docs/specs/0192-a-frame-costs-the-same-wherever-the-cursor-is.md
        (`sibling_ordinal`, added to the slot after S12 was written),
      docs/specs/0202-an-override-is-refused-rather-than-fatal.md (the
        headroom guard, which reads `size_of::<TreeNode>()`),
      docs/specs/0203-the-override-arena-is-compacted.md (`compact.rs`
        rewrites links; `verify_arena` is this spec's safety net),
      docs/specs/0206-the-arena-reuses-its-dead-slots.md (drafted; it
        wants a settled index type, which this spec provides),
      docs/specs/0210-a-node-counts-its-own-lines.md (added the two
        line counters, taking the slot to 272 B)

## Background

`protolens` opening `googleapis.desc` holds 4 501 014 nodes. Committing
one document-wide override takes the process to a measured
3.9–4.0 GiB peak, which
`docs/protolens/design/arena-and-batch.md` breaks into three roughly
equal terms: the arena's superseded half (~1.37 GB), `local_tree`
(1.19 GB), and the render cache's clone (432 MB).

That brief's annex makes the case that **the per-node constant is the
single largest lever**, worth about 2.2 GB, because the same number is
paid four separate times — the live arena, the arena's superseded half
mid-batch, the throwaway `local_tree`, and the render cache's span copy.
It is also the only lever that is pure arithmetic: no invariant of the
pipeline changes and nothing about *when* a node may be read is
affected. The annex enumerates eleven rows taking the slot from 264 B to
72 B.

Two things have moved since it was written. Spec 0210 added
`lines_total` and `lines_visible`, so the slot is **272 B** today, not
264. And spec 0210's S11 established that `span.text_range` is
build-time-only, which turns the annex's row 3 from an 8-byte narrowing
into a 16-byte deletion.

### Why the links, and why first

Seven of the slot's fields are links to other nodes:

```rust
pub parent: Option<usize>,
pub first_child: Option<usize>,
pub last_child: Option<usize>,
pub next_sibling: Option<usize>,
pub prev_sibling: Option<usize>,
pub doc_next: Option<usize>,
pub doc_prev: Option<usize>,
```

`Option<usize>` is 16 bytes — 8 for the payload and 8 more for a
discriminant that uses one bit of them — so the links are **112 of the
272 bytes, 41% of the slot**, to hold indices that have never come close
to needing 32 bits. The largest `tree.len()` ever observed here is
13 499 684, with `capacity()` 18 004 056, both from the run that ended
in the OOM kill. `u32` affords 4 294 967 295: a 238× margin over the
worst case ever recorded, and over 900× over a healthy one.

This spec does that row and only that row. Three reasons it goes first
rather than following the annex's own suggested order:

1. **It is the largest single row** — 88 B of the 200 that separate
   272 from 72.
2. **It is entirely inside protolens.** The links are `TreeNode`'s own
   fields. Every other large row (`type_fqdn`, `raw_range`, `level`,
   `wire_type`, `is_message`, `packed_record_start`) lives on
   `prototext-core`'s `NodeSpan`, and reaching them means first
   flattening the span into a protolens-owned node — a bigger,
   differently-shaped change, and the subject of the next spec.
3. **It settles the index type.** Spec 0206's slot reuse, and any
   future compact window store, both want to name a node in something
   narrower than a pointer. Deciding that once, here, is cheaper than
   deciding it three times.

The call sites are disjoint from the next spec's, too: 241 accesses of
the seven links, against 175 accesses of `.span.<field>`.

## Goals

- **G1.** `size_of::<TreeNode>()` goes 272 → 184 B, and a compile-time
  assertion prevents it silently going back up.
- **G2.** One index type and one sentinel, each named in exactly one
  place, so the ceiling is one line to revisit rather than a repo-wide
  `usize` hunt.
- **G3.** Reading a link keeps its present shape at the call site. The
  201 read sites are `if let Some(p) = node.parent`-shaped; none of that
  `Option` logic is rewritten by hand, because hand-rewriting 201
  `Some`/`None` decisions into sentinel comparisons is how a silent
  navigation bug gets in.
- **G4.** No user-visible behavior changes at all: same rendering, same
  navigation, same overrides, same messages.

## Non-goals

- **N1.** The rest of the annex. The intended sequence after this spec
  is: flatten `NodeSpan` into a protolens-owned node and narrow its
  scalars (184 → 136 B), then intern `type_fqdn` and `rendered_as`
  (136 → 72 B). Neither is specified here.
- **N2.** The hot/cold column split (annex row 11). It is a refactor,
  not a retype, and its case should be re-argued against the navigation
  profile after the slot has shrunk.
- **N3.** The three transient levers — slot reuse (spec 0206), building
  `local_tree` in place, and the render cache's own-vs-share split. This
  spec makes each of their remaining terms smaller and none of them
  unnecessary.
- **N4.** Re-deciding the refusal guard's `STRING_ALLOWANCE = 64`. That
  constant exists to cover the per-node `String` heap, which this spec
  does not touch; `per_node` correctly falls from 336 to 248 by picking
  up the new `size_of`. Only the stale figure in its comment ("~305
  B/node against a 250 B struct") is corrected.
- **N5.** Narrowing the *other* structures that hold node indices —
  `folded`, `pending_heat_recheck`, `cursor`, `first_node`,
  `override_target`. They are not per-node costs, and the accessors
  specified below deliberately keep handing out `usize` so those stay
  untouched.
- **N6.** Spec 0210's S7–S9 (moving the rendered text into the nodes).
  It would add a field to this same slot and is deferred until the slot
  work concludes.

## Implementation steps

1. Introduce `NodeIdx`, `NO_NODE`, the seven narrowed fields and their
   accessors in `decode.rs`. The tree does not compile at this point.
2. Convert the 201 read sites. Compiler-driven; no judgement calls.
3. Convert the 40 write sites — `decode.rs` 7, `compact.rs` 14,
   `override_apply.rs` 13, and 6 in tests. These are the ones that
   matter; see the Test plan.
4. Add the size assertion and correct the two stale comments.
5. Measure RSS at rest and the commit peak on `googleapis.desc`.

## Specification

### S1 — the index type

In `protolens/src/decode.rs`, next to `TreeNode`:

```rust
/// How a node names another node. The arena is indexed by `usize`
/// (it is a `Vec`), but a *stored* index only ever has to span the
/// arena, and the largest arena ever observed here held 13 499 684
/// nodes at a capacity of 18 004 056 — a 238x margin under `u32`.
/// Paying 8 bytes per link for that, twice over once `Option`'s
/// discriminant is counted, cost 112 of the slot's 272 bytes.
pub type NodeIdx = u32;

/// The absent link. `u32::MAX` and not `0`, because index 0 is a
/// real node: `build_tree` is post-order, so slot 0 holds a leaf.
/// An index-plus-one encoding would let `Option<NonZeroU32>` carry
/// absence for free, but it puts an off-by-one at all 241 link
/// sites for a saving of nothing — `NodeIdx::MAX` is already
/// unreachable, so the sentinel costs no representable index.
pub const NO_NODE: NodeIdx = NodeIdx::MAX;
```

### S2 — the fields and their accessors

The seven fields become `NodeIdx` and lose `pub`, so that `NO_NODE`
cannot leak into arithmetic by accident:

```rust
parent: NodeIdx,
first_child: NodeIdx,
last_child: NodeIdx,
next_sibling: NodeIdx,
prev_sibling: NodeIdx,
doc_next: NodeIdx,
doc_prev: NodeIdx,
```

Each gains an inline getter returning `Option<usize>` and an inline
setter taking one:

```rust
#[inline]
pub fn parent(&self) -> Option<usize> {
    (self.parent != NO_NODE).then_some(self.parent as usize)
}

#[inline]
pub fn set_parent(&mut self, idx: Option<usize>) {
    self.parent = Self::pack(idx);
}
```

with one shared

```rust
#[inline]
fn pack(idx: Option<usize>) -> NodeIdx {
    match idx {
        Some(i) => {
            debug_assert!(i < NO_NODE as usize, "node index {i} exceeds NodeIdx");
            i as NodeIdx
        }
        None => NO_NODE,
    }
}
```

The getters return `Option<usize>` rather than `Option<NodeIdx>`
deliberately (G3, N5): `self.tree[p]` and every index-keyed structure
keep working unchanged, and the conversion compiles away. The cost is
that a later spec narrowing `cursor` and friends will revisit these
signatures; that is the right trade against rewriting 201 call sites
twice.

### S3 — the ceiling

`pack`'s `debug_assert` is not the guard. The thing that keeps a
document from approaching 4.29 G nodes is spec 0202's refusal, which
already refuses any batch whose arena would not fit in half of
`MemAvailable` — at 184 B/node that bounds the arena by available memory
long before it bounds it by the index type. `NodeIdx` is therefore
documented as resting on that guard rather than on a runtime check in
the hot path.

The one place the arena grows is `splice_override`'s push loop. It gets
the only *checked* conversion, phrased as a `debug_assert` on
`tree.len()` after the loop, so a release build pays nothing.

### S4 — the size assertion

Beside `TreeNode`:

```rust
const _: () = assert!(
    std::mem::size_of::<TreeNode>() == 184,
    "the arena slot is paid 4.5 M times over on a large descriptor set; \
     see docs/protolens/design/arena-and-batch.md's annex before changing it"
);
```

An equality and not a `<=`: the point is to notice movement in either
direction, and a growth of the slot is exactly the regression the annex
is trying to prevent. A spec that legitimately changes the number
changes this line and says so.

### S5 — the stale comments

- `override_apply.rs:1314-1317`'s `STRING_ALLOWANCE` comment says
  "measured ~305 B/node against a 250 B struct". The struct was 264 when
  that was written, is 272 now, and becomes 184 here. Restate it as the
  string-heap allowance it is, with the current numbers.
- `arena-and-batch.md`'s annex table and its "eleven opportunities"
  list: mark row 1 done, and correct the running total to the 184 B this
  spec lands.

## Test plan

1. **S4's assertion** is the primary test: it fails at compile time if
   the layout is not what this spec claims.
2. **The existing suite is the regression test.** No behavior change is
   intended, so all 524 protolens tests must pass unchanged. Three
   groups carry most of the weight:
   - `App::verify_arena`'s four well-formedness properties
     (`compact.rs:302`), which run from the tail of
     `assert_line_counts_are_exact` (`override_apply.rs:1945`) and so
     are witnessed by every override test in the suite, including the
     randomized sequences. A link written with a wrong sentinel is
     exactly what they catch.
   - the `compact.rs` suite: 14 of the 40 write sites are in
     `relocate_node`, where a mis-set link corrupts the arena silently
     and permanently. `tests/compact.rs:247-283` additionally checks
     that `verify_arena` *fails* on four deliberately broken arenas, so
     the safety net itself is pinned.
   - `assert_line_counts_are_exact` (spec 0210) itself, which walks
     children via `first_child`/`next_sibling` and would notice a
     truncated chain.
3. **The three refusal-guard tests** at `tests/override_apply.rs:2643`,
   `:2690` and `:2759` compute `per_node` from `size_of::<TreeNode>()`
   themselves, so they follow the new size automatically. Confirm they
   still assert what they mean rather than merely still passing.
4. **A new test that a link round-trips through `None`.** The one
   behavior the accessors could plausibly get wrong is confusing "no
   link" with "node 4 294 967 295": set every one of the seven links to
   `Some(0)` and then to `None`, and read each back. Index 0
   specifically, because it is the value a `NonZeroU32` encoding would
   have gotten wrong.
5. **Measurement**, on `googleapis.desc` via the pty driver:
   RSS at rest, and the peak across one document-wide override. Expected
   at rest: 4 501 014 × 88 B ≈ 0.40 GB less. Expected peak: about 1.1 GB
   less, since the constant is paid on the live arena, on its superseded
   half, and on `local_tree`, but not on the render cache's `NodeSpan`
   copy, which holds no links. Record what actually happens — the
   accounted peak already has a 432 MB discrepancy the brief flags as
   unexplained, so a prediction is not a result.

## Open questions

1. **Does `sibling_ordinal` want to move next to the links?** At `u32`
   it packs with them into 32 contiguous bytes, which is most of a cache
   line and is what annex row 11's hot column would want. Reordering
   fields costs nothing and changes no size. Left out of this spec
   because it is a bet on row 11, which N2 defers.
2. **Do the getters want to be `Option<NodeIdx>` after all?** Only if a
   later spec narrows the arena's other index holders, at which point
   the `as usize` conversions this spec keeps would become
   `as usize`-in-reverse. Revisit then, with the call sites in hand.
3. **Is 40 write sites really all of them?** The count is from
   `\.(parent|first_child|...)\s*=`, which misses a struct literal that
   sets the fields positionally. `TreeNode` has no `Default` and derives
   only `Debug`, so every construction is a literal naming its fields —
   `decode.rs`'s `build_tree` and `extract.rs`'s test module are the two
   known ones. Making the fields private turns any missed literal into a
   compile error, so this resolves itself during step 1.

## Measured outcome

Peak RSS on `googleapis.desc` (4 501 014 nodes, 25.6 MB), via the
non-interactive batch repro — `export /1 [--load-overrides ovr.yaml]`
under `command time -v`, with `PROTOLENS_NO_MEMORY_GUARD=1` so that the
guard's own change of threshold does not confound the comparison.
Baseline is commit `bee6985` built in a throwaway worktree; output is
byte-identical between the two.

| | baseline (272 B) | this spec (184 B) | delta |
|---|---|---|---|
| override applied | 2 410 248 kB | 2 023 616 kB | −386 632 kB |
| no override | 2 410 320 kB | 2 023 368 kB | −386 952 kB |
| predicted, 4 501 014 × 88 B | | | −386 806 kB |

The prediction lands within 0.04%, which confirms both the arithmetic
and the node count. G1's 88 B is real memory, not just a smaller
`size_of`.

Two things the batch run also settled, neither of them a goal of this
spec:

- **This workload does not exercise the three-copy peak.** Applied and
  not-applied cost the same to within noise, so the override commit's
  transients stay below the decode peak here and the whole 387 MB is the
  arena at rest — one copy of the slot, not the three the annex's peak
  model prices. The annex's 3.9-4.0 GiB figure for this same command is
  **stale**: `c6e91f9` (on-demand descriptor set) is the likely bulk of
  the missing 1.6 GiB, with specs 0203 and 0210 taking the rest.
- **The three-copy model needed the interactive path**, and it was
  measured there. See below.

### The peak prices the slot 2.3 times, not 3

Driven under a pty (`/tmp/measure_0211_tui.py`): open the same blob, wait
for the first frame, then `:type-as-raw` on **line 0** — the root's own
header, so the commit rewrites the whole document. This is the only
commit that materializes a full-document `local_tree`; `:type-as-raw` one
row lower, on the first top-level record, is a 35-line subtree and moves
peak RSS not at all. Memory figures only; a pty driver's timings are not
user-facing latency.

| | baseline (272 B) | this spec (184 B) | delta | in units of 4 501 014 × 88 B |
|---|---|---|---|---|
| `VmRSS` at rest | 1 959 900 kB | 1 573 032 kB | −386 868 kB | **1.00** |
| `VmHWM`, root retype | 4 379 916 kB | 3 493 144 kB | −886 772 kB | **2.29** |
| `VmRSS` after the commit | 2 152 308 kB | 1 902 200 kB | −250 108 kB | 0.65 |

The peak falls **4.18 → 3.33 GiB, −20.2%**, from one row of the annex's
slot plan. The at-rest column reproduces the batch result exactly.

**Why 2.29 and not 3.** The annex prices the slot three times at the peak
— the surviving arena, the arena's new half, and `local_tree` — and calls
the three terms "roughly equal". They are equal only when the replacement
subtree is about the same size as the original. Here it is not: the
document is 5 281 124 lines and its raw-message replacement is 1 487 288,
so the two terms that scale with the *new* tree are each well under
4.5 M nodes while the surviving arena is exactly 4.5 M. Solving
`1 + 2 × N_new / 4.5 M = 2.29` puts the replacement at ≈2.9 M nodes,
which is consistent with its line count.

Two consequences for the specs that follow:

- The multiplier at the peak is **a property of the workload**, not of
  the slot. It is bounded above by 3 and reaches 3 only for a
  size-preserving retype. Spec 0212 should quote a range, or quote this
  workload by name, rather than assume 3.
- The render cache's clone copies `NodeSpan`s, which this spec does not
  narrow, so it contributes 0 here. That term only starts moving once the
  scalars (rows 2-8) shrink `NodeSpan`'s protolens-local replacement —
  which is a reason to expect spec 0212's measured multiplier to be
  *higher* than this one's, not lower.

The last row is not evidence of anything: compaction runs in idle time
(spec 0203), so the post-commit sample catches each binary at a different
point in the reclaim and the two are not comparable.
