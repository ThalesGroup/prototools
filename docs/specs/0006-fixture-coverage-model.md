<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0006 — Fixture coverage model and gap-filling fixtures

**Status:** implemented
**App:** prototext
**Implemented in:** 2026-03-12

## Problem

The fixture set in `fixtures/` was assembled incrementally, each fixture
motivated by a specific bug or feature.  There is no explicit coverage model
stating which inputs the fixtures are *intended* to cover, nor any systematic
record of what is *deliberately* left uncovered.

Without a coverage model:
- It is impossible to evaluate whether a new proto feature or code-path change
  is adequately tested.
- When external contributors add fixtures, there is no reference model.
- The analysis also identifies concrete gaps: code paths in `render_text.rs`,
  `encode_text.rs`, and `schema.rs` that are currently exercised by no fixture.

## Goals

- Define an explicit coverage model: the space of proto inputs and the
  dimensions along which it is partitioned.
- Audit every dimension against the existing fixtures and document the
  coverage.
- Add new fixtures for the identified gaps (see §4).

## Non-goals

- Achieving exhaustive coverage of every possible proto encoding.
- Adding new proto schemas (all new fixtures use the existing `knife.proto` /
  `SwissArmyKnife`, `SwissArmyKnifeRq`, or `enum_collision.proto` /
  `EnumCollision` schemas, except for the nesting addition in §4.5).
- Benchmarking or performance testing.

---

## Specification

### 1. Coverage model

A prototext fixture is a pair `(wire_bytes, schema)`.  The fixture exercises
the decoder (`render_text.rs`) and the encoder (`encode_text.rs`) in sequence
via the round-trip invariant `wire → text → wire' == wire`.

The space of proto inputs is partitioned along the following **six dimensions**:

#### Dimension A — Wire type

Every proto encoding maps to one of six wire types on the wire:

| A-code | Wire type | Value | Proto field types |
|---|---|---|---|
| A0 | `VARINT` | 0 | int32, int64, uint32, uint64, bool, enum, sint32, sint64 |
| A1 | `I64` | 1 | fixed64, sfixed64, double |
| A2 | `LEN` | 2 | string, bytes, message, packed repeated |
| A3 | `SGROUP` | 3 | group (start) |
| A4 | `EGROUP` | 4 | group (end) |
| A5 | `I32` | 5 | fixed32, sfixed32, float |

#### Dimension B — Schema relationship

| B-code | Relationship | Meaning |
|---|---|---|
| B0 | Unknown | Field number not in schema |
| B1 | Mismatch | Field number known but wire type ≠ declared type |
| B2 | Known | Field number and wire type match schema |

#### Dimension C — Field cardinality

| C-code | Cardinality | Meaning |
|---|---|---|
| C0 | Optional | `optional` field (default in proto3; explicit in proto2) |
| C1 | Repeated unpacked | `repeated` field, one record per value |
| C2 | Repeated packed | `repeated ... [packed=true]`, values length-prefixed |
| C3 | Required | `required` field (proto2 only) |

#### Dimension D — Encoding anomaly

| D-code | Anomaly | Meaning |
|---|---|---|
| D0 | Canonical | Minimal, spec-compliant encoding |
| D1 | Overhang | Extra bytes in tag or value varint (non-minimal encoding) |
| D2 | Truncated | Varint or bytes field terminates early / is missing |
| D3 | Out-of-range tag | Field number ≥ 2^29 (protobuf limit) |
| D4 | Truncated negative | int32/enum encoded as 5-byte (proto2 quirk) |
| D5 | Invalid packed | Corrupt varint record inside packed array |

#### Dimension E — Nesting depth

| E-code | Depth | Meaning |
|---|---|---|
| E0 | Flat | No sub-messages or groups |
| E1 | One level | Field is a message or group |
| E2 | Two levels | Nested message inside message/group |

#### Dimension F — Enum-specific (applies only to A0 enum fields)

