<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0012 — Extension field rendering

**Status:** implemented
**Implemented in:** 2026-03-23
**App:** prototext

---

## Problem

Spec 0011 made extension descriptors available via
`MessageDescriptor::get_extension(field_number)`, but explicitly deferred
rendering as a non-goal.  As a result, extension fields are currently
rendered as unknown fields by number:

```
1000: 42  #@ varint
```

protoc renders extension fields with a bracketed fully-qualified name:

```
[acme.blade_count]: 42
```

---

## Goals

1. Render extension fields as `[full.qualified.name]: value` in the output,
   matching protoc's format exactly.
2. Emit a correct annotation for extension fields (cardinality, kind, field
   number) using the same annotation syntax as regular fields.
3. No change to the rendering of regular fields, unknown fields, or any other
   wire type.

## Non-goals

- Extension fields inside nested messages — they are handled automatically
  since the renderer already passes the nested `MessageDescriptor` for
  recursive calls.

---

## Specification

### 1. Field lookup in `render_message`

`render_message` in `prototext-core/src/serialize/render_text/mod.rs`
currently does:

```rust
let field_schema: Option<FieldDescriptor> =
    schema.and_then(|s| s.get_field(field_number as u32));
```

Change to a two-step lookup: first try `get_field`; if that returns `None`,
try `get_extension`:

```rust
let field_schema: Option<FieldOrExt> = schema.and_then(|s| {
    if let Some(f) = s.get_field(field_number as u32) {
        Some(FieldOrExt::Field(f))
    } else {
        s.get_extension(field_number as u32).map(FieldOrExt::Ext)
    }
});
```

### 2. `FieldOrExt` adapter

Add a `FieldOrExt` enum (private to `render_text/`) that unifies
`FieldDescriptor` and `ExtensionDescriptor` for the subset of accessors
the renderer needs:

```rust
enum FieldOrExt {
    Field(FieldDescriptor),
    Ext(ExtensionDescriptor),
}

impl FieldOrExt {
    fn kind(&self) -> Kind { ... }
    fn cardinality(&self) -> Cardinality { ... }
    fn number(&self) -> u32 { ... }
    fn is_group(&self) -> bool { ... }  // always false for extensions
    fn display_name(&self) -> String {
        match self {
            FieldOrExt::Field(f) => f.name().to_owned(),
            FieldOrExt::Ext(e)   => format!("[{}]", e.full_name()),
        }
    }
    fn as_field(&self) -> Option<&FieldDescriptor> { ... }
}
```

All downstream render functions (`render_scalar`, `render_len_field`,
`render_varint_field`, `render_packed`, `render_group_field`) currently
take `Option<&FieldDescriptor>`.  These signatures change to
`Option<&FieldOrExt>`.

### 3. Field name in output

`field_decl` (in `helpers.rs`) currently emits the field number when no
schema is present, or `field.name()` when a `FieldDescriptor` is available.

With `FieldOrExt`, the display name is `FieldOrExt::display_name()`, which
returns:
- regular field: `"blade_count"` (unchanged)
- extension field: `"[acme.blade_count]"`

### 4. Annotation

The annotation for an extension field uses the same format as a regular
field:

```
[acme.blade_count]: 42  #@ optional int32 blade_count = 1000
```

`cardinality`, `kind`, and `number` come from `ExtensionDescriptor` in the
same way as `FieldDescriptor`.

### 5. Encoding (text → binary)

The encoder in `prototext-core/src/serialize/encode_text/mod.rs` does not
require changes.  It derives the field number exclusively from the `= N`
suffix in the annotation's field declaration (via `ann.field_number`), which
takes priority over parsing the LHS name.  Since every extension field line
carries a field declaration with `= N`, the bracketed name `[acme.blade_count]`
on the LHS is simply ignored — the correct field number and type are already
present in the annotation.

This holds for both scalar fields (`[ext]: value  #@ int32 = 1000`) and
message-typed extension open-brace lines (`[ext] {  #@ message Foo = 1001`).

### 6. Group check

`ExtensionDescriptor` does not support group fields (proto2 groups cannot be
extensions).  `FieldOrExt::is_group()` always returns `false` for the `Ext`
variant, so the group-mismatch path in `render_len_field` is unaffected.

---

## Impact on existing tests

All existing fixtures contain no extension fields, so the round-trip tests
are unaffected.  Two new tests are added in `prototext/tests/roundtrip.rs`:

- `extension_field_renders_with_bracketed_fqn`: asserts that wire bytes
  containing a field number in the extension range render as
  `[acme.blade_count]: 42  #@ int32 = 1000`.
- `extension_field_roundtrip`: performs a full wire → text → wire round-trip
  and asserts byte-for-byte identity, confirming that the encoder correctly
  reconstructs the original binary from the annotated extension field text.
