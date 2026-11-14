<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0425 — Hopcroft with a sparse worklist

Status: implemented
Implemented in: 2026-11-14
App: prototext-graph
Refs: docs/specs/0062-hopcroft-worklist-fix.md (the textbook algorithm
      this keeps, and its mapping to the scoring graph);
      docs/specs/0059-hopcroft-test-harness.md (the fixtures and suite
      that must pass unchanged);
      docs/specs/0177-reproducible-schema-db-artifacts.md (S3: block
      numbering, which this must not disturb);
      docs/specs/0238-an-extension-range-is-what-makes-an-unknown-field-innocent.md
      (S7: extensibility in the initial partition, kept);
      docs/specs/0371-packing-mismatch-is-neutral-evidence.md (S4: the
      edge symbol, kept);
      docs/specs/0424-protoback-reproto-in-rust-on-upb.md (step 0 of its
      work plan)

## Background

`build_graph` takes 55 s of reproto's 2 min 57 s on googleapis with
`googleapisDb`'s flags (spec 0424, Background). `hopcroft_dump`, built
with the `profiling` profile and run under `perf` on the 7,771 scoring
graphs reproto writes for googleapis, takes 54.6 s and 1.17 GB; 95.5% of
the samples are in `hopcroft::minimize` (2026-11-14, the development VM).

The graph: 49,255 messages, 164,642 edges, and an alphabet of 2,903
symbols, (field number, symbol) pairs (`RawEdge::symbol`, spec 0371 S4).

`minimize` (`hopcroft.rs`) is the textbook algorithm of spec 0062, taken
literally:

- the worklist starts with every (block, symbol) pair: every block of
  the initial partition times all 2,903 symbols;
- each split adds a pair for every symbol, 2,903 of them, whether or not
  any edge with that symbol enters the block;
- each pair popped computes its predecessor set by scanning the reverse
  edges of every member of the block, filtering on the symbol;
- blocks are `HashSet`s, and the worklist's membership a `HashSet` of
  triples.

A node has a few incoming edges, so for almost every pair the
predecessor set is empty and the work is wasted; but its cost, a pop, two
hash operations and a scan, is paid tens of millions of times.

## Goals

- **G1.** The same partition as today, so the same `hopcroft.rkyv`, byte
  for byte, on every input.
- **G2.** Minimization in time proportional to the edges (O(m log n)),
  not to blocks × alphabet: about a second on googleapis.

## Non-goals

- **N1.** Changing what is minimized: the initial partition (wire type,
  extensibility, outgoing signature; spec 0238 S7), the symbols (spec
  0371 S4), the reserved singleton blocks (spec 0089 §9).
- **N2.** Another algorithm (Moore's, or Valmari–Lehtinen's in full):
  Hopcroft's with a sparse worklist is the textbook algorithm minus
  work that cannot split anything, which keeps spec 0062's reasoning
  valid.
- **N3.** Parallel minimization.

## Specification

- **S1. Only pairs that can split.** A pair (A, c) whose block A has no
  incoming edge labeled c has an empty predecessor set, and splits
  nothing. Such pairs are never queued:
  - the initial worklist holds, for each block, the pairs of the symbols
    on its incoming edges;
  - when Y splits into Y₁ and Y₂, the rule of spec 0062 applies to each
    symbol on an edge entering Y (Y₁ ∪ Y₂), and only those: if (Y, c) is
    queued, (Y₁, c) is added; otherwise the smaller of Y₁ and Y₂, if it
    has an incoming edge labeled c.

  Omitting a pair that splits nothing changes no split, so the
  refinement reaches the same fixed point: the coarsest stable partition,
  which is unique.
- **S2. Incoming edges by symbol.** The reverse adjacency of each node
  is sorted by symbol, so the predecessors of a block through c are a
  range per member, found by binary search, and the symbols entering a
  block are read from the same lists.
