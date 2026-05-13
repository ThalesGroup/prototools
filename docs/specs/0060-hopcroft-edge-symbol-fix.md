<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0060 — Hopcroft minimizer: correct edge symbol includes origin node kind

**Status:** dropped
**App:** score-graph

---

## Background

### How Hopcroft bisimulation minimization works in our graph

The scoring graph is a labeled transition system (LTS).  Nodes are either
message nodes (non-leaf) or leaf nodes (VARINT, I64, LEN, LEN_STRING, I32,
or ENUM).  Edges go from message nodes to other message nodes or leaf nodes.

Each edge carries three attributes:

- `field_number` — the protobuf field number.
- `label` — the cardinality: optional (0), required (1), or repeated (2).
- `origin_node_kind` — the wire type of the source node: LENDEL (2) or
  GROUP (3).

The **edge symbol** is the triple `(field_number, label, origin_node_kind)`.
Two nodes are bisimulation-equivalent iff, for every edge symbol, they
transition to the same equivalence class.

Hopcroft's algorithm computes the coarsest such partition in O(n log n) time:

1. **Initial partition**: group nodes by their complete outgoing signature —
   the sorted set of `(field_number, label, origin_node_kind)` triples for
   all outgoing edges, plus the node's own kind (which equals
   `origin_node_kind` on all its outgoing edges).  Leaf nodes are grouped by
   their wire-type attributes.

2. **Reverse adjacency**: for each node index `di`, store the list of
   predecessors `(si, field_number, label, origin_node_kind)` — all source
   nodes that have an edge with that symbol pointing to `di`.

3. **Worklist**: initialized with all `(block, symbol)` pairs for every
   initial block and every symbol `(field_number, label, origin_node_kind)`
   present in the graph.

4. **Refinement loop**: for each `(splitter_block, symbol)` dequeued:
   - Find all predecessors of any node in `splitter_block` via `symbol`.
   - For each block `C` that contains some but not all of those predecessors,
     split `C`: move the predecessors into a new block `C'`.
   - Add `(smaller_of(C, C'), every_symbol)` to the worklist.

The invariant maintained is: after all splitters are processed, no block can
be further split — every pair of nodes in the same block is genuinely
bisimulation-equivalent.

### The current bug

The current implementation uses `(field_number, label)` as the edge symbol,
**omitting `origin_node_kind`**.  Concretely:

```rust
// all_field_labels — the set of symbols used for the worklist and rev
let all_field_labels: HashSet<(u32, u8)> = raw
    .edges
    .iter()
    .map(|e| (e.field_number, e.label))
    .collect();

// rev — predecessors keyed on (src_index, field_number, label)
let mut rev: Vec<Vec<(usize, u32, u8)>> = vec![Vec::new(); n];
for edge in &raw.edges {
    rev[di].push((si, edge.field_number, edge.label));
}

// sig — outgoing signature: Vec<(field_number, label)>
let mut sig: Vec<Vec<(u32, u8)>> = vec![Vec::new(); n];
for edge in &raw.edges {
    sig[si].push((edge.field_number, edge.label));
}
```

`origin_node_kind` is absent from `all_field_labels`, `rev`, and `sig`.

The initial partition key `(wt, sig)` does include `wt` (the node's own wire
type, which equals `origin_node_kind` on all outgoing edges), so LENDEL and
GROUP nodes start in separate blocks.  But:

- The `rev` array stores `(src_index, field_number, label)` — no
  `origin_node_kind`.  When a splitter `(block, field, label)` fires, it
  collects predecessors regardless of their `origin_node_kind`.
- The worklist contains `(block, field, label)` tuples — no
  `origin_node_kind`.  A LENDEL predecessor and a GROUP predecessor of the
  same `splitter_block` via `(field, label)` are treated as one splitter
  rather than two separate ones.

This means the refinement can fail to split a block.  Specifically: suppose
block `B` contains both a LENDEL node `X` (with edge `(field=1, label=opt,
wt=2)` → child_block `P`) and a different LENDEL node `Y` (with edge
`(field=1, label=opt, wt=2)` → child_block `Q`).  If `P ≠ Q` the splitter
`(P, field=1, opt)` should put `X` into a new block.  That part works.

But consider: block `B` contains LENDEL node `X` and LENDEL node `Y`, both
with `(field=1, label=opt)` edges but pointing to different child blocks `P`
and `Q`.  The splitter `(P, field=1, opt)` fires and correctly splits `X` out
of `B`.  That is fine.

The actual failure mode arises when `origin_node_kind` is missing from the
predecessor lookup.  Suppose a GROUP node `G` (in a different block) also
has a `(field=1, label=opt)` edge pointing into `splitter_block`.  When the
splitter `(splitter_block, field=1, opt)` fires, `G` is included in the
predecessor set alongside LENDEL nodes.  The split then separates nodes that
reach `splitter_block` via `(field=1, opt)` — regardless of whether they are
LENDEL or GROUP — from those that don't.  Since LENDEL and GROUP nodes are
already in separate blocks, this particular mixing cannot directly merge them.

However, the missing `origin_node_kind` dimension in the worklist means that
the splitter `(block, field=1, opt, wt=2)` and `(block, field=1, opt, wt=3)`
are collapsed into one splitter `(block, field=1, opt)`.  When that splitter
splits a block, it splits based on "reaches `splitter_block` via field=1,
opt" — irrespective of origin kind.  In a graph where both LENDEL and GROUP
predecessors of `splitter_block` exist via the same `(field, label)`, this
can produce an overly coarse split that leaves distinguishable nodes together.

