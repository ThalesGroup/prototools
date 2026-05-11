<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# build-scoring-graph: implementation notes

This document is a guided tour of the `score-graph build-scoring-graph`
command.  It is intended as a standalone introduction for someone reading
the source for the first time, with emphasis on non-obvious algorithmic
choices and the rationale for data-structure decisions.

---

## 1. What the command does

`score-graph build-scoring-graph -o graph.bin <yaml-dir>` takes a
collection of per-file scoring-graph YAML descriptions (produced by
`reproto --emit-scoring-graphs`) and produces a single compiled binary
graph that the `score` and `match` commands consume at runtime.

The pipeline is:

```
YAML files  →  load & merge  →  raw graph  →  Hopcroft minimize  →  compile  →  serialize
```

Each stage is a pure function of its inputs; no global mutable state is
shared between stages.

---

## 2. YAML input format

Each YAML file describes one proto source file.  It has two top-level keys:

- **`entries`** — a list of fully-qualified message type names that are
  designated root entry points (i.e. top-level message types for that
  file).

- **`messages`** — a map from FQDN to a list of fields.  Each field has:
  - `number` — protobuf field number.
  - `kind` — one of `VARINT`, `I64`, `LEN_STRING`, `LEN_BYTES`,
    `LEN_MSG`, `LEN_PACKED`, `GROUP`, `I32`, `ENUM`.
  - `child` — FQDN of the child message type (for `LEN_MSG` and `GROUP`).
  - `enum_min`, `enum_max` — valid value range (for `ENUM`).
  - `label` — `optional` (default), `required`, or `repeated`.

Multiple YAML files may define the same message type (it is shared across
proto files).  On conflict the first definition wins and a warning is
emitted.

The `kind` vocabulary deliberately collapses the full protobuf type system
down to what is observable on the wire:

| Kind | Wire type | Notes |
|---|---|---|
| `VARINT` | 0 | plain integer, no range constraint |
| `ENUM` | 0 | varint with a declared [min, max] range |
| `I64` | 1 | fixed 8-byte little-endian |
| `LEN_STRING` | 2 | length-delimited, UTF-8 check |
| `LEN_BYTES` | 2 | length-delimited, no check |
| `LEN_MSG` | 2 | length-delimited sub-message |
| `LEN_PACKED` | 2 | packed repeated scalars |
| `GROUP` | 3 | start/end group pair |
| `I32` | 5 | fixed 4-byte little-endian |

---

## 3. Load and merge

`load::load_and_merge` iterates over all YAML paths, deserializes each
with `serde_yaml`, and accumulates two collections:

- `states: HashMap<FQDN, Vec<ScoringField>>` — the union of all message
  type definitions.
- `roots: Vec<FQDN>` — all designated entry points, in encounter order,
  deduplicated.

Fields are sorted by field number after parsing (the YAML spec guarantees
this order, but the code is defensive).

The result is a `Merged` struct that is the sole input to the raw graph
builder.

---

## 4. Raw graph construction

`graph::build` converts `Merged` into a `RawGraph`:

```
node_ids: HashMap<FQDN, u32>   // message nodes → dense integer IDs 0..msg_count
edges:    Vec<RawEdge>          // (src, field_number, dst, label)
num_nodes: u32                  // msg_count + num_leaves
```

### 4.1 Message nodes

Every FQDN that appears either as a key in `merged.states` or as a `child`
reference from any field is assigned a dense integer node ID in 0..msg_count.
Two passes are needed: one over the keys, one over the child references,
to catch message types that are referenced but not defined (e.g. imported
types whose definitions live in another YAML file that was not loaded).

### 4.2 Leaf nodes

Scalar field kinds map to shared sentinel node IDs near `u32::MAX`:

```
LEAF_VARINT  = u32::MAX - 4   (wire type 0, non-enum)
LEAF_I64     = u32::MAX - 3   (wire type 1)
LEAF_LEN     = u32::MAX - 2   (wire type 2, non-string)
LEAF_STRING  = u32::MAX - 1   (wire type 2, is_string)
LEAF_I32     = u32::MAX       (wire type 5)
ENUM leaf i  = u32::MAX - 5 - i
```

Enum fields are special: each distinct `(enum_min, enum_max)` pair gets
its own dynamically allocated leaf sentinel, because two enum fields with
different value ranges are not interchangeable during scoring even though
they both arrive on wire type 0.  A `LeafRegistry` deduplicates these:
the first time a given range pair is seen a new sentinel is allocated;
subsequent occurrences reuse the same one.

