<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0013 — Protocraft schema-aware builder

**Status:** implemented
**Implemented in:** 2026-03-24
**App:** prototext

---

## Problem

The current Rust protocraft builder (`prototext/src/protocraft/`) only accepts
numeric field identifiers.  `craft_a.rs` therefore consists of magic numbers
that are hard to read, impossible to verify against the schema without a
field-number lookup table, and easy to get wrong — as demonstrated by
repeated field-number bugs during the initial port of `craft_a.py`.

The Python protocraft (`builders.py`) is schema-aware: every `Message` carries
a `Descriptor` and resolves string field names to numbers at call time.  This
makes `craft_a.py` readable, auditable, and maintainable:

```python
with Message(schema=SwissArmyKnife) as test_varint_optional:
    UInt64('uint64Op', 0)
    Fixed64('fixed64Op', 0)
    Bytes('bytesOp', b'')
```

The Rust equivalent should be equally readable:

```rust
pub fn test_varint_optional() -> Vec<u8> {
    let mut m = Message::with_schema(KNIFE);
    m.uint64("uint64Op", 0u64);
    m.fixed64("fixed64Op", 0u64);
    m.bytes_("bytesOp", b"");
    m.build()
}
```

---

## Goals

1. Extend `Message` with an optional schema binding so that string field names
   are accepted wherever a field identifier is required.
2. Add a `FieldSpec` trait implemented for both `&str` (name lookup) and
   integer types (raw field number), so the same builder method accepts either.
3. Propagate the descriptor automatically into nested messages and groups.
4. Add `fixture!` / `msg_fields!` macros for compact, one-line-per-field syntax
   mirroring Python's `with Message(...):` blocks.
5. Rewrite `craft_a.rs` to use field names throughout, making it a
   near-line-for-line port of `craft_a.py`.

## Non-goals

- Runtime schema resolution outside of test code — protocraft remains
  test-only.
- Validating that the correct builder method is used for the declared field
  type (e.g., calling `double_` on an `int32` field).  The caller is
  responsible; protocraft is a wire-level tool that deliberately allows
  mismatched encodings.
- Group end-tag name resolution — end tags always use the same field number
  as the start tag, so no separate name is needed.

---

## Specification

### 1. Schema representation

Protocraft uses `prost_reflect::MessageDescriptor` (already available in the
workspace) as its schema type.  Two pre-built descriptors are provided as
constants for use in `craft_a.rs`:

```rust
/// Descriptor for SwissArmyKnife (knife.proto).
pub fn knife_descriptor() -> MessageDescriptor { ... }

/// Descriptor for EnumCollision (enum_collision.proto).
pub fn enum_collision_descriptor() -> MessageDescriptor { ... }
```

Each function builds a `MessageDescriptor` from the compiled `.pb` bytes
embedded via `include_bytes!(concat!(env!("OUT_DIR"), "/knife.pb"))` etc.

### 2. Schema-bound `Message`

Add an optional descriptor field to `Message`:

```rust
pub struct Message {
    buf: Vec<u8>,
    desc: Option<MessageDescriptor>,
}
```

Add a constructor:

```rust
impl Message {
    pub fn with_schema(desc: MessageDescriptor) -> Self {
        Message { buf: Vec::new(), desc: Some(desc) }
    }
}
```

`Message::new()` continues to work for schema-less messages.

### 3. `FieldSpec` trait

Builder methods that accept a field identifier use a new trait:

```rust
pub trait FieldSpec {
    /// Returns (field_number, nested_descriptor).
    /// nested_descriptor is Some only for message-typed fields resolved by name.
    fn resolve(self, desc: Option<&MessageDescriptor>) -> (u64, Option<MessageDescriptor>);
}
```

Implemented for:
- `&str` — resolves name against `desc`; panics if `desc` is `None` or name unknown.
- `u64`, `u32`, `i32`, `i64`, `usize` — field number passed through; nested descriptor is `None`.

The builder methods on `Message` change from `field: impl IntoTag` to
`field: impl FieldSpec`, using `self.desc.as_ref()` for resolution.

`IntoTag` is retained unchanged for the low-level `encode_tag` API.

### 4. `Tag` with named-field support

Add a `Tag::named` constructor for the rare cases where a field name and an
explicit wire-type override are both needed (mirroring Python's
`Tag('fieldName', type=2, ohb=N)`):

