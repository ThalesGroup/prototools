<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0056 — CLI polish: reproto graph build + prototext match/infer/instantiate

**Status:** draft
**Implemented in:** —
**App:** prototools (`reproto`, `prototext`, `score-graph`)

---

## Background

The current toolchain has three separate CLI tools that a user must invoke in
sequence to go from `.proto` sources to a scored identification of an unknown
protobuf blob:

1. `reproto --emit-scoring-graphs` — emits per-file scoring-graph YAMLs.
2. `score-graph build-scoring-graph -I <yaml_dir> -o graph.bin` — merges
   YAMLs into a compiled Hopcroft-deduplicated binary graph.
3. `score-graph match <graph.bin> <blob.pb>` — scores the blob against all
   entries and prints a ranked list.

There is also no way to decode a matched blob using the inferred type: the
user must separately obtain a `.pb` descriptor and call `prototext -d
--descriptor ... --type ...`.

Finally, generating a pseudo-random valid protobuf instance for testing
currently requires the external `proto-gen` Python tool, which in turn
shells out to `prototext -d`.  This functionality belongs natively in
`prototext` as a subcommand.

This spec defines three improvements:

- **reproto**: add a `--build-schema-db PATH` option that drives the
  `score-graph build-scoring-graph` step via a proper pyo3 Rust extension
  (not a subprocess shell-out), so a single reproto invocation produces both
  the YAML artifacts, the compiled `.rkyv` graph, and the sibling
  `schemas.pb`.
- **prototext `instantiate-schema`**: a new subcommand that generates a
  pseudo-random valid protobuf instance for a given message type, renders it
  as `#@` prototext, and writes a `# ground_truth:` hint.  Replaces the
  standalone `proto-gen` Python tool.
- **prototext**: give it a first-class notion of a schema DB (a pair of files:
  a multi-FDP `FileDescriptorSet` `.pb` and a compiled scoring graph `.rkyv`),
  configurable via environment variable and overridable via a CLI option —
  analogous to `--proto-variant`.  With a DB available, `prototext -d` can
  infer the message type automatically when no `--descriptor`/`--type` is
  given, behaving like a "I'm Feeling Lucky" decoder.

---

## Goals

1. `reproto --build-schema-db PATH` compiles the scoring graph in-process via
   a pyo3 extension, producing `PATH` (`.rkyv`) and the sibling
   `<stem>/schemas.pb` alongside the YAML output.
2. `prototext` gains a DB concept: a `PROTOTEXT_DB` env var (or `--db DIR`)
   pointing to a directory containing `schemas.pb` (multi-FDP FDS) and
   `graph.rkyv` (compiled scoring graph).
3. `prototext -d` with no `--descriptor` and no `--type`, given a single
   input, auto-infers the message type using the DB: if the top-scoring
   non-vetoed entry is unique it decodes using that type; if multiple entries
   tie for top score it prints an info message listing the tied FQDNs and does
   not decode.  When the type is successfully inferred, its FQDN is written to
   stderr.
4. `prototext -d` with no `--descriptor`/`--type` given multiple inputs is an
   error: a specific `--type` must be provided when batch-decoding.
5. `prototext -d` with `--type` but no `--descriptor` looks up the descriptor
   in `schemas.pb` from the DB.
6. `prototext -d` with both `--descriptor` and `--type` behaves exactly as
   today (DB is ignored for decoding, though it may still be used for tab
   completion).
7. A new `--list-schemas [N]` option prints the ranked candidate list (FQDN +
   score, non-vetoed only, score-descending, ties broken by FQDN
   lexicographic order); `N=0` or omitted means print all.  Can be used
   standalone (no decode) or combined with `-d` (list to stderr, decode to
   stdout).
8. Tab completion for `--type` draws candidates from the DB when no
   `--descriptor` is given.
9. A new `prototext instantiate-schema` subcommand generates a pseudo-random
   valid protobuf instance for a given message type, renders it as `#@`
   prototext with a `# ground_truth:` hint, and writes it to stdout or a file.
   The descriptor is supplied via `--descriptor` or looked up from the DB.
10. The existing `reproto`, `prototext -d/-e`, and `score-graph` interfaces are
    fully preserved (no breaking changes).

---

## Non-goals

