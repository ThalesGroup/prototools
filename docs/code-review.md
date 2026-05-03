<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# Code Quality Review — prototools

Date: 2026-03-24

---

## 1. Files too large

### `prototext-core/src/serialize/encode_text/mod.rs` (~1 230 lines)

Handles placeholder management, wire-type dispatch, field encoding for every
kind (varint, fixed, string, bytes, message, group), and packed record
reconstruction.  Recommended split:

- `placeholder.rs` — write_placeholder / fill_placeholder / compact logic
- `frame.rs` — Frame enum and encoding stack
- `fields.rs` — per-kind field encoders
- `mod.rs` — public API + top-level dispatcher

### `prototext-core/src/serialize/render_text/helpers.rs` (~807 lines)

Contains low-level byte-output helpers, prefix writers, AnnWriter, and the
full implementations of `render_scalar`, `render_invalid`, `render_len_field`,
`render_group_field`.  Recommended split:

- `output.rs` — push_indent, write_dec_*, low-level byte helpers
- `annotations.rs` — AnnWriter, push_field_decl, proto_type_str
- `scalar.rs` — render_scalar, render_invalid, render_invalid_tag_type
- `len_field.rs` — render_len_field, render_group_field, render_truncated_bytes

### `prototext-core/src/serialize/common.rs` (~827 lines)

Mixes escape logic, float/double formatting, per-type scalar formatters, and
the `format_protoc_value` dispatcher.  Recommended split:

- `escape.rs` — escape_bytes, escape_string
- `floats.rs` — format_double / format_float and all helpers
- `scalars.rs` — format_int32_protoc, format_uint64_protoc, …
- `format.rs` — format_protoc_value

### `prototext-core/src/decoder.rs` (~711 lines)

Mixes data structures, the main `parse_message` loop, packed-field decoders,
and per-kind type decoders.  Recommended split:

- `types.rs` — ProtoTextMessage / ProtoTextField / ProtoTextContent
- `parse.rs` — parse_message and wire-type dispatch
- `packed.rs` — decode_packed, decode_packed_varints
- `codec.rs` — type-specific decode_* functions

### `prototext-core/src/helpers.rs` (~657 lines)

Mixes varint parsing, numeric codecs, wire-encoding helpers, and 230 lines of
inline tests.  Recommended split:

- `varint.rs` — parse_varint, VarintResult
- `codecs.rs` — decode_int32 / decode_float / … family
- `wire.rs` — wire-type constants, write_varint, write_tag

---

## 2. Code duplication

### 2.1 Repeated packed-fixed decoding loop in `decoder.rs`

`decode_packed` (lines ~489–565) repeats the same `while i < length { … i +=
elem_size }` body six times, once per fixed-width type (Double, Float, Fixed64,
Sfixed64, Fixed32, Sfixed32).  A single generic helper would replace all six:

```rust
fn decode_fixed_vec<T>(data: &[u8], elem_size: usize, f: impl Fn(&[u8]) -> T)
    -> Result<Vec<T>, ()>
{
    if data.len() % elem_size != 0 { return Err(()); }
    Ok(data.chunks_exact(elem_size).map(f).collect())
}
```

### 2.2 Repeated annotation-modifier block

The sequence

```rust
if let Some(v) = tag_ohb { aw.push_u64_mod(out, b"tag_ohb: ", v); }
if tag_oor               { aw.push(out, b"TAG_OOR"); }
if let Some(v) = len_ohb { aw.push_u64_mod(out, b"len_ohb: ", v); }
```

appears in at least four places: `render_scalar`, `render_varint_field`,
`render_len_field`, `render_invalid`, and `render_packed`.  Extract to:

```rust
fn push_tag_modifiers(
    aw: &mut AnnWriter, out: &mut Vec<u8>,
    tag_ohb: Option<u64>, tag_oor: bool, len_ohb: Option<u64>,
) { … }
```

---

## 3. Overly complicated logic

### 3.1 `rust_sci_to_g_style` allocates a `Vec<char>` unnecessarily (`common.rs`)

```rust
let digits_str = mantissa_str.trim_start_matches('-').replace('.', "");
let sig_digits: Vec<char> = digits_str.chars().collect();
```

The `Vec<char>` is collected only to index into it.  The same result can be
obtained by working with byte indices on the ASCII string directly, avoiding
the allocation.

