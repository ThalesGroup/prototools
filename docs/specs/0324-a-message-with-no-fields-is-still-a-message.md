<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0324 — a message with no fields is still a message

Status: implemented
Implemented in: 2026-08-18
App: prototext-graph
Refs: docs/specs/0295-… (`TransitionEntry::child_wire_type`, and the
        rule that a denormalized fact is filled by `link_transitions`),
        docs/specs/0238-… (graph format v3, `ext_range_idx`, and the
        precedent for a state attribute that no edge carries),
        docs/specs/0058-… and docs/specs/0060-… (`node_wire_types`, the
        Hopcroft initial partition, and why message nodes and leaf nodes
        are seeded from two separate maps), docs/specs/0153-…
        (`find_node`/`state_has_transitions`), docs/specs/0266-…
        (the schema-free probe verdict), docs/specs/0176-…
        (an open enum gets no range, so it cannot be out of range)

## Background

`/1/3` of `googleapis.desc`, read as
`google.api.expr.v1alpha1.Type.AbstractType`, scores **13** — thirteen
matched fields and every other counter zero — while the very same bytes
render with `OPEN_GROUP` and `INVALID_TAG_TYPE` inside them. The reader
is shown a document full of faults over a score that says the schema fits
perfectly.

The faults live under `Type.dyn`, whose declared type is
`google.protobuf.Empty`. Its payload's first tag is field 14, wire type 3
— a group that is never closed.

`walk.rs:2046` decides whether to descend:

```rust
let is_message = state_has_transitions(ws.graph, child);
```

`Empty` declares no field, so its state has no outgoing transition, so
`is_message` is false, so the `Verdict::Found` arm takes the leaf branch
(`walk.rs:2054-2072`): `matches += 1`, no `child_pairs` entry, no
recursion. `propagate_vetoes` (`walk.rs:1284-1307`, called at
`walk.rs:2149`) is then correct and useless — there is no child walk for
it to lift a veto out of. Meanwhile `prototext-core` *does* descend, which
is why the render and the score disagree.

Two measurements bound the defect.

**It is exactly zero-field types, and nothing else.** The same payload
`0f` — field 1, wire type 7, which no tag may carry — under two declared
message fields:

| bytes | scored as | field's declared type | result |
|---|---|---|---|
| `22 01 0f` | `FileDescriptorProto` | `DescriptorProto` (has fields) | `vetoed: true` |
| `0a 01 0f` | `…v1alpha1.Type` | `Empty` (no fields) | `score: 1` |

So recursion, veto propagation and the framing checks all work. Only the
predicate that gates them is wrong.

**The framing checks the descent would reach already fire on unknown
fields.** Scored against `google.protobuf.Empty` itself, which declares
nothing, so every tag below is unknown by construction:

| payload | verdict |
|---|---|
| `08 01` unknown varint | `unknowns: 1`, score −10 |
| `73 74` unknown group, closed | `unknowns: 1`, score −10 |
| `73` unknown group, never closed | **vetoed** |
| `73 7c` group 14 closed by end-group 15 | **vetoed** |
| `74` stray end-group | **vetoed** |
| `0f` invalid wire type | **vetoed** |
| `0a 20 61` LEN overruns the frame | **vetoed** |
| `0a 01 73` unknown LEN whose *payload* holds an open group | `unknowns: 1`, no veto |

That last row is not an omission, and the line it draws is the one this
spec relies on: a tag's wire type and a LEN length prefix are bytes of
**the frame being scored**, so no reinterpretation of the field can
rescue them. A payload belongs to the field, and calling that field
`bytes` rescues everything inside it — so there is nothing to charge. A
group has no length prefix, which is why an open group falls on the
enclosing side of the line rather than being an exception to it.

So the whole fix is to make the walk enter `dyn`'s payload. Everything it
needs to find there is already implemented.

## Goals

- **G1.** A declared message field is walked as a message whatever its
  arity, so a fault inside a zero-field type is scored exactly as the
  same fault inside any other type — including the veto, and including
  the propagation of that veto to every ancestor.
- **G2.** The predicate is read from the graph, not inferred from the
  transition table. "Has an outgoing edge" and "is a message" are
  different questions and must stop sharing an answer.

## Non-goals

