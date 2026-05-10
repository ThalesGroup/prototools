<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0044 — Scoring graph builder and visualiser

**Status:** draft
**Implemented in:** —
**App:** schema-db (new crate)

---

## Background

Spec 0043 defines a four-stage pipeline from `.proto` source files to a
Hopcroft-deduplicated `CompiledGraph`.  This spec covers the first concrete
implementation step: given a single self-contained `.pb` file (one
`FileDescriptorProto`), build the *scoring graph* for that schema and render
it as an interactive HTML visualisation.

This is the foundation for the graph builder (spec 0043 Sub-step A) and the
first artifact that can be inspected by a human.

---

## Goals

1. Given a single `.pb` file, construct the scoring graph for all message
   types it declares.
2. The graph uses only the information required for scoring (spec 0042
   §Schema information required for scoring) — no field names, no enum
   tables, no options.
3. Produce an interactive HTML visualisation of the graph using `pyvis`,
   following the same approach as `reproto --graph`.
4. Validate the output on `google/longrunning/operations.proto` (trivial:
   one message, two string fields, no recursion) and
   `google/rpc/status.proto` (richer: three field types including a repeated
   message field that introduces a cycle via `google.protobuf.Any`).

## Non-goals

- Multi-schema graph (that is spec 0043 Sub-step A over the whole corpus).
- Hopcroft minimisation (spec 0043 Sub-step B).
- Serialisation to `CompiledGraph` binary (spec 0043 Sub-step C).
- A Rust implementation (this step is a Python prototype for rapid
  iteration and visual debugging; Rust comes later).

---

## Specification

### §1 — ScoringKind

Every field in a `FileDescriptorProto` is mapped to exactly one of eight
`ScoringKind` values.  This is the only per-field information the graph
retains:

```python
class ScoringKind(enum.Enum):
    VARINT     = 0   # INT32 INT64 UINT32 UINT64 SINT32 SINT64 BOOL ENUM
                     #   → expected wire type 0
    I64        = 1   # FIXED64 SFIXED64 DOUBLE
                     #   → expected wire type 1
    LEN_STRING = 2   # STRING  → expected wire type 2, triggers UTF-8 veto
    LEN_BYTES  = 3   # BYTES   → expected wire type 2, no recursion
    LEN_MSG    = 4   # MESSAGE → expected wire type 2, recurse into child
    LEN_PACKED = 5   # packed repeated (any varint or fixed type)
                     #   → expected wire type 2, no element-level check
    GROUP      = 6   # GROUP   → expected wire type 3
    I32        = 7   # FIXED32 SFIXED32 FLOAT
                     #   → expected wire type 5
```

The mapping from `FieldDescriptorProto.type` (and `label` + `options.packed`
for the packed case) to `ScoringKind`:

| Proto type | Packed repeated? | ScoringKind |
|---|---|---|
| TYPE_INT32, TYPE_INT64, TYPE_UINT32, TYPE_UINT64, TYPE_SINT32, TYPE_SINT64, TYPE_BOOL, TYPE_ENUM | no | VARINT |
| same | yes (label=REPEATED, packed=true) | LEN_PACKED |
| TYPE_FIXED64, TYPE_SFIXED64, TYPE_DOUBLE | — | I64 |
| TYPE_STRING | — | LEN_STRING |
| TYPE_BYTES | — | LEN_BYTES |
| TYPE_MESSAGE | — | LEN_MSG |
| TYPE_GROUP | — | GROUP |
| TYPE_FIXED32, TYPE_SFIXED32, TYPE_FLOAT | — | I32 |

### §2 — Graph nodes

The scoring graph for one `.pb` file has two kinds of nodes:

**MessageNode** — one per message type in the `FileDescriptorProto` (and
transitively in any imported `FileDescriptorProto` loaded into the same
pool):

```python
@dataclass
class MessageNode:
    full_name: str          # fully-qualified proto name, e.g. "google.rpc.Status"
    fields:    list[ScoringField]
```

```python
@dataclass
class ScoringField:
    field_number: int
    kind:         ScoringKind
    child:        str | None  # full_name of child MessageNode; set iff kind == LEN_MSG
```

**LeafNode** — five singletons, one per wire type, shared across all schemas:

```python
LEAF_VARINT = LeafNode(wire_type=0)
LEAF_I64    = LeafNode(wire_type=1)
LEAF_LEN    = LeafNode(wire_type=2)   # for LEN_STRING, LEN_BYTES, LEN_PACKED
LEAF_GROUP  = LeafNode(wire_type=3)
LEAF_I32    = LeafNode(wire_type=5)
```

`LEN_MSG` fields do NOT point to a leaf node — they point to the child
`MessageNode`.

### §3 — Graph edges

Each `ScoringField` in a `MessageNode` is one directed edge:

```
source:  MessageNode (the declaring message type)
target:  MessageNode  (when kind == LEN_MSG, target = child message node)
         LeafNode     (all other kinds, target = the matching leaf)
label:   (field_number, wire_type)
         wire_type derived from ScoringKind:
           VARINT/LEN_PACKED/GROUP/I32 → their respective wire types
           LEN_STRING/LEN_BYTES/LEN_MSG → wire type 2
```