Placing leaf sentinels far from zero avoids collisions with message node
IDs and allows the Hopcroft code to tell them apart from message nodes by
a simple threshold comparison.

### 4.3 Edges

One `RawEdge` is emitted per field per message type:

```rust
RawEdge { src: node_ids[fqdn], field_number, dst: leaf_or_msg_node, label }
```

`label` is encoded as `0 = optional, 1 = required, 2 = repeated`.

---

## 5. Hopcroft minimization

### 5.1 Why minimize?

The raw graph has one node per distinct message type FQDN.  Many of these
types are structurally identical after abstracting away their names — for
example, two messages that both have a single optional `LEN_MSG` child at
field 3 are indistinguishable on the wire.  Merging them into a single
state:

- reduces the number of states the walker must maintain at runtime,
- allows the scoring walk to process multiple candidate message types in
  a single traversal step when they share a state (the key performance
  property of the multi-entry walk).

The correct notion of structural equivalence here is **bisimulation** on
the labeled transition system formed by the raw graph.  Two nodes are
bisimilar if and only if they have identical outgoing transition signatures
(same set of `(field_number, label, child_block)` triples, where
`child_block` is the bisimulation equivalence class of the child).
Hopcroft's algorithm computes the coarsest such partition — the one with
the fewest equivalence classes — in O(|Σ| · |E| · log |V|) time, where
|Σ| is the number of distinct (field_number, label) pairs, |E| the total
edge count, and |V| the node count.

### 5.2 Academic reference

The algorithm is due to:

> John Hopcroft, "An n log n algorithm for minimizing states in a finite
> automaton", in *Theory of Machines and Computations*, Academic Press,
> 1971, pp. 189–196.

A widely-cited modern exposition is:

> Jean-Claude Fernandez, "An implementation of an efficient algorithm for
> bisimulation equivalence", *Science of Computer Programming* 13(2-3),
> 1989, pp. 219–236.
> https://doi.org/10.1016/0167-6423(89)90010-0

A clear pedagogical treatment appears in:

> Antti Valmari and Petri Lehtinen, "Efficient minimization of DFAs with
> partial transition functions", *STACS 2008*, pp. 645–656.
> https://doi.org/10.4230/LIPIcs.STACS.2008.1328

For the connection between DFA minimization and partition refinement more
broadly, see:

> Robert Paige and Robert Tarjan, "Three partition refinement algorithms",
> *SIAM Journal on Computing* 16(6), 1987, pp. 973–989.
> https://doi.org/10.1137/0216062

### 5.3 Why Hopcroft specifically?

The raw scoring graph is a deterministic labeled transition system: from
any given node, each `(field_number, label)` pair leads to at most one
child.  This is exactly the structure of a DFA, so Hopcroft's DFA
minimization algorithm applies directly and gives the optimal
(coarsest-partition) result in worst-case O(|Σ| · |E| · log |V|) time.

Bisimulation algorithms for non-deterministic or weighted transition
systems (e.g. Paige-Tarjan in its general form) would also be correct but
are more complex and slower for this use case.

### 5.4 Implementation

The implementation in `hopcroft::minimize` proceeds in three phases.

#### Phase 1 — index mapping

Leaf sentinels (near `u32::MAX`) cannot be used as array indices directly.
A closure `node_index(node: u32) -> usize` maps each node — whether a
message node (already a small integer) or a leaf sentinel — to a
contiguous index in `0..n`, where `n = msg_count + num_leaves`.  The five
fixed leaves occupy `msg_count..msg_count+5`; dynamic enum leaves follow
immediately after.

#### Phase 2 — initial partition

Two kinds of nodes receive different initial groupings:

- **Message nodes** are grouped by their *outgoing signature*: the sorted
  set of `(field_number, label)` pairs on their outgoing edges.  Two
  message nodes with different outgoing signatures can never be bisimilar,
  so placing them in different initial blocks is correct.  Two nodes with
  the same signature might still be split later by the refinement loop if
  their children end up in different blocks.

- **Leaf nodes** are grouped by their `(wire_type, is_string,
  enum_range_idx)` triple.  Two leaves with the same triple are
  indistinguishable on the wire and can always be merged.