- **N1.** Descending into *unknown* LEN fields. The walk charges
  `unknowns: 1` for one and does not look inside, and the table above is
  why that is right rather than lazy: the payload of an unknown field is
  rescued by reading the field as `bytes`, so nothing in it is evidence
  against the candidate. What is *not* rescuable — a bad wire type, an
  unclosed group, a length prefix that overruns — is part of the
  enclosing frame and already vetoes today.

- **N2.** Changing the schema-free renderer. The suspicion that a
  would-be message containing a vetoed group should be demoted to
  `string`/`bytes` is already the implemented behavior (spec 0266): an
  unclosed group in a candidate payload disqualifies it, measured —
  `0a 07 "seconds"` renders `1: "seconds" #@ string`, `0a 01 0f` renders
  `1: "\017" #@ string`, and only a clean payload renders `1 { … } #@
  message`. The `OPEN_GROUP` a reader sees in this document comes from
  the *declared* path, where a declared message type is rendered
  unconditionally and the probe is deliberately not consulted (spec
  0311). That is the mirror image of this defect, not a second one.

- **N3.** A penalty tier between "scored" and "vetoed" for framing
  faults. The veto is correct here: the bytes cannot be read as the
  declared type at all. Softening it needs the per-boundary snapshot
  deferred as spec 0238 N6.

- **N4.** The `ENUM_UNKNOWN` visible next to these faults. An open
  (proto3) enum is emitted as `int32` with no range at all
  (`reproto/src/reproto/phases.py:1834`), so `out_of_range` has nothing
  to test and the value costs nothing — by design, spec 0176. It shares
  an example with this defect and is not part of it.

## Specification

- **S1.** The graph records, per state, that the state is a message. It
  cannot be re-derived: a zero-field message node and the `bytes` leaf
  node carry identical `NodeEntry` attributes today —
  `wire_type: 2, is_string: false, range_idx: 0xFFFF, trans_len: 0`.
  They are nonetheless *distinct states*, and permanently so: the
  Hopcroft initial partition seeds messages and leaves from two separate
  maps (`hopcroft.rs:136-178`, `sig_to_block` and `leaf_attr_to_block`),
  each allocating fresh block ids, and refinement never merges. The
  information exists in the build; only the serialized node forgets it.

- **S2.** It is carried as a new `NodeEntry::wire_type` discriminant,
  **10 = MESSAGE**, alongside the internal `8 = UINT32` and `9 = INT32`
  that byte already holds. `node_wire_types` (`graph.rs:242-245`) emits
  `10` where it emits `2` today; `3` for a group is unchanged, because a
  group is already unambiguous. `hopcroft.rs:154`'s `unwrap_or(2)`
  follows. `NodeEntry` does not grow, and — since every message node
  moves together and groups do not move at all — the initial partition
  is renamed, not repartitioned.

  `link_transitions` (`graph.rs:487-498`) normalizes it back to the
  on-wire `2` for `TransitionEntry::child_wire_type`, in the same `match`
  that already maps `8 | 9 => 0`. That is the one place the distinction
  must *not* leak: `child_wire_type` is compared against a tag's wire
  type at `walk.rs:1766`.

  `GRAPH_VERSION` goes 5 → 6. A v5 file read as v6 would find `2` on
  every message node and behave exactly as today — a plausible wrong
  answer, which is what the version check exists to prevent.

- **S3.** `walk.rs:2046` becomes a read of the node it already fetched on
  the next line:

  ```rust
  let node = find_node(ws.graph, child);
  let is_message = node.is_some_and(|n| n.wire_type == WT_NODE_MESSAGE);
  ```

  `state_has_transitions` then has no caller and is deleted. This is the
  only site, so the fix *removes* a binary search from the hot path
  rather than adding one.

- **S4.** `serial.rs`'s text dump gains `10 => "message"`. Today a
  message state dumps as `type: bytes`, which is the same confusion in
  the same byte, visible to anyone reading a graph by hand.

## Alternatives considered

**Widening `NodeEntry::is_string` to a `kind: u8`.** Same size, same
information, and arguably the more honest name. Rejected because it
splits one fact across two fields — the reader would have to know that
`wire_type` answers "how is it framed" while `kind` answers "what is it",
and the `8`/`9` discriminants already sitting in `wire_type` prove that
byte is where this project puts scoring kind.

