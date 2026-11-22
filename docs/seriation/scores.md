<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Seriation: the scores

The scores the sort maximizes and the ones it reports. Every score follows
one convention: **higher is better, and 0 is the optimum**. Terms are those
of [design.md](design.md).

## Edge score (phase 3)

### The graph

Phase 3's comparator answers become one directed graph over snapshots. An
edge `(u → v, w)` means that `w` files independently judged snapshot `u`
older than snapshot `v`. Both directions may be present between one pair,
with independent weights: there is no cancellation, each judgment counts.
Same versions contribute both directions.

```
out[u] = [(v, w), …]    in[v] = [(u, w), …]
```

Built once, after every file's comparisons, and never changed. The code
keeps one weight-1 edge per judgment, and for each snapshot the list of
its edges, which gives the same score.

### The score

An edge `u → v` **agrees** with the order when `pos[u] < pos[v]`. The edge
score is the negated weight of the edges that disagree:

```
edge_score = −Σ { w | (v, w) ∈ out[u], pos[u] > pos[v] }
```

### Moving one snapshot

Only edges touching the moved snapshot `x` change. Moving it from `p` to
`q`, the change is, over its out-edges `x → y` and in-edges `y → x`:

```
for (y, w) in out[x]: agree before = p < pos[y], after = q' < pos[y]
for (y, w) in in[x]:  agree before = pos[y] < p, after = pos[y] < q'
delta += w × (after − before)
```

where `q'` and the `pos[y]` are read in the order after the move (the
snapshots between `p` and `q` shift by one). `O(deg(x))`: on a sparse graph,
close to constant. Updating `pos` and the order array themselves costs
`O(|p − q|)`, the shift.

## Interleaving score (phase 4)

### The straddle count

A triple `(F, v, c)` is a **straddle** when:

1. `F_v ≠ nil`: file `F` is in the snapshot at position `v`;
2. `c` is a version of `F` that occurs in the order, and `c ≠ F_v`;
3. `lo(F, c) < v < hi(F, c)`: position `v` is strictly inside `c`'s span.

That is, a different version `c` of the same file occurs both before and
after `v`. The count is over positions, not versions: a position inside
the spans of two other versions counts twice, and a version straddling
three positions counts three times. The interleaving score is the negated
straddle count.

A single A, B, A of one file is −1: the B is straddled by A's span. In the
true order such patterns are rare, so minimizing them is a reliable guide.

### Per-file state

The score is kept per file, on the file's **subsequence**: its instances
read in the current order, one slot per instance. For each file with at
least two distinct versions (the others can never straddle, and are left
out at no cost):

```
version_at[j]  the version at subsequence slot j
span_of[c]     (lo, hi) in slot space, real spans only
layers         the real spans, partitioned so that within a layer no two
               open interiors overlap; each layer sorted by lo
```

The layers are a greedy interval coloring by `lo`. Within a layer, the one
span whose interior holds a slot, if any, is found by one binary search,
so asking "which versions straddle slot `j`" costs `O(L)`, `L` the number
of layers, which is single-digit in practice.

The state is **kept across moves**, not rebuilt per probe: the search
probes about `N` destinations per move it makes, so rebuilding per probe
would pay `O(K log K)` per file `N` times over. A move pays it once: it
rotates `version_at` over the moved range, updates the spans the rotation
touches, and rebuilds the layers.

### Removing and reinserting

The search probes a move as a removal, then a reinsertion:

- **remove** writes a sentinel into the moved snapshot's slot of each of
  its files, in place, and updates the spans; **undo** restores them.
  Between the two the array is not canonical; that window never outlasts
  one candidate;
- **reinsert probe** counts the straddles the snapshot would create and
  remove at a destination, without changing anything. A destination slot
  maps to an insertion position as: moving right, after the slot the
  destination snapshot holds; moving left, at it. Because the move is a
  rotation, a span ending exactly at the insertion position does straddle
  it (the bound is non-strict on `hi`).

Per file of the moved snapshot, a probe costs `O(K + L)`, `K` the file's
instance count.

## Reported scores

After every phase, the sort reports all scores, whichever one drove it:

- **edge score** and **interleaving score**, as above;
- **Kendall's tau** against the true hints, hidden ones included, as a
  distance: the number of pairs of snapshots with a hint in the opposite
  order to their hints, negated; equal hints are no inversion. Range
  `[−n(n−1)/2, 0]` for `n` snapshots with a hint. Computed in
  `O(n log n)`: list the hint ranks in the current order and count the
  inversions with a Fenwick tree. It is the global form of the edge
  score's question (how many pairwise judgments the order contradicts),
  which is why it, rather than a displacement measure such as Spearman's
  footrule, judges the sort;

The reported scores never drive the sort.
