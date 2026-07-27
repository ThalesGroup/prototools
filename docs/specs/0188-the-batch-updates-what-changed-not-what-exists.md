<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0188 — the batch updates what changed, not what exists

Status: implemented
Implemented in: 2026-07-27
App: protolens
Refs: docs/protolens/rendering-flaws.md (P3, P1),
      docs/protolens/rendering-scaling-roadmap.md (S2, S3, S5),
      docs/specs/0160-protolens-render-overrides-batch-scaling.md,
      docs/specs/0162-protolens-tree-node-reclamation.md,
      docs/specs/0167-protolens-nested-patch-scope.md,
      docs/specs/0183-prune-the-override-walk.md,
      docs/specs/0186-the-commit-touches-only-what-moved.md,
      docs/specs/0187-highlighting-is-a-property-of-the-viewport.md

## Background

Once spec 0187 removes the whole-document syntax parse, what is left of
an override commit is dominated by two passes that still touch the whole
document to apply a change confined to one subtree.

Measured on `/tmp/pdb.desc` after root-type resolution (382,032 nodes /
276,515 lines), committing an override on a genuinely nested target —
node 382029, `google.protobuf.SourceCodeInfo`, lines 276247..276513, two
splices, 757 lines replaced:

| phase | time | share |
|---|---|---|
| line-map repair | 0.069 s | **55.1%** |
| `render_node_as` | 0.031 s | 24.7% |
| ↳ `render_cache` insert (deep clone) | 0.026 s | 20.8% |
| `compute_descend_marks` | 0.017 s | **13.5%** |
| `colorize` | 0.004 s | 3.3% (deleted by 0187) |
| `materialize_line_patches` | 0.003 s | 2.2% |
| `mark_fresh_subtree` | 0.003 s | 2.4% |
| `rebuild_visible_rows_from` | 0.000 s | 0.01% |
| **total** | **0.1255 s** | |

The two entries in bold are this spec. Together they are **69% of a
nested commit**, and both are O(document) for a change that is, by
construction, local.

### Pass 1 — the line maps are scanned in full to shift a suffix

`finalize_override_batch` (`override_apply.rs:1789-1795`) repairs
`line_to_node` and `footer_line_to_node` by dropping every entry at or
after the first disturbed line:

```rust
self.line_to_node.retain(|&line, _| line < from);
self.footer_line_to_node.retain(|&line, _| line < from);
```

**`HashMap::retain` is O(map size) regardless of the predicate.** It
iterates every occupied slot — and, for hashbrown, every control byte —
to decide. Spec 0186 introduced this to replace an O(document)
clear-and-refill, and it succeeded only in replacing an O(document)
*refill* with an O(document) *scan*. No choice of `from` makes it
sublinear.

The measurement above is the most favorable case that exists:
`from = 276023` of `276515` lines, i.e. 99.8% of both maps survive the
predicate — and it still costs 69 ms. The other components of that phase
cannot account for it: the target sat at the very end of the document, so
the "everything after `idx`'s subtree" walk was nearly empty, the
subtree walk covered 757 lines, and the ancestor walk is O(depth). The
two `retain`s are the only O(document) term present.

The root cause is a representation mismatch. The maps are keyed by line
number; line numbers are dense, contiguous, and — this is the point — a
splice *shifts a suffix of them by a constant*. That is a `memmove` in an
array and a full rehash in a hash map. The code is doing the second to
express the first.

### Pass 2 — the impacted-node marks are rebuilt from the whole arena

`compute_descend_marks` (`override_apply.rs:1356`) runs once per batch:

```rust
self.descend = vec![false; self.tree.len()];
let targets = self.collect_descend_targets(0, self.tree.len(), None);
self.mark_targets(targets);
```

That is an allocate-and-zero of `tree.len()` bytes plus a loop over every
node in the arena (`:1433`), at a measured **35-44 ns/node** — 17 ms at
382 k nodes, and ~90 ms at the 2.23 M the arena reaches after two
document-wide commits.