| F-code | Enum condition | Meaning |
|---|---|---|
| F0 | Known value | Decoded integer is in `enum_values` table |
| F1 | Unknown value | Decoded integer is not in `enum_values` table (ENUM_UNKNOWN) |
| F2 | Zero value | Decoded integer is 0 (proto default; must not be confused with "unset") |
| F3 | Negative value | Enum constant has a negative numeric value (proto2 allows this) |
| F4 | Primitive-name collision | Enum type name matches a proto primitive keyword (e.g. `float`) |
| F5 | Mixed in packed | A single packed array contains both known and unknown values |

### 2. Coverage audit

**Enum coverage (A0 × F × C):**

| | F0 known | F1 unknown | F2 zero | F3 negative | F4 collision | F5 mixed-packed |
|---|---|---|---|---|---|---|
| C0 optional | ✓ `enum_collision_color_known` | ✓ `enum_collision_color_unknown` | ✓ `num_enum_zero` | ✗ | ✓ `enum_collision_float_kind` | ✗ |
| C1 repeated | ✓ `enum_collision_color_repeated` | ✗ | ✗ | ✗ | ✗ | ✗ |
| C2 packed | ✓ `enum_collision_color_packed` | ✗ | ✗ | ✗ | ✗ | ✗ |
| C3 required | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |

Note: `num_enum_{zero,one,neg_one}` use `enumOp` which is declared `int32` in
`knife.proto`, not a real protobuf `enum` type — they exercise the varint path
but NOT the `binary_search_by_key` symbolic-lookup path in `render_text.rs`.

**Enum in nested context:**

| | Flat (E0) | In message (E1) | In group (E1) |
|---|---|---|---|
| Enum field | ✓ all `enum_collision_*` | ✗ | ✗ |

**Varint types (A0 × B2 × C0):**

All proto varint scalar types covered: int32, int64, uint32, uint64, bool,
sint32, sint64. ✓

**Varint boundary values:**

| Type | Min | Max | Zero | Notes |
|---|---|---|---|---|
| sint32 | ✓ `num_sint32_min` | ✓ `num_sint32_max` | ✓ | |
| sint64 | ✓ `num_sint64_min` | ✓ `num_sint64_max` | ✗ | |
| int32 | ✓ | ✓ | ✓ | |
| int64 | ✓ | ✓ | ✓ | |
| uint32 | ✓ | ✓ | ✓ | |
| uint64 | ✓ | ✓ | ✓ | |

**String escape sequences:**

`escape_string_into()` (`serialize/common.rs`) handles `\n`, `\t`, `\r`, `\"`,
`\'`, `\\`, and octal `\NNN`.  Covered by `string_escapes`. ✓

**Packed arrays:**

| Type | Non-empty canonical | Empty | With unknown enum |
|---|---|---|---|
| int32 | ✓ | ✓ `test_varint_packed` | N/A |
| enum (real type) | ✓ `enum_collision_color_packed` | ✓ `enum_collision_empty_packed` | ✓ `enum_collision_packed_mixed` |
| sint32 | ✓ | ✗ | N/A |
| sint64 | ✓ | ✗ | N/A |

### 3. Deliberate non-goals of the fixture set

The following combinations are **intentionally not covered** by fixtures
because they are either impossible in valid protobuf, covered by fuzz testing,
or represent proto3-only semantics outside the current scope:

- Wire type A3/A4 (group) with anomaly D3 (out-of-range tag) — redundant with
  non-group coverage.
- Proto3 optional / oneof semantics — `knife.proto` is proto2.
- Field numbers > 536,870,911 (2^29 − 1) — impossible per spec; `TAG_OOR`
  covers the boundary.
- Enum alias (two constants with same numeric value) — the lookup is
  deterministic (first match after sort); no distinct code path.
- Empty enum (zero constants) — schema.rs produces an empty `enum_values`
  slice and the field degrades gracefully to numeric rendering.

### 4. Fixture definitions

#### 4.1 `num_sint32_min`, `num_sint64_max`, `num_sint64_min`

