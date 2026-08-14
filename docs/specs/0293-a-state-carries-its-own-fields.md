<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0293 — a state carries its own fields

Status: implemented
Implemented in: 2026-08-14
App: prototext-graph
Refs: docs/specs/0291-a-name-is-looked-up-not-searched-for.md (the
        build-once/use-many shape, and the rule of measuring the
        amortization ratio before implementing),
      docs/specs/0292-nothing-was-vetoed-so-nothing-is-removed.md (the
        baseline this is measured against),
      docs/specs/0047-build-scoring-graph.md (the graph file format
        this bumps to version 4)

## Background

`find_transition` answers "does state *s* declare field *n*?" — the
innermost question of the scoring walk, asked once per candidate per
tag. On a googleapis startup it is called **55 517 968** times.

It answered by binary-searching the *whole* `transitions` table, keyed
`(state_id, field_number)`: 85 806 entries, 1.37 MB. That is ~17 probes,
almost all of them missing L2, for a question whose answer lives in a
run of mean 5.14 entries (median 3, p90 10, max 250).

The table is already sorted by `(state_id, field_number)`, so every
edge out of a state is contiguous. The state simply did not know where
its own run began.

## Goals

- **G1.** `find_transition` searches only the source state's own run.
- **G2.** The run is resolved once per `ActiveEntry`, not once per call.
- **G3.** Scoring output is unchanged, byte for byte.

## Non-goals

- **N1.** No change to the sort key or the contents of `transitions`.
  The run is derived from the existing order; nothing is reordered.
- **N2.** No per-state hash map or perfect-hash table. At a median
  fanout of 3 the search is over before a hash would have been computed,
  and the table would cost more than the 0.13 MB this does.
- **N3.** No `slice::binary_search_by`. See *Alternatives considered*.

## Specification

- **S1.** `NodeEntry` gains `trans_offset: u32` and `trans_len: u32`,
  delimiting that state's run in `transitions`. The struct goes 12 → 20
  bytes; the googleapis node table goes 0.20 → 0.33 MB, still inside L2.

- **S2.** `link_transitions(nodes, transitions)` computes both fields by
  one linear co-walk of the two sorted tables, after both are sorted.
  It is called from `compile` and from `compile_initial` — the two
  places that build a `CompiledGraph` — so no builder can forget it.

- **S3.** `GRAPH_VERSION` 3 → 4. The fields are pure derived data, so a
  v3 file *could* be read and relinked; it is rejected instead, because
  reading one without relinking gives every state an empty run and
  therefore scores every field unknown — a plausible wrong answer, which
  is the exact failure the version check exists to prevent.

- **S4.** `ActiveEntry` carries `trans_offset`/`trans_len`, copied from
  the state's `NodeEntry` when the entry is built in `group_by_state`
  (which therefore now takes the graph). This costs one `find_node` per
  entry and saves one whole-table search per call: measured over a
  googleapis startup, **2 571 866 entries served 55 517 968 calls —
  21.6 each**. Per spec 0291's rule, that ratio was measured before the
  change was written.

- **S5.** `find_transition` takes the run as a parameter and binary
  searches within it on `field_number` alone.

## Alternatives considered

### `slice::binary_search_by` over the run

Built, measured, reverted: **+16.5% wall clock**, 5 pairs of 5.
Callgrind showed Ir flat (+0.25%), so the loss is purely
microarchitectural, from two causes. The library search is
deliberately branchless, which serializes the dependent probe chain and
forfeits the memory-level parallelism a mispredicted branchy search
gets for free; and comparing a `(state_id, field_number)` tuple loads
`field_number` on every probe, where `&&` short-circuits it away on
every state mismatch. The hand-rolled loop in S5 is kept for both
reasons, and this paragraph is why it must not be "cleaned up".

### A per-state offset table beside the nodes

A separate `Vec<(u32, u32)>` indexed by state id avoids widening
`NodeEntry`. Rejected: the walk already holds the `NodeEntry` when it
needs the run, so a second table is a second cache line for data that
fits in the first.

## Test plan

1. The existing `prototext-graph` suite (95 + 10) — the graph is built
   and consumed through the same paths, so any mislinked run shows up as
   a wrong score.
2. A v3 database is rejected with the rebuild instruction, not read.
3. `protolens … export /` over googleapis is byte-identical to 0292's.

## Measured outcome

Dev VM, `taskset -c 4-11`, googleapis (25.6 MB descriptor set, 49 255
roots), `--descriptor-set $SET $SET -j 8 quit`.

| | 0292 | 0293 |
|---|---|---|
| wall clock, median of 5 | 2.60 s | 2.33 s |
| same, run order reversed | 2.64 s | 2.41 s |
| instructions (`-j 1`) | 35.08 G | 27.37 G |
| `score_message_multi` share | 78.8% | 72.3% |

**−22.0% instructions for −9 to −11% time.** The gap is the campaign's
recurring lesson in the other direction from 0292: the instructions
removed here were cache-missing probes into a 1.37 MB table, so they
were worth more than average — but the walk is memory-bound enough that
even removing the worst instructions does not return their Ir share.

`export /` over the whole corpus is byte-identical to 0292's output,
5 278 322 lines.

Regenerating `prototext/wkt/prebuilt/wkt.rkyv` is part of this change
(7 568 → 8 144 bytes); `wkt_index.rkyv` is untouched by the format bump
and came back byte-identical.
