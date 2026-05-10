<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0042 — Single-schema match score

**Status:** draft
**Implemented in:** —
**App:** prototext-core

---

## Background

`docs/schema-match.md` describes a future multi-schema matcher: given a
binary protobuf message of unknown schema and a corpus of ~100 k
`FileDescriptorProto`s, rank every candidate schema by how well it explains
the wire content.  The ranking is driven by three counters per schema:
`matches`, `unknowns`, `vetoed`.

Before building the multi-schema infrastructure, this spec delivers the
single-schema variant: `score_message(pb_bytes, schema) -> MatchScore`.
It is a stepping stone and a validation of the scoring model, not the final
design.

### Longer-term context

Eventually `prototext -d` may gain a **scoring mode** alongside its current
**rendering mode**.  The scoring mode will run the multi-schema parallel
walk described in `schema-match.md`, using a pre-compiled, deduplicated
schema graph rather than live `prost-reflect` descriptors.

That future walk will only need a stripped-down view of each schema — far
less information than full rendering requires.  This spec identifies
precisely what that stripped-down view must contain (see §Schema information
required for scoring).

---

## Goals

1. Add `pub struct MatchScore { matches: u64, unknowns: u64, vetoed: bool }`
   to `prototext-core`.
2. Add `pub fn score_message(pb_bytes: &[u8], schema: &ParsedSchema)
   -> MatchScore` to `prototext-core`.
3. The function is correct per the taxonomy in `schema-match.md`.
4. No changes to the CLI or to any existing public API.

## Non-goals

- Multi-schema parallel walk.
- New CLI flags or output changes.
- Ranking or comparison across schemas.
- Full group recursion (deferred; groups are uncommon).
- Replacing or refactoring the existing rendering walk.

---

## Specification

### §1 — MatchScore type

```rust
pub struct MatchScore {
    pub matches:  u64,
    pub unknowns: u64,
    pub vetoed:   bool,
}
```

Once `vetoed` is set, `matches` and `unknowns` are frozen at the values they
held at the moment of the veto — no further counting.

### §2 — score_message

```rust
pub fn score_message(pb_bytes: &[u8], schema: &ParsedSchema) -> MatchScore
```

**Phase 1 — walk**: call `ingest_pb(pb_bytes, schema, false)` (annotations
disabled).  This returns a `ProtoTextMessage` tree using the existing
rendering walk.

**Phase 2 — score**: recursively traverse the `ProtoTextMessage` tree and
derive `MatchScore` from the `ProtoTextContent` variants (see §3).

This two-phase approach is a deliberate prototype choice.  It reuses all of
the correctness work in the existing decoder without duplicating the walk.
The spec notes explicitly (see §Future work) that Phase 1 should eventually
be replaced by a direct scoring walk that avoids building the
`ProtoTextMessage` tree altogether.

**No-schema mode**: `ParsedSchema::empty()` gives `root_descriptor() = None`,
so `ingest_pb` treats every field as unknown.  The resulting tree contains
only `WireVarint`, `WireFixed64`, `WireFixed32`, `WireBytes`, `WireGroup`
content variants.  The scoring pass maps all of these to `unknowns`.
Result: `MatchScore { matches: 0, unknowns: N, vetoed: false }` where N is
the total number of wire fields encountered.

### §3 — Scoring rules (ProtoTextContent → MatchScore)

The scoring pass walks the `ProtoTextMessage` tree recursively, visiting
every `ProtoTextField` in every `MessageVal` sub-message.  At each field:

**Veto — stop immediately, freeze counters:**

| Content variant | Reason |
|---|---|
| `InvalidTagType` | Wire type 6 or 7; buffer uninterpretable |
| `InvalidVarint` | Truncated or overflowing varint |
| `InvalidFixed64` | Fewer than 8 bytes for a 64-bit field |
| `InvalidFixed32` | Fewer than 4 bytes for a 32-bit field |
| `InvalidBytesLength` | Truncated length prefix |
| `TruncatedBytes` | Content shorter than declared length |
| `InvalidGroupEnd` | END_GROUP outside a group |
| `InvalidString` | `string` field with invalid UTF-8 |
| `InvalidPackedRecords` | Packed repeated field with undecodable elements |
| `proto2_has_type_mismatch == true` | Wire-type / proto-type conflict (varint path) |

