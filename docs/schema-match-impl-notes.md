<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Implementation Notes — Schema Matching in Rust

## 1. Library Survey

### 1.1 Protobuf schema loading and wire parsing

**`prost-reflect` 0.16.3** *(recommended)*
Loads a `FileDescriptorSet` at runtime via `DescriptorPool::decode()`, gives
access to `MessageDescriptor` objects (field names, types, numbers), and can
parse raw wire messages into a `DynamicMessage`.  The reflection API is clean
and actively maintained.

- Pro: runtime schema loading without codegen; preserves unknown fields.
- Pro: `DynamicMessage::decode()` handles all wire types including groups.
- Con: still requires a schema to parse a message — for the matching walk we
  do not want to parse *through* a schema, we want to walk the wire bytes
  while consulting schemas in parallel.  `prost-reflect` is the right tool
  for *loading* schemas and building the graph, not for the hot-path walk.

**Raw wire parsing (manual)** *(recommended for the hot path)*
The protobuf wire format is simple: read a varint tag, decode
`(field_number, wire_type)`, skip or recurse based on wire type.  This is
~100 lines of Rust and gives complete control over what happens at each
field.  The matching walk needs to observe every `(field_number, wire_type)`
pair and optionally recurse into length-delimited blobs; it does not need
proto-level type decoding.  A thin hand-written wire reader is the right
tool here.

- Pro: no schema dependency during the walk; full control over recursion
  decisions; minimal allocations.
- Con: needs to be written and tested carefully (varint overflow, malformed
  input, group end matching).

**`rust-protobuf` v3 (stepancheg)** — approaching end-of-life, avoid.
**`protobuf` v4 (Google official)** — beta, API unstable, watch for 2026.

---

### 1.2 Graph data structures

**`petgraph`** *(recommended)*
`StableGraph<N, E, Directed, u32>` stores nodes and edges with stable
integer indices even after removals.  With `N = MessageNodeData` and
`E = WireTag`, it directly models the schema graph.

- Pro: mature, well-tested, supports millions of nodes, `u32` indices are
  compact.
- Pro: integrates with `fixedbitset` (same maintainer) for partition
  refinement bookkeeping.
- Con: no built-in Hopcroft/bisimulation — must be implemented on top.
- Con: `StableGraph` has a small overhead vs. raw `Vec` adjacency lists; for
  a read-only compiled graph, a CSR (Compressed Sparse Row) layout would be
  faster (see §4).

**`indexmap` 2.13.0** *(useful)*
A hash map that also supports integer indexing.  Useful during construction
for mapping `(schema_id, message_type_name)` → `StateID` before the graph
is finalized.

- Pro: stable insertion-order iteration; easy to convert to a flat array
  once construction is done.
- Con: not needed after compilation — replaced by plain arrays at runtime.

---

### 1.3 Partition refinement

**`refinery` 0.1.1** *(viable starting point)*
Implements partition refinement over the set `[0, n)` with an O(n log n)
refinement loop and a callback on each split.

- Pro: the core algorithm is already there; just wire it to the graph.
- Con: early version (0.1.1), minimal documentation, not explicitly designed
  for DFA minimization; may need patching.

**Recommendation**: start with `refinery` for the partition data structure;
implement the Hopcroft-specific logic (initial partition by out-edge
signature, refinement loop over splitters) manually on top.  If `refinery`
proves awkward, the partition data structure is straightforward to write from
scratch (~150 lines).

No crate implements Hopcroft DFA minimization directly.

---

### 1.4 Serialization and fast loading

**`rkyv` 0.8.12** *(recommended for the compiled graph)*
Derives `Archive`, `Serialize`, `Deserialize` on your graph structs.  The
archived form is a flat byte buffer that can be memory-mapped and used
zero-copy — no parse pass on startup.

- Pro: truly zero-copy; cast a `&[u8]` to `&ArchivedGraph` and start using
  it.
- Pro: supports `HashMap`, `Vec`, slices, enums — enough for any graph
  representation we'd want.
- Con: pre-1.0 (API may change between minor versions); Rust-only format.
- Con: `rkyv`'s archived types are different from the normal types (e.g.
  `ArchivedVec<T>` rather than `Vec<T>`), requiring some care in the read
  path.

**`memmap2` 0.9.9** *(essential)*
Opens a file and maps it into the process address space.  The OS pages in
only the accessed regions.  Combined with `rkyv`, this gives instant startup
at any graph size.

- Pro: instant load regardless of file size; OS manages paging.
- Con: `unsafe`; file must not be externally modified while mapped.

