<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0059 — Hopcroft minimizer regression suite

**Status:** implemented
**Implemented in:** 2026-05-13
**App:** score-graph

---

## Background

The Hopcroft minimizer in `score-graph` has been observed to produce
incorrect (too-coarse) partitions — merging distinct message types into the
same equivalence class.  Two bugs have been found and fixed:

- **Spec 0062**: incorrect worklist maintenance (root cause of the
  non-deterministic large-corpus failure).
- **Spec 0060**: dropped (the `origin_node_kind` issue turned out not to be
  a separate bug once 0062 was fixed).

This spec defines:

1. A Rust binary (`hopcroft_dump`) for manual inspection of compiled graphs.
2. A Rust integration-test suite with pass/fail assertions for each fixture.
3. A **full-corpus regression test** that loads the stress-test scoring-graph
   YAMLs and asserts specific structural properties of the compiled graph.

---

## Goals

1. `hopcroft_dump` binary: reads `<fixture-dir>/input/**/*.yaml`, compiles,
   prints compiled-graph YAML to stdout.  Already implemented; no change.
2. Integration tests in `score-graph/tests/` with explicit pass/fail
   criteria for TC-01 through TC-07.
3. Full-corpus test: loads all YAMLs from the stress DB `reproto-out/`
   directory (path supplied via environment variable) and asserts that
   `test.field.MapWithOptions.AnnotatedMapEntry` and
   `test.field.MapWithOptions.MultiOptionMapEntry` end up in **distinct**
   Hopcroft states.
4. Update `scoring_graph_lib.build_graph()` to return `(bytes, str | None)`
   tuple (already implemented; no change needed).

## Non-goals

- Changes to the `.rkyv` binary format.
- Changes to the scoring walk.
- The `--emit-yaml-db` reproto CLI flag and new `--build-schema-db` file
  layout (deferred to a later spec).
- Fuzzing or property-based testing.

---

## Specification

### §1 — Dump binary (already implemented)

`score-graph/src/bin/hopcroft_dump.rs` accepts a `<fixture-dir>` argument,
reads all `*.yaml` files under `<fixture-dir>/input/` recursively, compiles
them via `build_from_strings(&yamls, true)`, and prints the compiled-graph
YAML to stdout.

No changes required to the existing implementation.

### §2 — Compiled-graph YAML format (already implemented)

The dump format is a YAML document with three top-level keys: `states`,
`transitions`, `roots`.  See `serial::dump_compiled` for the full format.

### §3 — Fixture format

Each fixture lives under `score-graph/tests/fixtures/hopcroft/<case>/`:

```
<case>/
  input/
    **/*.yaml     ← one or more scoring-graph YAMLs (spec 0045 §2 format)
```

All `*.yaml` files found recursively under `input/` are loaded and merged
before compilation.

### §4 — Integration test binary

Add `score-graph/tests/hopcroft_suite.rs` as an integration test (declared
in `Cargo.toml` under `[[test]]`).

The test binary:

1. Locates each fixture directory under `tests/fixtures/hopcroft/`.
2. Loads all `input/**/*.yaml` files for that fixture.
3. Calls `build_from_strings(&yamls, false)` to get the compiled graph bytes.
4. Deserializes the compiled graph from the rkyv bytes.
5. Applies the check criteria for that fixture (see §5).

Each fixture is a separate `#[test]` function.

### §5 — Check criteria per fixture

Check criteria are expressed in terms of the compiled `CompiledGraph`:
specifically the `roots` table (FQDN → state ID) and the `transitions`
table (state ID × field number → child state ID).

#### TC-01: Two identical nodes collapse

FQDNs: `A`, `B`.

**Assert**: `state_of("A") == state_of("B")`.

#### TC-02: Different field number → distinct

FQDNs: `A` (field 1 → VARINT), `B` (field 2 → VARINT).

**Assert**: `state_of("A") != state_of("B")`.

#### TC-03: Different field type → distinct

FQDNs: `A` (field 1 → LEN_STRING), `B` (field 1 → VARINT).

**Assert**: `state_of("A") != state_of("B")`.

#### TC-04: Different label → distinct

FQDNs: `A` (field 1 optional), `B` (field 1 repeated).

**Assert**: `state_of("A") != state_of("B")`.

#### TC-05: LENDEL vs GROUP → distinct

FQDNs: `A` (LENDEL, field 1 → VARINT), `B` (GROUP, field 1 → VARINT).

**Assert**: `state_of("A") != state_of("B")`.

#### TC-06: AnnotatedMapEntry / MultiOptionMapEntry regression

FQDNs: `AnnotatedMapEntry`, `MultiOptionMapEntry`, `MapWithOptions`.

**Assert**:
- `state_of("AnnotatedMapEntry") != state_of("MultiOptionMapEntry")`.
- `child_state_of("MapWithOptions", field=1, label=repeated) != child_state_of("MapWithOptions", field=2, label=repeated)`.

#### TC-07: Bisimilar roots collapse

FQDNs: `Root1`, `Root2` (structurally identical sub-trees).

**Assert**: `state_of("Root1") == state_of("Root2")`.

### §6 — Full-corpus regression test

Add a test `hopcroft_full_corpus` gated on the environment variable
`HOPCROFT_CORPUS_DIR`.  When the variable is not set, the test is skipped
(`cargo test` passes without the corpus).  When set, it must point to the
`reproto-out/` directory produced by the stress DB derivation (e.g.
`/nix/store/.../reproto-out`).

The test:

1. Walks `HOPCROFT_CORPUS_DIR` recursively and collects all `*.yaml` files.
2. Reads each file as a `String`.
3. Calls `build_from_strings(&yamls, false)` **8 times** in a loop.
   The bug (spec 0062) fired ~75% of runs due to HashMap non-determinism;
   8 independent compilations give high confidence of detection.
4. Deserializes the compiled graph.
5. **Asserts**:
   - `state_of("test.field.MapWithOptions.AnnotatedMapEntry") != state_of("test.field.MapWithOptions.MultiOptionMapEntry")`.
   - `child_state_via("test.field.MapWithOptions", field=1, label=repeated) != child_state_via("test.field.MapWithOptions", field=2, label=repeated)`.

This test catches any regression of the bug fixed in spec 0062: the two
map-entry types that were incorrectly merged must remain distinct in the
compiled graph regardless of HashMap iteration order (i.e. across multiple
runs).

The test is placed in `score-graph/tests/hopcroft_suite.rs` alongside the
fixture tests.

### §7 — Cargo.toml additions

Add to `score-graph/Cargo.toml`:

```toml
[[test]]
name = "hopcroft_suite"
path = "tests/hopcroft_suite.rs"
```

The existing `[[bin]]` entry for `hopcroft_dump` and the `walkdir`
dependency are already present.

---

## Files changed

| File | Change |
|---|---|
| `score-graph/tests/hopcroft_suite.rs` | New: integration tests for TC-01 through TC-07 and the full-corpus regression test |
| `score-graph/Cargo.toml` | Add `[[test]]` entry for `hopcroft_suite` |