Four things can make a node a target. Three of them do not need a scan:

| source | where | why no scan is needed |
|---|---|---|
| `Path` / `PathField` override origins | `:1468-1491` | **already** resolved per entry, not per node |
| "was spliced before" (`rendered_as.is_some()`) | `:1437` | changes at exactly **two** production sites: `resettle_node` (`:1203`) and `App::new`'s root seeding (`mod.rs:1370`) |
| Any/MessageSet auto-expansion seed | `:1442` | a purely structural predicate over one `NodeSpan`; never changes for a given node |

The fourth — an `FqdnField` origin, "field N of every message of type T,
anywhere in the document" — genuinely is a search with no path to follow.
It is also the rare one: `FqdnField` origins are never auto-seeded. They
exist only when the user deliberately widens an override's scope in the
management pane (`override_pane.rs:113`) or loads a YAML override file,
and `manage_pane.rs:278` records that the widening has no keyboard
shortcut precisely because it is a considered action.

The loop already guards its `FqdnField` test on
`if !fqdn_fields.is_empty()` (`:1447`). The loop itself survives only
because the two cacheable sources sit inside it.

### Why the mark set wants a different shape too

`descend` is not an arbitrary set: `mark_targets` (`:1395`) walks each
target's ancestor chain, so the marked set is **ancestor-closed** — it is
a subtree of the document tree, and a sparse one. Representing a sparse,
ancestor-closed set as a dense `Vec<bool>` over an append-only arena that
grows to 2.23 M entries (most of them superseded and unreachable) is the
same category of mismatch as pass 1.

(This last paragraph turned out to be wrong, and S3 records why: the
mismatch here is in *when* the set is computed, not in how it is
stored. A `Vec<bool>` that is never rebuilt is both cheaper and safer
than a sparse map that is maintained by hand.)

## Goals

- **G1.** A batch's cost is proportional to what the batch changed, plus
  the document suffix that genuinely moved — not to the size of the
  document or the arena.
- **G2.** The line maps are represented so that a splice's effect on them
  is the same array operation the splice already performs on `lines`.
- **G3.** The impacted-node marks are maintained incrementally across
  batches instead of rebuilt per batch. (Storing them *sparsely* was
  part of this goal and was dropped: see S3's rejected alternative. One
  byte per arena node is not the problem; rescanning every one of them
  was.)
- **G4.** Spec 0186's G3 acceptance criterion — the repaired maps must be
  bit-identical to a full rebuild, asserted after *every* splice in the
  suite — is preserved and extended to the new representations. It found
  a real pre-existing bug on its first run and is the only thing bounding
  the loss of self-healing.
- **G5.** No new invalidation obligation is added that is not discharged
  at a single, named site.

## Non-goals

- **N1.** The `(parent type, field number) -> nodes` index for
  `FqdnField` origins. **Deliberately not built.** S4 makes the scan
  conditional, so with no such override active it does not run at all,
  and when one is active it costs what it costs today (17-90 ms on a rare
  deliberate action). The index would cost 8 bytes per node — 17.8 MB at
  2.23 M nodes, up to double with `Vec` slack — plus a permanent
  maintenance tax on every node creation, and it would faithfully index
  the arena's superseded nodes too, so its real cost would track flaw
  P3's arena growth rather than the document.
  **Trigger for revisiting:** evidence that `FqdnField` overrides occur
  in normal use. If they do, the version to build is the *scoped, lazy*
  one — index only the types with a currently-active `FqdnField` origin,
  built by one scan on the toggle that activates the first, dropped when
  the last goes away, extended inside the fresh-node scan
  `mark_fresh_subtree` already performs. That is O(matching nodes) of
  memory and one scan per toggle instead of one per batch.
- **N2.** Arena reclamation (spec 0162). It compounds with this spec —
  a smaller arena makes every remaining walk cheaper — but it is
  independent and much riskier.
- **N3.** Syntax highlighting. Spec 0187 owns it. This spec assumes 0187
  has landed, because the table above only becomes the interesting one
  once `colorize` is out of it.
