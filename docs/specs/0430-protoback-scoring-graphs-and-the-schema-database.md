<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0430 — protoback, scoring graphs and the schema database

Status: draft
App: protoback, prototext-graph
Refs: docs/specs/0424-protoback-reproto-in-rust-on-upb.md (step 6 of its
      work plan; S8, Python's formatting; S16, the googleapis target;
      S20, no YAML between protoback and prototext-graph);
      docs/specs/0429-protoback-rendering.md (the descriptors the
      database collects, and the pool emulation, S4)

## Background

After step 5, protoback writes reproto's `.proto` and `.pb` files on
every corpus (spec 0429, measured outcome). What it still refuses are
the outputs after rendering. They are what `prototext`, `protolens` and
the WKT prebuilt are built from:

- `--emit-scoring-yaml`: one scoring graph per summoned file, in YAML,
  beside its `.proto` (`_phase_emit_scoring_graphs`);
- `--schema-db-out DB.desc`: the schema database
  (`_phase_build_schema_db`):
  - `DB.desc`, a `FileDescriptorSet` of the rendered descriptors, the
    WKT dependencies phase 6 set aside included;
  - `DB/hopcroft.rkyv`, the scoring graphs merged and minimized by
    `prototext-graph`;
  - `DB/index.rkyv`, the index of `DB.desc` by type, file and
    extension;
- `--emit-extension-ranges`: each message's extension ranges in its
  scoring graph;
- `--scoring-html-out`, `--with-leaf-nodes`, `--hide`: the graph drawn
  with pyvis, before and after minimization.

The harness's 77 refused cases are these: every database case, among
them googleapis', the target of spec 0424 S16.

## Goals

- **G1.** The scoring YAML reproto writes, byte for byte.
- **G2.** The schema database, byte for byte: `DB.desc`, `hopcroft.rkyv`
  and `index.rkyv`. WKT's reproduces `prototext/wkt/prebuilt`.
- **G3.** The HTML of `--scoring-html-out` (S8).
- **G4.** The same bytes with `--jobs 1` and with all cores.
- **G5.** googleapis with `googleapisDb`'s flags timed against spec 0424
  S16: under 10 s and 1.29 GB on vCPUs 0–7.

## Non-goals

- **N1.** Warnings and progress as reproto prints them (W6 included),
  and the database's summary lines on stderr: step 9.
- **N2.** reproto's defects found on the way: recorded and reproduced,
  fixed after the port, as in step 5.
- **N3.** Changing `prototext-graph`'s algorithms or formats: the
  structures entry point (S6) feeds the same merge.

## Specification

### What the scoring graphs see (S1–S2)

- **S1. reproto's pool, continued.** reproto collects each file's
  scoring graph from `ctx.pool`, the lazy pool rendering loaded
  (`FindFileByName`, then `FindAllExtensions` on each message). The
  extensions of a message it records are the ones of the files loaded
  *by then*. The order:
  1. phase 7's renders (spec 0429 S4);
  2. with `--emit-scoring-yaml`, each summoned file looked up in turn;
  3. with `--schema-db-out`, the WKT dependencies' renders (S4), then
     each database file looked up in turn.

  `PoolLoads` (`render/options.rs`) is extended over these lookups, so
  that a file's visible extensions are reproto's at the same point. A
  file the pool refuses gets no graph (W6, N1). This is the render-order
  dependence of spec 0429 S4, kept (N2).
- **S2. Each field's scoring entry, from the input's definitions.**
  reproto reads the input pool's descriptors, not the rendered ones:
  - the kind: from the type; an enum closed by its resolved
    `features.enum_type` gives its range, an open one `int32`;
  - the label: required by resolved presence, repeated, optional;
  - `packed`: a repeated packable field packed by its resolved encoding
    (`is_packed`), in proto2, proto3 and editions;
  - `child`: the message or group type;
  - the message's kind: `GROUP` when some field of the file is a group
    of it; a MessageSet gets its synthesized `Item` group;
  - a pruned message or field is left out.

  protoback resolves the features with `features.rs` over the input's
  definitions, as Python's upb does.

