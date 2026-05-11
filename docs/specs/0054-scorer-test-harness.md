<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0054 — Scorer test harness

**Status:** draft
**Implemented in:** —
**App:** prototools (cross-component)

---

## Background

The scorer (spec 0042 / 0048) identifies the protobuf message type of an
unknown binary blob by matching it against a scoring graph DB.  We need an
automated regression harness that:

- Builds a realistic, large-scale scoring graph DB from public and custom
  proto corpora.
- Runs the scorer against a curated set of protobuf message instances.
- Asserts that each instance scores highest for its known ground-truth type.

The harness also serves as the foundation for the scoring tutorial (spec
0050): it produces the DB and fixture instances that the tutorial references.

---

## Goals

1. A repeatable pipeline that builds the scoring graph DB from a pinned
   corpus at test time.
2. A curated fixture inventory (committed to the repo) listing the types
   to test and the `.pb` instance files to score against them.
3. A test runner that invokes the scorer for each fixture and asserts
   correctness.
4. Clean separation between the DB-build step (slow, cacheable) and the
   scoring step (fast, per-fixture).

---

## Non-goals

- Polishing the `hopcroft-db` CLI wrapper (future work, separate spec).
- Benchmarking scorer performance.
- Covering every type in the corpus — only the curated inventory is tested.

---

## Corpus

### Remote corpora (fetched at test time with pinned Git hashes)

| Repo | Pinned hash | Notes |
|---|---|---|
| `googleapis/googleapis` | TBD | Remove `preview/` subtree before compiling |
| `open-telemetry/opentelemetry-proto` | TBD | |

### Local corpus

Custom `.proto` files committed under `tests/harness/protos/` in this repo.
These cover types and patterns not well represented in the public corpora.

---

## Pipeline

```
fetch corpora (pinned hashes)
        │
        ▼
protoc (per .proto → mono-fdp .pb, no --include_imports)
        │
        ▼
reproto --use-variant all --emit-scoring-graphs
        │
        ▼
hopcroft-db build  →  scoring_graph.db
        │
        ▼
scorer --db scoring_graph.db  <fixture.pb>  →  ranked type list
        │
        ▼
assert ground_truth rank == 1
```

### Step 1 — Fetch and compile

For each remote corpus, `git clone --depth=1` at the pinned hash into a
temp directory.  For googleapis, `rm -rf preview/` before compiling.

Each `.proto` is compiled independently:

```bash
protoc --descriptor_set_out=<out>/<flat_name>.pb \
       --proto_path=<corpus_root> \
       --proto_path=<wkt_root> \
       <relative_proto_path>
```

where `<flat_name>` is the proto path with `/` replaced by `_` and
`.proto` stripped.

### Step 2 — reproto

```bash
reproto --use-variant all --emit-scoring-graphs \
        -I <pb_dir> -O <graph_dir> .
```

All corpora are compiled into a single `<pb_dir>` so reproto sees them
together and deduplicates as needed.

### Step 3 — hopcroft-db build

```bash
cargo run -p hopcroft-db -- build \
    --input <graph_dir> \
    --output scoring_graph.db
```

If the merged build fails due to unresolvable conflicts, fall back to
per-corpus DBs (out of scope for this spec; noted as a contingency).

### Step 4 — Score and assert

For each fixture listed in the inventory (§ Fixture inventory):

```bash
scorer --db scoring_graph.db <fixture.pb>
```

Parse the ranked output, extract the top-ranked type FQDN, compare with
the `ground_truth` embedded in the fixture's leading `#` comment.  Assert
equality.

---

## Fixture inventory

A YAML file committed at `tests/harness/fixtures.yaml`:

```yaml
fixtures:
  - path: tests/harness/fixtures/google_api_http_rule.pb
    ground_truth: .google.api.HttpRule
  - path: tests/harness/fixtures/otel_trace_span.pb
    ground_truth: .opentelemetry.proto.trace.v1.Span
  - path: tests/harness/fixtures/custom_my_message.pb
    ground_truth: .mycompany.myservice.MyMessage
  # ... more entries
```

Each `.pb` fixture file is a `#@` prototext file (suffix `.pb`, magic line
`#@ prototext: protoc`, ground-truth comment on the second line).  See spec
0055 for how these files are generated.

---

## Test runner

Implemented as a pytest test (`tests/harness/test_scorer_harness.py`) that:

1. Calls the pipeline build steps (corpus fetch, protoc, reproto,
   hopcroft-db) once per session via a `pytest` session-scoped fixture,
   caching results under a configurable cache directory.
2. Parametrizes over the fixture inventory YAML.
3. For each fixture, invokes the scorer subprocess and asserts
   `top_ranked_fqdn == ground_truth`.

The build steps are skipped if the cache directory already contains a
valid `scoring_graph.db` (cache invalidated by hash of pinned corpus
hashes + local proto files).

---

## Directory layout

```
tests/harness/
├── fixtures.yaml          # fixture inventory (committed)
├── fixtures/              # .pb prototext fixture files (committed)
│   ├── google_api_http_rule.pb
│   └── ...
├── protos/                # custom .proto files (committed)
│   └── ...
└── test_scorer_harness.py # pytest entry point
```

---

## Open questions

- Exact pinned hashes for googleapis and opentelemetry-proto (to be
  recorded once corpus selection is finalised).
- Whether a single merged DB is feasible or per-corpus DBs are needed
  (depends on hopcroft-db conflict behaviour).
- Cache invalidation strategy for CI vs. local development.
