<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0054 — Scorer stress-test harness

**Status:** implemented
**Implemented in:** 2026-05-12
**App:** prototools (cross-component)

---

## Background

The scorer (spec 0042 / 0048) identifies the protobuf message type of an
unknown binary blob by matching it against a schema DB.  We need an
automated stress-test harness that:

- Builds a realistic, large-scale schema DB from public proto corpora using
  `reproto --build-schema-db` (spec 0056).
- Generates pseudo-random protobuf instances at test time using
  `prototext instantiate-schema` (spec 0056).
- Runs `prototext -d --db` auto-inference against each instance and asserts
  that it identifies the correct ground-truth type.

The harness is intentionally excluded from the regular `nix-build` target
(too slow) and exposed only via `nix-build -A stress-tests`.

---

## Goals

1. A repeatable pipeline that builds the schema DB from pinned corpora at
   test time using `reproto --build-schema-db`.
2. A config file (committed to the repo) listing the message types for
   which protobuf instances are to be generated and tested.
3. Protobuf instances are generated at test time with
   `prototext instantiate-schema` (default seed); they are never committed
   to Git.
4. A test runner that invokes `prototext -d --db` auto-inference for each
   instance and asserts that the inferred type matches the ground truth.
5. Clean separation between the DB-build step (slow, cacheable) and the
   instance-generation + scoring step (fast, per-type).
6. Triggered by `nix-build -A stress-tests`; not triggered by regular
   `nix-build` (i.e. not part of the `ci` closure).

---

## Non-goals

- Benchmarking scorer performance.
- Covering every type in the corpus — only the types listed in the config
  are tested.
- Committing fixture `.pb` files to Git.

---

## Corpus

### Remote corpora (fetched at test time with pinned Git hashes)

| Repo | Pinned hash | Notes |
|---|---|---|
| `googleapis/googleapis` | `83e70370751716489986478edc8713b455b21e86` | Remove `preview/` subtree before compiling |
| `open-telemetry/opentelemetry-proto` | `1d70aa012dc42a5e74a215ce31c1fd84244ce89e` | |

### Local corpus

Custom `.proto` files committed under `tests/stress/protos/` in this repo.
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
reproto --build-schema-db stress.rkyv
        │  (produces stress.rkyv + stress/schemas.pb)
        ▼
for each FQDN in types.yaml:
    prototext instantiate-schema --db stress.rkyv <FQDN> -o instance.pb
        │
        ▼
prototext --list-schemas --db stress.rkyv inst1.pb inst2.pb ...
        │  (YAML: list of {path, types} dicts; DB loaded once)
        ▼
for each result:
    FAIL  if FQDN not in top-tied list  (wrong inference)
    WARN  if len(top-tied) > max_ties   (too many ties, shown in output)
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

### Step 2 — Build schema DB

```bash
reproto --build-schema-db stress.rkyv \
        -I <pb_dir> .
```

All corpora are compiled into a single `<pb_dir>` so reproto sees them
together and deduplicates as needed.  This produces `stress.rkyv` (the
compiled Hopcroft scoring graph) and the sibling `stress/schemas.pb` (the
`FileDescriptorSet` for all types).

### Step 3 — Generate instance and assert

For each type listed in `types.yaml`, generate a pseudo-random instance and
score it with `--list-schemas`:

```bash
prototext instantiate-schema --db stress.rkyv <TYPE> -o instance.pb
prototext --list-schemas --db stress.rkyv instance.pb
```

`--list-schemas` (with default `top=0`) prints only the FQDNs that tie at the
highest score, one per line, sorted lexicographically.  Assert that the
expected FQDN appears in the list and that the number of tied FQDNs does not
exceed `max_ties` (from `types.yaml`, default 5).

---

## Type config

A YAML file committed at `tests/stress/types.yaml`:

```yaml
types:
  - google.api.HttpRule                            # bare FQDN (no leading dot)
  - {fqdn: opentelemetry.proto.trace.v1.Span, max_ties: 10}
  # ... more entries
```

Each entry is either a bare FQDN string or a dict with a `fqdn` key and an
optional `max_ties` key (default 5).  No leading dot.  Entries are sorted
lexicographically by FQDN.

`max_ties` controls a test warning threshold: if `--list-schemas` returns more
than `max_ties` tied FQDNs, the test emits a `warnings.warn` (visible in
pytest output) but still passes, since excessive ties indicate a structurally
ambiguous type in the given corpus rather than a scorer bug.  The warning
message includes the actual tie count.

A test fails (hard `FAIL`) only when the expected FQDN is absent from the
top-tied list — meaning the scorer ranked a different type higher, which
indicates wrong inference.

One `prototext instantiate-schema` call is made per type using the default
seed (0), yielding one instance file.

---

## Test runner

Implemented as a pytest test (`tests/stress/test_stress.py`) that:

1. Calls the DB-build steps (corpus fetch, protoc, reproto) once per
   session via a `pytest` session-scoped fixture, caching results under a
   configurable temp directory.
2. Loads `types.yaml` and parametrizes over the listed FQDNs.
3. For each FQDN, runs `prototext instantiate-schema --db stress.rkyv`
   (default seed 0) to generate an instance, then runs
   `prototext --list-schemas --db stress.rkyv` on it.
4. Asserts the expected FQDN appears in the top-tied list and that
   `len(top-tied) <= max_ties` (from `types.yaml`, default 5).

The DB-build step is skipped if the cache directory already contains a
valid `stress.rkyv` (cache invalidated by a hash of the pinned corpus
hashes + local proto files).

---

## Nix integration

The stress-test derivation is a `pkgs.runCommand` that:

- Takes `reproto`, `prototext`, `protobuf` (for `protoc`), and a Python
  environment with `pytest` as `buildInputs`.
- Runs the full pipeline (corpus fetch → protoc → reproto → pytest).
- Is exposed as `nix-build -A stress-tests`.
- Is **not** included in the `ci` closure (kept separate to avoid slowing
  down the regular CI build).

```nix
stress-tests = pkgs.runCommand "stress-tests" {
  buildInputs = [
    pkgs.protobuf
    pkgs.git
    reproto
    prototext
    (pythonPkgs.python.withPackages (_: [ pythonPkgs.pytest ]))
  ];
} ''
  pytest -p no:cacheprovider ${./tests/stress}/
  touch $out
'';
```

---

## Directory layout

```
tests/stress/
├── types.yaml             # type config (committed)
├── protos/                # custom .proto files (committed)
│   └── ...
└── test_stress.py         # pytest entry point (committed)
```

Generated artefacts (instances, `stress.rkyv`, `stress/schemas.pb`) are
written to a temporary directory at test time and never committed to Git.

---

## Open questions

- Cache invalidation strategy for CI vs. local development.
