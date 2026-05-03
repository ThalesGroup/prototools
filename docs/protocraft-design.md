<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# Protocraft design

Protocraft is a test-only library for constructing protobuf wire bytes
programmatically.  Its primary use is generating fixtures for the prototext
test suite — both canonical encodings and deliberate anomalies (overhanging
varints, wrong wire types, truncated fields, mismatched group tags, raw byte
injection, etc.).

Standard protobuf libraries only produce canonical output.  Protocraft can
produce any byte sequence.

The Rust implementation (`prototext/src/protocraft/`) is a port of the Python
implementation (`src/protocraft/` in the reference repo).

---

## Design principles

### 1. The output is bytes

Every builder method ultimately appends bytes to a buffer.  There is no
intermediate representation, no AST, no validation pass.  `Message::build()`
returns the raw wire bytes and nothing else.

### 2. Scalar fields encode immediately; nested scopes encode on completion

Scalar field methods (`uint64`, `string`, `fixed32`, …) encode their tag and
value and append the result to the current message buffer immediately.

Nested messages and groups are different.  The `msg_fields!` macro builds the
child as a separate `Message` value — its fields accumulate in a child buffer
— and passes the completed value to the parent's `.message()` or `.group()`
method.  The parent then encodes the full field (tag + length prefix + payload
for messages; start tag + body + end tag for groups) and appends it atomically.

There is no hidden stack or deferred state inside `Message`.  The sequencing
comes from Rust's ordinary call-by-value: a child `Message` is fully
constructed before the call that folds it into the parent.  This differs from
the Python implementation, which uses a thread-local stack and context
managers (`__enter__`/`__exit__`) to achieve the same nesting.

### 3. All wire-level overrides go through `Tag` and `Integer`

Wire-level overrides are expressed through two structs, not through
proliferating builder variants:

- **`Tag`** carries all tag-level overrides: field number, wire type, tag
  overhanging bytes (`ohb`), length override (`length`), and length overhanging
  bytes (`length_ohb`).
- **`Integer`** carries varint value overrides: the raw value (`value`),
  overhanging bytes (`ohb`), and a raw (non-encoded) mode (`raw`) for
  injecting truncated encodings.

This mirrors the Python design exactly.  There are no `_with_len`,
`_with_len_ohb`, or similar builder variants — `Tag` covers all of those cases
uniformly for every field type.

### 4. Progressive disclosure

Simple cases are simple:

```rust
uint64("uint64Rp", 42u64)
string("stringRp", "hello")
```

Anomalies require explicit opt-in:

```rust
// Overhanging tag bytes and overhanging value bytes
uint64(Tag { field: "uint64Rp", ohb: 2 }, Integer { value: 42, ohb: 4 })

// Raw (non-encoded) value bytes — bypasses varint encoding entirely
uint64("uint64Rp", RawData(b"\x80"))

// Truncated length — only 5 bytes declared for a 12-byte payload
packed_float(Tag { field: "floatPk", length: 5 }, [0.0, 1.0, f32::consts::PI])

// Overhanging length bytes
string(Tag { field: "stringRp", length_ohb: 3 }, "hello2")
```

No magic, no hidden defaults beyond the wire type implied by the method name.

### 5. Schema awareness is optional and additive

When a `MessageDescriptor` is bound to a `Message`, string field names resolve
to field numbers and nested `message(...)` / `group(...)` calls automatically
propagate the correct sub-descriptor.  When no descriptor is bound, integer
field numbers are used directly.

The two modes compose freely: a top-level fixture is always schema-bound (via
`knife_descriptor()` etc.), but individual fields may use raw integers or
`Tag` structs to bypass name resolution for out-of-range or wire-level tests.

---

## User-facing API (macro syntax)

The `fixture!` and `msg_fields!` macros are the only user-facing API.  The
`Message` builder and its methods are implementation details.

### Core types

#### `Tag`

```rust
Tag {
    field:      impl IntoField,  // &str (name), u64 (number), or expression
    wire_type:  u8,              // optional: overrides the default for the entry type
    ohb:        u8,              // optional: tag varint overhanging bytes (default 0)
    length:     usize,           // optional: length override for LEN fields
    length_ohb: u8,              // optional: length varint overhanging bytes (default 0)
}
```