### The scoring YAML (S3)

- **S3. PyYAML's output, reproduced.** `render/yaml.rs` writes the
  documents `_scoring_yaml_doc` builds: mappings in insertion order,
  block style, sequences of sequences as PyYAML nests them, integers and
  booleans plainly, strings plain unless PyYAML's resolver would read
  them back as something else (`true`, `null`, `1.0`, …) or its scalar
  analysis forbids it, then single-quoted. `allow_unicode=True`, width
  80. A table of PyYAML's own output over awkward names checks it, as
  `pyfmt` is checked (spec 0429 S8). One file per summoned file,
  `NAME.yaml` beside `NAME.proto`, names as reproto canonizes them,
  FQDNs not renamed.

### The schema database (S4–S7)

- **S4. The descriptors.** Each rendered file's descriptor, whether or
  not it is written (`--dry-run`, schema database without `-O`,
  `--no-emit-descriptor`), in phase 7's order, then the WKT
  dependencies phase 6 set aside (`schema_db_extra_nodes`), each
  rendered with its subtree promoted to summoned and reachable. protoback
  keeps `Shared` read-only: the promoted flags are an overlay the
  extras' render reads. A render failure leaves a file out silently, as
  reproto's `except` does.
- **S5. `DB.desc`.** Two descriptors canonized to one name with
  different bytes stop the run with reproto's error and exit 1; a
  duplicate otherwise is dropped. Then reproto's Kahn ordering: rounds
  over the remaining files, each placing those whose dependencies in the
  set are placed, in order; the leftovers appended. Serialized by upb,
  as `SerializeToString`.
- **S6. `hopcroft.rkyv`, without YAML (spec 0424 S20).**
  `prototext-graph` gains a public structure for one file's scoring
  graph, `ScoringFile` (extension-ranges marker, entries, messages in
  order), and `build_from_files(&[ScoringFile], …)`; its YAML loader
  becomes one producer of them, so both reach the same merge and the
  same bytes. protoback builds the database's graphs as structures,
  names renamed by the variant's namespace rules (`_canonical_scoring_name`),
  one per database file in `DB.desc`'s collection order, and builds
  them in parallel (G4: the merge takes them in order). An empty set is
  reproto's W6 and no database.
- **S7. `index.rkyv`.** Each file's span in `DB.desc`'s bytes, its
  types, dependencies and extensions, as `build_index.py` computes
  them, serialized by `prototext_graph::fds_index`, called directly.

### The HTML (S8)

- **S8. pyvis's page, byte for byte.** reproto writes it with pyvis
  (`cdn_resources='in_line'`), which inlines vis-network 9.1.2's
  JavaScript and CSS and pyvis's `utils.js` into its Jinja template,
  then replaces the tooltip's first `white-space:nowrap`. protoback
  writes the template's output for reproto's settings, the nodes, edges
  and options formatted as pyvis's `json.dumps` formats them (spec 0429
  S8's `json_dumps`), with the three assets embedded (`include_str!`).
  They are copied from pyvis 0.3.2 into the repository, annotated in
  `REUSE.toml` with their upstream licenses (vis-network Apache-2.0 or
  MIT, pyvis BSD-3-Clause).
  - The cost: about 690 KB more in the binary, on disk and in the
    closure. A run that writes no HTML never reads those pages, so
    neither its memory nor its startup changes.

### Threads (S9)

- **S9.** The scoring graphs are collected per file on the render pool,
  each from the state `Shared` holds plus the pool loads of S1, decided
  before; files are written, and graphs handed to the merge, in
  reproto's order.

## Alternatives considered

### The scoring graphs through YAML, as reproto passes them

`build_from_strings` exists and would need no change to
`prototext-graph`. Ruled out by spec 0424 S20: on googleapis reproto
spends 40.8 s in `yaml.dump`, and a Rust emitter plus serde's parser
would still be the costliest step after minimization. The YAML stays for
`--emit-scoring-yaml`, where it is the output.