- Shell-out from reproto to `score-graph`; the graph build uses a pyo3 Rust
  extension instead (see § reproto `--build-schema-db`).
- `score-graph` CLI changes (it remains the low-level tool).
- A single-file archive bundling graph + schemas (possible future spec).
- Exhaustive handling of tie-breaking beyond a warning.
- Keeping the standalone `proto-gen` Python tool; it is superseded by
  `prototext instantiate-schema`.

---

## Design

### The DB layout

A schema DB consists of two sibling paths that share a base name, mirroring
the layout of reproto's `variants/` directory:

```
path/to/<myprotodb>.rkyv          ← compiled Hopcroft scoring graph
path/to/<myprotodb>/
    schemas.pb                    ← FileDescriptorSet (all FDPs)
```

| File | Format | Description |
|---|---|---|
| `<name>.rkyv` | rkyv-serialised `CompiledGraph` | Compiled Hopcroft scoring graph |
| `<name>/schemas.pb` | `FileDescriptorSet` (protobuf binary) | All FDPs for all known message types |

Both files are designed to be memory-mapped: `<name>/schemas.pb` is parsed
lazily on first lookup; `<name>.rkyv` is zero-copy mmap via `rkyv`.

The DB is identified by the path to the `.rkyv` file.  The sibling
`<name>/schemas.pb` is found by stripping the `.rkyv` suffix and appending
`/schemas.pb`.

The DB location is resolved in this order:

1. `--db PATH` CLI option (path to the `.rkyv` file).
2. `PROTOTEXT_DB` environment variable (same convention).
3. A built-in default pointing at the WKT-only fallback DB shipped with
   prototext (covers all `google.protobuf.*` types with a minimal graph).

---

### reproto `--build-schema-db`

#### New option

```
reproto [existing options] --build-schema-db PATH
```

`PATH` is the output path for the compiled graph (e.g. `out/mydb.rkyv`); the
sibling `out/mydb/schemas.pb` is written automatically.

`--build-schema-db` does **not** imply `--emit-scoring-graphs`.  The YAML
scoring-graph content is generated in memory and passed directly to the pyo3
extension without touching disk.  `--emit-scoring-graphs` remains independently
useful for inspection and debugging.

#### pyo3 Rust extension — interface design

The repo already contains two pyo3 extensions (`prototext_codec` and
`fdp_scan_lib`) that establish the conventions: a Rust cdylib crate under
`<name>-pyo3/`, a `pyproject.toml` wrapper, stub generation via
`pyo3-stub-gen`, and a Nix derivation pair (extension + Python package) in
`default.nix`.  The new extension follows the same pattern.

The extension is named **`score_graph_lib`** (cdylib name) / **`score_graph`**
(Python package), living under `score-graph-pyo3/`.

Three interface options were considered:

---

**Option A — Thin path-in / path-out wrapper**

```python
score_graph.build_graph(
    input_root: str,   # directory walked for *.yaml
    output: str,       # path for <name>.rkyv written by the extension
    quiet: bool = False,
) -> None
```

The extension handles all I/O: walks `input_root` for `*.yaml`, runs the
pipeline, writes the `.rkyv`.  reproto passes only path strings; no binary
data crosses the boundary.

*Pros*: minimal ABI surface; mirrors the existing `score-graph
build-scoring-graph` CLI exactly; easy to use standalone.

*Cons*: requires YAML files to exist on disk first, coupling `--build-schema-db`
to `--emit-scoring-graphs`; the YAML content is already in memory in reproto
and writing it to disk just to re-read it is a redundant round-trip.  The FDPs
for `schemas.pb` are the pruned in-memory versions — not the original on-disk
files in `-I` — so the extension cannot assemble `schemas.pb` correctly from
the filesystem; reproto would have to write them out first too.

---

**Option B — Bytes-in / bytes-out**

```python
score_graph.build_graph(
    yaml_contents: list[bytes],   # YAML content, one entry per scoring-graph file
) -> bytes                        # serialised .rkyv content
```

reproto passes in-memory YAML strings (already constructed during phase 3) as
a list of `bytes`; the extension returns the `.rkyv` content as `bytes`;
reproto writes the file.  `schemas.pb` is assembled entirely on the Python
side from the in-memory descriptor pool, then written by reproto.  No
intermediate files needed.