`Tag` is only needed when overriding defaults.  In normal use, a plain string
or integer is passed as the field specifier.

#### `Integer`

```rust
Integer {
    value: u64,   // raw bits (no sign extension, no zigzag)
    ohb:   u8,    // overhanging bytes on the varint (default 0)
    raw:   bool,  // if true: value is emitted as-is without varint encoding
}
```

`Integer` is only needed for non-canonical varint encoding.  In normal use,
plain numeric literals (`42u64`, `true`, `-1i32`) are accepted.

The `raw` flag is for injecting truncated encodings that no correct varint
encoder would produce — e.g. `Integer { value: 0xFFFFFFFF, raw: true }` for
the 5-byte truncated encoding of -1 in a 32-bit field.

#### `RawData`

```rust
RawData(b"...")
```

Passes raw bytes as the encoded value of a field, bypassing all encoding.
The tag (and length prefix for LEN fields) is still emitted normally — only
the payload bytes are raw.  Equivalent to Python's `RawData(b'...')`.

### Fixture definition

```
fixture!(name, descriptor_expr;
    field_entry,
    ...
)
```

Expands to `pub fn name() -> Vec<u8>`.

### Field entries

#### Scalar varint

```
uint64(field, val)
int64(field, val)
uint32(field, val)
int32(field, val)
bool_(field, val)
enum_(field, val)
sint32(field, val)    -- zigzag-encodes val: i32
sint64(field, val)    -- zigzag-encodes val: i64
```

`val` accepts: numeric literal, `bool`, `Integer { value, ohb, raw }`,
or `RawData(bytes)`.  `sint32`/`sint64` apply zigzag first; they do not
accept `Integer` or `RawData`.

#### Fixed-width

```
fixed64(field, val)    -- val: u64,  wire type 1
sfixed64(field, val)   -- val: i64,  wire type 1
double_(field, val)    -- val: f64,  wire type 1
fixed32(field, val)    -- val: u32,  wire type 5
sfixed32(field, val)   -- val: i32,  wire type 5
float_(field, val)     -- val: f32,  wire type 5
```

`val` also accepts `RawData(bytes)`.

#### Length-delimited scalar

```
string(field, val)    -- val: &str or RawData
bytes_(field, val)    -- val: &[u8] or RawData
```

Length and length_ohb overrides go through `Tag`.

#### Nested message

```
message(field;
    field_entry, ...
)
```

#### Nested group

```
group(field;
    field_entry, ...
)

group(start_tag => end_tag;
    field_entry, ...
)
```

The `start_tag => end_tag` form is used when the end tag field number
intentionally differs from the start (mismatched group test), or when either
tag needs a non-zero `ohb`.

#### Packed repeated fields

```
packed_varints(field, [val, ...])    -- int32/int64/uint32/uint64/bool/enum
packed_sint32(field, [i32, ...])
packed_sint64(field, [i64, ...])
packed_float(field, [f32, ...])
packed_double(field, [f64, ...])
packed_fixed32(field, [u32, ...])
packed_fixed64(field, [u64, ...])
packed_sfixed32(field, [i32, ...])
packed_sfixed64(field, [i64, ...])
```

For `packed_varints`, each element accepts `Integer { value, ohb, raw }` or a
plain integer.  For fixed-width packed fields, each element is a plain numeric
literal; `RawData` per element is not supported.  Length overrides go through
`Tag`.

#### Raw bytes

```
raw(expr)    -- expr: &[u8], appended verbatim (no tag, no length)
```

Equivalent to Python's `CustomField(bytes)` and message-level `RawData`.

---

## Implementation design

### `Message` struct

```rust
pub struct Message {
    buf: Vec<u8>,
    desc: Option<MessageDescriptor>,  // test-only
}
```

All builder methods take `&mut self` and append to `buf`.  `build(self) ->
Vec<u8>` consumes the message and returns the buffer.

### `Tag` struct

```rust
pub struct Tag {
    pub field:      u64,
    pub wire_type:  u8,
    pub ohb:        u8,
    pub length:     Option<usize>,
    pub length_ohb: u8,
}
```

`length: None` means use the actual payload length (canonical).
`length: Some(n)` overrides the declared length.