**`bytemuck` 1.x** *(useful for simple flat arrays)*
For sections of the graph that are pure flat arrays of `#[repr(C)]` structs
(e.g. the transition table as a `(StateID, WireTag) -> ChildStateID` flat
array), `bytemuck::cast_slice` gives zero-copy typed access directly from
the mmap'd bytes.

---

### 1.5 Active-set representation

**`fixedbitset` 0.5.7** *(recommended)*
Dense bitset with SIMD acceleration (SSE2/AVX on x86, WASM SIMD).  If the
maximum state count after deduplication fits in memory (e.g. up to ~10M
states = 1.25 MB per bitset), this is the simplest and fastest option for
the active set at shallow depths.

- Pro: SIMD AND/OR, popcount; maintained by the petgraph team; integrates
  naturally.
- Con: fixed maximum size; wastes memory if the active set is very sparse.

**`roaring` 0.11.3** *(alternative for deep levels)*
Compressed sparse bitmaps.  Better memory efficiency when the active set is
small relative to the total state count (which is the case at depth ≥ 2).

A **hybrid** is worth considering: use a plain `Vec<StateID>` (sorted list)
at deep levels where only a few tens of states remain, and `fixedbitset` at
shallow levels.  The crossover point is measurable empirically.

---

## 2. Data Structures

### 2.1 Schema graph (pre-deduplication)

Built during the compilation phase from the FDP database.

```rust
/// Wire-level tag: field number + wire type packed as in the protobuf spec.
/// field_number in bits [31:3], wire_type in bits [2:0].
type WireTag = u32;

/// Node in the schema graph.  Each (schema, message_type) pair is one node.
struct SchemaNode {
    schema_id:    u32,
    /// Index into a global message-type string table (avoids allocation).
    msg_type_id:  u32,
}

/// Edge label.
type EdgeLabel = WireTag;

/// The raw schema graph, built from all FDPs.
/// NodeIndex = u32 (petgraph StableGraph).
type SchemaGraph = StableGraph<SchemaNode, EdgeLabel, Directed, u32>;
```

Leaf nodes (wire-type scalars with no outgoing edges) are represented as
nodes with no outgoing edges; five canonical leaf nodes (one per wire type)
are pre-inserted and shared.

### 2.2 Partition during Hopcroft refinement

```rust
/// One block in the current partition.  Stored as a contiguous range in a
/// permutation array for O(1) split.
struct Block {
    /// Index range into `perm` for nodes in this block.
    start: u32,
    end:   u32,
}

struct Partition {
    /// Permutation of node indices; nodes in the same block are contiguous.
    perm:       Vec<u32>,
    /// Inverse permutation: perm_inv[node] = position in perm.
    perm_inv:   Vec<u32>,
    /// Which block each node belongs to.
    block_of:   Vec<u32>,
    /// All blocks.
    blocks:     Vec<Block>,
}
```

Split operation: given a block B and a subset S ⊆ B to split off, swap
elements in `perm` so that S is contiguous, update `perm_inv` and
`block_of`, create a new `Block` for S.  O(|S|) time.

### 2.3 Compiled graph (post-deduplication)

After Hopcroft, the quotient graph is serialized into a flat, cache-friendly
layout.

```rust
/// One entry in the transition table.
/// Stored as a flat array sorted by (state_id, wire_tag) for binary search.
#[repr(C)]
struct TransitionEntry {
    state_id:       u32,
    wire_tag:       u32,
    child_state_id: u32,
    /// True if this field is a message type (triggers recursion).
    is_message:     bool,
    _pad:           [u8; 3],
}

/// Static back-reference: one (schema_id, msg_type_id) pair per StateID.
/// Records which (schema, message_type) nodes were merged into this StateID
/// by Hopcroft — a compile-time annotation.  Size is 1 for most states.
/// Used only to initialise the root active set; runtime scoring uses the
/// per-entry ActiveEntry::back_refs lists, not these static annotations.
///
/// Stored in a separately indexed CSR structure:
///   back_ref_data[back_ref_start[s]..back_ref_start[s+1]]
/// gives all static back-refs for state s.
#[repr(C)]
struct StaticBackRef {
    schema_id:   u32,
    msg_type_id: u32,
}

/// Root state for each schema (one entry per schema_id).
#[repr(C)]
struct RootEntry {
    schema_id: u32,
    state_id:  u32,
}

/// The full compiled graph, laid out for mmap + bytemuck zero-copy access.
/// Serialized via rkyv; accessed as ArchivedCompiledGraph after mmap.
struct CompiledGraph {
    /// Flat sorted array of all transitions.
    transitions:            Vec<TransitionEntry>,
    /// CSR offsets for static back-references (len = num_states + 1).
    static_back_ref_start:  Vec<u32>,
    /// Static back-reference data (compile-time; used only for root initialisation).
    static_back_ref_data:   Vec<StaticBackRef>,
    /// Root state per schema.
    roots:                  Vec<RootEntry>,
    /// Total number of states.
    num_states:             u32,
}
```