*Pros*: no disk I/O inside the extension; no coupling between
`--build-schema-db` and `--emit-scoring-graphs`; reproto controls all file
writes; testable without a filesystem; clean separation — Rust does only graph
computation, Python does all I/O.  Size is not a concern: the YAML corpus is a
few MB in total, well within comfortable in-memory passing.

*Cons*: pyo3 copies `list[bytes]` into a Rust `Vec<Vec<u8>>` — one allocation
per YAML string.  Acceptable given the size; zero-copy is not needed here.

---

**Option C — Hybrid: path-in for YAML, bytes-in for FDPs**

```python
score_graph.build_graph(
    input_root: str,          # walked for *.yaml (on disk)
    output: str,              # path for <name>.rkyv
    quiet: bool = False,
) -> None
```

Same as A for graph building, but `schemas.pb` assembled on the Python side.

*Cons*: still requires YAML on disk; mixed responsibility harder to reason
about.

---

**Chosen: Option B.**

The YAML content is already in memory in reproto (produced during
`--emit-scoring-graphs` emission, or generated on the fly when
`--build-schema-db` is given without `--emit-scoring-graphs`).  The pruned
FDPs for `schemas.pb` are also in memory in reproto's descriptor pool.
No intermediate files are needed; the two final outputs (`<name>.rkyv` and
`<stem>/schemas.pb`) are the only disk writes.

`schemas.pb` is assembled on the Python side:

```python
from google.protobuf import descriptor_pb2

fds = descriptor_pb2.FileDescriptorSet()
seen: set[str] = set()
for name, fdp_bytes in reproto_pool_db.items():
    fdp = descriptor_pb2.FileDescriptorProto()
    fdp.ParseFromString(fdp_bytes)
    if fdp.name not in seen:
        fds.file.append(fdp)
        seen.add(fdp.name)
schema_db_dir.mkdir(parents=True, exist_ok=True)
(schema_db_dir / "schemas.pb").write_bytes(fds.SerializeToString())
```

The pyo3 extension interface:

```python
score_graph.build_graph(
    yaml_contents: list[bytes],   # one entry per scoring-graph YAML
) -> bytes                        # .rkyv content, written to disk by caller
```

Errors are surfaced as Python `RuntimeError` with the Rust error message.
The extension is built by the `default.nix` apparatus following the existing
`prototext_codec` / `fdp_scan_lib` derivation pattern, and installed into the
same Python environment as reproto.

After the normal reproto pipeline finishes, reproto calls:

```python
import score_graph

rkyv_bytes = score_graph.build_graph(yaml_contents=list_of_yaml_bytes)
schema_db_path.write_bytes(rkyv_bytes)

# assemble schemas.pb from in-memory pool_db
fds = descriptor_pb2.FileDescriptorSet()
seen: set[str] = set()
for fdp_bytes in pool_db_values:
    fdp = descriptor_pb2.FileDescriptorProto()
    fdp.ParseFromString(fdp_bytes)
    if fdp.name not in seen:
        fds.file.append(fdp)
        seen.add(fdp.name)
schema_db_dir.mkdir(parents=True, exist_ok=True)
(schema_db_dir / "schemas.pb").write_bytes(fds.SerializeToString())
```

If the extension is not importable, reproto exits with a clear error.

---

### prototext: multi-FDP descriptor support

`prototext --descriptor PATH` already accepts both single-FDP and multi-FDP
`FileDescriptorSet` files transparently: `prost-reflect`'s `DescriptorPool::decode`
handles both formats, and cross-file type references are resolved as long as
all transitive dependencies are present in the same FDS.  Reproto's per-file
`.pb` outputs are single-FDP FDS files (not bare FDPs), so they already work.

No code change is required here.  `schemas.pb` (a multi-FDP FDS containing
all corpus FDPs) will work with the existing `parse_schema` call once
`load_schema` in `run.rs` is taught to locate and load it from the DB.

---

### prototext: DB-aware `-d` mode

When a DB is available (via `--db` or `PROTOTEXT_DB`) and the user does not
supply `--descriptor`:

| Inputs | `--type` given? | Behaviour |
|---|---|---|
| Single | No | Score against `graph.rkyv`; if top non-vetoed entry is unique, look up its FDP in `schemas.pb` and decode; write inferred FQDN to stderr.  If tied, print info listing tied FQDNs and exit without decoding. |
| Multiple | No | Error: `--type` is required for batch decoding. |
| Any | Yes | Look up the named type's FDP in `schemas.pb`; decode using that type. |

When both `--descriptor` and `--type` are given, the DB is not used for
decoding (existing behaviour, unchanged).

When `--type` is given and the type is not found in `schemas.pb`, prototext
falls back to the embedded WKT descriptor (existing behaviour) before
erroring.

#### `--list-schemas` and `--top N`

```
prototext -d --list-schemas [--top N] <blob.pb>
```

`--list-schemas` is a boolean flag.  When set, scores the blob and prints the
ranked candidate list: one line per non-vetoed entry, score-descending, ties
broken by FQDN lexicographic order.  Format is the same as `score-graph match`
today.

`--top N` limits the list to the top N entries.  It requires `--list-schemas`
to be present (clap `requires`); specifying `--top` without `--list-schemas`
is an error.

Used standalone (without `-d`): list goes to stdout, no decode.
Combined with `-d`: list goes to stderr, decode proceeds to stdout.

#### Output for auto-inferred type

When the type is auto-inferred (no `--type` given), the magic line carries a
`matched:` annotation:

```
#@ prototext: protoc matched=.google.api.HttpRule score=14
field_name: value  #@ string = 1
```

---

### prototext `instantiate-schema`

#### Subcommand interface

```
prototext [--descriptor PATH] [--db PATH] instantiate-schema [--seed N] <TYPE>
```

| Argument/option | Meaning |
|---|---|
| `<TYPE>` | Fully-qualified message type name (e.g. `.google.protobuf.Timestamp`) |
| `--seed N` | Integer seed (default: 0) |
| `--descriptor PATH` | Descriptor source (single-FDP or multi-FDP FDS); overrides DB |
| `--db PATH` | Schema DB to look up the descriptor from (falls back to env/default) |

The effective PRNG seed is derived as:

```
effective_seed = SHA256(decimal_string(N) + ":" + fqdn)
```

truncated to 32 bytes and used to seed `StdRng::from_seed([u8; 32])`.  This
ensures that two calls with the same integer seed but different types produce
uncorrelated instances, while remaining fully reproducible from the
user-visible `(seed, type)` pair.  The `# seed:` hint comment records the
user-visible integer `N`, not the derived hash.

#### Output format

Identical to `prototext -d` output for the generated binary, with two
additional hint comment lines inserted after the magic line:

```
#@ prototext: protoc
# ground_truth: .google.protobuf.Timestamp
# seed: 0
seconds: 25  #@ int64 = 1
```

Output goes to stdout; the filename hint (if writing to a file) is
`<sanitized_type>_<seed>.pb` where dots are replaced with underscores and the
leading underscore stripped.

#### Generation algorithm

The recursive walk mirrors the Python `proto-gen` tool (spec 0055) exactly,
implemented in Rust using `prost-reflect`'s `DynamicMessage` and
`MessageDescriptor`:

| Field kind | Choice |
|---|---|
| `Cardinality::Optional` | include with probability `p_optional` (default 0.7) |
| `Cardinality::Required` | always include |
| `Cardinality::Repeated` | count drawn from `rng.gen_range(0..=max_repeated)` (default 3) |
| oneof | choose one member uniformly; skip with probability `1 - p_optional` |
| depth ≥ `max_depth` (default 4) | leave `TYPE_MESSAGE` / `TYPE_GROUP` unset |

Leaf value generation mirrors spec 0055 §Leaf field values exactly.

Well-known types `google.protobuf.{Any,Struct,Value,ListValue}` are left
empty with a warning (same as Python v1).

Map fields (detected via `MessageDescriptor::is_map_entry()` on the synthetic
entry message) are treated as repeated with count 0–`max_repeated`.

#### Implementation notes

- `prost-reflect` is already a dependency of `prototext-core`; no new heavy
  dependency needed for the descriptor walk.
- The PRNG is `rand::rngs::StdRng`; `sha2` is the only new dependency (small,
  well-audited).
- `DynamicMessage::set_field` takes a `prost_reflect::Value`; repeated fields
  are set as `Value::List(Vec<Value>)` (built in one pass, not appended).
