<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0058 — Node framing in the scoring graph: LENDEL vs GROUP

**Status:** implemented
**Implemented in:** 2026-05-12
**App:** score-graph, reproto

---

## Background

The scoring graph models every proto message type (including group types) as a
non-leaf node.  When the scorer walks a binary protobuf stream, it needs to
know what wire type to expect when *entering* a non-leaf node: length-delimited
messages arrive as `WT_LEN` (wire type 2), while group-typed fields arrive as
`WT_START_GROUP` (wire type 3).

The compiled graph stores a `wire_type` field on every `NodeEntry`.  For
non-leaf nodes this is the wire type at which the node is entered.  The
Hopcroft minimization must therefore keep LENDEL nodes and GROUP nodes in
separate equivalence classes — even when their outgoing field structure is
identical — so that the scorer can apply the correct wire-type check.

### The bug

Currently the `wire_type` for non-leaf nodes is assigned in `compile()` *after*
Hopcroft minimization, by scanning incoming edges with `or_insert`.  If two
nodes — one a LENDEL message and one a GROUP — happen to have identical
outgoing field structures, Hopcroft merges them into one block *before*
`compile()` runs, and `or_insert` assigns whichever wire type is encountered
first.  The surviving block then has the wrong wire type for one of the two
original node types, causing the scorer to veto the correct schema or accept
the wrong one.

### Root cause

The Hopcroft initial partition currently groups non-leaf nodes by their
*outgoing signature* `Vec<(field_number, label)>` only.  It does not include
the node's own framing (`LENDEL` or `GROUP`), so structurally identical nodes
with different framing land in the same initial block and are never split.

---

## Goals

1. Add a `node_kind` field (`LENDEL` or `GROUP`) to the YAML message entry
   (spec 0045 §2), carrying the framing of the node.
2. Propagate `node_kind` through the graph builder so that the Hopcroft
   initial partition places LENDEL and GROUP nodes in separate blocks.
3. Store the resolved `wire_type` on `NodeEntry` correctly — derived from the
   source node's `node_kind` before Hopcroft runs, not inferred from incoming
   edges after minimization.
4. Revert the temporary workaround in `reproto phases.py` that collapsed
   `TYPE_GROUP` → `LEN_MSG` in the YAML output.
5. Revert the temporary workaround in `walk.rs` that accepted `WT_START_GROUP`
   as a match for a `WT_LEN` schema node and vice versa.
6. Rename the YAML field `kind` values `LEN_MSG` and `GROUP` (on field entries)
   to `MESSAGE` (a single value), since the framing is now a node-level
   attribute, not an edge-level attribute.
7. Rename internal Rust identifiers: `ScoringKind::LenMsg` → `ScoringKind::Node`
   (covering both LENDEL and GROUP children); drop `ScoringKind::Group`.
8. All existing tests must continue to pass.

## Non-goals

- Changes to the scorer walk logic beyond reverting the GROUP/LEN workaround.
- Changes to how leaves are handled (leaves have no framing).
- Changes to the `TransitionEntry` format (framing is a node property, not an
  edge property).
- New scoring features.

---

## Specification

### §1 — Updated YAML format (spec 0045 §2 revision)

Each message entry in the YAML gains a top-level `kind` field:

```yaml
messages:
  pkg.Outer:
    kind: LENDEL
    fields:
      - number: 1
        kind: MESSAGE
        child: pkg.Inner
      - number: 2
        kind: MESSAGE
        child: pkg.MyGroup
  pkg.Inner:
    kind: LENDEL
    fields:
      - number: 1
        kind: VARINT
  pkg.MyGroup:
    kind: GROUP
    fields:
      - number: 1
        kind: VARINT
```

Rules:

- `kind: LENDEL` — the node is entered via a length-delimited field
  (`WT_LEN`, wire type 2).  This is the default and applies to all
  `TYPE_MESSAGE` fields.
- `kind: GROUP` — the node is entered via a start-group tag
  (`WT_START_GROUP`, wire type 3).  Applies to all `TYPE_GROUP` fields.
- When `kind` is absent on a message entry, `LENDEL` is assumed (backward
  compatibility with existing YAML files that predate this spec).
- Field entries use `kind: MESSAGE` for both LENDEL and GROUP children.
  The child's framing is encoded in the child node's own `kind`, not in the
  edge.

### §2 — reproto: restore GROUP output for TYPE_GROUP fields

Revert the change introduced as a temporary workaround in `phases.py`:

```python
# Before workaround (correct):
if TYPE == FD.TYPE_GROUP:
    return 'GROUP', field.message_type.full_name, None, None

# After workaround (to revert):
if TYPE == FD.TYPE_GROUP:
    return 'LEN_MSG', field.message_type.full_name, None, None
```

Restore to the original `GROUP` output.

Also update `_phase_emit_graph` / `_scoring_kind` in `phases.py` to:
- Emit `kind: MESSAGE` for both `TYPE_MESSAGE` and `TYPE_GROUP` field entries.
- Emit `kind: GROUP` on the *message entry* for group-typed message definitions
  (i.e., messages whose FQDN corresponds to a `TYPE_GROUP` field's
  `message_type`).
- Emit `kind: LENDEL` on all other message entries.

#### Detecting whether a message is a GROUP

A message type is a GROUP if and only if it appears as the `message_type` of
at least one `TYPE_GROUP` field anywhere in the loaded pool.  In practice,
protoc generates a synthetic nested message type for each group field, named
after the group with the first letter capitalized; the group field itself has
`type == TYPE_GROUP` and `message_type` pointing to that synthetic type.

Implementation: during `_phase_emit_graph`, collect the set of FQDNs that are
referenced as group message types:

```python
group_fqdns: set[str] = set()
for fd in pool_files:
    for msg in all_messages_in_file(fd):
        for field in msg.fields_by_number.values():
            if field.type == FD.TYPE_GROUP:
                group_fqdns.add(field.message_type.full_name)
```

Then when emitting each message entry, set `kind: GROUP` if its FQDN is in
`group_fqdns`, else `kind: LENDEL`.

### §3 — load.rs: parse node_kind

`YamlMessage` gains a `kind` field:

```rust
#[derive(Debug, Deserialize)]
struct YamlMessage {
    #[serde(default)]
    kind: String,          // "LENDEL" (default) or "GROUP"
    fields: Vec<YamlField>,
}
```

`ScoringField` and the `Merged` state map do not need to change — the
node_kind is a property of the state (FQDN), not of individual fields.

Add a parallel map in `Merged`:

```rust
pub struct Merged {
    pub states: HashMap<String, Vec<ScoringField>>,
    pub node_kinds: HashMap<String, NodeKind>,  // FQDN → LENDEL or GROUP
    pub roots: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    LenDel,   // wire_type = 2
    Group,    // wire_type = 3
}
```

`ScoringKind::LenMsg` is renamed to `ScoringKind::Node`; `ScoringKind::Group`
is removed.  `is_message()` becomes `is_node()` and matches only
`ScoringKind::Node`.  The YAML field kind value `"LEN_MSG"` is accepted as an
alias for `"MESSAGE"` (backward compatibility); `"GROUP"` as a field kind is
no longer valid (error or ignored with a warning).

### §4 — graph.rs: derive wire_type before Hopcroft

`RawEdge` gains an `own_wire_type: u8` field — the wire type of the *source*
node:

```rust
pub struct RawEdge {
    pub src: u32,
    pub field_number: u32,
    pub dst: u32,
    pub label: u8,
    pub own_wire_type: u8,   // 2=LENDEL, 3=GROUP (wire type of src node)
}
```

When building `RawEdge` entries in `build()`, look up the source FQDN's
`NodeKind` from `merged.node_kinds` and set `own_wire_type` accordingly:

```rust
let own_wire_type = match merged.node_kinds.get(fqdn) {
    Some(NodeKind::Group) => 3u8,
    _ => 2u8,
};
```

Additionally, populate `node_attrs` (the map from raw node ID to wire_type)
*before* Hopcroft runs, using `merged.node_kinds` rather than inferring from
incoming edges in `compile()`.  Specifically, in `build()`:

```rust
// node_wire_types[node_id] = wire_type (2 or 3) for non-leaf nodes.
let mut node_wire_types: HashMap<u32, u8> = HashMap::new();
for (fqdn, node_id) in &node_ids {
    let wt = match merged.node_kinds.get(fqdn) {
        Some(NodeKind::Group) => 3u8,
        _ => 2u8,
    };
    node_wire_types.insert(*node_id, wt);
}
```

Return `node_wire_types` alongside `RawGraph` so `minimize()` and `compile()`
can use it.

### §5 — hopcroft.rs: include own_wire_type in initial partition

The outgoing signature for a non-leaf node must include its `own_wire_type`
so that LENDEL and GROUP nodes with identical field structures start in
different initial blocks.

