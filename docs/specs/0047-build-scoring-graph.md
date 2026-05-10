<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0047 — prototext build-scoring-graph subcommand

**Status:** draft
**Implemented in:** —
**App:** prototext

---

## Background

Spec 0045 adds `reproto --emit-scoring-graph`, which writes one YAML file per
summoned FileDescriptorProto.  Each YAML describes the message types in that
file as a list of `(field_number, ScoringKind[, child_fqdn])` triples, plus
an `entries` list naming the top-level (non-nested) message types that serve
as candidate entry points for the scoring walk.

The next step in the pipeline (spec 0043 Stages 3–4) is to:

1. Merge all per-file YAMLs into a single directed scoring graph.
2. Run Hopcroft minimization to deduplicate structurally equivalent states.
3. Serialize the result to a compact binary file (`CompiledGraph`) that can be
   memory-mapped at scoring time.

This spec adds `prototext build-scoring-graph` as a subcommand of the existing
`prototext` binary to implement that three-step pipeline.  The existing
`prototext -d / -e` interface is unchanged.

---

## Goals

1. Add a `build-scoring-graph` subcommand to `prototext` (clap `Subcommand`
   alongside the existing flat `-d`/`-e` flags via `Option<Subcommand>`).
2. Accept one or more YAML files (or a directory glob) produced by
   `reproto --emit-scoring-graph` as input.
3. Merge them into a single scoring graph, run Hopcroft minimization, and
   write a `CompiledGraph` binary to a caller-specified output path.
4. Preserve `entries` (the per-file lists of top-level message FQDNs from the
   YAML) as **root entries** in the `CompiledGraph`, so the scoring walk knows
   which states to try as starting points for each original schema.
5. Emit a human-readable summary to stderr: number of YAML files read, message
   types before and after deduplication, transition count, output file size.

## Non-goals

- The multi-schema parallel scoring walk (spec 0042 Future work).
- Incremental graph updates.
- A separate `schema-db` crate (spec 0043 originally proposed this; this spec
  supersedes that decision by placing the logic in `prototext` instead).
- HTML visualisation of the compiled graph.
- Python bindings (deferred).

---

## Specification

### §1 — CLI

```
prototext build-scoring-graph [OPTIONS] <YAML_FILES>...

Arguments:
  <YAML_FILES>...
        One or more YAML files produced by reproto --emit-scoring-graph,
        or glob patterns.  Directories are walked recursively for *.yaml
        files (same glob machinery as the existing prototext path handling).

Options:
  -o, --output <PATH>      Output path for the CompiledGraph binary [required]
  -I, --input-root <DIR>   Resolve YAML paths relative to DIR
  -q, --quiet              Suppress the summary written to stderr
```

The subcommand is wired into `lib.rs` via:

```rust
#[derive(Debug, Parser)]
#[command(name = "prototext", ...)]
pub struct Cli {
    // existing flat flags unchanged
    #[arg(short = 'd', ...)] pub decode: bool,
    #[arg(short = 'e', ...)] pub encode: bool,
    // ...

    #[command(subcommand)]
    pub subcommand: Option<Subcommand>,
}

#[derive(Debug, Subcommand)]
pub enum Subcommand {
    /// Merge scoring-graph YAMLs and write a Hopcroft-deduplicated CompiledGraph.
    BuildScoringGraph(BuildScoringGraphArgs),
}

#[derive(Debug, Args)]
pub struct BuildScoringGraphArgs {
    #[arg(short = 'o', long = "output", value_name = "PATH", required = true)]
    pub output: PathBuf,

    #[arg(short = 'I', long = "input-root", value_name = "DIR")]
    pub input_root: Option<PathBuf>,

    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,

    #[arg(value_name = "YAML_FILES", required = true)]
    pub yaml_files: Vec<String>,
}
```

`run.rs` dispatches on `cli.subcommand` before the existing `-d`/`-e` logic.
If neither a subcommand nor `-d`/`-e` is supplied, the existing error path is
unchanged.

### §2 — YAML input format

The YAML produced by spec 0045 has the following shape (reproduced for
reference):