- The subcommand is a new `run` branch in `prototext/src/run.rs` alongside
  the existing decode/encode paths; the `Cli` struct in `lib.rs` gains an
  `instantiate_schema` subcommand variant.
- Because `instantiate-schema` produces `#@` prototext directly (via
  `render_as_text` from `prototext-core`), no subprocess call is needed —
  the entire pipeline is in-process.

---

### Type completion against DB

When completing `--type` values and no `--descriptor` is given, the shell
completion handler queries `schemas.pb` (the DB, if available) to enumerate
known FQDNs.  This mirrors how `--proto-variant` completion works today.

---

## DB packaging

`reproto --build-schema-db PATH` (where `PATH` ends in `.rkyv`) produces the
full DB in one step, entirely from in-memory data:

1. Scoring-graph YAML content (already in memory) is passed as `list[bytes]`
   to the pyo3 extension → returns `.rkyv` bytes → Python writes `PATH`.
2. All FDPs from reproto's in-memory `pool_db` (the pruned, deduplicated
   versions) are assembled into a `FileDescriptorSet` → Python writes
   `<stem>/schemas.pb`.

No intermediate YAML or `.pb` files are written to disk.
`--emit-scoring-graphs` remains independently available for inspection.
Both outputs together constitute a complete schema DB ready for use with
`prototext --db PATH`.

---

## CLI summary

### reproto (additions only)

```
--build-schema-db PATH   Build the full schema DB at PATH (must end in .rkyv):
                         writes PATH (compiled graph) and PATH-stem/schemas.pb
                         (FDS of all loaded FDPs).
                         Does not imply --emit-scoring-graphs (YAML and
                         FDPs stay in memory; no intermediate files written).
```

### prototext (additions only)

```
    --db PATH             Path to the .rkyv schema DB (also sets schemas.pb
                          location via sibling-directory convention).
                          Overrides PROTOTEXT_DB env var.
    --list-schemas        Score input and print all non-vetoed candidates,
                          score-descending, ties by FQDN.
                          Standalone: stdout, no decode.
                          With -d: stderr, decode proceeds to stdout.
    --top N               Limit --list-schemas output to top N entries.
                          Requires --list-schemas (error if used alone).

  instantiate-schema [--seed N] <TYPE>
                        Generate a pseudo-random valid protobuf instance for
                        TYPE, rendered as #@ prototext with ground_truth and
                        seed hint comments.  Descriptor sourced from
                        --descriptor or the DB (--db / PROTOTEXT_DB).
                        --seed N: integer seed (default 0); effective PRNG
                        seed is SHA256(N + ":" + FQDN) → StdRng.
```

### prototext `-d` behaviour changes

```
prototext -d [--db PATH] [--type NAME] [--descriptor PATH] [blob.pb ...]
```

- Single input, no `--descriptor`, no `--type`: auto-infer from DB.
  Unique winner → decode + write FQDN to stderr.
  Tied winners → print tied FQDNs as info, no decode.
- Multiple inputs, no `--type`: error.
- No `--descriptor`, with `--type`: look up descriptor in DB's schemas.pb.
- With `--descriptor` + `--type`: unchanged from today.

---

## Implementation order

1. Implement `prototext instantiate-schema` in Rust (`prototext-core` +
   `prototext`): recursive `DynamicMessage` walk, SHA256-derived seed,
   `render_as_text` output, `# ground_truth:` / `# seed:` hints.  Add `rand`
   and `sha2` dependencies.  Supersedes the standalone `proto-gen` Python tool.
2. Build pyo3 `score_graph_lib` extension (`score-graph-pyo3/`) with
   `build_graph(yaml_contents: list[bytes]) -> bytes`; wire
   `reproto --build-schema-db` to call it in-memory and write
   `<name>.rkyv` + `<stem>/schemas.pb` with no intermediate disk artifacts.
3. Add `--db PATH` / `PROTOTEXT_DB` support: teach `load_schema` in `run.rs`
   to load `schemas.pb` from the DB for `--type` lookup (multi-FDP FDS
   already supported by existing `parse_schema`; no changes to `schema.rs`).
4. Add auto-inference (no `--type`, single input) using `<name>.rkyv` scoring.
5. Add `--list-schemas` / `--top N` flags.
6. Add type-completion against DB.
