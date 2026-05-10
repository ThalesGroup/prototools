<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0043 — Hopcroft-deduplicated schema database

**Status:** draft
**Implemented in:** —
**App:** prototext-core (new crate: `schema-db`)

---

## Background

Spec 0042 defines `score_message`, which scores one wire message against one
`ParsedSchema`.  The longer-term goal (see `docs/schema-match.md`) is to
score a wire message against a large corpus of schemas simultaneously.  That
requires a pre-compiled, Hopcroft-deduplicated representation of the corpus.

This spec defines how to build that representation incrementally from a
collection of `.proto` source files, with a validation checkpoint at each
stage boundary.

The construction pipeline has three stages:

```
Stage 1   .proto source files (corpus)
            │  reproto --proto-variant <variant> --use-variant all
            │  (one invocation over the whole corpus)
            ▼
Stage 2   canonical .proto files  (one per schema, in a single output tree)
            │  protoc --include_imports --descriptor_set_out
            │  (one .pb per .proto — MUST be one-to-one; reproto cannot
            │   ingest multi-FDP .pb files)
            ▼
Stage 3   .pb FileDescriptorSet files  (one per .proto, self-contained)
            │  graph builder + Hopcroft minimiser + serialiser
            ▼
Stage 4   CompiledGraph  (mmap-ready binary; structure from
                          docs/schema-match-impl-notes.md §2.3)
```

The one-to-one constraint between `.proto` and `.pb` in Stage 2→3 is
critical: each `.pb` must be a self-contained `FileDescriptorSet` produced
from a single root `.proto` with `--include_imports`.  reproto's graph
builder loads schemas one `.pb` at a time and cannot handle `.pb` files
that bundle multiple unrelated `FileDescriptorProto` records.

---

## Goals

1. Define the three-stage pipeline and the artifact produced at each stage.
2. Define a validation checkpoint at each stage boundary.
3. Produce a `CompiledGraph` from a corpus of `.proto` files.
4. Implement the pipeline as a new binary crate `schema-db` in this repo.
5. Keep the pipeline runnable on any corpus of `.proto` files — the specific
   corpus used for development is not part of this spec (see companion
   document, gitignored).

## Non-goals