### The pool as it stands after rendering, for every scoring graph

Simpler than S1, and arguably what reproto means. It changes which
extensions a message's graph records whenever a later lookup loads a
file extending it, so the database would differ from reproto's: ruled
out while reproto is the reference (N2).

### The HTML from the CDN, or through Python

A page loading vis-network from cdnjs keeps the binary small, but it
needs a network to view and can only be compared by its nodes and edges.
Running pyvis through a Python adapter, as plugins run (spec 0424 S9),
is byte-identical without copied assets, but needs Python whenever the
flag is given. Both were ruled out for S8's embedding, whose only cost
is the binary's size.

## Test plan

1. The harness, `--compare out` on every corpus, schema database cases
   included: `.yaml`, `.desc`, `hopcroft.rkyv`, `index.rkyv` and the
   HTML, with `--jobs 1` and all cores.
2. `wkt_database_reproduces_the_prebuilt`: protoback's WKT database
   against `prototext/wkt/prebuilt`.
3. `yaml_scalars_quoted_as_pyyaml_quotes_them`: the emitter against
   PyYAML's output on a table of names.
4. `prototext-graph`: `build_from_files` and `build_from_strings` give
   the same bytes on every fixture.
5. The order dependence of S1: a harness case where a file looked up
   after a message's graph extends it.

## Measured outcome

- **Stage 1, the scoring YAML** (2026-11-16): `scoring.rs` collects each
  file's graph from the upb pool `pool_loads` fills, which it now keeps:
  kinds, labels, packing and closed enums are upb's, as Python's upb
  resolves them, with no feature resolved by hand; an extension is seen
  when its file's rank is at most the summoned files' (after rendering)
  or the file's own (`--dry-run` without `--emit-binary`).
  `scoring/pyyaml.rs` writes PyYAML's block style; checked against a
  table PyYAML generated (`scoring/pyyaml/cases.rs`), which found that a
  simple key is under 128 characters *with its tag*: PyYAML counts
  `!!str` (5) even implicit, so a name of 123 characters or more is
  written `? name` / `: value`.
  - `prototext-graph`: `ScoringFile` and `build_from_files` (S6); the
    YAML loader parses into `ScoringFile`, its messages in the document's
    order (`serde_yaml::Mapping`), then `merge_files`, the merge both
    take. Its 130 tests, the `.rkyv` byte-identity ones included, pass.
  - The harness's `scoring-yaml` cases: every fixture, also with
    extension ranges, and under `--dry-run`. `--compare out/proto`, every
    corpus, on all cores and with `--jobs 1`: 1,274 cases, 1,197
    identical; the 77 others are the schema database's (stage 2),
    refused.
- **Stage 2, the schema database** (2026-11-16): `schema_db.rs`. Phase 7
  collects each rendered file's descriptor, written or not; the WKT
  dependencies render after it, one by one, over a copy of the graph
  their subtrees are promoted in, each seeing the promotions before it,
  loaded into the emulated pool after the summoned files; then the
  scoring graphs (`build_from_files`), the name collision check, Kahn's
  rounds, `DB.desc`, `index.rkyv` (`prototext_graph::fds_index`, called
  directly). `Shared` borrows the pool loads and the options pool, so a
  copy of it renders over the promoted graph.
  - WKT's database identical to reproto's, and its `hopcroft.rkyv` and
    `index.rkyv` to `prototext/wkt/prebuilt` (G2). Test plan 2 holds by
    the harness's `wkt-rkyv` case (protoback against reproto) and
    `wkt-prebuilt-check` (reproto against the prebuilt), both in `ci`.
  - The harness, `--compare out` (every output, the database's
    included), every corpus, on all cores and with `--jobs 1`: 1,274
    cases, all identical; googleapis' database among them.
  - `ci`'s `protobackHarness` now runs protoback against reproto, every
    output compared; it had kept reproto on both sides since step 4.