```yaml
entries:
  - google.rpc.Status
messages:
  google.rpc.Status:
    fields:
      - number: 1
        kind: VARINT
      - number: 3
        kind: LEN_MSG
        child: google.protobuf.Any
        required: true
```

- `entries`: sorted list of fully-qualified proto names (no leading dot) of
  top-level message types defined in this file.  These become root entries in
  the CompiledGraph.
- `messages`: map from FQDN to field list.  Keys include both top-level and
  nested message types from this file.  Cross-file child FQDNs appear only as
  `child` values; their definitions are in their own YAML file.
- `kind`: one of `VARINT`, `ENUM`, `I64`, `LEN_STRING`, `LEN_BYTES`,
  `LEN_MSG`, `LEN_PACKED`, `GROUP`, `I32` (spec 0045 §3).
- `child`: present iff `kind` is `LEN_MSG` or `GROUP`; the FQDN of the child
  message type.
- `enum_min`, `enum_max`: present iff `kind` is `ENUM`; the min/max integer
  values of the enum type.  An observed varint outside this range is a veto.
- `required`: present and `true` iff the field has `LABEL_REQUIRED` (proto2
  only); absent otherwise.

### §3 — Graph construction

**Step 1 — Load and merge.**  Parse all YAML files.  Build a map
`fqdn → ScoringState` (a `Vec<ScoringField>` sorted by `field_number`).  If
the same FQDN appears in multiple YAML files (shared well-known types such as
`google.protobuf.Any` are commonly referenced from many files), the
definitions must be identical; emit a warning and use the first occurrence if
they differ.

Collect the union of all `entries` lists into a flat list of
`(fqdn, source_yaml_path)` pairs — these become the root entries.

**Step 2 — Assign node IDs.**  Assign a dense `NodeId: u32` to each unique
FQDN.  Fixed leaf nodes occupy IDs just above the message node range:

| Index (relative) | LeafKind |
|---|---|
| 0 | Varint |
| 1 | Fixed64 |
| 2 | LenDel (bytes + packed) |
| 3 | String |
| 4 | Fixed32 |
| 5 + i | Enum, with range at `enum_ranges[i]` |

ENUM fields are interned: the first occurrence of a distinct `(min, max)` pair
allocates a new leaf node; subsequent occurrences reuse the same NodeId.
LEN_STRING maps to the `String` leaf; LEN_BYTES and LEN_PACKED both map to
the `LenDel` leaf.

Fields with kind `LEN_MSG` or `GROUP` point to their child message node (not
a leaf); `is_message` is set to `true` for both, recording that recursion is
possible.  All other kinds point to the appropriate leaf node.

> **Deferred — GROUP recursion**: the first implementation of the scoring walk
> will not recurse into GROUP fields even when `is_message = true` and
> `wire_tag` has wire type 3.  The child pointer and `is_message` flag are
> stored so the format does not need to change when GROUP recursion is added.

**Step 3 — Build the raw graph.**  For each `ScoringState`:

```
for each ScoringField(number, kind, child):
    wire_tag   = encode_wire_tag(number, kind)
    is_message = kind in {LEN_MSG, GROUP}
    target     = if is_message { node_id(child) }
                 else          { leaf_node(wire_type_of(kind)) }
    edges.push((node_id(fqdn), wire_tag, target, is_message))
```

`encode_wire_tag(number, kind)` packs `(number << 3) | wire_type_of(kind)`
into a `u32`.

Wire type per ScoringKind:

| ScoringKind | Wire type | Leaf | Notes |
|---|---|---|---|
| VARINT | 0 | Varint | |
| ENUM | 0 | Enum(min,max) | out-of-range varint is a veto |
| I64 | 1 | Fixed64 | |
| LEN_STRING | 2 | String | |
| LEN_BYTES | 2 | LenDel | |
| LEN_PACKED | 2 | LenDel | packed blob; element type not checked at graph level |
| LEN_MSG | 2 | — | `is_message = true`; recurse into child state |
| GROUP | 3 | — | `is_message = true`; child pointer stored; recursion deferred |
| I32 | 5 | Fixed32 | |

`LEN_BYTES` and `LEN_PACKED` share the `LenDel` leaf — both are opaque byte
blobs from the scorer's perspective.  `LEN_STRING` gets its own `String` leaf
so the scorer can distinguish string fields from byte/packed fields.  ENUM
fields each get a distinct leaf per `(min, max)` pair; an observed varint
outside `[min, max]` is a **veto** with the same weight as a wire-type
mismatch.