Similarly, `trim_trailing_zeros_after_dot` allocates a new String:

```rust
let trimmed = s.trim_end_matches('0').trim_end_matches('.');
*s = trimmed.to_owned();
```

Replace with in-place mutation using `String::truncate` + `String::pop`:

```rust
while s.ends_with('0') { s.pop(); }
if s.ends_with('.') { s.pop(); }
```

### 3.2 `render_len_field` is 173 lines of nested early-returns (`helpers.rs`)

The function has six distinct branches (unknown field, packed, string, bytes,
message, wire-type mismatch), each of which writes its own annotation block
inline and returns early.  The problem is not the length per se but that the
annotation-writing code is copy-pasted into every branch (see also §2.2): if
the annotation format ever changes, all six copies must be updated in sync.
Extracting each branch into a named helper and factoring out the shared
`push_tag_modifiers` call (§2.2) would make the invariant explicit and the
dispatcher trivially auditable.

### 3.3 Excessive function arity

`render_scalar` takes 10 parameters:

```rust
pub(super) fn render_scalar(
    field_number: u64,
    field_schema: Option<&FieldOrExt>,
    tag_ohb: Option<u64>,
    tag_oor: bool,
    len_ohb: Option<u64>,
    wire_type_name: &str,
    value_str: &str,
    is_wire: bool,
    nan_bits: Option<u64>,
    out: &mut Vec<u8>,
)
```

Group the rendering context into a struct:

```rust
struct ScalarCtx<'a> {
    field_number: u64,
    schema: Option<&'a FieldOrExt>,
    tag_ohb: Option<u64>,
    tag_oor: bool,
    len_ohb: Option<u64>,
    wire_type_name: &'a str,
    nan_bits: Option<u64>,
}
```

---

## 4. Non-idiomatic Rust

### 4.1 Unnecessary `.clone()` on `FieldDescriptor` in `packed.rs`

```rust
let foe = FieldOrExt::Field(fs.clone());
```

`fs` is already a `&FieldDescriptor`.  If `FieldOrExt` accepted a reference
(or `fs` could be moved), the clone is avoidable.

Clippy does not flag this because `FieldDescriptor` is not a reference-counted
pointer type (`Arc`/`Rc`) — clippy's `clone_on_ref_ptr` lint only fires for
those.  For plain `Clone` impls, clippy has no lint that detects "clone forced
by an owned-value constructor when a borrow-based variant would suffice" —
that is a design issue (the `FieldOrExt` enum owns its payload rather than
borrowing it) which only a code review can surface.

### 4.2 `.unwrap_or(0)` silently swallows parse errors in `common.rs`

```rust
let exp: i32 = exp_str.parse().unwrap_or(0);
```

A malformed exponent string silently produces 0, yielding a wrong result
rather than an error.  Use `expect` or propagate a `Result`.

### 4.3 `format!` in a rendering loop allocates a temporary String

```rust
out.extend_from_slice(format!("{:016x}", bits).as_bytes());
```

`format!` always heap-allocates a `String` that is immediately discarded after
`.as_bytes()`.  The fix is to use `write!` into a fixed-size stack buffer:

```rust
use std::io::Write as _;
let mut buf = [0u8; 16];
write!(&mut buf[..], "{:016x}", bits).unwrap();
out.extend_from_slice(&buf);
```

This appears in `write_packed_elem_ann` (the 8-byte case) and the 4-byte case
just below it in `packed.rs`.  Both occurrences should be fixed.

### 4.4 Inline test block not wrapped in a `mod tests {}` in `helpers.rs`

The 230-line test block at the bottom of `helpers.rs` sits directly under
`#[cfg(test)]` without a module wrapper.  Wrap it in `mod tests { use super::*;
… }` to follow the standard Rust convention and avoid name collisions.

---

## 5. Dead code

### 5.1 `wob_prefix` in `render_text/helpers.rs`

```rust
#[allow(dead_code)]
pub(super) fn wob_prefix(name: &str, out: &mut Vec<u8>) { … }
```

All call sites use `wob_prefix_n` instead.  Either remove this function or
document why it is kept.

### 5.2 Same issue in `prototext/src/protocraft/mod.rs`

A `wob_prefix` function is also suppressed with `#[allow(dead_code)]`.  Same
recommendation: remove or document.
