<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# Protocraft fixture macro reference

This document describes the `fixture!` / `msg_fields!` macros used to write
protobuf binary fixtures in `prototext/src/protocraft/craft_a.rs`.

Both macros are test-only (`#[cfg(test)]`) and are defined in
`prototext/src/protocraft/mod.rs`.

---

## `fixture!`

```
fixture!(name, descriptor_expr;
    field_entry,
    field_entry,
    ...
);
```

Expands to:

```rust
pub fn name() -> Vec<u8> { ... }
```

The function builds a top-level message bound to `descriptor_expr`, appends
every `field_entry` to it in order, and returns the raw wire bytes.

`descriptor_expr` is any Rust expression that returns a `MessageDescriptor`.
The pre-built helpers are:

| Expression | Schema | Root message |
|---|---|---|
| `super::knife_descriptor()` | `knife.proto` | `SwissArmyKnife` |
| `super::knife_rq_descriptor()` | `knife.proto` | `SwissArmyKnifeRq` |
| `super::enum_collision_descriptor()` | `enum_collision.proto` | `EnumCollision` |
| `super::schema_simple_descriptor()` | `knife.proto` | `SchemaSimple` |
| `super::schema_overhang_descriptor()` | `knife.proto` | `SchemaOverhang` |
| `super::schema_interleaved_descriptor()` | `knife.proto` | `SchemaInterleaved` |
| `super::schema_hidden_descriptor()` | `knife.proto` | `SchemaHidden` |
| `super::fdp_descriptor()` | `descriptor.pb` | `google.protobuf.FileDescriptorProto` |

A trailing comma after the last entry is optional.  An empty body is valid:

```rust
fixture!(test_empty, super::knife_descriptor();
);
```

---

## Field entries

### Field specifiers

Every entry that takes a `field` argument accepts one of three forms:

| Form | Resolution |
|---|---|
| `"name"` (string literal) | Resolved against the current message's descriptor; panics if not found. |
| `42u64` / `42u32` / `42` (integer) | Used as the raw field number; wire type is chosen by the entry type. |
| `Tag { field: N, wire_type: W, ohb: O }` | Used verbatim; see [Tag overrides](#tag-overrides). |

String literals require the enclosing message to have a descriptor (i.e. the
top-level message, or any `message("name"; ...)` / `group("name"; ...)` /
`message_with_len("name", ...; ...)` entry).  Integer and `Tag` specifiers work
without a descriptor, but their body cannot use string-name field specifiers.

---

### Scalar varint entries

```
uint64(field, val)
int64(field, val)
uint32(field, val)
int32(field, val)
bool_(field, val)
enum_(field, val)
sint32(field, val)   -- zigzag-encodes val (i32)
sint64(field, val)   -- zigzag-encodes val (i64)
```

Wire type 0.  `val` is a Rust expression implementing `IntoInteger`:
`u64`, `i64`, `u32`, `i32`, `bool`, or `Integer { value: V, ohb: N }`.
`sint32` / `sint64` apply zigzag encoding first; their `val` must be `i32` /
`i64` and do not accept `Integer`.

---

### Fixed-width entries

```
fixed64(field, val)    -- val: u64, wire type 1
sfixed64(field, val)   -- val: i64, wire type 1
double_(field, val)    -- val: f64, wire type 1
fixed32(field, val)    -- val: u32, wire type 5
sfixed32(field, val)   -- val: i32, wire type 5
float_(field, val)     -- val: f32, wire type 5
```

---

### Length-delimited scalar entries

```
string(field, val)                   -- val: &str; canonical length
bytes_(field, val)                   -- val: &[u8]; canonical length
string_with_len(field, len; val)     -- val: &str; length truncated to len bytes
string_with_len_ohb(field, ohb; val) -- val: &str; length encoded with ohb extra varint bytes
```

Wire type 2.  The plain forms use the actual byte length of `val` as the
length prefix.  `string_with_len` declares a shorter-than-actual length
(the payload is then truncated accordingly — useful for testing truncation
handling).  `string_with_len_ohb` encodes the true length with extra
non-canonical continuation bytes in its varint (useful for testing
overhanging length fields).

---

### Nested message entry

```
message(field;
    field_entry,
    ...
)
```

Wire type 2.  Builds a sub-message, appends all its `field_entry` items, then
serializes it as a length-prefixed field.

When `field` is a string literal, the sub-message is automatically bound to
the field's `message_type` descriptor, so nested string-name resolution works
without any extra annotation.  When `field` is an integer or `Tag`, the
sub-message has no descriptor; only integer/`Tag` field specifiers work inside.

---

### Nested message with custom length

```
message_with_len(field, len, len_ohb;
    field_entry,
    ...
)
```

Like `message`, but the serialized length prefix is `len` (possibly wrong or
truncated) encoded with `len_ohb` extra continuation bytes.  `len_ohb = 0`
gives canonical encoding.  Descriptor propagation works the same way as
`message`.

---

### Group entries

```
group(field;
    field_entry,
    ...
)
```

Emits a proto2 group: a start tag (wire type 3), the body, and an end tag
(wire type 4), both using the same field number derived from `field`.  When
`field` is a string literal, the group body is bound to the group field's own
`message_type` descriptor.  When `field` is an integer, the group body has no
descriptor.

```
group(start_tag => end_tag;
    field_entry,
    ...
)
```

Explicit start/end form.  `start_tag` and `end_tag` are `Tag` expressions —
use this when the end tag field number intentionally differs from the start
(malformed group), or when either tag needs a non-zero `ohb`.  The body has
no descriptor regardless of the tag values.

---

### Packed repeated field entries

```
packed_varints(field, [Integer, ...])       -- packed varint field (int32/int64/uint32/uint64/bool/enum)
packed_sint32(field, [i32, ...])            -- packed zigzag sint32
packed_sint64(field, [i64, ...])            -- packed zigzag sint64
packed_fixed32(field, [[u8; 4], ...])       -- packed fixed32/sfixed32/float
packed_fixed32_with_len(field, len, [[u8; 4], ...])  -- packed fixed32 with truncated length
```

All emit wire type 2 (length-delimited) with the concatenated encoded records
as the payload.  `field` accepts string literal, integer, or `Tag`.  The
element list uses `[...]` syntax (not `&[...]`); trailing commas are allowed.
An empty list `[]` emits a zero-length packed field (tag + length byte `\x00`).

For `packed_varints`, each element is `Integer { value: V, ohb: N }` or a
plain integer literal (which is coerced to `Integer { value, ohb: 0 }`).
For `packed_fixed32`, each element is a `[u8; 4]` array expression such as
`f32_val.to_le_bytes()`.

---

### Raw bytes entry

```
raw(expr)
```

Appends `expr: &[u8]` verbatim to the current message — no tag, no length
prefix.  Useful for intentionally malformed encodings:

```rust
raw(b"\x80\x80\x80\x80\x80"),
raw(&encode_varint_ohb((49 << 3) | 2, 0)),
```

`encode_varint_ohb(value, ohb)` is available in scope within `craft_a.rs` —
it encodes `value` as a varint with `ohb` extra continuation bytes.

---

## Tag overrides

`Tag { field: N, wire_type: W, ohb: O }` overrides the wire type and/or adds
overhanging bytes (non-canonical varint encoding of the tag itself).

| Field | Meaning |
|---|---|
| `field` | Raw field number (u64). Any expression is valid, including `1u64 << 28`. |
| `wire_type` | Wire type byte (0–5). Overrides the type that the entry would normally choose. |
| `ohb` | Number of extra varint continuation bytes on the tag. 0 = canonical. |

Examples:

```rust
// Tag with 17 extra tag bytes
uint64(Tag { field: 1,  wire_type: 0, ohb: 17 }, 0u64)

// Deliberately wrong wire type: a string field encoded as fixed64
double_(Tag { field: 49, wire_type: 1, ohb: 0 }, std::f64::consts::PI)

// Field number as an expression
bytes_(Tag { field: 1u64 << 28, wire_type: 2, ohb: 0 }, b"ok")

// Mismatched group start/end tags (start=4, end=44)
group(Tag { field: 4, wire_type: 3, ohb: 0 } => Tag { field: 44, wire_type: 4, ohb: 0 };
    uint64(11, 0u64),
)
```

---

## Integer overrides

`Integer { value: V, ohb: N }` overrides the varint payload of any scalar
varint entry.  `value` is always interpreted as raw bits (no sign extension,
no zigzag); `ohb` adds extra continuation bytes.

```rust
uint64("int64Rq", Integer { value: 0, ohb: 3 })  // value 0, 3 extra bytes
bool_(48, Integer { value: 1, ohb: 4 })           // bool 1, 4 extra bytes
```

Plain numeric literals also work:

```rust
uint64("uint64Rp", 0u64)
int32("int32Op",   42_i32)
bool_("boolOp",    true)
```

---

## What `fixture!` cannot express

The only case where an imperative `pub fn` is needed is when the fixture must
return bytes that no `Message` builder would produce — for example, returning
`Vec::new()` to represent a fixture where the Python reference implementation
suppresses the field entirely for an empty input.  Currently the only such
fixture is `enum_collision_empty_packed`.

Every other wire-level anomaly — wrong wire types, mismatched group tags,
overhanging bytes, truncated lengths, packed fields, deeply nested descriptors,
raw byte injection — is expressible through the macro entries above.

---

## Worked examples

### Canonical fixture: named fields, nested message and group

```rust
fixture!(test_proto2_level, super::knife_descriptor();
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

The top-level message is `SwissArmyKnife`.  `group("group"; ...)` looks up
"group" in that descriptor and binds its `message_type` to the group body, so
`message("nested"; ...)` inside can use "nested" by name, and its body in turn
uses "fixed64Rp" etc.

### Mixing named and numeric fields

```rust
fixture!(test_varint_required, super::knife_rq_descriptor();
    uint64(1,               0u64),
    int64("int64Rq",        Integer { value: 0, ohb: 3 }),
    uint64("uint64Rq",      0u64),
    sint32("sint32Rq",      0i32),
);
```

Field 1 is given numerically (no lookup needed); the rest use names.

### Injecting malformed wire bytes

```rust
fixture!(test_FIELD_INVALID, super::knife_descriptor();
    message("messageRp";
        raw(b"\x07"),
    ),
    message("messageRp";
        raw(b"\x87\x80\x80\x00"),
    ),
);
```

### Overhanging tag bytes and value bytes

```rust
fixture!(test_n_overhanging_bytes, super::knife_descriptor();
    message(51;
        uint64(Tag { field: 1,  wire_type: 0, ohb: 17 }, 0u64),
        uint64(Tag { field: 44, wire_type: 0, ohb: 2  }, 0u64),
    ),
);

fixture!(test_varint_overhanging_bytes, super::knife_descriptor();
    message(51;
        uint64(44, Integer { value: 42, ohb: 4 }),
    ),
    uint64(44, 0u64),
);
```

### Mismatched group tags (wire-level anomaly)

```rust
fixture!(test_wire_level, super::knife_descriptor();
    uint64(1, 0u64),
    fixed64(2, 0u64),
    fixed32(2, 0u32),
    bytes_(3, b""),
    group(Tag { field: 4, wire_type: 3, ohb: 0 } => Tag { field: 44, wire_type: 4, ohb: 0 };
        uint64(11, 0u64),
        fixed64(12, 0u64),
        bytes_(13, b""),
        uint32(15, 0u32),
    ),
    uint32(5, 0u32),
);
```

### Custom message length and overhanging length bytes

```rust
fixture!(test_titi, super::knife_descriptor();
    message_with_len("messageRp", 10, 0;
        string("stringOp", "Some pamling here..."),
    ),
);

fixture!(test_FIELD_INVALID_LENGTH, super::knife_descriptor();
    message(51;
        string(49, "hello1"),
        string_with_len_ohb(49, 3; "hello2"),   // length=6, encoded with 3 extra bytes
        raw(&encode_varint_ohb((49 << 3) | 2, 0)),
        raw(b"\x06hello3"),
    ),
    ...
);
```

### Packed repeated fields

```rust
fixture!(test_varint_packed, super::knife_descriptor();
    packed_varints(83, []),
    packed_varints(83, [Integer { value: 4, ohb: 0 }]),
    packed_varints(83, [
        Integer { value: 1, ohb: 3 },
        Integer { value: 2, ohb: 0 },
        Integer { value: 3, ohb: 0 },
        Integer { value: 4, ohb: 0 },
    ]),
    packed_sint32(97, [1, 2, 3, 4]),
    packed_sint64(98, [1, 2, 3, 4]),
);

fixture!(test_INVALID_PACKED_RECORDS, super::knife_descriptor();
    packed_fixed32(82, [
        0.0f32.to_le_bytes(),
        0.0f32.to_le_bytes(),
        0.0f32.to_le_bytes(),
    ]),
    packed_fixed32_with_len(82, 5, [      // 3 floats = 12 bytes, truncated to 5
        0.0f32.to_le_bytes(),
        1.0f32.to_le_bytes(),
        std::f32::consts::PI.to_le_bytes(),
    ]),
);
```

### Deeply nested descriptor propagation (FileDescriptorProto)

```rust
fixture!(fdp_complex, super::fdp_descriptor();
    string("name",    "test/complex.proto"),
    string("package", "test.complex"),
    string("syntax",  "proto3"),
    message("message_type";
        string("name", "ComplexMessage"),
        message("field";
            string("name",   "id"),
            uint32("number", 1u32),
            uint32("type",   5u32),
        ),
        message("field";
            string("name",   "name"),
            uint32("number", 2u32),
            uint32("type",   9u32),
        ),
    ),
    message("enum_type";
        string("name", "Status"),
        message("value";
            string("name",   "UNKNOWN"),
            uint32("number", 0u32),
        ),
    ),
);
```

Each nested `message("field_name"; ...)` automatically picks up its
`message_type` descriptor from the parent, so string-name resolution works
at every level of nesting without any manual descriptor passing.

### Adding a new fixture to the registry

After writing the `fixture!(my_test, ...)` definition, add an entry to
`ALL_FIXTURES` at the bottom of `craft_a.rs`:

```rust
pub static ALL_FIXTURES: &[(&str, fn() -> Vec<u8>)] = &[
    ...
    ("my_test", my_test),
];
```

Then add a matching entry to `prototext/fixtures/index.toml` (schema path and
message name) so the round-trip tests pick it up, and commit the generated
`.pb` file under `prototext/fixtures/cases/`.