**Step 4 — `required` fields.**  The `required: true` annotation on a field
is stored in the `ScoringField` but does not affect graph edges.  It is
retained in the `CompiledGraph` as a per-field flag alongside `is_message`,
for use by future scoring heuristics (e.g. penalizing schemas where a
required field is absent from the wire data).

### §4 — Hopcroft minimization

Run the Hopcroft DFA minimization algorithm on the raw graph as described in
`docs/schema-match-impl-notes.md` §3:

1. **Initial partition**: group nodes by their outgoing-edge signature (sorted
   list of wire tags).  The five leaf nodes are pre-placed in five singleton
   blocks.
2. **Refinement**: standard Hopcroft worklist loop.  A reverse adjacency index
   `(BlockId, WireTag) → Vec<NodeId>` is built once and updated on splits.
3. **Output**: a `Partition` mapping each `NodeId` to a `StateId: u32`.

### §5 — CompiledGraph binary format

The output file extends the format defined in
`docs/schema-match-impl-notes.md` §2.3 with `required` and `entries` support.

#### File layout

```
Offset  Size  Content
0       8     Magic: b"PTSGRAPH"
8       4     Format version: u32 little-endian (current: 1)
12      4     Reserved: 0x00000000
16      8     Byte offset to the rkyv root (from start of file): u64 LE
24      ...   rkyv-serialized ArchivedCompiledGraph
```

#### Rust types

```rust
/// One entry in the flat transition table.
/// Sorted by (state_id, wire_tag) for binary search.
#[derive(Archive, Serialize, Deserialize)]
struct TransitionEntry {
    state_id:       u32,
    wire_tag:       u32,   // (field_number << 3) | wire_type
    child_state_id: u32,
    /// True for LEN_MSG and GROUP fields: child_state_id is a message node
    /// and recursion is possible.  The first implementation recurses only for
    /// LEN_MSG; GROUP recursion is deferred.
    is_message:     bool,
    /// True for proto2 required fields (LABEL_REQUIRED).  Stored for future
    /// use by scoring heuristics; not acted upon in the first implementation.
    is_required:    bool,
    _pad:           [u8; 2],
}

/// One root entry: a FQDN that is an entry point into the graph.
/// Multiple root entries may share the same state_id after deduplication.
#[derive(Archive, Serialize, Deserialize)]
struct RootEntry {
    /// Fully-qualified proto name of the original top-level message type,
    /// e.g. "google.rpc.Status".  Stored as a length-prefixed string.
    fqdn:     String,
    state_id: u32,
}

/// Identifies the kind of a leaf state.
/// kind 0..=4 are fixed scalars; kind >= 5 are ENUM leaves.
/// For kind >= 5: enum_ranges[kind - 5] gives (min, max).
///
///   0 = Varint
///   1 = Fixed64
///   2 = LenDel  (bytes + packed)
///   3 = String
///   4 = Fixed32
///   5+ = Enum, range at enum_ranges[kind - 5]
#[derive(Archive, Serialize, Deserialize)]
struct LeafEntry {
    state_id: u32,
    kind:     u32,
}

/// The compiled, Hopcroft-deduplicated scoring graph.
#[derive(Archive, Serialize, Deserialize)]
struct CompiledGraph {
    /// Flat sorted array of all transitions.
    transitions:  Vec<TransitionEntry>,
    /// One entry per root (entry point) across all input YAML files.
    /// Multiple entries may share the same state_id.
    roots:        Vec<RootEntry>,
    /// One entry per leaf state (fixed scalars + dynamic ENUM leaves).
    leaves:       Vec<LeafEntry>,
    /// ENUM value ranges, indexed by (LeafEntry.kind - 5).
    /// An observed varint outside [min, max] is a veto.
    enum_ranges:  Vec<(i32, i32)>,
    /// Total number of states (= number of blocks in final Hopcroft partition).
    num_states:   u32,
}
```

No `static_back_ref` CSR is included in this version (deferred until the
multi-schema walk is implemented and the back-reference lookup pattern is
validated empirically).

