<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0004 — Enum annotation syntax and `#@` delimiter

**Status:** implemented
**App:** prototext
**Implemented in:** 2026-03-11

## Problem

Three related issues affect the annotation syntax emitted by the `protoc`
kernel:

1. **Ambiguous delimiter.**  Annotations use `#` — the standard proto comment
   character — with no visual distinction from free-form human comments.
   This makes it hard for readers (and future parsers) to recognise that
   annotations are machine-generated and carry semantics.

2. **Numeric enum values.**  When a field with an enum type is decoded, the
   emitted value is the raw integer (`9`), not the symbolic constant
   (`TYPE_STRING`).  The original intent was to match `protoc --decode` output,
   but `protoc --decode` actually emits the symbolic name.  Using the numeric
   value is therefore both a divergence from the reference and a usability
   regression.

3. **Latent encoder bug: enum type name collides with primitive name.**  The
   encoder (`encode_text.rs`) dispatches on the type token in the annotation to
   determine the wire encoding.  For unrecognised tokens it falls back to raw
   varint — which is correct for enums by coincidence.  However, proto syntax
   permits enum type names that collide with primitive keywords (e.g.
   `enum float { … }`).  In that case the encoder would match the `"float"` arm
   and silently emit a fixed32 value instead of a varint, producing wrong wire
   bytes.  The parenthesised format introduced in this spec eliminates this bug
   (see §5.1).

Current output (erroneous, field number 5, numeric enum value 9):
```
type: 9  # Type = 5
```

Target output after this spec:
```
type: TYPE_STRING  #@ Type(9) = 5
```

Reading the annotation: `Type(9)` = enum type `Type`, raw wire value `9`;
` = 5` = field number 5.

## Goals

- Replace the `#` annotation delimiter with `#@` throughout.
- For enum fields, emit the symbolic constant name as the field **value**
  instead of the raw integer.
- In the annotation, record the original numeric value as `EnumType(numeric)`
  so the annotation is self-contained and the round-trip can reconstruct the
  wire bytes without re-resolving names.
- The output format remains lossless: `encode` can reconstruct the exact wire
  bytes from the new textual form.
- Fix the latent encoder bug where an enum type named after a primitive keyword
  would be mis-encoded.

## Non-goals

- Changing any other annotation token (wire-type labels, modifiers, etc.).
- Changing the value representation for unknown enum values beyond what §3
  specifies (they use `ENUM_UNKNOWN`, not a full unknown-field fallback).
- Changing the `#@ prototext: protoc` magic header line (it already uses `#@`).

---

## Specification

### 1. Delimiter change: `#` → `#@`

Every field-level annotation currently starts with `  # ` (two spaces, hash,
space).  After this change it starts with `  #@ ` (two spaces, hash, at-sign,
space).

This applies to **all** annotation tokens — wire-type labels, field
declarations, modifiers — not just enum annotations.

Examples:

| Before | After |
|---|---|
| `9: 5  # varint` | `9: 5  #@ varint` |
| `name: "hello"  # string = 3` | `name: "hello"  #@ string = 3` |
| `9: 5  # varint; TAG_OOR` | `9: 5  #@ varint; TAG_OOR` |
| `items: 3  # repeated int32 = 2` | `items: 3  #@ repeated int32 = 2` |

The separator between annotation tokens remains `"; "` (semicolon-space).

### 2. Enum field rendering

When a field has proto type `ENUM` **and** the schema contains a value name for
the decoded integer:

- **Field value:** emit the symbolic constant name (e.g. `TYPE_STRING`).
- **Annotation:** emit the field declaration with the enum type short name,
  followed by the numeric value in parentheses, then ` = <field_number>`.

Full format for a known enum value:

```
<field_name>: <SYMBOLIC_NAME>  #@ <EnumTypeName>(<numeric>) = <field_number>
```

Examples (field number 5, enum type `Type`, numeric value 9 = `TYPE_STRING`):

```
type: TYPE_STRING  #@ Type(9) = 5
```