### §4 — Building the graph from a .pb file

```python
def build_scoring_graph(pb_path: Path, pool: DescriptorPool | None = None
                        ) -> dict[str, MessageNode]:
    """
    Load pb_path into pool (or a fresh pool if None), then build and return
    a dict mapping full_name -> MessageNode for every message type reachable
    from the root FileDescriptorProto.
    """
```

Steps:
1. Read `pb_path` as a binary `FileDescriptorSet`; decode with `prost-reflect`
   (or, in the Python prototype, with `google.protobuf.descriptor_pb2`).
2. Add the `FileDescriptorProto`(s) to the pool.
3. For every message type in the root `FileDescriptorProto` (recursing into
   nested message types), create a `MessageNode`.
4. For every field in each message type, create a `ScoringField`:
   - Determine `ScoringKind` from the field's type and options.
   - If `kind == LEN_MSG`, record `child = field.type_name` (strip leading
     dot; resolve against the pool).
5. Return the dict of `MessageNode`s.

Cross-file message references (e.g. `google.protobuf.Any` referenced from
`google.rpc.Status`) are resolved by loading all imported `.pb` files into
the same pool before building the graph.  The pool is passed in so the
caller controls which imports are available.

### §5 — Visualisation

The visualiser renders the scoring graph as an interactive HTML file using
`pyvis`, matching the style of `reproto --graph` (`show.py`).

```python
def render_scoring_graph(
    nodes:       dict[str, MessageNode],
    output_path: Path,
    title:       str = "Scoring graph",
) -> None:
```

**Node rendering:**

| Node type | Shape | Colour | Label |
|---|---|---|---|
| MessageNode (root FDP) | `dot`, large (size 30) | `#97fc9a` (green) | short name (last component) |
| MessageNode (imported) | `dot`, small (size 15) | `#aaaaff` (blue) | short name |
| LeafNode | `square`, tiny (size 10) | `#ffcc44` (yellow) | wire type, e.g. `wt=0` |

**Edge rendering:**

Each `ScoringField` becomes one directed edge from source `MessageNode` to
target node.  Edge label: `f{field_number}` (e.g. `f3`).  Edge colour
encodes `ScoringKind`:

| ScoringKind | Colour |
|---|---|
| VARINT | `#888888` |
| I64 | `#884400` |
| LEN_STRING | `#ff4444` |
| LEN_BYTES | `#ff9944` |
| LEN_MSG | `#4444ff` |
| LEN_PACKED | `#884488` |
| GROUP | `#44aa44` |
| I32 | `#448844` |

Physics: `barnes_hut` with the same settings as `show.py`.

### §6 — CLI

The Python prototype is a standalone script `schema-db/tools/graph_proto.py`:

```
python graph_proto.py <pb-file> [<pb-file> ...] [-o output.html]
```

- Accepts one or more `.pb` files.
- The first `.pb` is the "root" (its message types are highlighted green);
  remaining `.pb`s provide imported types (blue).
- Writes `output.html` (default: `scoring_graph.html`).

### §7 — Validation examples

#### Example A — `google/longrunning/operations.proto`

One message type `OperationInfo`, two fields:
- `response_type` (field 1, TYPE_STRING) → `ScoringKind.LEN_STRING`
- `metadata_type` (field 2, TYPE_STRING) → `ScoringKind.LEN_STRING`

Expected graph:
```
OperationInfo --[f1, wt=2]--> LEAF_LEN
              --[f2, wt=2]--> LEAF_LEN
```
Two edges, both to `LEAF_LEN`.  No recursion.

#### Example B — `google/rpc/status.proto`

One message type `Status`, three fields:
- `code` (field 1, TYPE_INT32) → `ScoringKind.VARINT`
- `message` (field 2, TYPE_STRING) → `ScoringKind.LEN_STRING`
- `details` (field 3, TYPE_MESSAGE → `google.protobuf.Any`) → `ScoringKind.LEN_MSG`

`google.protobuf.Any` has two fields:
- `type_url` (field 1, TYPE_STRING) → `ScoringKind.LEN_STRING`
- `value` (field 2, TYPE_BYTES) → `ScoringKind.LEN_BYTES`

Expected graph (with `google/protobuf/any.pb` also loaded):
```
Status  --[f1, wt=0]--> LEAF_VARINT
        --[f2, wt=2]--> LEAF_LEN
        --[f3, wt=2]--> Any

Any     --[f1, wt=2]--> LEAF_LEN
        --[f2, wt=2]--> LEAF_LEN
```
Five edges.  `Status.details` recurses into `Any`; `Any` has no message-type
fields, so no further recursion.

---

## Implementation location

Python prototype: `schema-db/tools/graph_proto.py`

This is deliberately a standalone Python script, not a Rust crate, so it can
be iterated on quickly.  It uses:
- `google.protobuf.descriptor_pb2` — already available (used by reproto tests).
- `pyvis` — already available (used by `reproto --graph`).

The Rust graph builder (spec 0043 Sub-step A) will replicate the same logic
using `prost-reflect`.