This choice of initial partition is important for correctness.  Using a
coarser initial partition (e.g. putting all message nodes in one block)
would require more refinement iterations to converge.  Using a finer
initial partition is always safe — it may just mean that some redundant
refinement steps fire — but the choice here is already the natural one.

#### Phase 3 — refinement loop

A worklist of `(block_id, field_number, label)` splitters is initialized
with every `(block, field_number, label)` triple from the initial
partition.  Each splitter `(B, f, l)` asks: "are there nodes that have an
edge `(f, l)` into block B, and nodes that do not, currently sharing a
block?  If so, split them."

For each splitter popped from the worklist:

1. Collect the set of *predecessors* of block B via edge label `(f, l)`:
   all nodes `s` such that there exists an edge `s --(f,l)--> t` with `t`
   in block B.  This uses a reverse adjacency list built once before the
   loop.

2. For each block C that has a non-empty intersection with the predecessor
   set and is not entirely contained in it, split C into `C_inside`
   (predecessors) and `C_outside` (non-predecessors).

3. Add new splitters for the smaller of the two halves (the Hopcroft
   optimization that achieves the O(log |V|) factor).

The label `l` is part of the splitter key — this is essential for
correctness.  Two nodes that differ only in the label (optional vs.
required) on one of their outgoing edges must not be merged: they produce
different cardinality penalties during scoring.  A version of this
algorithm that used only field numbers as splitters (ignoring labels)
would incorrectly merge nodes that are not truly bisimilar.

#### Phase 4 — renumbering

After refinement, the block IDs are arbitrary integers.  They are
renumbered to produce a compact, deterministic layout:

- Message/group blocks are assigned IDs `0..num_msg_blocks` in encounter
  order during a scan of `block_of`.
- Leaf blocks are assigned IDs `num_msg_blocks..num_msg_blocks+num_leaves`
  in a fixed order (VARINT, I64, LEN, STRING, I32, then enum leaves).

This layout ensures that leaf states are always at the high end of the ID
space, which is convenient for the scorer when distinguishing message nodes
from leaf nodes.

---

## 6. Compilation

`graph::compile` converts the `(RawGraph, Partition)` pair into a
`CompiledGraph` ready for serialization.

### 6.1 Transition table

For each raw edge `(src, field_number, dst, label)`:

- `src_block = partition.block_of(src)`
- `dst_block = partition.block_of(dst)` (using `block_of_sentinel` for
  leaf sentinels)

The pair `(src_block, field_number)` uniquely identifies the edge in the
compiled graph (because after Hopcroft, no two outgoing edges from the
same block have the same field number with the same label; and a given
field number cannot appear with different labels on the same state).  The
compiled entry is inserted with `.or_insert` (first-seen wins; subsequent
raw edges that map to the same block pair are redundant by bisimulation).

The resulting `Vec<TransitionEntry>` is sorted by `(state_id, field_number)`
to enable binary search during scoring.

### 6.2 Node table

Each state in the compiled graph needs to know:

- Its wire type (for wire-type matching at the parent level).
- Whether it is a string node (for UTF-8 checking).
- Its enum range index (for value range checking), or `0xFFFF` if not
  an enum.

For **message/group state blocks**, the wire type is derived from the field
kind that points *to* them (e.g. a `LEN_MSG` field points to a state with
`wire_type = 2`).  Root states that have no incoming edges default to
`wire_type = 2` (LEN, the standard top-level encoding).

For **leaf state blocks**, the attributes come directly from `leaf_attrs`,
which maps each sentinel back to `(wire_type, is_string, enum_range_idx)`.

The resulting `Vec<NodeEntry>` is sorted by `state_id` to enable binary
search during scoring.

### 6.3 Root table

One `RootEntry { fqdn, state_id }` is emitted per entry in
`merged.roots`, mapping the original FQDN to the block ID of its node in
the compiled partition.  Root entries preserve the original encounter order
from the YAML files.

---

## 7. Binary format and serialization

### 7.1 Rationale for `rkyv`

The binary format uses [`rkyv`](https://rkyv.org/) for serialization.
`rkyv` takes a different approach from formats like `bincode`, `postcard`,
or `protobuf`: the serialized bytes are a valid in-memory representation of
the data structure, so no deserialization step is required at load time.
The loader simply memory-maps the file and casts a pointer — the OS pages
in only what is actually accessed.

For `build-scoring-graph`, which is an offline tool, serialization
performance is not critical.  The benefit accrues entirely at runtime in
the `score`/`match` commands, where startup latency matters: a graph of
~180 KiB is available for use in microseconds regardless of how many
entries it contains.

The tradeoff is that `rkyv`'s archived types (`ArchivedVec<T>`,
`ArchivedString`, etc.) are distinct from their normal Rust equivalents and
require calling `.to_native()` on integer fields to handle endianness.
This is a small but real friction in the reading code.