For a repeated enum field (field number 2, type `Label`, value 1 = `LABEL_OPTIONAL`):

```
label: LABEL_OPTIONAL  #@ repeated Label(1) = 2
```

For a packed repeated enum field:

```
label: LABEL_OPTIONAL  #@ repeated Label(1) [packed=true] = 2
```

The parenthesised numeric suffix — `(<numeric>)` — is unique to enum
annotations.  No other annotation token uses parentheses, so the format is
unambiguous.

#### Field declaration structure for enums

For non-enum known fields the annotation contains:

```
[repeated |required ]<type_or_display_name>[ [packed=true]] = <field_number>
```

For enum known fields the annotation contains:

```
[repeated |required ]<EnumTypeName>(<numeric>)[ [packed=true]] = <field_number>
```

where `<EnumTypeName>` is the short (unqualified) enum type name (last
component of the fully-qualified type name).

#### `optional` label

`optional` continues to be omitted as the default label.

### 3. Unknown enum values — `ENUM_UNKNOWN` modifier

When the decoded integer is **not present in the enum's value table** in the
schema (an unrecognised value), the field is rendered with:

- **Field value:** the raw integer (cannot emit a symbolic name).
- **Annotation:** `EnumType(numeric) = field_number; ENUM_UNKNOWN`.

```
type: 99  #@ Type(99) = 5; ENUM_UNKNOWN
```

#### Casing rationale

`ENUM_UNKNOWN` uses ALL_CAPS to match the existing token convention for anomaly
flags (`TYPE_MISMATCH`, `TAG_OOR`, `TRUNCATED_BYTES`): the wire type is
correct (varint), the field is schema-known, but the value is outside the
declared enum set — a semantic anomaly the user should notice.

#### Round-trip for unknown enum values

The encoder sees the raw integer `99` as the value token and encodes it
directly as a varint.  The `ENUM_UNKNOWN` modifier is ignored by the encoder
(comments-are-stripped rule applies).  Round-trip is lossless.

### 4. Schemaless / unknown field rendering

When no schema is available (schemaless mode), enum fields cannot be
identified; they are emitted as plain `varint` with the numeric value:

```
9: 5  #@ varint
```

### 5. Round-trip (`encode`)

The encoder already carries the numeric value in the annotation (`Type(9)`),
so it does **not** need to resolve symbolic names via a schema lookup.  The
encode path works as follows:

- `split_at_annotation` splits the line into value-part and annotation-string.
  After the delimiter change, it looks for `  #@ ` instead of `  # `; the
  SIMD-accelerated `memrchr(b'#')` + surrounding-byte verification is
  preserved unchanged, with the verification pattern updated from
  `b[p+1] == b' '` to `b[p+1] == b'@' && b[p+2] == b' '`.
- `parse_annotation` / `parse_field_decl_into` parse the annotation string.
  For enum fields the field-type token now has the form `Type(9)` rather than
  `Type`; the parser scans for `(` to split the type name from the embedded
  numeric, which it stores as the effective field value.  No allocation; the
  scan operates on the existing `&str` slice.
- The value-part token (`TYPE_STRING` or `99`) is **ignored for encoding
  purposes** — the numeric extracted from `Type(9)` in the annotation is used
  instead.
- `ENUM_UNKNOWN` in the annotation is silently ignored by the encoder (falls
  into the existing catch-all `_ => {}` branch).

Consequence: the encoder requires **no schema access** and **no name-resolution
logic**.  Lossless round-trip is guaranteed by the annotation carrying the
numeric value explicitly.

### 5.1 Enum type name vs primitive name disambiguation

The encoder's `encode_num` function dispatches on `ann.field_type` (a `&str`
extracted from the annotation) to select the wire encoding:

| `field_type` token | Wire encoding |
|---|---|
| `"double"`, `"fixed64"`, `"sfixed64"` | fixed 64-bit |
| `"float"`, `"fixed32"`, `"sfixed32"` | fixed 32-bit |
| `"sint32"`, `"sint64"` | zigzag varint |
| `"bool"` | varint (masked to 1 bit) |
| `"int32"`, `"enum"` | varint (with truncation flag) |
| `"uint32"`, `"int64"`, `"uint64"`, … | varint |
| anything else (`_`) | varint fallback |

Before this spec, enum fields emit e.g. `Label = 4` in the annotation.  If an
enum is named `float`, the annotation would be `float = 4`, and the encoder
would match the `"float"` arm and emit a fixed32 value.

After this spec, enum fields emit `Label(1) = 4`.  The `(` character cannot
appear in any primitive type name, so `parse_field_decl_into` detects `(`
unconditionally and routes it through the varint path.  The primitive dispatch
table is never consulted for enum fields.

### 6. Schema changes — Rust

`FieldInfo` gains one new field (in `prototext-core/src/schema.rs`):

```rust
/// Numeric value → symbolic name table for ENUM fields.
/// Populated at schema-parse time; empty for non-ENUM fields.
/// Sorted by numeric value for O(log n) lookup via binary_search_by_key.
pub enum_values: Box<[(i32, Box<str>)]>,
```

**Data structure rationale:**
- `Box<[…]>` (boxed slice) rather than `Vec<…>`: the table is built once and
  never mutated; saves 8 bytes per `FieldInfo` and communicates immutability.
- `Box<str>` rather than `String`: saves 8 bytes per entry (no capacity word).
- Sorted slice with `binary_search_by_key` rather than `HashMap<i32, …>`: enum
  value sets are small (typically 5–20 entries); contiguous `i32` keys fit in
  a single cache line and avoid hash-table overhead.

#### Build procedure

`build_message_schema` is updated with a two-pass approach:

1. **Collect enums**: walk all `EnumDescriptorProto` entries in all
   `FileDescriptorProto` files, building a temporary
   `HashMap<String, Vec<(i32, Box<str>)>>` keyed by fully-qualified enum type
   name (e.g. `.google.protobuf.FieldDescriptorProto.Type`).  Sort each
   `Vec` by numeric value.
2. **Resolve per field**: for each `FieldInfo` with `proto_type == ENUM`, look
   up `enum_type_name` in the temporary map, sort the entries by `i32` key,
   and store as `Box<[(i32, Box<str>)]>`.  Fields with an unresolvable enum
   type get an empty slice.

---

## Additional implementation hazards

### `ENUM_UNKNOWN` silencing in `parse_annotation`

In `encode_text.rs`, `parse_annotation` handles bare tokens (no `:`, no `=`)
with a `match token` that explicitly silences `"TAG_OOR"`, `"ETAG_OOR"`, and
`"TYPE_MISMATCH"`.  Everything else falls to `_ => ann.wire_type = token`,
which would set `ann.wire_type = "ENUM_UNKNOWN"`.  The fix is to add
`"ENUM_UNKNOWN"` to the explicit ignore list:

```rust
"TAG_OOR" | "ETAG_OOR" | "TYPE_MISMATCH" | "ENUM_UNKNOWN" => {}
```

### Bounds check update in `split_at_annotation`

The current bounds check after `memrchr(b'#')` is:

```rust
p + 1 < b.len() && b[p + 1] == b' '
```

After the change:

```rust
p + 2 < b.len() && b[p + 1] == b'@' && b[p + 2] == b' '
```

The bound must increase from `p + 1` to `p + 2` to avoid an out-of-bounds
read when `#` is the second-to-last byte of the line.

### Packed enum decoder — structural change

The existing `decode_packed_to_str` / `decode_packed_varints_to_str` functions
return a single formatted `String` such as `"[1, 2, 3]"` for the entire value
list.  For enum fields, each integer must become a symbolic name; the numeric
values must additionally be collected for the annotation.

`decode_packed_varints_to_str` is extended to carry a parallel `Vec<i32>` of
raw numeric values for enum fields.  `render_packed` then:

- Writes the symbolic-name list as the field value: `[LABEL_OPTIONAL,
  LABEL_REQUIRED]`.
- Writes `repeated Label([1, 2]) [packed=true] = N` in the annotation.

For elements not found in `fi.enum_values`, the raw integer is emitted at that
position and `; ENUM_UNKNOWN` is appended (one modifier covers all unknown
values — no per-element flag).

### Packed enum encoder — value list ignored

`encode_packed_array_line` iterates over the comma-separated elements of the
`[v1, v2, …]` LHS list, calling `parse_num(elem)` for each element.  After
this spec the LHS list will contain symbolic names (e.g. `[LABEL_OPTIONAL,
LABEL_REQUIRED]`), for which `parse_num` returns `None`.

The fix: the encoder ignores the LHS value list for enum fields and instead
extracts the numeric values from `Label([1, 2])` in the annotation.  A new
`Ann` field (`enum_packed_values: Vec<i64>`) is populated by
`parse_field_decl_into` when it detects the `([…])` form.

### Truncated-negative enum values

A packed or scalar enum field with a truncated 5-byte negative value is
annotated by the decoder with a `truncated_neg` modifier.  This modifier is
used by the encoder to select the 5-byte encoding path.

After this spec the value is rendered symbolically (if the decoded `i32` is in
`enum_values`) or as the raw `i32` (if unknown).  The `truncated_neg` modifier
in the annotation continues to carry the encoding information.  No change is
needed for this case.

---

## New test schema: `enum_collision.proto`

A new proto schema `fixtures/schemas/enum_collision.proto` contains:

```proto
syntax = "proto2";

// An enum whose name collides with a primitive keyword.
// Under the old annotation format this would be encoded as fixed32 (wrong).
// Under the new format the (N) suffix makes it unambiguously a varint.
enum float {
  FLOAT_ZERO  = 0;
  FLOAT_ONE   = 1;
  FLOAT_TWO   = 2;
}

// A normal enum with a non-colliding name, for the happy-path and
// ENUM_UNKNOWN cases.
enum Color {
  RED   = 0;
  GREEN = 1;
  BLUE  = 2;
}

message EnumCollision {
  optional float  kind      = 1;  // enum named after primitive keyword
  optional Color  color     = 2;  // normal enum, known value
  optional Color  unknown_color = 3;  // populated with value 99
  repeated Color  colors    = 4;
  repeated Color  colors_pk = 5 [packed=true];
  optional EnumCollision nested = 6;   // for nesting tests
  optional group EnumGroup = 7 {       // for group + enum tests
    optional Color group_color = 1;
  }
}
```

The compiled `.pb` descriptor lives at `fixtures/schemas/enum_collision.pb`.

---

## Fixture coverage

Four core fixtures exercise this spec's paths:

| Fixture name | Purpose |
|---|---|
| `enum_collision_float_kind` | Enum named `float` — exercises the primitive-keyword collision path |
| `enum_collision_color_known` | Normal enum, value present in schema — happy path |
| `enum_collision_color_unknown` | Normal enum, value `99` not in schema — exercises `ENUM_UNKNOWN` |
| `enum_collision_color_packed` | Packed repeated enum |

All four must pass the round-trip invariant:
```
wire → [decode] → text → [encode] → wire'
assert wire' == wire
```

---

## References

- [`docs/annotation-format.md`](../annotation-format.md) — annotation grammar
- `prototext-core/src/schema.rs` — `FieldInfo`, `MessageSchema`, `ParsedSchema`
- `prototext-core/src/serialize/render_text/mod.rs` — `AnnWriter`, annotation format
- `prototext-core/src/serialize/render_text/varint.rs` — `render_varint_field`
- `prototext-core/src/serialize/render_text/packed.rs` — `render_packed`
- `prototext-core/src/serialize/encode_text/mod.rs` — encoder
- `prototext-core/src/serialize/encode_text/encode_annotation.rs` — `parse_field_decl_into`
- `fixtures/index.toml` — fixture registry
