<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0050 — Scoring tutorial and regression test suite

**Status:** draft
**Implemented in:** —
**App:** score-graph

---

## Background

The `score-graph` tool now has a complete scoring pipeline: `reproto`
emits per-file scoring-graph YAMLs, `build-scoring-graph` compiles them
into a deduplicated `.bin` graph, and `match` scores a binary payload
against all candidate types simultaneously.

What is missing is an end-to-end demonstration suitable for new users, and
a regression test that verifies the scorer correctly identifies known
message instances.

---

## Goals

1. Provide a self-contained tutorial (a Markdown document plus companion
   files) that walks through the full scoring workflow from proto schemas
   to ranked match results, using well-known public schemas as the
   narrative examples.

2. Provide a regression test (a Rust integration test or shell script)
   that encodes a set of known message instances and asserts that the
   correct type appears among the top-scoring results.

3. Exercise all major scoring features in the fixture set:
   - matched optional fields
   - matched repeated fields
   - matched enum fields (with range checking)
   - matched nested sub-messages (LEN_MSG)
   - unknown fields (score penalty)
   - required fields present (no penalty) and absent (mismatches penalty)
   - group fields (GROUP wire type)
   - non-canonical encoding (non_canonical penalty)

4. Validate robustness of the tooling against large schema repositories
   and large message instances (via the OpenTelemetry schema set).

---

## Non-goals

- Benchmarking or performance testing.
- Testing the `reproto` tool itself.
- Covering every possible veto condition (those are already covered by the
  unit and e2e tests in `score-graph/src/score/tests.rs` and
  `score-graph/tests/score_e2e.rs`).

---

## Specification

### §1 — Schema sources

The tutorial uses three schema sets.

**Set A — googleapis/api-common-protos (Apache 2.0)**
Source: https://github.com/googleapis/api-common-protos

Selected types (proto3, rich nesting and enums):
- `google.rpc.Status` — code (enum), message (string), details (repeated Any)
- `google.rpc.BadRequest` — nested FieldViolation messages (repeated)
- `google.rpc.RetryInfo` — retry_delay (Duration sub-message)
- `google.longrunning.Operation` — name (string), done (bool), oneof result

These types are well-known, hierarchically nested, and immediately
recognizable to any reader familiar with Google APIs.

**Set B — bespoke proto2 supplement**
A small hand-crafted `tutorial.proto` (proto2 syntax) living in
`score-graph/tests/fixtures/tutorial/` that exercises required fields and
groups, which proto3 does not have:
- `tutorial.Person` — required string name, required int32 id, optional
  string email, repeated `tutorial.PhoneNumber` (with PhoneType enum),
  optional group `tutorial.Address` (street, city, zip)
- `tutorial.AddressBook` — repeated Person