### 7.2 File layout

```
Offset  Size  Content
──────  ────  ───────────────────────────────────────────────
0       8     Magic: b"PTSGRAPH"
8       4     Version: u32 little-endian (currently 2)
12      4     Reserved: 0x00000000
16      8     Root offset: u64 little-endian (currently 24)
24      *     rkyv-serialized ArchivedCompiledGraph
```

The 24-byte fixed header is entirely our own invention — `rkyv` has no
notion of a file header and would be content with a bare blob.  The header
serves two purposes: it identifies the file as a score-graph compiled graph
(magic + version), distinguishing it from any other rkyv-serialized data or
binary file; and it records where the `rkyv` root object starts (offset
field).  Currently the root always starts at offset 24, but the offset
field allows for future header extensions without breaking the format.

Future work: both `PTSGRAPH` and the `#@ prototext:` magic used by the
prototext format should be submitted to the `file(1)` magic database
(https://github.com/file/file) so that the Unix `file` command recognises
them correctly.

### 7.3 `CompiledGraph` structure

```
CompiledGraph
├── nodes:        Vec<NodeEntry>         sorted by state_id
├── transitions:  Vec<TransitionEntry>   sorted by (state_id, field_number)
├── roots:        Vec<RootEntry>
├── enum_ranges:  Vec<(i32, i32)>
└── num_states:   u32
```

**`NodeEntry`** (per state):

| Field | Type | Meaning |
|---|---|---|
| `state_id` | `u32` | The state's ID in the compiled partition |
| `wire_type` | `u8` | Protobuf wire type expected on incoming edges (0/1/2/3/5) |
| `is_string` | `bool` | True iff wire_type=2 and UTF-8 validation is required |
| `enum_range_idx` | `u16` | Index into `enum_ranges`, or `0xFFFF` if not an enum |

`wire_type` and `is_string` are needed at the parent level to perform
wire-type matching before descending into a field.  They describe *how
this state's payload arrives*, not what it contains.

`enum_range_idx` is stored in the node rather than the edge because the
range constraint belongs to the leaf's type, not to the field number that
leads to it.  `0xFFFF` as the sentinel for "not an enum" fits in a `u16`
and avoids a separate `Option` variant in the serialized form.

**`TransitionEntry`** (per edge):

| Field | Type | Meaning |
|---|---|---|
| `state_id` | `u32` | Source state |
| `field_number` | `u32` | Protobuf field number on this edge |
| `label` | `u8` | 0 = optional, 1 = required, 2 = repeated |
| `child_state_id` | `u32` | Destination state |

Sorting by `(state_id, field_number)` supports binary search in
`find_transition`.  `label` is stored in the edge (not the node) because
it is a property of the field declaration, not of the child type.

**`RootEntry`** (per candidate type):

| Field | Type | Meaning |
|---|---|---|
| `fqdn` | `String` | Fully-qualified type name |
| `state_id` | `u32` | Root state in the compiled partition |

**`enum_ranges`**: a flat `Vec<(i32, i32)>` indexed by `enum_range_idx`.
Storing enum ranges out-of-line keeps `NodeEntry` small (4 bytes of useful
payload + `state_id`) and avoids duplicating the same range for multiple
nodes that reference it.

**`num_states`**: redundant with the maximum `state_id` seen in the node
table, but stored explicitly so readers can allocate data structures of
the right size without scanning the table.

---

## 8. End-to-end data flow summary

```
load.rs           parse YAML → Merged { states: HashMap<FQDN, fields>, roots }
                       │
graph::build      assign dense node IDs; emit RawEdge list; register leaf sentinels
                       │
hopcroft::minimize compute bisimulation partition (Hopcroft); renumber blocks
                       │
graph::compile    project edges through partition; derive node attributes
                       │
serial::write     prepend 24-byte header; rkyv-serialize CompiledGraph → .bin
```

The binary file is then read by `score-graph score` and `score-graph match`
via a zero-copy `rkyv` access over a memory-mapped file, with no additional
parse or decode step.