Transition lookup at runtime: binary search in `transitions` on
`(state_id, wire_tag)` — O(log F) where F is the max field count per
message (~few hundred).  Alternatively, if memory allows, a 2D array
`transition_table[state_id][wire_tag]` allows O(1) lookup but is sparse and
large.

### 2.4 Per-message walk state

```rust
/// Accumulated counters for one message, reset between calls.
/// Vetoed schemas are tracked separately (not in these arrays).
struct MatchCounters {
    matches:  Vec<i64>,   // indexed by schema_id; field declared + wire type compatible
    unknowns: Vec<i64>,   // indexed by schema_id; field not declared by schema
    vetoed:   BitVec,     // indexed by schema_id; proto-level error → definitive elimination
}

/// One entry in the active set: a StateID and the list of initial schema IDs
/// currently routing through it at this nesting level.
/// The back-reference list starts at size 1 per schema and shrinks as schemas are vetoed.
struct ActiveEntry {
    state_id: u32,
    back_refs: SmallVec<[u32; 4]>,   // schema_ids; SmallVec avoids heap alloc for the common case
}

/// Active state list for one recursion level.
type ActiveSet = Vec<ActiveEntry>;
```

The walk is a recursive function taking `&[u8]` (current wire buffer) and
`&ActiveSet` (current active states), and writing into `&mut MatchCounters`.
Stack frames are implicit in the call stack; no explicit stack data structure
needed.

---

## 3. Graph Deduplication — Pseudo-code

```
fn build_graph(fdp_set) -> SchemaGraph:
    for each fdp in fdp_set:
        for each message_type in fdp:
            add node (schema_id, message_type)
            for each field in message_type:
                tag = encode_wire_tag(field.number, field.wire_type)
                if field.type == MESSAGE:
                    add edge (node, child_message_node, tag)
                else:
                    add edge (node, canonical_leaf_node(field.wire_type), tag)

fn hopcroft_minimize(graph: SchemaGraph) -> Partition:
    // Step 1: Initial partition
    // Two nodes are initially in the same block iff they have the
    // same set of outgoing wire tags (their "signature").
    signatures = for each node: sorted list of outgoing wire tags
    partition = Partition::new(num_nodes)
    partition.split_by(|node| signatures[node])   // group by signature

    // Step 2: Refinement loop
    // Maintain a worklist of (block, tag) splitters to process.
    worklist = all (block, tag) pairs from initial partition

    while worklist not empty:
        (B, tag) = worklist.pop()

        // Find all nodes that have an incoming edge with this tag
        // whose target is in block B.
        predecessors_of_B_via_tag = {
            node | exists edge (node --[tag]--> target) with target in B
        }

        // For each block C that is split by predecessors_of_B_via_tag:
        // i.e. C ∩ predecessors ≠ ∅ and C \ predecessors ≠ ∅
        for each block C intersected by predecessors_of_B_via_tag:
            (C1, C2) = split C into (C ∩ predecessors, C \ predecessors)
            partition.apply_split(C, C1, C2)

            // Hopcroft trick: only add the smaller half to the worklist.
            smaller = if |C1| <= |C2| { C1 } else { C2 }
            for each tag t with incoming edges into smaller:
                worklist.push((smaller, t))

    return partition

fn build_compiled_graph(graph, partition) -> CompiledGraph:
    // Assign StateIDs: one per block in the final partition.
    state_id = map each block to a dense integer

    // Build transition table.
    for each block B (StateID s):
        representative = any node in B
        for each outgoing edge (representative --[tag]--> target):
            target_state = state_id[block_of(target)]
            transitions.push(TransitionEntry {
                state_id: s, wire_tag: tag,
                child_state_id: target_state,
                is_message: target is a message node,
            })

    // Build static back-references (compile-time; used only for root initialisation).
    for each node n:
        s = state_id[block_of(n)]
        static_back_refs[s].push(StaticBackRef {
            schema_id: n.schema_id,
            msg_type_id: n.msg_type_id,
        })

    // Build root table.
    for each schema:
        root_node = schema.root_message_node
        roots.push(RootEntry {
            schema_id: schema.schema_id,
            state_id: state_id[block_of(root_node)],
        })

    sort transitions by (state_id, wire_tag)
    return CompiledGraph { transitions, static_back_refs, roots, ... }
```