- **S3. Blocks stay `HashSet`s.** S1 and S2 bring `minimize` to about
  40 ms on googleapis (Measured outcome), of which the `HashSet`s of
  blocks and predecessor sets are about 30 ms. The refinable-partition
  layout (one array of elements grouped by block, each block a range of
  it, each element's position stored) would remove most of that; it waits
  for a corpus on which a profile shows the minimization.
- **S4. The numbering is unchanged.** Internal block IDs come out in a
  different order, but the final renumbering (`hopcroft.rs`, after the
  refinement loop) numbers blocks by their first node in node order,
  leaf blocks last: it depends on the partition alone. The sort of spec
  0177 S3 stays, so that the internal order is deterministic too.
- **S5. Progress.** `on_progress` keeps its contract: `(0, budget)`
  first, nondecreasing values, `(budget, budget)` last, with the budget
  computed as now (the sum of log₂ of the initial blocks' sizes). Only
  the scale of the drain changes, with the worklist's new size.
- **S6. The textbook version stays as a test oracle**, under
  `#[cfg(test)]`, as `minimize_textbook`: the previous refinement loop,
  its rules unchanged. It shares with `minimize` the setup (node indices,
  reverse adjacency, initial partition), the split, the grouping of X by
  block and the renumbering, and computes the alphabet itself, the only
  thing it alone needs.

## Alternatives considered

### Keep the textbook version and make its pairs cheaper

Faster hash sets, or a bitmap for worklist membership, divide a cost
that is wasted anyway; tens of millions of pairs remain.

### Moore's algorithm

Simpler, but O(n²) in the worst case, and a different algorithm from the
one spec 0062 established and spec 0059's suite was written for.

### Valmari–Lehtinen

The asymptotically best known, with transitions partitioned alongside
states. S1–S2 reach the same bound for this graph's shape with less
change; S3 records its data structure for when it is needed.

## Test plan

1. **Unchanged suite:** `hopcroft_suite` and its fixtures (spec 0059)
   pass without edits.
2. **Oracle:** for every fixture, and for 500 random graphs (a seeded
   generator, `random_graph`: 2 to 41 messages, one per file, up to three
   fields each among few numbers and types, so that many messages are
   equivalent; cycles, empty messages, groups, extension ranges, packed
   fields, entries), `minimize` and `minimize_textbook` give the same
   `block_of` and number of blocks.
3. **Artifacts:** `wktPrebuiltCheck` (in `ci`) passes, the committed
   `prototext/wkt/prebuilt` unchanged; `googleapisDb`'s `hopcroft.rkyv`
   has the same SHA-256 before and after.
4. **Downstream:** prototext's and protolens's test suites, and
   `googleapisTests`, pass.
5. **Timing:** `hopcroft_dump` on googleapis's scoring graphs, and
   reproto on googleapis with `googleapisDb`'s flags, before and after,
   with `hyperfine` as spec 0416 benchmarks.

## Measured outcome

2026-11-14, the development VM (12 cores), googleapis as `googleapisPbs`
builds it, `googleapisDb`'s flags.

- **Same output (G1).** The compiled graph `hopcroft_dump` prints for
  the 7,771 scoring graphs has the same SHA-256 before and after
  (`7de79059…`); `googleapisDb`'s `hopcroft.rkyv` (`9b2c5cb1…`) and
  `index.rkyv` are those of a reproto run before the change, and
  reproto's whole output tree is identical (`diff -r`). The tests pass:
  `hopcroft_suite` unchanged, the oracle on the fixtures and on 500
  random graphs, the progress contract, and `full-tests` (`ci`,
  `wktPrebuiltCheck` included, `googleapisDb`, `googleapisTests`,
  `customDb`, `customTests`).
- **Time (G2).**

  | | Before | After |
  |---|---|---|
  | `hopcroft_dump` (profiling build), wall clock, YAML parsing and dump included | 34.2 s | 0.71 s |
  | its peak memory | 1.17 GB | 98 MB |
  | `minimize` alone, from a perf profile of the run above | 95.5% of the samples | about 40 ms (5.4%) |
  | `build_graph`, inside reproto | 55.5 s | 1.6 s |

  What remains of `hopcroft_dump`'s time is mostly parsing the 41 MB of
  scoring YAML, which protoback does not produce (spec 0424 S20).
- **reproto's total**, not measured cleanly: the run after the change took
  174.5 s against 181.6 s, but its unchanged Python phases ran slower
  than in the run before (rendering 109 s against 70 s), so the machine
  was not in the same state. The expected saving is `build_graph`'s,
  about 54 s; `scripts/bench-protoback` (spec 0424 S21) will measure it
  with pinned cores.