**Adding a field to `NodeEntry`.** `state_id: u32, wire_type: u8,
is_string: bool, range_idx: u16` packs into exactly 8 bytes at alignment
4, so a fifth field pads to 12 — the same 50% the `ext_range_idx` note at
`serial.rs:29-35` already records. There is a spare discriminant; there
is no spare byte.

**Keeping `state_has_transitions` and special-casing zero-field states at
build time** — for instance, giving every message at least a sentinel
transition. Rejected: it would put a field number that no schema declares
into the transition table, which `find_transition` searches by field
number, and every reader of `trans_len` would have to learn the
exception.

## Test plan

1. `a_zero_field_message_is_walked` — a schema with a field whose type
   declares no field; a payload putting one clean unknown tag inside it
   scores `unknowns: 1`, not `matches: 1`. The predicate, isolated from
   any veto. G1, S3.
2. `a_fault_in_a_zero_field_message_vetoes_its_parent` — the A/B from the
   Background as an assertion: payload `0f` under a zero-field type
   vetoes, exactly as it does under a type with fields.
3. `an_open_group_in_a_zero_field_message_vetoes` — the reported case:
   `73` as the payload of a declared `Empty`-shaped field, and the veto
   present on the *root* entry, which is what makes it `propagate_vetoes`
   and not a local verdict.
4. `an_empty_zero_field_message_still_matches` — the common, legitimate
   case: a present-but-empty `Empty` field scores `matches: 1` and
   nothing else. This is what stops the fix from making every `Empty` in
   the corpus a penalty.
5. `a_message_node_is_not_a_bytes_leaf` — over a built graph, no state is
   both `wire_type == 10` and reachable as a `bytes` leaf; and every
   `TransitionEntry::child_wire_type` is a value a tag can carry, i.e.
   never 10. S2's normalization, pinned where it would silently break
   `walk.rs:1766`.
6. The version check needs no new test:
   `graph_with_older_version_is_rejected_with_a_rebuild_instruction`
   is written against `GRAPH_VERSION - 1`, so it moved to 5 → 6 by
   itself.
7. The existing scoring suite is the real gate: it is built on hand-made
   schemas whose message types mostly *do* declare fields, so any
   movement in it is a message/leaf confusion this change introduced.

## Measured outcome

**The reported defect is gone, and the right answer is now alone.** The
Background's `/1/3` of `googleapis.desc` — the `DescriptorProto` for
`Interval` — scored 13 under both `google.protobuf.DescriptorProto` and
`google.api.expr.v1alpha1.Type.AbstractType`, the second of which renders
with `OPEN_GROUP` inside it. `AbstractType` is now vetoed and
`DescriptorProto` stands at 13 with the runners-up at 3.

The same holds one level down, which is where it was reported from.
`/1/3/2`, the 24-byte `FieldDescriptorProto` for `seconds`, offered
`google.api.expr.v1alpha1.Type` at `matches: 5` — a perfect reading, over
a `Type.dyn` payload (`"seconds"`, i.e. an unclosed group 14) the walk had
not looked at. `Type` is vetoed now; surviving candidates for that node
fall 22 400 → 22 349, and `google.protobuf.FieldDescriptorProto` keeps its
5.

**The blast radius is 524 edges.** Of googleapis' 16 696 states, 16 681
are messages and exactly **2** of those declare no field — every
zero-field message in the corpus is bisimilar to every other, so Hopcroft
collapses them to a closed one and an extensible one. **524** of the
85 806 transitions point at one of the two; those are the edges that were
being read as `bytes` leaves.

Two numbers were not taken. The `score_all` wall clock, because S3 removes
a binary search from the hot path and adds none, so the direction is not
in doubt and a fair A/B needs two binaries — the version bump means a v5
and a v6 corpus cannot be read by the same one. And the rank check over
the 375 `instances/**/*.pb` blobs, for the same reason: with only two
zero-field states in the graph, the population that could move is the 524
edges above, and the two nodes measured are both in it.

The regeneration cost is the one the version bump always carries:
`prototext/wkt/prebuilt/wkt.rkyv` rebuilt through the local-pyo3 shim
(`wkt_index.rkyv` came back byte-identical, which is the shim's own
sanity check) and `nix-build -A googleapis-db` re-run.