**Unknown — `unknowns += 1`:**

| Content variant | Reason |
|---|---|
| `WireVarint` | Field number absent from schema (no schema, or schema does not declare this field number); wire type 0 |
| `WireFixed64` | Same, wire type 1 |
| `WireBytes` | Same, wire type 2 — field not declared as message/string/bytes/packed by schema, or no schema |
| `WireFixed32` | Same, wire type 5 |
| `WireGroup` | Same, wire type 3; the group content is not recursed into |

Note: `WireBytes` is also produced by `decode_len_field` when the schema
declares the field as a non-LEN proto-type (e.g. `int32`) but the wire type
is `WT_LEN` — this is a wire-type / proto-type mismatch.  Currently the
decoder does not set `proto2_has_type_mismatch` in this path (it is only set
for the varint path in `codec.rs`).  **This spec requires that the decoder
be fixed to also set `proto2_has_type_mismatch = true` when `decode_len_field`
falls through to its mismatch fallback** (the final `WireBytes` at line 208
of `packed.rs`), so that the scoring pass can treat it as a veto rather than
an unknown.  This is the only change required to the existing decoder.

**Match — `matches += 1`:**

All other content variants indicate that the field was declared by the schema
and the wire content was compatible.  These are, exhaustively:

`Double`, `Float`, `Int64`, `Uint64`, `Int32`, `PFixed64`, `PFixed32`,
`Bool`, `StringVal`, `Group` (schema-declared group, schema matched),
`MessageVal`, `BytesVal`, `Uint32`, `Enum`, `Sfixed32`, `Sfixed64`,
`Sint32`, `Sint64`,
`Doubles`, `Floats`, `Int64s`, `Uint64s`, `Int32s`, `Fixed64s`, `Fixed32s`,
`Bools`, `Uint32s`, `Enums`, `Sfixed32s`, `Sfixed64s`, `Sint32s`, `Sint64s`.

**Recursion**: when the content is `MessageVal(inner)`, after counting the
match for the enclosing field, recurse into `inner` and continue counting.
`WireGroup` is NOT recursed into in this spec (the group content is opaque
at the scoring level).  `Group` (schema-declared) counts as a match but its
contents are also not recursed into in this spec.

### §4 — Decoder fix required

In `prototext-core/src/decoder/packed.rs`, `decode_len_field`, the final
fallback branch (currently `field.content = ProtoTextContent::WireBytes(...)`)
is reached when `field_schema` is `Some` but the schema-declared kind is not
string, bytes, or message.  This is a wire-type / proto-type mismatch.

Fix: also set `field.proto2_has_type_mismatch = true` in that branch,
matching the treatment in `codec.rs`.

### §5 — Tests

Unit tests in `prototext-core` (Rust):

- **TC-1 all-match**: message with two declared fields, both wire-compatible
  → `MatchScore { matches: 2, unknowns: 0, vetoed: false }`.
- **TC-2 all-unknown**: message with no schema (`ParsedSchema::empty()`) →
  `MatchScore { matches: 0, unknowns: N, vetoed: false }`.
- **TC-3 mixed**: some declared fields, some undeclared →
  correct split of `matches` and `unknowns`, `vetoed: false`.
- **TC-4 wire-type mismatch on declared field** (varint path): schema
  declares `bool`, wire carries a varint > 1 → `vetoed: true`, counters
  frozen.
- **TC-5 invalid UTF-8 on string field** → `vetoed: true`.
- **TC-6 LEN mismatch on declared field**: schema declares `int32` (varint),
  wire carries `WT_LEN` → `vetoed: true` (requires the §4 decoder fix).
- **TC-7 sub-message recursion**: root message has a declared `message`
  field; sub-message has its own declared fields.  Counters accumulate across
  both levels.
- **TC-8 wire invalid** (`InvalidVarint` / `TruncatedBytes`) → `vetoed: true`.

---

## Schema information required for scoring

This section identifies the minimal schema data that a future scoring walk
would need.  It is derived by tracing exactly which schema queries drive
the scoring outcomes above.

The existing rendering walk consults `prost-reflect` for:

1. **`MessageDescriptor::get_field(field_number) -> Option<FieldDescriptor>`**
   — presence/absence of a field number in the current message type.
   Absence → unknown.  Presence → proceed to wire-type check.