Change the outgoing signature from `Vec<(field_number, label)>` to
`Vec<(field_number, label)>` augmented with a per-node `own_wire_type` prefix:

The initial partition key for non-leaf nodes becomes:
`(own_wire_type, Vec<(field_number, label)>)`.

Concretely:

```rust
// Before: grouped by outgoing sig only.
let mut sig_to_block: HashMap<Vec<(u32, u8)>, u32> = HashMap::new();
// ...
let b = sig_to_block.entry(sig[i].clone()).or_insert_with(...);

// After: grouped by (own_wire_type, outgoing sig).
let mut sig_to_block: HashMap<(u8, Vec<(u32, u8)>), u32> = HashMap::new();
// ...
let wt = node_wire_types[&(i as u32)];
let b = sig_to_block.entry((wt, sig[i].clone())).or_insert_with(...);
```

The reverse adjacency and refinement loop are unchanged — they already
operate on block IDs, which now correctly separate LENDEL and GROUP nodes.

### §6 — graph.rs compile(): use pre-computed wire_types

Remove the `or_insert` wire_type inference from incoming edges in `compile()`.
Instead, use `node_wire_types` (passed in from `build()`) to populate
`node_attrs` for non-leaf nodes directly:

```rust
for (fqdn, &node_id) in &raw.node_ids {
    let block = partition.block_of(node_id);
    let wt = node_wire_types.get(&node_id).copied().unwrap_or(2);
    node_attrs.entry(block).or_insert((wt, false, 0xFFFF));
}
```

The `or_insert` here is now safe because all nodes in the same block have the
same `wire_type` (guaranteed by the updated initial partition in §5).

### §7 — walk.rs: revert GROUP/LEN workaround

Remove the cross-matching introduced as a temporary workaround:

```rust
// Remove this in schema_verdict:
|| (stream_wire_type == WT_START_GROUP && expected_wt == WT_LEN)
|| (stream_wire_type == WT_LEN && expected_wt == WT_START_GROUP)

// Remove this in score_message_multi inline check:
|| (wire_type == WT_START_GROUP && expected_wt == WT_LEN)
|| (wire_type == WT_LEN && expected_wt == WT_START_GROUP)
```

The scorer now relies on the compiled graph having correct wire_types for all
non-leaf nodes, which the Hopcroft fix (§5) guarantees.

### §8 — Spec 0045 update

Update spec 0045 §2 and §3 to reflect:
- Message entries now have a top-level `kind: LENDEL | GROUP` field.
- Field entries use `kind: MESSAGE` for both message and group child references.
- The `_scoring_kind` function emits `("MESSAGE", child_fqdn)` for both
  `TYPE_MESSAGE` and `TYPE_GROUP` field types.
- The `group_fqdns` set is built during `_phase_emit_graph` to correctly tag
  group message entries with `kind: GROUP`.

### §9 — Test updates

- Existing E2E tests and unit tests must pass without modification (the YAML
  format change is backward compatible since `kind` on message entries defaults
  to `LENDEL`).
- Add a new unit test: two nodes with identical field structure, one LENDEL and
  one GROUP, must land in different Hopcroft blocks and produce distinct
  `wire_type` values in the compiled `NodeEntry` table.
- The stress test DB must be rebuilt after this change (the previous DB was
  built with the GROUP→LEN_MSG workaround and is stale).

---

## Files changed

| File | Change |
|---|---|
| `reproto/src/reproto/phases.py` | Revert `TYPE_GROUP → LEN_MSG`; emit `kind: MESSAGE` for field entries; emit `kind: GROUP/LENDEL` on message entries |
| `score-graph/src/build_scoring_graph/load.rs` | Add `NodeKind` enum; add `node_kinds` to `Merged`; parse `kind` on `YamlMessage`; rename `ScoringKind::LenMsg` → `ScoringKind::Node`, remove `ScoringKind::Group`; accept `"MESSAGE"` and `"LEN_MSG"` as aliases |
| `score-graph/src/build_scoring_graph/graph.rs` | Add `own_wire_type` to `RawEdge`; compute `node_wire_types` in `build()`; pass to `minimize()` and `compile()`; fix `compile()` to use pre-computed wire_types |
| `score-graph/src/build_scoring_graph/hopcroft.rs` | Include `own_wire_type` in initial partition key |
| `score-graph/src/score/walk.rs` | Revert GROUP/LEN cross-matching workaround |
| `docs/specs/0045-reproto-emit-graph.md` | Update §2, §3 for new YAML format |
