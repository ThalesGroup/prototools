<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0108 — `MessageSet` structural recognition in scoring-graph YAML (Tier 0)

**Status:** implemented
**Implemented in:** 2026-07-07
**Refs:** `docs/specs/0045-reproto-emit-graph.md`, `docs/specs/0089-any-expansion.md`,
`docs/specs/0100-message-set-expansion.md`
**App:** reproto

---

## Background

The protobuf wire format supports a `message_set_wire_format` option on
message types (`MessageOptions.message_set_wire_format`). A message with
this option set (a `MessageSet`) acts as a typed container for arbitrary
sub-messages, keyed by extension field number rather than a string
`type_url`. On the wire, a `MessageSet` payload consists of repeated groups
at field number 1, each containing:

- field 2 (`type_id`): varint — the extension field number identifying the
  type
- field 3 (`message`): length-delimited bytes — the serialised sub-message

Per `descriptor.proto`, a `MessageSet`-flagged message **has no declared
fields** — only `extension_range` entries. The `Item` group structure is
implicit in the wire format, not declared as fields on the descriptor.

Spec 0100 already implements `MessageSet` expansion at **render time**
(`prototext decode`), using a structural check
(`message_set_wire_format == true && fields().count() == 0`) rather than any
hardcoded FQDN — real-world descriptor sets use custom names
(`bridge.MessageSet`, `proxy.MessageSet`), never the literal
name `google.protobuf.MessageSet`. Spec 0100 explicitly listed
"Scoring-walk expansion of MessageSet" as a non-goal, deferred to future
work.

The scoring graph (`prototext-graph`) has a matching gap today:

- `graph.rs:183-194` reserves raw node ID 1 (`MESSAGE_SET_NODE_ID`) keyed to
  the **literal, non-existent FQDN** `"google.protobuf.MessageSet"` — a
  name that never appears in any real corpus (unlike `google.protobuf.Any`,
  which is a genuine, singleton well-known type). No YAML file emitted by
  `reproto` today declares a message under that name, so this reservation
  is permanently dead: `MESSAGE_SET_NODE_ID` is referenced only in
  `hopcroft.rs` (to protect it from equivalence-class merging) and in the
  `graph.rs` insertion itself. `walk.rs` never consumes it.
- Because a `MessageSet`-flagged message has zero declared fields (per the
  descriptor.proto invariant), `reproto`'s existing `_phase_emit_graph`
  (spec 0045 §4) emits it in YAML as a message with an **empty `fields`
  list**. Nothing in the emitted YAML distinguishes it from any other
  fieldless message.
- Consequently, at score time a `MessageSet` field's `Item` groups fall
  through to the walker's generic unrecognized-`GROUP` handling
  (`parse_group_blind` in `walk.rs`), which already degrades gracefully
  (`unknowns += 1` per group, no veto) — but no match credit is ever given
  for the (protocol-fixed, always-present) `type_id` and `message`
  sub-fields.

Because `Item`'s wire shape is 100% protocol-fixed and identical across
every `MessageSet` in existence — equivalent to writing, in plain proto2:

```proto
message Item {
  optional int32 type_id = 2;
  optional bytes  message  = 3;
}
```

— it can be declared and wired up as an ordinary node with ordinary fields,
using constructs the YAML schema (spec 0045 §2) and the Rust
loader/walker (`prototext-graph`) already fully support. No new
`ScoringKind`, no reserved sentinel, no side-table, and no `walk.rs` change
are needed to get real match credit for `type_id` and `message`.

Recursing into `message`'s bytes using the specific extension type keyed by
`(extendee_fqdn, type_id)` — analogous to `Any`'s `type_url`-keyed
resolution (spec 0089) — is a separate, harder problem (needs a
per-corpus side-table built from `extend` declarations, plus one reserved
sentinel node and one raw-bytes extraction function). That is out of scope
here; see Non-goals.

---

## Goals

1. In `reproto`'s scoring-graph emission (`_phase_emit_graph`, spec 0045
   §4), detect messages with `message_set_wire_format == true` (and, per
   the descriptor.proto invariant, zero declared fields).
2. For each such message, synthesize in the emitted YAML:
   - An `Item` message entry (`kind: GROUP`) with two ordinary fields:
     `type_id` (`kind: VARINT`, `number: 2`) and `message` (`kind:
     LEN_BYTES`, `number: 3`).
   - A synthetic field on the `MessageSet`-flagged container itself:
     `{number: 1, kind: MESSAGE, child: <Item FQDN>, label: repeated}`.
3. Use these YAML constructs exclusively as they already exist in spec
   0045's schema — no changes to the YAML format, the Rust YAML loader
   (`load.rs`), the compiled/rkyv graph representation, or `walk.rs`.
4. Add reproto unit tests (parallel to the existing
   `test_emit_scoring_graphs.py` suite) covering the synthesized `Item`
   node and edge, using the existing `message_set_proto2.proto` fixture.

## Non-goals

- **Tier 1**: recursing into `message`'s bytes using the resolved extension
  type (keyed by `(extendee_fqdn, type_id)`, built from `extend`
  declarations). This requires a new per-corpus side-table, a reserved
  sentinel node (parallel to `ANY_NODE_ID`), and a raw-bytes extractor for
  repeated groups — deferred as explicit future work.