2. **`FieldDescriptor::kind() -> Kind`** — the proto-type of a declared
   field.  Used to determine:
   - Which wire type is expected (to detect mismatches).
   - Whether to recurse into a `WT_LEN` blob (only for `Kind::Message`).
   - How to interpret the content (for rendering; irrelevant to scoring).

3. **`FieldDescriptor::is_group() -> bool`** (via `Kind::Message` check on
   a group field) — whether a `WT_START_GROUP` field has a schema-declared
   group type.

4. **`FieldDescriptor::cardinality() -> Cardinality` and
   `FieldDescriptor::is_packed() -> bool`** — to detect packed repeated
   fields.  Used in `decode_len_field` to route to `decode_packed`.

5. **`Kind::Message(MessageDescriptor)`** — the nested `MessageDescriptor`
   to recurse into.

For **scoring**, items 2–4 reduce to a much smaller set.  The rendering code
uses `kind()` to pick the right decoded representation (Int32, StringVal,
etc.); the scorer only needs to answer two questions:

**Q1: Is the wire type compatible with the declared proto-type?**
This is a function of `(proto_type_group, wire_type)` only, where
`proto_type_group` is one of five buckets:

| Group | Members | Expected wire type |
|---|---|---|
| `Varint` | INT32, INT64, UINT32, UINT64, SINT32, SINT64, BOOL, ENUM | 0 |
| `I64` | FIXED64, SFIXED64, DOUBLE | 1 |
| `Len` | STRING, BYTES, MESSAGE, packed-repeated | 2 |
| `Group` | GROUP | 3 |
| `I32` | FIXED32, SFIXED32, FLOAT | 5 |

**Q2: Should we recurse into a `WT_LEN` blob?**
Only when the declared proto-type is `Message` (not `String`, `Bytes`, or
packed-repeated).

**Q3: If recursing, what is the nested `MessageDescriptor`?**
The child message type for the nested walk.

**Q4: Is a `string` field's content valid UTF-8?**
This is a check on the wire bytes, not on the schema — but it is only
triggered when the schema declares `Kind::String`.  So the scorer needs to
know whether a field is declared as `String` (to trigger the check), but
needs nothing else about the string type.

### Minimal schema representation for scoring

The above analysis shows that the only per-field schema information a
scoring walk needs is:

```
enum ScoringKind {
    Varint,          // INT32, INT64, UINT32, UINT64, SINT32, SINT64, BOOL, ENUM
    I64,             // FIXED64, SFIXED64, DOUBLE
    LenString,       // STRING — triggers UTF-8 veto
    LenBytes,        // BYTES — match, no recursion
    LenMessage(StateID),  // MESSAGE — match, recurse into child state
    LenPacked,       // packed repeated — match, no recursion, no element check
    GroupField,      // GROUP — match, no recursion
    I32,             // FIXED32, SFIXED32, FLOAT
}

struct ScoringField {
    field_number: u32,
    kind:         ScoringKind,
}

struct ScoringState {
    fields: Vec<ScoringField>,  // sorted by field_number for binary search
}
```

A scoring schema is then a collection of `ScoringState`s (one per message
type), with one root `StateID`.  After Hopcroft deduplication, structurally
equivalent message types share a single `ScoringState`.

This is strictly less information than a full `FieldDescriptor`:

- No field names.
- No enum value tables.
- No `oneof` membership.
- No default values.
- No options (beyond `packed`).
- No source info.
- Proto-type collapsed to a 3-bit enum (8 values cover all cases).

The compressed schema is therefore amenable to the compact, mmap-friendly
layout described in `schema-match-impl-notes.md`.

---

## Future work

- **Direct scoring walk**: replace the `ingest_pb` + tree-traversal approach
  with a direct walk that accumulates `MatchScore` without building a
  `ProtoTextMessage`.  This is the prerequisite for the multi-schema parallel
  walk.  The visitor/callback seam needed to share the wire-traversal loop
  between rendering and scoring should be introduced at that point.
- **Multi-schema parallel walk**: extend `score_message` to accept a set of
  candidate schemas and propagate all of them simultaneously, as described
  in `schema-match.md`.
- **Group recursion**: score fields inside schema-declared groups.
- **Packed element-level veto**: validate individual varint elements inside
  packed fields for range (BOOL > 1, INT32 ≥ 2³², etc.).