This is modelled loosely on the classic protobuf tutorial schema
(https://protobuf.dev/getting-started/gotchas/) but extended with a group
field to exercise that code path.

**Set C — OpenTelemetry proto (Apache 2.0)**
Source: https://github.com/open-telemetry/opentelemetry-proto

Used for robustness validation rather than narrative tutorial.  The
OpenTelemetry schema is proto3, moderately large (~30 files, hundreds of
message types), and features deeper nesting than the googleapis types —
traces, spans, attributes, and events nest three to four levels deep, and
`ExportTraceServiceRequest` wraps multiple `ResourceSpans` each containing
multiple `ScopeSpans` each containing multiple `Span` records.

The robustness test loads the full OpenTelemetry schema into the compiled
graph alongside Sets A and B, crafts a large `ExportTraceServiceRequest`
instance with many spans and attributes, and asserts:
- `build-scoring-graph` completes without error and produces a valid graph.
- `score-graph match` completes without error and ranks
  `opentelemetry.proto.collector.trace.v1.ExportTraceServiceRequest`
  at the top (or tied for top).

### §2 — Descriptor pipeline

`reproto` takes compiled `.pb` file descriptor sets (or their `.textpb`
equivalents) as input.  Proto source files must be compiled to descriptors
first.  `protoc` is therefore a prerequisite for generating descriptors
from Sets A and B.

The compiled `.pb` descriptors for Sets A, B, and C are committed to the
repository under `score-graph/tests/fixtures/tutorial/descriptors/`.
They are produced once (by the developer, not at CI test time) using:

```sh
# Set A — googleapis
protoc --descriptor_set_out=descriptors/api-common-protos.pb \
  --include_imports --proto_path=<googleapis-checkout> \
  google/rpc/status.proto google/rpc/error_details.proto \
  google/longrunning/operations.proto

# Set B — bespoke proto2
protoc --descriptor_set_out=descriptors/tutorial.pb \
  --include_imports tutorial.proto

# Set C — OpenTelemetry
protoc --descriptor_set_out=descriptors/opentelemetry.pb \
  --include_imports --proto_path=<otel-checkout> \
  opentelemetry/proto/collector/trace/v1/trace_service.proto \
  opentelemetry/proto/trace/v1/trace.proto \
  opentelemetry/proto/common/v1/common.proto \
  opentelemetry/proto/resource/v1/resource.proto
```

The committed `.pb` files mean `protoc` is not required at CI test time;
it is only needed when updating the descriptor files (e.g. when pulling
new upstream schema versions).

### §3 — Fixture files

All fixture files live under `score-graph/tests/fixtures/tutorial/`.

```
score-graph/tests/fixtures/tutorial/
  tutorial.proto                  bespoke proto2 schema (Set B)
  descriptors/
    api-common-protos.pb          compiled descriptor for Set A
    tutorial.pb                   compiled descriptor for Set B
    opentelemetry.pb              compiled descriptor for Set C
  proto/
    rpc_status_full.textpb        google.rpc.Status — all fields populated
    rpc_status_minimal.textpb     google.rpc.Status — only code field
    rpc_badrequest.textpb         google.rpc.BadRequest — two violations
    rpc_retryinfo.textpb          google.rpc.RetryInfo — with Duration
    person_full.textpb            tutorial.Person — all fields including group
    person_missing_req.textpb     tutorial.Person — required id absent
    person_repeated_req.textpb    tutorial.Person — required name duplicated
    addressbook.textpb            tutorial.AddressBook — two persons
    otel_trace_large.textpb       ExportTraceServiceRequest — many spans (robustness)
```

All `.textpb` files use the `#@ prototext: 1` magic line and may include
human-readable `#`-comments (the encoder ignores them correctly).  They
are encoded to binary at test time using `prototext -e`; the binary form
is not committed.

### §4 — Tutorial document

A Markdown document at `docs/tutorial-scoring.md` walks through:

1. **Setup** — building `score-graph` and `reproto` from source.
2. **Schema description** — brief explanation of the chosen schemas and
   what makes them interesting for scoring.
3. **Generating the YAML graphs** — running `reproto --emit-scoring-graphs`
   on the googleapis protos and on `tutorial.proto`.
4. **Building the compiled graph** — running `build-scoring-graph` and
   interpreting the summary output (message count, dedup ratio,
   transitions, root entries).
5. **Encoding fixtures** — using `prototext -e` to convert `.textpb` to
   binary.
6. **Running the scorer** — running `score-graph match` on each fixture
   and interpreting the output columns (`matches`, `unknowns`,
   `mismatches`, `non_canonical`, `score`).
7. **Interpreting ties** — explaining why structurally identical types
   share the same top score (bisimulation equivalence), and what that
   means for the application.
8. **Scoring features demonstrated** — a table mapping each fixture to the
   scoring feature it primarily exercises.

### §5 — Regression test

A Rust integration test at `score-graph/tests/tutorial_regression.rs`,
consistent with the existing `score_e2e.rs` pattern:

1. Runs `reproto --emit-scoring-graphs` on all three descriptor sets
   (subprocesses), producing YAML files in a temp directory.
2. Runs `build-scoring-graph` on the combined YAML output (subprocess).
3. For each fixture, encodes the `.textpb` to binary using `prototext -e`
   (subprocess).
4. Runs `score-graph match` (subprocess).
5. Asserts correctness per fixture (see §6).

The correctness assertion is deliberately permissive about ties: two types
that share a state after Hopcroft minimization are structurally
indistinguishable, and forcing a strict rank-1 assertion would make the
test fragile with respect to schema additions.  The assertion is:

> The known-correct type appears in the output with a score equal to the
> highest score among all non-vetoed entries.

For fixtures that deliberately exercise a penalty (missing required field,
duplicated required field, unknown fields), the assertion additionally
checks the expected counter values (e.g. `mismatches=1`).

**Robustness assertions** (Set C / OpenTelemetry):
- `build-scoring-graph` completes without error on the combined graph.
- The combined graph's state count and transition count are logged to
  stderr (for manual inspection) but not hard-coded in the assertion,
  since upstream schema changes may alter them.
- `score-graph match` on `otel_trace_large.textpb` completes without
  error and ranks the correct type at the top.

### §6 — Expected fixture scores (informational)

These are the expected results for each fixture once implemented, listed
here to guide implementation and catch regressions.

| Fixture | Correct type | Expected matches | Expected mismatches | Expected unknowns | Notes |
|---|---|---|---|---|---|
| rpc_status_full | google.rpc.Status | ≥5 | 0 | 0 | all fields known |
| rpc_status_minimal | google.rpc.Status | 1 | 0 | 0 | only code set |
| rpc_badrequest | google.rpc.BadRequest | ≥4 | 0 | 0 | repeated sub-messages |
| rpc_retryinfo | google.rpc.RetryInfo | ≥2 | 0 | 0 | Duration sub-message |
| person_full | tutorial.Person | ≥6 | 0 | 0 | enum + group |
| person_missing_req | tutorial.Person | ≥1 | ≥1 | 0 | required id absent |
| person_repeated_req | tutorial.Person | ≥2 | 0 | 0 | non_canonical ≥1 |
| addressbook | tutorial.AddressBook | ≥8 | 0 | 0 | repeated Person |

Exact counts will be filled in from observed test output and locked into
the regression assertions.

---

## Open questions

1. The compiled `.pb` descriptors for Sets A, B, and C are pre-committed
   to the repository so that `protoc` is not required at CI time.  The
   developer workflow for updating descriptors (when pulling new upstream
   schema versions) should be documented in the tutorial.  Should
   `protoc` be added to the project's Nix shell, or left as an
   out-of-band prerequisite documented only in a README?

2. Should the regression test be a Rust integration test (like
   `score_e2e.rs`) or a shell script?  Recommendation: Rust test for CI,
   with the equivalent shell commands also shown verbatim in the tutorial
   Markdown so a reader can follow along interactively.

3. The `otel_trace_large.textpb` fixture needs to be large enough to be
   a meaningful robustness test.  How large?  Suggestion: ~50 spans, each
   with ~10 attributes, giving O(500) nested message instances and a
   binary payload of several kilobytes.  Confirm the target size.