**Gap:** `sint64` coverage is limited to `neg_one` and `neg_128`.  The
zigzag codec has boundary behaviour at INT64_MIN / INT64_MAX.

- `num_sint32_min`: SInt32 `−2147483648` → zigzag `4294967295` (u32 max)
- `num_sint64_max`: SInt64 `9223372036854775807` → zigzag `18446744073709551614`
- `num_sint64_min`: SInt64 `−9223372036854775808` → zigzag `18446744073709551615` (u64 max)

Schema: `SwissArmyKnife`

#### 4.2 `enum_collision_color_unknown_repeated`

**Gap:** Unknown enum value (`ENUM_UNKNOWN`) in a repeated (non-packed) field.
The existing `enum_collision_color_unknown` uses an optional field.  The
repeated case exercises a different code path in `render_text.rs`.

Values: `colors = [0, 99, 2]` — RED known, 99 unknown, BLUE known.

Schema: `EnumCollision`

#### 4.3 `enum_collision_packed_mixed`

**Gap:** Packed array where some elements are known enum values and others are
unknown.

Values: `colors_pk = [0, 99, 2]` — RED=0 known, 99 unknown, BLUE=2 known.

Schema: `EnumCollision`

#### 4.4 `enum_collision_empty_packed`

**Gap:** Empty packed array for a real enum field.

Values: `colors_pk = []` — zero elements.

Schema: `EnumCollision`

#### 4.5 `enum_in_nested_message`

**Gap:** Enum field inside a nested sub-message.  All existing enum fixtures
are flat.

A `nested` self-referential field is added to `enum_collision.proto`:

```proto
optional EnumCollision nested = 6;
```

Values: `nested.color = GREEN`, `nested.unknown_color = 99`.

Schema: `EnumCollision`

#### 4.6 `enum_in_group`

**Gap:** Enum field inside a proto2 group.

An `EnumGroup` group field is added to `enum_collision.proto`:

```proto
optional group EnumGroup = 7 {
  optional Color group_color = 1;
}
```

Values: `EnumGroup.group_color = BLUE`.

Schema: `EnumCollision`

#### 4.7 `string_escapes`

**Gap:** No fixture exercises the string escape-sequence paths in
`escape_string_into()` (`serialize/common.rs`).

Value: `stringOp = "tab:\there\nnewline\\backslash\"quote"` — exercises `\t`,
`\n`, `\\`, `\"`.

Schema: `SwissArmyKnife`

#### 4.8 `string_escapes_bytes`

**Gap:** Bytes fields containing non-UTF-8 / non-printable byte values.

Value: `bytesOp = bytes(range(256))` — all 256 byte values.

Schema: `SwissArmyKnife`

### 5. Schema additions to `enum_collision.proto`

See §4.5 and §4.6 above.  The final schema is:

```proto
syntax = "proto2";

enum float { FLOAT_ZERO = 0; FLOAT_ONE = 1; FLOAT_TWO = 2; }

enum Color { RED = 0; GREEN = 1; BLUE = 2; }

message EnumCollision {
  optional float  kind          = 1;
  optional Color  color         = 2;
  optional Color  unknown_color = 3;
  repeated Color  colors        = 4;
  repeated Color  colors_pk     = 5 [packed=true];
  optional EnumCollision nested = 6;
  optional group EnumGroup = 7 {
    optional Color group_color = 1;
  }
}
```

---

## References

- `fixtures/index.toml` — fixture registry
- `docs/specs/0004-enum-annotation-syntax.md` — enum fixtures
- `prototext-core/src/serialize/render_text/varint.rs` — `render_varint_field`
- `prototext-core/src/serialize/render_text/packed.rs` — `render_packed`
- `prototext-core/src/serialize/encode_text/mod.rs` — encoder
- `prototext-core/src/serialize/encode_text/encode_annotation.rs` — `parse_field_decl_into`
- `prototext-core/src/schema.rs` — `FieldInfo.enum_values`