#### Serialization

Use `rkyv 0.8` to serialize `CompiledGraph`.  The `rkyv` root offset at
bytes 16–23 allows the loader to skip the fixed header without parsing it.

#### Loading (zero-copy)

```rust
fn load_compiled_graph(path: &Path) -> Result<LoadedGraph> {
    let file  = File::open(path)?;
    let mmap  = unsafe { Mmap::map(&file)? };
    // Verify magic + version
    assert_eq!(&mmap[0..8], b"PTSGRAPH");
    assert_eq!(u32::from_le_bytes(mmap[8..12].try_into()?), 1);
    let root_offset = u64::from_le_bytes(mmap[16..24].try_into()?) as usize;
    let graph = unsafe {
        rkyv::access_unchecked::<ArchivedCompiledGraph>(&mmap[root_offset..])
    };
    Ok(LoadedGraph { _mmap: mmap, graph })
}
```

### §6 — Summary output

Unless `--quiet` is given, write to stderr:

```
prototext build-scoring-graph: loaded 4217 YAML files
  message types (before dedup): 183 421
  states (after Hopcroft):        12 847  (dedup ratio: 0.070)
  transitions:                   541 209
  root entries:                    9 103
  output: /path/to/graph.bin (14.2 MiB)
```

### §7 — New dependencies in prototext/Cargo.toml

| Crate | Version | Use |
|---|---|---|
| `serde` | 1 | YAML deserialization (derive) |
| `serde_yaml` | 0.9 | parse YAML input |
| `rkyv` | 0.8 | serialize CompiledGraph |
| `memmap2` | 0.9 | zero-copy load (used at scoring time, not build time) |

`petgraph` is not added: the raw graph is represented as a plain
`Vec<(NodeId, WireTag, NodeId, bool)>` edge list for construction, then
converted directly to the flat `TransitionEntry` array after Hopcroft.  The
`Partition` data structure is implemented from scratch (~150 lines) as
described in `docs/schema-match-impl-notes.md` §2.2.

### §8 — Source layout

New files within the `prototext` crate:

```
prototext/src/
  build_scoring_graph/
    mod.rs       — entry point: parse args, orchestrate steps, write output
    load.rs      — YAML loading and merging
    graph.rs     — node/edge construction, wire_tag encoding
    hopcroft.rs  — Hopcroft partition refinement
    serial.rs    — CompiledGraph serialization / file writing
```

`lib.rs`: add `pub mod build_scoring_graph;`, extend `Subcommand` enum.
`run.rs`: dispatch to `build_scoring_graph::run(args)`.

### §9 — Tests

Unit tests alongside each module:

- **load**: round-trip a hand-written YAML; verify `fqdn → ScoringState` map
  and `entries` list.
- **graph**: for a three-node graph (A → B → leaf), verify wire tags and edge
  list.
- **hopcroft**: two structurally identical message types in different YAMLs
  must merge to the same `StateId`.  A single well-known-type check:
  `google.protobuf.Timestamp` appearing in two different YAMLs must map to one
  state.
- **serial**: write a small `CompiledGraph`, read it back zero-copy, assert
  `num_states`, `roots`, and a sample transition.

Integration test (in `prototext/tests/`): run `build-scoring-graph` on the
fixtures produced by the `test_emit_graph.py` tests (compiled with `protoc`
and run through `reproto --emit-scoring-graph`); assert the output file has
the correct magic, loads without error, and the `RootEntry` for
`test.field.PrimitiveTypes` is present.

---

## Relation to spec 0043

Spec 0043 proposed a standalone `schema-db` crate implementing the same
pipeline from `.pb` files via a `protoc` subprocess.  This spec supersedes
the `schema-db` crate decision and Stages 1–3 of that pipeline: the
`build-scoring-graph` subcommand takes the YAML files produced by spec 0045
directly, skipping the `protoc` compilation step.  The Hopcroft algorithm
(spec 0043 Sub-step B) and the `CompiledGraph` format (spec 0043 Sub-step C,
`docs/schema-match-impl-notes.md` §2.3) are retained and refined here.

Spec 0043 remains the reference for the deduplication algorithm and the
mmap-friendly storage rationale.