```rust
impl Tag {
    pub fn named(name: &'static str, wire_type: u8, ohb: u8) -> Self { ... }
}
```

`Tag` itself keeps its existing `field: u64` representation.  `Tag::named`
is a `FieldSpec` that resolves the name against the descriptor at call time
and then constructs the numeric `Tag` with the overridden wire type.

### 5. Descriptor propagation into nested messages and groups

- **`message(field, nested)`**: the `field`'s `message_type` descriptor is
  propagated into `nested` when `nested.desc` is `None`.
- **`group(start, end, nested)`**: the group field's own `message_type`
  descriptor is propagated into `nested` when `nested.desc` is `None`.
  In proto2, a group field defines its own message type containing the group's
  fields; those fields do not belong to the enclosing message's namespace.

When `nested` is constructed via `Message::with_schema`, that descriptor takes
precedence.

### 6. `fixture!` / `msg_fields!` macros

Two macros are added to `mod.rs` to allow compact, one-line-per-field syntax:

```rust
macro_rules! fixture { ... }
macro_rules! msg_fields { ... }
```

`fixture!(name, DESCRIPTOR; fields...)` expands to:

```rust
pub fn name() -> Vec<u8> {
    let mut _m = Message::with_schema(DESCRIPTOR);
    msg_fields!(_m, fields...);
    _m.build()
}
```

`msg_fields!` supports the following field forms:

| Syntax | Meaning |
|--------|---------|
| `uint64(field, val)` | varint field; `field` is `&str` or integer |
| `fixed64(field, val)` | fixed-64 field |
| `bytes_(field, val)` | length-delimited bytes |
| `uint32(field, val)` | varint field (32-bit) |
| `message(field; nested...)` | length-delimited message; `nested` uses field's `message_type` |
| `group(field; nested...)` | group start/end; `nested` uses parent's descriptor |

In all cases `field` is either a string literal (name lookup) or an integer
literal (raw field number, no lookup).

### 7. Rewrite of `craft_a.rs`

`craft_a.rs` is rewritten using the `fixture!` / `msg_fields!` macros.
The field-number reference comment at the top of the file is removed — field
names are self-documenting.

Each fixture becomes a near-line-for-line port of its Python counterpart:

```rust
fixture!(test_varint_optional, knife_descriptor();
    uint64("uint64Op", 0),
    fixed64("fixed64Op", 0),
    bytes_("bytesOp", b""),
    uint32("uint32Op", 0),
    enum_("enumOp", 0),
    sfixed32("sfixed32Op", 0),
    sfixed64("sfixed64Op", 0),
    sint32("sint32Op", 0),
    sint64("sint64Op", 0),
);
```

```rust
fixture!(test_proto2_level, knife_descriptor();
    uint64("uint64Rp", 0),
    fixed64("fixed64Rp", 0),
    bytes_("bytesRp", b""),
    group(4;
        uint64(11, 0),
    ),
    group("group";
        message("nested";
            fixed64("fixed64Rp", 0),
            bytes_("bytesRp", b""),
            uint32("uint32Rp", 0),
        ),
    ),
    uint32("uint32Rp", 0),
);
```

Fixtures that use `Tag(name, type=N, ohb=M)` in Python use `Tag::named` in
the low-level builder call (outside the macro):

```rust
// Python: Double(Tag('doublePk', length=7), math.pi)
m.raw_len_field(Tag::named("doublePk", 2, 0), 7, &f64::consts::PI.to_le_bytes());
```

Fixtures using intentionally raw wire-level field numbers (e.g. `test_wire_level`)
continue to use integers directly.

---

## Impact on existing code

- `prototext/src/protocraft/mod.rs` — extended with `FieldSpec`,
  `Message::with_schema`, descriptor propagation, `fixture!` / `msg_fields!` macros.
- `prototext/src/protocraft/craft_a.rs` — rewritten; all symbolic field names
  restored; field-number comment removed.
- No changes to `prototext-core` or production code.
- The existing `prototext/tests/e2e.rs` tests are unaffected.

---

## References

- `../../code/prototools/src/protocraft/builders.py` — Python reference
  implementation
- `../../code/prototools/src/protocraft/craft_a.py` — Python fixture
  definitions
- `prototext/fixtures/schemas/knife.proto` — SwissArmyKnife schema
- `prototext/fixtures/schemas/enum_collision.proto` — EnumCollision schema
- `prototext-core/src/schema/` — existing `prost_reflect` integration