Currently `Tag` has no `length` or `length_ohb` fields — these are added by
the planned refactor.

### `Integer` struct

```rust
pub struct Integer {
    pub value: u64,
    pub ohb:   u8,
    pub raw:   bool,
}
```

`raw: true` emits `value` as a raw little-endian byte sequence (4 bytes for
32-bit contexts, 8 bytes for 64-bit) without varint encoding.  Used for
truncated int32/int64 values.

Currently `Integer` has no `raw` field — added by the planned refactor.

### `IntoFieldTag` trait

Resolves a field specifier to a `Tag`:

- `Tag` — passes through unchanged.
- Integer types — uses the supplied `default_wire_type`, `ohb = 0`,
  `length = None`, `length_ohb = 0`.
- `&str` (test only) — name lookup in bound `MessageDescriptor`.

### `IntoBytes` trait (replaces `IntoInteger`)

```rust
pub trait IntoBytes {
    fn into_bytes(self) -> Vec<u8>;
}
```

Replaces the current `IntoInteger` → `Integer` → `encode_varint_ohb`
indirection.  Implementations:

- Numeric types (`u64`, `i64`, `u32`, `i32`, `bool`) — canonical varint.
- `Integer { value, ohb, raw: false }` — `encode_varint_ohb(value, ohb)`.
- `Integer { value, raw: true }` — emit `value` as raw bytes (no encoding).
- `RawData<'a>` — return the slice as-is.

This makes `RawData` a first-class value specifier with no special cases in
builder methods.

### Encoding pipeline

For `uint64("uint64Rp", 42u64)`:

```
"uint64Rp" → IntoFieldTag (desc lookup) → Tag { field: 44, wire_type: 0, ohb: 0, .. }
               → encode_tag(44, 0, 0) → \xa2\x02

42u64 → IntoBytes (u64 impl) → encode_varint_ohb(42, 0) → \x2a

append \xa2\x02\x2a to buf
```

For `message("messageRp"; ...)`:

```
1. macro builds child Message _nm with sub-descriptor
2. msg_fields! populates _nm
3. parent.message("messageRp", _nm):
   a. payload = _nm.build()
   b. tag = IntoFieldTag → Tag { field: 51, wire_type: 2, length: None, length_ohb: 0, .. }
   c. encode_tag(51, 2, 0) → tag bytes
   d. encode_varint_ohb(payload.len(), 0) → length bytes
   e. append tag + length + payload to parent buf
```

For `message(Tag { field: "messageRp", length: 10 }; ...)`:

```
   ... same as above but step d uses encode_varint_ohb(10, 0) regardless of payload.len()
```

### Macro layer

`msg_fields!` is a recursive tt-muncher.  Each arm matches one entry and calls
the corresponding builder method, then recurses on the remaining tokens.

Nesting: the `message(field; ...)` arm creates a local `_nm: Message`,
recurses into it with `msg_fields!(_nm, ...)`, then calls
`$m.message(field, _nm)`.  The child is fully populated before the parent
call, matching Rust's call-by-value semantics.

The macro is the sole user-facing layer.  All builder methods on `Message`
are implementation details.

---

## Gap analysis vs. Python implementation

| Feature | Python | Rust (current) | Rust (after refactor) |
|---|---|---|---|
| `Tag { length }` | `Tag('f', length=N)` | `_with_len` variants | `Tag { length: Some(N) }` |
| `Tag { length_ohb }` | `Tag('f', length_ohb=N)` | `_with_len_ohb` variants | `Tag { length_ohb: N }` |
| `RawData` as value | `UInt64('f', RawData(b'...'))` | two `raw()` calls | `uint64("f", RawData(b"..."))` |
| `Integer { raw }` | `Integer(N, no_convert=True)` | not supported | `Integer { value: N, raw: true }` |
| Packed float/double | `Float('f', [1.0, 2.0])` | `Vec<[u8;4]>` (awkward) | `packed_float("f", [1.0, 2.0])` |
| `CustomField(bytes)` | `CustomField(b'...')` | `raw(b"...")` | unchanged |
| Message-level `RawData` | `RawData(b'...')` | `raw(b"...")` | unchanged |