- Removing or repurposing the existing (dead) `MESSAGE_SET_NODE_ID` /
  `"google.protobuf.MessageSet"` reservation in `graph.rs`/`hopcroft.rs`.
  It remains unused and untouched by this spec.
- Any change to render-time `MessageSet` expansion (spec 0100) — already
  correct and unaffected.
- Any change to `prototext-graph`, `prototext-graph-pyo3`, or `prototext`
  Rust code — Tier 0 is entirely a `reproto`-side (Python) change.
- Naming collisions between the synthesized `Item` type and a
  user-defined nested type also named `Item` within the same
  `MessageSet`-flagged message — not expected to occur in practice
  (`Item` would need to already be a nested type name — collision handling
  deferred if ever observed).

---

## Specification

### §1 — Detection

Note: the actual entry points are named `_phase_emit_scoring_graphs` (CLI
flag `--emit-scoring-yaml`) and `_phase_build_schema_db`
(`--build-schema-db`), each with its own (currently duplicated,
pre-existing) `_collect` closure in `reproto/src/reproto/phases.py` — spec
0045's documented names (`_phase_emit_graph`, `--emit-scoring-graph`) are
stale. Both closures build fields the same way, so the detection/synthesis
logic is factored into one shared module-level helper,
`_synthesize_message_set_item`, called from both:

```python
def _synthesize_message_set_item(desc: Any, messages: dict, fields_out: list) -> None:
    if not desc.GetOptions().message_set_wire_format or fields_out:
        return
    ...
```

`desc.GetOptions().message_set_wire_format` is a plain proto2-optional
bool (default `False`), so no `HasField` check is needed — same pattern
already used elsewhere in `phases.py` (e.g. `field.message_type
.GetOptions().map_entry`). The `fields_out` (already-collected real
fields) empty-check mirrors the render-time check in spec 0100 §1
(option flag set, zero declared fields).

### §2 — Synthesized `Item` node and edge

When the guard in §1 passes for `desc`:

1. Compute the synthetic type's FQDN: `f"{desc.full_name}.Item"` (nested
   under the `MessageSet`-flagged message's own FQDN, guaranteeing
   uniqueness across the corpus the same way real nested types are keyed).
2. Emit an `Item` entry in `messages`. Field `type` strings are the actual
   lowercase values `_scoring_kind`/`load.rs`'s `parse_kind` use (`int32`,
   `bytes`, `message`) — not spec 0045's documented uppercase names
   (`VARINT`, `LEN_BYTES`, `MESSAGE`), which do not match the real
   implementation:
   ```yaml
   <desc.full_name>.Item:
     kind: GROUP
     fields:
       - number: 2
         type: int32
       - number: 3
         type: bytes
   ```
3. Append, to `fields_out` (the container's own field list, otherwise
   empty), a single synthetic field:
   ```yaml
   - number: 1
     type: message
     child: <desc.full_name>.Item
     label: repeated
   ```

No other change to either `_collect`'s existing traversal (nested types,
`Item` and any genuine nested types coexist normally — recursion into
`desc.nested_types` is unaffected since `Item` is not a real nested type
in the descriptor and is added directly to `messages`, not recursed into).

### §3 — Tests

Added to `reproto/src/reproto/tests/test_emit_scoring_graphs.py` (the
real file — spec 0045's documented `test_emit_graph.py` name is stale),
using the existing fixture `message_set_proto2.proto`
(`mockup.MessageSetMessage`, option set, `extensions 100 to 199`;
`mockup.Payload`, plain message, as the negative control):

- **TC-MS1**: run with `--emit-scoring-yaml` on `message_set_proto2.proto`.
  Assert `mockup.MessageSetMessage` has exactly one field in its emitted
  `fields` list: `{number: 1, type: "message", child:
  "mockup.MessageSetMessage.Item", label: "repeated"}`.
- **TC-MS2**: assert `mockup.MessageSetMessage.Item` appears as a top-level
  key in `messages`, with `kind: GROUP` and exactly two fields:
  `{number: 2, type: "int32"}` and `{number: 3, type: "bytes"}` (neither
  carries a `child` key).
- **TC-MS3** (negative): `mockup.Payload` (no `message_set_wire_format`) is
  emitted normally — one real `string` field, no synthesized
  `mockup.Payload.Item` key in `messages`.

All three verified to fail on pre-fix code (bisected via
`git checkout`/`git apply`) and pass post-fix.

---

## Files changed

| File | Change |
|---|---|
| `reproto/src/reproto/phases.py` | Add `_synthesize_message_set_item` helper; call it from both `_collect` closures (`_phase_build_schema_db`, `_phase_emit_scoring_graphs`) |
| `reproto/src/reproto/tests/test_emit_scoring_graphs.py` | Add TC-MS1, TC-MS2, TC-MS3 |

---

## Gaps and follow-up work

- **Tier 1** (deferred): resolving `message`'s bytes to the specific
  extension type via a `(extendee_fqdn, type_id)` side-table, giving score
  credit for the extension's real fields instead of stopping at the opaque
  `message: bytes` leaf. Architecturally parallel to `Any`'s
  `extract_type_url`/type-registry mechanism (spec 0089), but keyed by
  `(container, type_id)` instead of a `type_url` string.
- The dead `MESSAGE_SET_NODE_ID` reservation in `graph.rs`/`hopcroft.rs`
  is left untouched; a future cleanup could remove it once Tier 1 (or a
  decision not to pursue it) is settled.