- **N4.** `render_cache`'s deep clone on insert (20.8% of the nested
  commit, the third-largest item). It is a real cost and it is flaw P4's
  territory (`blob`/renders as `Arc`), not this spec's.
- **N5.** `rebuild_visible_rows_from`. Spec 0186's S4 already made it
  incremental and it measures at 0.01%. Untouched.

## Specification

### S1. The line maps become dense arrays parallel to `lines`

Replace

```rust
line_to_node: HashMap<usize, usize>,
footer_line_to_node: HashMap<usize, usize>,
```

with

```rust
/// Line index -> the node whose text *starts* on that line, or `None`
/// for a line that opens no node (a message's own closing `}` line,
/// the truncation marker). Dense and exactly `lines.len()` long — see
/// spec 0188 S1 for why this is an array and not a map: a splice shifts
/// a suffix of line numbers by a constant, which is a `memmove` here and
/// a full rehash in a `HashMap`.
line_to_node: Vec<Option<u32>>,
/// The same, for a message/group node's own closing line.
footer_line_to_node: Vec<Option<u32>>,
```

`u32` rather than `usize`: the arena is indexed by `usize` everywhere
else, but 4 bytes halves the residency and 4 G nodes is far beyond any
document that fits the other constraints. Convert at the boundary. If
that is judged a false economy, `Option<usize>` is acceptable — the
representation change is the point, not the width.

Semantics are preserved exactly, including the packed-run case where
several nodes share a start line: the array keeps the last writer, which
is what the `HashMap` already did.

Read sites are mechanical — `.get(&l).copied()` becomes
`.get(l).copied().flatten()` — at `render.rs:80`, `render.rs:252-254`
(`line_has_active_override`), and wherever navigation resolves a line to
a node.

### S2. The maps are spliced where `lines` is spliced

`materialize_line_patches` (`override_apply.rs:1945-2049`) already merges
this batch's patches into `lines` in one pass. Spec 0187 removes
`line_styles` from that merge; this spec adds the two line-map arrays to
it, so the merge carries three parallel arrays instead of two.

The fresh lines' owners are not known at merge time — `map_node_lines`
learns them from the walks that run afterwards. So the merge splices in
runs of `None` of the correct new length, and `finalize_override_batch`'s
existing three walks fill them by index.

This deletes the two `retain` calls at `:1789-1795` and the `from == 0`
special case beneath them. The suffix shift stops being a repair step at
all: it is the tail memmove that `Vec::splice` performs anyway, already
measured at 3 ms for the whole `lines` merge.

