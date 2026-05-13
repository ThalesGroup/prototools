<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0062 — Hopcroft minimizer: rewrite from textbook

**Status:** implemented
**Implemented in:** 2026-05-13
**App:** score-graph

---

## Background

The current `hopcroft.rs` diverges from the textbook algorithm in its
worklist maintenance, producing incorrect (too-coarse) partitions for large
graphs.  This spec replaces it with a clean implementation of the textbook
algorithm.

---

## The textbook algorithm

Hopcroft (1971).  Inputs: a set of states Q, an alphabet Σ, a transition
function δ : Q × Σ → Q (or ∅ for no transition), an initial partition P₀
of Q into equivalence classes.

```
P := P₀
W := P₀         // worklist initially contains every block

while W is not empty:
    pick and remove any block A from W
    for each symbol c in Σ:
        X := { q ∈ Q | δ(q, c) ∈ A }      // predecessors of A under c
        for each block Y in P
                where Y ∩ X ≠ ∅  and  Y \ X ≠ ∅:
            split Y into  Y₁ = Y ∩ X  and  Y₂ = Y \ X
            replace Y in P with Y₁ and Y₂
            if Y ∈ W:
                replace Y in W with Y₁ and Y₂
            else:
                add the smaller of Y₁ and Y₂ to W
```

The algorithm terminates when no block can be further split; the resulting
partition P is the **coarsest bisimulation** of the input transition system.

---

## Mapping to the scoring graph

### States Q

Every node in the raw scoring graph: message nodes (dense IDs 0..msg_count)
and leaf sentinel nodes (LEAF_VARINT, LEAF_I64, LEAF_LEN, LEAF_STRING,
LEAF_I32, ENUM sentinels).  All nodes are mapped to a contiguous index
space 0..n via `node_index`.

### Alphabet Σ

The set of all `(field_number: u32, label: u8)` pairs that appear on any
edge in the graph.

### Transition function δ

δ(q, (f, l)) = the unique destination node reached from q via a
`(field_number=f, label=l)` edge, or ∅ if no such edge exists.

(The graph may have multiple edges from q with the same `(f, l)` if the
same FQDN was merged from conflicting YAML definitions.  This is treated as
non-determinism; each `(src, f, l, dst)` tuple contributes independently to
the predecessor set X.)

### Initial partition P₀

Non-leaf nodes are grouped by `(wire_type, outgoing_signature)` where:
- `wire_type` is 2 (LENDEL) or 3 (GROUP), from `node_wire_types`.
- `outgoing_signature` is the sorted, deduplicated list of `(field_number,
  label)` pairs on all outgoing edges.

Leaf nodes are grouped by `(wire_type, is_string, enum_range_idx)`.

Nodes in the same initial block have the same outgoing structure and cannot
be separated by any single-step observation — a necessary precondition for
bisimulation.

---

## Specification

### §1 — Data structures

```
P  : Vec<HashSet<usize>>   // P[block_id] = set of node indices in that block
                            // block IDs are stable; shrunk blocks keep their ID
block_of : Vec<u32>        // block_of[node_index] = current block ID
W  : worklist              // see §2
```

### §2 — Worklist representation

The worklist W holds `(block_id, field_number, label)` triples, one per
(block, symbol) pair that still needs to be used as a splitter.

W is represented as:
- a `VecDeque<(usize, u32, u8)>` for ordered processing, and
- a `HashSet<(usize, u32, u8)>` (`in_worklist`) to answer "is (B, sym) ∈ W?"
  in O(1).

**Initialization**: for every block B in P₀ and every symbol (f, l) in Σ,
add `(B, f, l)` to W.

**Pop**: remove the front element from the deque and from `in_worklist`.

### §3 — Refinement step

For each `(A, f, l)` popped from W:

1. Compute the predecessor set:
   ```
   X = { node_index(src) | edge (src, f, l, dst) in graph,
                            block_of[node_index(dst)] == block_id(A) }
   ```
   where `block_id(A)` is the block ID of A (the dequeued block).

2. For each block Y such that Y ∩ X ≠ ∅ and Y \ X ≠ ∅:

   a. Split: let Y₁ = Y ∩ X (nodes in Y that are predecessors) and
      Y₂ = Y \ X (the rest).  Move Y₁ into a fresh block with a new ID.
      Y keeps its original ID and now contains only Y₂.  Update `block_of`
      for all nodes in Y₁.

   b. Update worklist per the textbook rule:
      - If `(block_id(Y), f', l') ∈ W` for a given symbol (f', l'):
          - Remove `(block_id(Y), f', l')` from W.
          - Add `(block_id(Y₁), f', l')` and `(block_id(Y₂), f', l')` to W.
            (Y₁ is the new block; Y₂ retains the original block_id(Y).)
      - If `(block_id(Y), f', l') ∉ W` for a given symbol (f', l'):
          - Add only the smaller of Y₁ and Y₂ to W for that symbol.

   This must be applied for **every** symbol (f', l') in Σ.

### §4 — Termination

The algorithm terminates when W is empty.  At that point, no block in P
can be further split by any symbol in Σ, so P is the coarsest bisimulation.

### §5 — Renumbering

After the algorithm, renumber blocks so that non-leaf blocks come first
(IDs 0..num_msg_blocks) and leaf blocks follow (IDs num_msg_blocks..),
matching the layout expected by `compile()`.

### §6 — Tests

The `hopcroft_dump` binary (spec 0059) run against fixtures TC-01 through
TC-07 must produce the following results:

- TC-01: A and B → same state (both `{f1: VARINT}`; bisimilar).
- TC-02: A and B → distinct states (different field numbers).
- TC-03: A and B → distinct states (different leaf types on field 1).
- TC-04: A and B → distinct states (different labels on field 1).
- TC-05: A and B → distinct states (different node wire types: LENDEL vs GROUP).
- TC-06: `AnnotatedMapEntry` and `MultiOptionMapEntry` → distinct states;
  `MapWithOptions` → two distinct child states for its two `repeated` fields.
- TC-07: `Root1` and `Root2` → same state (bisimilar via identical
  sub-structure).

The large-corpus stress test (all reproto-out YAML files combined) must
produce distinct child states for the two `repeated` fields of
`test.field.MapWithOptions` on every run, without exception.

---

## Files changed

| File | Change |
|---|---|
| `score-graph/src/build_scoring_graph/hopcroft.rs` | Rewrite `minimize()` from scratch following this spec |