- The multi-schema parallel walk (spec 0042 Future work).
- Incremental graph updates when new schemas are ingested.
- Python bindings for the compiled graph.
- Scoring against the compiled graph (spec 0042's territory).

---

## Specification

### Stage 1 → Stage 2: canonicalise with reproto

`reproto` is invoked once over the entire corpus input directory, producing
a tree of canonical `.proto` files:

```sh
reproto \
  --proto-variant <variant-file> \
  --use-variant all \
  -I <corpus-input-dir> \
  -O <proto-output-dir> \
  .
```

`--use-variant all` substitutes all well-known-type variants (Timestamp,
Duration, Any, etc.) with the variant's canonical copies, ensuring the
output `.proto` files compile cleanly against a standard `protoc`
installation with no additional include paths beyond `<proto-output-dir>`.

The result is a directory tree of `.proto` files, one per schema in the
corpus.  The specific variant file and any corpus-specific exclusions (`-p`)
are documented in the companion document.

**Checkpoint 1** — reproto exits with code 0 and the output tree is
non-empty.  Report: number of `.proto` files produced.

### Stage 2 → Stage 3: compile to self-contained `.pb`

Each `.proto` in the output tree is compiled to a binary `FileDescriptorSet`
with `protoc`, one-to-one:

```sh
protoc \
  --proto_path=<proto-output-dir> \
  --descriptor_set_out=<pb-output-dir>/<rel-path>.pb \
  <proto-output-dir>/<rel-path>.proto
```

`--include_imports` is intentionally **omitted**.  Each `.pb` contains only
the single `FileDescriptorProto` for its root `.proto`; imports are
referenced by name but not embedded.  This preserves the one-FDP-per-file
invariant that the graph builder requires.  The reproto output tree is
already fully self-contained (every import resolves within the tree), so
cross-file references are resolved at graph-build time by loading all `.pb`
files together into a shared `DescriptorPool`.

`protoc` is invoked as a subprocess.  A system `protoc` is required (not
`protox`) because the canonical `.proto` files may use features that `protox`
does not yet support.

**Checkpoint 2** — for each `.pb`:
- Decodes as a valid `FileDescriptorSet` containing exactly one
  `FileDescriptorProto` (the root `.proto`; no embedded imports).
- When all `.pb` files are loaded together into a single `DescriptorPool`,
  every top-level message type across all root `FileDescriptorProto`s is
  resolvable (`pool.get_message_by_name` succeeds for all of them).

Report: number of `.pb` files produced, number failing checkpoint 2, list
of failures with error messages.

### Stage 3 → Stage 4: build the CompiledGraph

Three sub-steps, each independently invokable.

#### Sub-step A: build the schema graph

For each `.pb` from stage 3, load all message types into a single directed
graph `G = (V, E, λ)` as described in `docs/schema-match.md` Part 2:

- One node per `(schema_id, message_type_name)` pair across all `.pb` files.
- Five canonical leaf nodes, one per wire type (0, 1, 2, 3, 5), shared
  across all schemas.
- One directed edge per field: source = the message-type node, target =
  the child message-type node (for `Kind::Message`) or the matching leaf
  node (all other kinds).  Edge label: `WireTag = (field_number << 3) |
  expected_wire_type`.

`expected_wire_type` is determined by the `ScoringKind` mapping from spec
0042 §Schema information required for scoring.

"One schema" = one top-level message type in the root `FileDescriptorProto`
of a `.pb` file.  A `.pb` whose root `FileDescriptorProto` declares N
top-level message types contributes N schemas (N root nodes) to the graph.

**Checkpoint 3** — graph sanity:
- Node count = (sum of all message types across all `.pb` files, counting
  each unique `(schema_id, type_name)`) + 5 leaf nodes.
- Every edge label is a valid wire tag (field number 1–536870911, wire
  type in {0,1,2,3,5}).
- No dangling edges.

Report: schema count, node count, edge count, average and max out-degree.

#### Sub-step B: Hopcroft partition refinement

Run Hopcroft minimisation on `G` per `docs/schema-match.md` Part 2
§Deduplication Algorithm.

Initial partition: nodes grouped by outgoing-edge signature (sorted list of
wire tags).  The five leaf nodes are pre-placed in five singleton blocks.

Refinement: standard Hopcroft worklist loop until stable.  Cycles are
handled correctly by the fixpoint argument.

Output: a `Partition` mapping each node to a block (StateID).

**Checkpoint 4** — deduplication quality:
- States before = node count (from checkpoint 3, excluding leaf nodes).
- States after = number of blocks in the final partition.
- Deduplication ratio = states after / states before (expect < 1, often
  significantly so due to shared well-known types).
- Spot-check: `google.protobuf.Timestamp` nodes from different schemas must
  map to the same StateID.

Report: states before/after, ratio, spot-check pass/fail.

#### Sub-step C: serialise to CompiledGraph

Build and write the `CompiledGraph` from `docs/schema-match-impl-notes.md`
§2.3:

- `transitions`: flat array of `TransitionEntry { state_id, wire_tag,
  child_state_id, is_message }`, sorted by `(state_id, wire_tag)`.
- `static_back_ref_start` / `static_back_ref_data`: CSR layout of
  `StaticBackRef { schema_id, msg_type_id }` per StateID.
- `roots`: one `RootEntry { schema_id, state_id }` per schema.

Serialised with `rkyv`; written to a single file with an 8-byte magic
(`b"PTSGRAPH"`) and an 8-byte offset to the `rkyv` root.

**Checkpoint 5** — round-trip load:
- Deserialise; verify root count equals schema count from checkpoint 3.
- For a sample of schemas, confirm that the transition table reachable from
  the root StateID reproduces the field numbers and wire types of the
  original `.pb`.
- Measure mmap load time on a warm page cache (target: < 10 ms regardless
  of file size).

Report: file size, load time, transition entry count.

---

## Incremental development order

Each step produces a runnable artifact that can be validated independently:

1. **Stage 1→2** (reproto): shell out to reproto; validate checkpoint 1.
   No graph code yet.

2. **Stage 2→3** (protoc): shell out to protoc one-to-one; validate
   checkpoint 2.

3. **Sub-step A** (graph build): load all `.pb` files, build `G`; validate
   checkpoint 3.  No Hopcroft yet — output a node/edge count report and a
   human-readable dump of a sample schema's subgraph.

4. **Sub-step B** (Hopcroft): run minimisation; validate checkpoint 4.
   Output a debug partition dump for inspection before committing to the
   binary format.

5. **Sub-step C** (serialise): write `CompiledGraph`; validate checkpoint 5.

---

## New crate: `schema-db`

Binary crate at `schema-db/` in the workspace root.

```
schema-db/
  Cargo.toml
  src/
    main.rs          — CLI (clap subcommands: reproto, compile, build-graph,
                        minimise, serialise)
    stage_reproto.rs — Stage 1→2
    stage_compile.rs — Stage 2→3
    graph.rs         — Sub-step A
    hopcroft.rs      — Sub-step B
    serialise.rs     — Sub-step C
    checkpoint.rs    — Shared validation / reporting helpers
```

Dependencies: `prototext-core` (wire-type constants, `parse_schema`),
`prost-reflect`, `prost`, `rkyv`, `memmap2`, `clap`.

All intermediate and final artifacts are written under `schema-db/data/`
(gitignored — see `.gitignore`).