`pending_patch_min_line` (spec 0186's S2) stays — it is still the batch's
splice boundary, and `rebuild_visible_rows_from` still needs it.

### S3. `descend` stops being cleared

`descend` keeps its `Vec<bool>` shape and stops being reallocated per
batch. It is **monotone**: marks are added and never removed, and the
array's own length doubles as the watermark for how much of the arena
has already been examined — so any node appended by any path, whether
or not that path remembered to say so, is picked up by the next batch.

The asymmetry that makes this sound is the one spec 0183 already states
and this spec inherits: **over-marking costs a wasted descent into a
node that turns out to need nothing; under-marking is silent staleness**
— the node is never revisited and keeps the text it was last rendered
with (spec 0183 L3). Keeping a mark errs in the safe direction by
construction.

It is also barely an error at all. A mark can only become stale if the
override entry that produced it went away, and by then the node it named
has been spliced, so it carries `rendered_as` — which marks it
permanently under the existing rules anyway. So the kept set and the
recomputed set differ only on nodes whose splice *failed*.

> **Rejected: a sparse refcounted `HashMap<usize, u32>` with matching
> `unmark_target` calls at every `OverrideCollection` mutation site.**
> That was this spec's first design, and it is worse on its own terms.
> A refcount that drifts *downward* — one missed increment, one
> double-decrement, one entry whose path resolves to a different node
> at unmark time than at mark time — under-marks, which is precisely
> the failure mode that is silent. It buys a smaller `descend` (one
> byte per arena node, 2.2 MB at the arena's observed peak) at the cost
> of an invalidation obligation spread across a dozen sites, which is
> the opposite of G5. Monotonicity has no unmark, so it has no site to
> get wrong.

### S4. Only the arena's unexamined suffix is scanned

Two of the three per-node target sources never need re-examining:

- **Auto-expand eligibility** is structural. It depends on the node's
  own field number and wire type, and on its *parent's* resolved type —
  and retyping a parent re-decodes its children, producing different
  nodes, which are scanned as fresh. (This is the same fact that makes
  spec 0183's mid-batch MessageSet tier 2 case work.)
- **`rendered_as`** only ever goes from `None` to `Some` in production.
  It is written at exactly two sites, `resettle_node` and `App::new`'s
  root seeding, and both write `Some`. Nothing in a release build ever
  clears it.

So `compute_descend_marks` scans `descend.len()..tree.len()` instead of
`0..tree.len()`, which is normally empty: `mark_fresh_subtree` already
covered whatever the last batch appended, using the same helper.

The path-shaped sources (`Path`, `PathField`) are re-derived from the
override entries every batch, as today. That costs O(entries x depth),
touches no arena, and is what makes S3's kept marks not need removing —
an entry that goes away simply stops being re-derived.

### S5. The arena scan runs only when an `FqdnField` override is active

An `FqdnField` origin — "field N of every message of type T, anywhere in
the document" — is a genuine search with no path to follow, and a newly
activated one has to find its matches among nodes examined long ago. So
that case, and only that case, resets the scan's start to `0`.

The guard is on the whole loop, not on the innermost test: with no
`FqdnField` origin active there is nothing to allocate, hash or walk.
The ordering obligation resolves itself — every `FqdnField` toggle is
followed immediately by `render_overrides(first_node)`, so the next
batch's own `compute_descend_marks` is the scan.

### S7. A batch that patched nothing finalizes nothing

`finalize_override_batch` treated `pending_patch_min_line == None` as
"no safe lower bound, rebuild everything from line 0". It is the
opposite: `None` means the batch queued no patch, and
`pending_patch_min_line`, `pending_line_patches` and `pending_shift` are
all written at one site with no early exit between them, so no patch
means no text was replaced *and* no span was shifted. There is nothing
to repair.

Return immediately. This is the common case, not an exotic one —
opening or closing the override pane, toggling a management-pane entry
that resolves to what is already rendered, and the second of two
identical passes all land here.

### S8. `resolve_path` starts from `first_node`

`resolve_path` opened with `self.tree.iter().position(|n| n.parent.is_
none())` — a linear scan of the whole arena, 382 k nodes of 256 bytes
each on a 25 MB descriptor, run once per `Path` entry per batch. It
stayed invisible because it was smaller than the full-arena mark scan
beside it; with S4 deleting that, it *was* the batch.

`self.first_node` is the same node: nothing ever appends a second
parentless node, since `splice_override` re-decodes a subtree under an
existing node and always links what it appends. Pinned by
`the_root_stays_the_first_node_across_a_root_respice`, which retypes the
root to raw and back so the arena holds three full copies of the
document, and asserts there is still exactly one parentless node.

### S6. Spec 0186's disposition is settled here

Spec 0186 left open whether its S2/S3/S4 should be reverted or kept as
correct-but-inert code. This spec resolves it, item by item, in plain
terms:

- **The "move lines instead of copying them" change (0186 S1) stays.**
  Small, free, correct.
- **The "remember the first line this edit touched" change (0186 S2)
  stays.** It is the splice boundary, still needed by S2 above and by
  `rebuild_visible_rows_from`.
- **The "repair the line lookup tables instead of rebuilding them"
  change (0186 S3) is replaced, not reverted.** Its `retain` is deleted
  by S2 above; its three re-mapping walks survive unchanged and are what
  fill the freshly spliced range.
- **The "extend the visible-rows list instead of rebuilding it" change
  (0186 S4) stays.** Measured at 0.01%; nothing to gain from touching it.
- **The bit-identity assertion (0186 G3) stays and is extended** to the
  new array representation and to the incremental `descend` set (G4
  above).

## Test plan

1. **The line maps are bit-identical to a full rebuild.** Done — 0186's
   G3 check, ported to the array representation, and extended to cover
   the no-patch early return (S7) so that skipping the finalizer is
   asserted to be a no-op on every such batch in the suite. It earned
   its keep immediately: it caught the packed-run bug below.
2. **The pruned walk still reaches everything.** Spec 0183's existing
   `assert_unpruned_walk_changes_nothing`, which renders the settled
   document a second time with the gate forced to its old blanket shape
   and requires byte equality. This is what catches a mark that S4's
   watermark failed to add — the failure mode a `descend` set-equality
   check would only have caught if the check itself were exhaustive.
3. **`FqdnField` still works.**
   `an_fqdn_field_override_marks_every_node_of_that_type`, re-run
   against the conditional scan.
4. **Packed runs still resolve.** Covered by the G3 check on the packed
   fixtures; see the bug it found, below.
5. **The root is `first_node`.**
   `the_root_stays_the_first_node_across_a_root_respice` (S8).
6. **Cost.** Done — see Outcome.
7. **Cost, document-wide.** The `from == 0` path is unchanged by S1/S2
   (the same array work), strictly reduced by S4/S5/S8, and not reached
   at all by S7. Measured at 15.2 s for a root retype on `pdb.desc`,
   dominated by `render_node_as` and `colorize` (N4 and 0187), i.e. the
   change is well below that figure's noise. Not treated as a number
   this spec moves.

**Not done, deliberately: a from-scratch `descend` equality check.** It
would assert the wrong thing. S3 makes the kept set a deliberate
*superset* of the recomputed one, so an exact comparison would fail by
design, and a subset check would pass vacuously. Item 2 asserts the
property that actually matters — that the walk reaches everything — and
does it in bytes.

## Outcome

Measured on `/tmp/pdb.desc` after root-type resolution (382,032 nodes /
276,515 lines), `flock` + `taskset -c 4`, release, G3 check off:

| phase | before | after |
|---|---|---|
| line-map repair, nested commit | 69 ms | **410 ns** |
| `compute_descend_marks` | 9-17 ms | **2.8 µs** |
| a batch that patches nothing | 21.5 ms | **34-190 µs** |
| nested commit, total | 125.5 ms | **52 ms** |

The two rows this spec set out to delete are gone. What is left of a
nested commit is `render_node_as` (N4's `render_cache` deep clone) and
`colorize` (0187).

`compute_descend_marks` did **not** land at microseconds on the first
try: with the arena scan gone it still measured 2.6 ms, and the cause
was `resolve_path`'s full-arena search for the root, which the scan had
been hiding. That is S8, and it is the reason the mark work is now four
orders of magnitude cheaper rather than three.

## Lessons

**L1 — positional preservation of line-map entries is unsound.** S2's
first version carried the two maps through `materialize_line_patches`'s
merge and *kept* the entries outside the replaced range, on the
reasoning that an entry riding the splice's memmove moves by the same
delta as the node it names. It does not always: a packed run's elements
are re-spanned by the normalization (spec 0135 G1) rather than shifted,
so an entry can land on a line its node no longer starts on. The G3
check found it on its first run. The fix is to clear the suffix at or
above the batch's earliest patch and let the three existing walks
re-assert it — which is what spec 0186 did, except as an O(suffix)
`fill` rather than an O(map size) `retain`.

**L2 — a cheap O(document) pass hides an expensive one.** Both S8's root
search and S7's no-patch rebuild had been there all along, and both were
invisible while `compute_descend_marks` was spending 17 ms next to them.
Removing a dominant cost does not finish an investigation; it starts the
next one. Re-profile after every removal, not at the end.