**Complexity**: The refinement loop is O(|Σ| · |E| log |V|) where |Σ| is the
number of distinct tags, |E| the total edge count, and |V| the node count.
The key data structure need is an efficient reverse adjacency list: for each
`(block, tag)` pair, quickly enumerate all nodes with an incoming edge
labeled `tag` whose target is in `block`.  This is built once before the
loop as a `HashMap<(BlockId, WireTag), Vec<NodeId>>` and updated on splits.

---

## 4. Pre-compiled Graph: Storage and Instant Loading

### Format

The compiled graph is serialized to a single flat file using `rkyv`.  The
file layout:

```
[ 8-byte magic + version ]
[ 8-byte offset to rkyv root ]
[ rkyv-serialized ArchivedCompiledGraph ]
```

The `ArchivedCompiledGraph` contains flat arrays of `#[repr(C)]` structs
(all `bytemuck::Pod`), so the inner arrays are themselves zero-copy even
within the `rkyv` envelope.

### Instant loading

```rust
fn load_compiled_graph(path: &Path) -> Result<LoadedGraph> {
    let file   = File::open(path)?;
    let mmap   = unsafe { Mmap::map(&file)? };   // memmap2
    let graph  = unsafe {
        rkyv::access_unchecked::<ArchivedCompiledGraph>(&mmap[16..])
    };
    // `graph` is a zero-copy reference into the mmap'd pages.
    // OS pages in only the accessed regions on first access.
    Ok(LoadedGraph { _mmap: mmap, graph })
}
```

On a cold start, the OS does not read any of the file until a memory access
triggers a page fault.  On a warm start (file already in the page cache),
the "load" is O(1) regardless of file size.

### Python API integration

The compiled graph file is generated once (offline, when the FDP database
is updated) and shipped alongside the Python package.  The Rust extension
exposes:

```rust
#[pyfunction]
fn load_graph(path: &str) -> PyResult<PyCompiledGraph> { ... }

#[pyfunction]
fn match_message(graph: &PyCompiledGraph, wire_bytes: &[u8])
    -> PyResult<Vec<(u32, i64, i64, bool)>>   // (schema_id, matches, unknowns, vetoed)
{ ... }
```

`PyCompiledGraph` wraps the `LoadedGraph` in an `Arc` so it can be shared
cheaply across Python objects and threads.

### Incremental updates

When new FDPs are ingested, the compiled graph must be regenerated.
Regeneration is offline (not on the hot path).  The Hopcroft algorithm
operates on the full graph each time; incremental variants exist but add
significant complexity and are deferred.

A simple optimization: store a content hash of the FDP database alongside
the compiled graph file.  On startup, check the hash; if unchanged, skip
regeneration.

---

## 5. Additional Ideas for the Prototyping Phase

### 5.1 Measure deduplication ratio first
Before investing in the full Hopcroft implementation, run a quick
experiment: build the schema graph from a sample of real FDPs, assign
initial signature-based blocks (depth-0 partition only), and count how many
nodes collapse.  This gives a lower bound on deduplication and will validate
whether the effort is worthwhile.

### 5.2 Wire reader as a standalone crate
The raw wire reader (varint decode, tag parse, skip, recurse) is reusable
across the matching walk, the FDP loader, and potentially future tools.
Write it as a standalone module with a clean interface early; it is the most
load-bearing piece of the hot path.

### 5.3 Prototype the walk before the graph
The walk algorithm (§ Part 1 of SCHEMA_MATCH.md) can be prototyped without
any deduplication: load FDPs via `prost-reflect`, build a flat
`HashMap<(StateID, WireTag), ChildStateID>` transition table, and run the
parallel walk.  This validates the scoring logic and reveals the actual
active-set decay rate (`p` and `q` parameters) on real data before the
Hopcroft implementation is written.

### 5.4 Benchmark harness
Set up a Criterion benchmark from day one that measures:
- `compile_graph(fdp_set)` — one-time cost
- `load_graph(path)` — startup cost
- `match_message(graph, wire_bytes)` — hot-path latency

Real protobuf messages from production traffic (anonymized) are the right
benchmark inputs; synthetic messages may not reflect the actual active-set
decay.

### 5.5 Fuzz the wire reader
The wire reader will encounter malformed, truncated, and adversarial input
in production.  Add a `cargo-fuzz` target for the wire reader and the walk
function early; this is much cheaper to do before the code is complex.

### 5.6 Consider a tiered transition lookup
At depth 0 the active set is ~100,000 states.  Binary search in a sorted
flat array is O(log F) per state per field — for 100,000 states and 10
fields that is ~1.7M comparisons.  A hash map lookup per state would be
O(1) but cache-unfriendly.  A third option: sort the transition array by
`state_id` only, store per-state offsets, and do a linear scan within each
state's transitions (typically <50 fields per message type).  Profile all
three on real data before committing.
