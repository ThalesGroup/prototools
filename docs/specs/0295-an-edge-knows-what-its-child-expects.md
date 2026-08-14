<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0295 — an edge knows what its child expects

Status: implemented
Implemented in: 2026-08-14
App: prototext-graph
Refs: docs/specs/0293-a-state-carries-its-own-fields.md (the same shape,
        one level up: 0293 put the *edge run* on the state, this puts the
        *child's wire type* on the edge),
      docs/specs/0238-an-extension-range-is-what-makes-an-unknown-field-innocent.md
        (graph format v3, the version-bump policy this follows),
      docs/specs/0294-a-group-owes-its-unknowns-once.md (the baseline
        this is measured against)

## Background

0293 stopped the walk binary-searching the whole 85 806-entry transition
table for a state's fields. It left the level below untouched. Having
found the edge, the walk then asked *what does the child expect?* — and
that was a hand-rolled binary search over all 16 696 nodes:

```rust
let expected_wt = node_wire_type(ws.graph, tr.child_state_id) as u32;
```

once per `Found` candidate per tag, ~14 probes scattered over a table
with no locality, because `transitions` is ordered by *source* state and
this reads the *destination*. It was **1.39 G instructions, 6.07%** of a
googleapis startup — the single largest remaining item in the profile.

A second, smaller instance of the same mistake survived in
`apply_cardinality_multi`, which still opened with a whole-table
`partition_point` to find the run 0293 had already put on the
`ActiveEntry`.

The answer to `node_wire_type` is a property of the schema. It cannot
change during a walk, and the number of *distinct* questions is the
number of edges — 85 806 — against the tens of millions of times the
walk asked. It belongs in the graph.

## Goals

- **G1.** The verdict loop reads the child's expected wire type off the
  edge it already has in hand, with no second lookup.
- **G2.** The remaining whole-table search in `apply_cardinality_multi`
  uses 0293's run instead.
- **G3.** Scoring output is unchanged, byte for byte.
- **G4.** The transition table does not grow.

## Non-goals

- **N1.** No compatibility shim for a v4 file. Consistent with 0238 N4
  and 0293: a database is a build artifact that reproto regenerates, and
  a v4 file read as v5 would find whatever byte the old padding held
  where `child_wire_type` now lives, so every field would be judged a
  match or a mismatch at random. That is exactly the
  plausible-but-wrong-answer failure the version check exists to
  prevent. Reject with a rebuild instruction.

- **N2.** No denormalization of the child's other facts (`is_string`,
  `range_idx`, `ext_range_idx`). They are read on paths that are not
  hot, and unlike the wire type they would not fit in the padding.

## Specification

- **S1.** `TransitionEntry` gains `pub child_wire_type: u8`. It lands in
  padding the struct already had between `label: u8` and
  `child_state_id: u32`, so the entry stays 16 bytes and the table stays
  1.37 MB on googleapis. G4 is satisfied by placement, not by
  compression.

- **S2.** The value is *already normalized*: the internal discriminants
  8 (UINT32) and 9 (INT32) are stored as 0 (VARINT), matching what
  `node_wire_type` returned. A child with no node entry — which cannot
  happen for a well-formed graph — is stored as `u8::MAX`, again
  matching. The walk therefore needs no adjustment at the point of use.

- **S3.** `link_transitions` fills it, in a second loop after the one
  that assigns each node its transition run. That loop is a co-walk of
  two tables sorted the same way; this one cannot be, so it is a binary
  search per edge — 85 806 of them, once, at build time.

- **S4.** `fn node_wire_type` is deleted. It had exactly one call site.

- **S5.** `apply_cardinality_multi` takes the `ActiveEntry` and iterates
  `transitions[trans_offset .. trans_offset + trans_len]`, dropping its
  `partition_point` over the whole table and the sentinel `break` that
  went with it.

- **S6.** `GRAPH_VERSION` 4 → 5.

## Alternatives considered

### Caching the last `(state_id → wire_type)` answer

A one-entry memo would have needed no format change, but the walk's
accesses are not repetitive in that way: consecutive tags of one message
point at different children by construction. The hit rate would track
the repeated-field rate, not the traffic.

### A dense `state_id → wire_type` side table

`num_states` is 16 696, so a `Vec<u8>` indexed by state id would be 16 KB
and fit in L1 — a lookup with no search at all. It was not taken because
it is a *second* random access where S1 needs none: the edge is already
in a register when the question is asked.

## Test plan

1. The existing `prototext-graph` suite (95 + 10). Wire-type mismatch is
   what most of it turns on, so a wrong `child_wire_type` is not a
   subtle failure there.
2. `protolens … export /` over googleapis is byte-identical to 0294's.
3. The WKT prebuilt and the googleapis corpus regenerate at v5 and both
   load.

## Measured outcome

Dev VM (8 E-cores, two L2 clusters), googleapis (25.6 MB descriptor
set, 49 255 roots), `--descriptor-set $SET $SET quit`.

| | 0294 | 0295 |
|---|---|---|
| wall clock `-j 1`, `taskset -c 4`, median of 5 | 3.018 s | 2.770 s |
| wall clock `-j 8`, `taskset -c 0-7`, median of 11 | 1.493 s | 1.406 s |
| instructions (`-j 1`) | 22.92 G | 20.64 G |
| `score_message_multi` Ir (both frames) | 15.46 G | 13.42 G |
| its share of the run | 67.5% | 65.0% |

**−9.9% instructions for −8.2% single-threaded time**, −5.8% at eight
workers. `node_wire_type`, `find_node` and `state_has_transitions` have
all left the profile; `find_transition` is now inlined into its caller.

Unlike 0294 this returns slightly *less* time than instructions, which
is what a search over a 268 KB node table rather than a 3.5 MB scatter
predicts: most of `node_wire_type`'s probes were already resident.

Both binaries were re-measured in the same quiet window. 0294's `-j 8`
median reads 1.49 s here against the 1.70 s recorded in its own spec —
that is the neighbours, not a regression, and it is why the two figures
above are only ever compared with each other.

`export /` over the whole corpus is byte-identical to 0294's output,
5 278 322 lines.