The concrete symptom: `test.field.MapWithOptions` (which has two map fields
with structurally different entry types) ends up in the same Hopcroft block as
`google.cloud.dataproc.v1.KubernetesSoftwareConfig` (which has two map fields
with identical entry types).  This is incorrect: the two message types are not
bisimulation-equivalent.  The bug manifests only in large corpora where the
predecessor sets for common `(field, label)` symbols are large enough to
suppress the necessary splits.

---

## Goals

1. Fix the Hopcroft minimizer so that the edge symbol is the full triple
   `(field_number, label, origin_node_kind)`.
2. Update `rev`, `all_field_labels`, and the refinement loop accordingly.
3. Keep `sig` (the initial partition outgoing signature) consistent: it
   already includes `wt` as a per-node prefix, which is equivalent to
   including `origin_node_kind` in each edge tuple since all outgoing edges of
   a node share the same `origin_node_kind`.  No change needed to the initial
   partition.
4. All TC-01 through TC-06 fixture tests from spec 0059 must pass.
5. The stress test suite must pass after the fix and a rebuilt stress DB.

## Non-goals

- Changes to the `.rkyv` binary format.
- Changes to the scoring walk.
- Changes to `compile()` in `graph.rs`.
- Performance optimization of the minimizer (a future concern).

---

## Specification

### §1 — Correct edge symbol

The edge symbol throughout `hopcroft.rs` must be the triple:

```
(field_number: u32, label: u8, origin_wt: u8)
```

where `origin_wt` is the wire type of the source node (2 for LENDEL, 3 for
GROUP), obtained from `node_wire_types[src_node_id]`.

### §2 — Updated `rev` array

Change `rev` from `Vec<Vec<(usize, u32, u8)>>` to
`Vec<Vec<(usize, u32, u8, u8)>>` — adding `origin_wt` as the fourth element:

```rust
// Before:
let mut rev: Vec<Vec<(usize, u32, u8)>> = vec![Vec::new(); n];
for edge in &raw.edges {
    let si = node_index(edge.src);
    let di = node_index(edge.dst);
    rev[di].push((si, edge.field_number, edge.label));
}

// After:
let mut rev: Vec<Vec<(usize, u32, u8, u8)>> = vec![Vec::new(); n];
for edge in &raw.edges {
    let si = node_index(edge.src);
    let di = node_index(edge.dst);
    let origin_wt = node_wire_types.get(&edge.src).copied().unwrap_or(2);
    rev[di].push((si, edge.field_number, edge.label, origin_wt));
}
```

### §3 — Updated `all_field_labels` worklist symbols

Change `all_field_labels` from `HashSet<(u32, u8)>` to
`HashSet<(u32, u8, u8)>` — including `origin_wt`:

```rust
// Before:
let all_field_labels: HashSet<(u32, u8)> = raw
    .edges
    .iter()
    .map(|e| (e.field_number, e.label))
    .collect();

// After:
let all_field_labels: HashSet<(u32, u8, u8)> = raw
    .edges
    .iter()
    .map(|e| {
        let origin_wt = node_wire_types.get(&e.src).copied().unwrap_or(2);
        (e.field_number, e.label, origin_wt)
    })
    .collect();
```

### §4 — Updated worklist and refinement loop

The worklist type changes from `VecDeque<(usize, u32, u8)>` to
`VecDeque<(usize, u32, u8, u8)>`.  The refinement loop unpacks all four
components and filters `rev` entries on all four:

```rust
// Before:
while let Some((splitter_block, field, label)) = worklist.pop_front() {
    // ...
    let predecessors: HashSet<usize> = blocks[splitter_block]
        .iter()
        .flat_map(|&dst| {
            rev[dst]
                .iter()
                .filter(|(_, f, l)| *f == field && *l == label)
                .map(|(src, _, _)| *src)
        })
        .collect();
    // ...
    for &(f, l) in &all_field_labels {
        worklist.push_back((smaller, f, l));
    }
}

// After:
while let Some((splitter_block, field, label, origin_wt)) = worklist.pop_front() {
    // ...
    let predecessors: HashSet<usize> = blocks[splitter_block]
        .iter()
        .flat_map(|&dst| {
            rev[dst]
                .iter()
                .filter(|(_, f, l, owt)| *f == field && *l == label && *owt == origin_wt)
                .map(|(src, _, _, _)| *src)
        })
        .collect();
    // ...
    for &(f, l, owt) in &all_field_labels {
        worklist.push_back((smaller, f, l, owt));
    }
}
```

### §5 — Initial partition: no change

The initial partition key `(wt, sig)` where `wt = origin_node_kind` and
`sig = Vec<(field_number, label)>` is equivalent to grouping by the full
set of `(field_number, label, origin_wt)` triples (since all edges from one
node share the same `origin_wt = wt`).  No change is needed here.

### §6 — sig array: no change

The `sig` array is used only for the initial partition and is not used during
refinement.  Its structure `Vec<(field_number, label)>` is sufficient for the
initial partition because `origin_wt` is captured separately in the `(wt, s)`
key.  No change needed.

### §7 — Tests

The TC-06 fixture test (from spec 0059) must fail before this fix and pass
after.  TC-01 through TC-05 must continue to pass.

The stress test DB must be rebuilt after this fix.

---

## Files changed

| File | Change |
|---|---|
| `score-graph/src/build_scoring_graph/hopcroft.rs` | Update `rev`, `all_field_labels`, worklist, and refinement loop as specified in §2–§4 |
| `score-graph/tests/fixtures/hopcroft/tc-06/expected.yaml` | Update to reflect correct (fixed) output |
