<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0011 — Replace hand-rolled schema with prost-reflect

**Status:** implemented
**Implemented in:** 2026-03-23
**App:** prototext

---

## Problem

`prototext-core` currently parses protobuf descriptor files into a small set
of hand-rolled structs (`FieldInfo`, `MessageSchema`, `ParsedSchema`) defined
in `prototext-core/src/schema.rs`.  This layer was built to extract only the
subset of descriptor information needed at the time: field numbers, names,
types, labels, packed flags, and enum value tables.

Several features that are now in scope — or naturally arise from the
protoc-compatibility work — require descriptor information that the hand-rolled
layer does not capture:

- **Extension fields** (spec 0010 / `docs/protoc-decode-compatibility.md`):
  require the fully-qualified extension name, the extendee, and the extension
  range, none of which are stored today.
- **Proto2 vs proto3 syntax**: currently inferred heuristically; needed
  precisely for correct UTF-8 validation behavior (per
  `docs/protoc-decode-anomalous-input.md` §2.7) and closed-enum handling (§2.3).
- **Package name**: discarded today after use; needed for FQN construction of
  extension names.
- **OneOf fields**: not stored; needed for correct presence semantics.

Each new feature requires another bespoke traversal pass through the
`prost-types` descriptor structs, growing the `schema.rs` file and
accumulating the risk of subtle bugs (wrong FQN construction, missed
nested type scopes, etc.).

---

## Rationale for prost-reflect

The `prost-reflect` crate (current version 0.16.3, released 2025-12-01,
actively maintained with 8 releases in 2025) provides a complete runtime
reflection API over a `DescriptorPool`.  It solves the same problem as the
hand-rolled layer, but correctly, completely, and with a maintained public API.

### Why prost-reflect is the right call

**1. Extension support comes for free.**

The hand-rolled layer has no concept of extensions.  Adding it would require:
parsing `FileDescriptorProto.extension` and `DescriptorProto.extension` lists,
building a cross-file extension registry, resolving extendee message FQNs, and
storing fully-qualified extension names.  With `prost-reflect` this is:

```rust
// Look up extensions registered on a message:
for ext in msg.extensions() {
    let fqn = ext.full_name();          // "com.example.my_ext"
    let extendee = ext.containing_message();
    let number = ext.number();
}
// Or look up by field number:
if let Some(ext) = msg.get_extension(field_number) { ... }
```

**2. Correct FQN construction without reimplementation.**

The current `collect_message_types` / `collect_nested_enum_types` helpers
hand-construct FQNs by concatenating package and message name segments.  Edge
cases (empty package, nested types, groups) are handled correctly by
`prost-reflect` internally.  `MessageDescriptor::full_name()` always returns
the correct, dot-separated FQN without a leading dot (unlike the current
hand-rolled convention which adds a leading dot).

**3. Proto2 vs proto3 is a first-class property.**

`FileDescriptor::syntax()` returns `Syntax::Proto2` or `Syntax::Proto3`
(an enum, not an `Option<String>`).  This eliminates the current heuristic
of relying on `options.packed` being set by protoc.

**4. Kind and Cardinality replace raw integer constants.**

The hand-rolled layer uses `proto_type::INT32 = 5` integer constants (copied
from Python's `FieldDescriptor.TYPE_*`), requiring a `match` on `i32`.
`prost-reflect`'s `Kind` enum (`Kind::Int32`, `Kind::Float`, etc.) and
`Cardinality` enum provide type safety and eliminate the constants module
entirely.  This also removes a maintenance burden: if a new proto type is
added, the compiler will flag non-exhaustive matches.

**5. The API is stable and tested upstream.**

FQN construction, enum alias handling, packed detection, group detection,
nested type scoping — all are tested by the prost-reflect test suite.  The
hand-rolled code re-tests none of this implicitly.

**6. Directly decodes raw bytes.**

`DescriptorPool::decode(bytes)` accepts raw `FileDescriptorSet` bytes
directly — the same input currently accepted by `parse_schema()`.  No
intermediate deserialization step is needed; the call signature of the public
`parse_schema` function is unchanged at the API level.

**7. prost compatibility.**

`prost-reflect` tracks `prost` closely: version 0.16.x targets `prost` 0.14.
Since `prototext-core` already depends on `prost = "0.13"`, a minor version
bump of `prost` to `"0.14"` is required alongside adding `prost-reflect`.
This is an expected, low-risk upgrade.

---

## Goals

1. Replace `FieldInfo`, `MessageSchema`, and `ParsedSchema` with a thin wrapper
   over `prost-reflect`'s `DescriptorPool` and `MessageDescriptor`.
2. Make extension fields visible to the renderer: when a field number on the
   wire matches an extension registered on the message, the extension's
   fully-qualified name is available for rendering as `[com.example.my_ext]`.
3. Make proto2 / proto3 syntax available to the decoder.
4. Preserve the existing public API of `parse_schema()` and `ParsedSchema`
   so that callers (`prototext/src/main.rs`, tests) require no changes beyond
   the dependency bump.
5. Remove `proto_type::*` and `proto_label::*` integer constants in favour of
   `Kind` and `Cardinality` throughout `prototext-core`.

## Non-goals

- Rendering extension fields in the output (a separate rendering change;
  this spec only makes the descriptor information available).
- Using `prost-reflect`'s `DynamicMessage` for wire decoding (prototext has
  its own wire decoder; `prost-reflect` is used only for schema reflection).
- Exposing `DescriptorPool` or `MessageDescriptor` in the public API of
  `prototext-core` (they remain internal to the schema layer).

---

## Specification

### 1. Dependency changes

**`prototext-core/Cargo.toml`:**

```toml
[dependencies]
prost          = "0.14"
prost-reflect  = "0.16"
```

Remove `prost-types` from direct dependencies — `prost-reflect` re-exports
or depends on it; direct use of `prost_types::*` structs in `schema.rs` is
eliminated.

**`prototext/Cargo.toml`** (the binary crate):

```toml
prost      = "0.14"
prost-types = "0.14"   # still needed for the embedded descriptor.proto types
```

**`prototext/build.rs`**: no change — `protox` already produces a
`FileDescriptorSet` binary; `DescriptorPool::decode()` accepts it directly.

### 2. New schema layer (`prototext-core/src/schema.rs`)

#### 2.1 ParsedSchema

`ParsedSchema` is restructured to own a `DescriptorPool` and cache the root
`MessageDescriptor`:

```rust
pub struct ParsedSchema {
    pool: DescriptorPool,
    root_full_name: String,
}

impl ParsedSchema {
    pub fn empty() -> Self { ... }

    /// Return the MessageDescriptor for the root message, or None (no-schema mode).
    pub fn root_schema(&self) -> Option<MessageDescriptor> {
        if self.root_full_name.is_empty() {
            None
        } else {
            self.pool.get_message_by_name(&self.root_full_name)
        }
    }

    /// Look up a message by fully-qualified name (for nested message traversal).
    pub fn get_message(&self, fqn: &str) -> Option<MessageDescriptor> {
        self.pool.get_message_by_name(fqn)
    }
}
```

The `messages` and `all_schemas` fields on the old `ParsedSchema` are removed.
The `Arc<HashMap<String, Arc<MessageSchema>>>` passed to the renderer is
replaced by `&DescriptorPool` (or `&ParsedSchema`).

#### 2.2 parse_schema

```rust
pub fn parse_schema(
    schema_bytes: &[u8],
    root_msg_name: &str,
) -> Result<ParsedSchema, SchemaError> {
    if schema_bytes.is_empty() || root_msg_name.is_empty() {
        return Ok(ParsedSchema::empty());
    }

    let pool = DescriptorPool::decode(schema_bytes)
        .map_err(|e| SchemaError::InvalidDescriptor(e.to_string()))?;

    // Normalise: prost-reflect uses no leading dot.
    let root_full_name = root_msg_name.trim_start_matches('.').to_string();

    if pool.get_message_by_name(&root_full_name).is_none() {
        let available = pool.all_messages()
            .map(|m| m.full_name().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(SchemaError::MessageNotFound(format!(
            "root message '{}' not found in schema (available: {})",
            root_full_name, available
        )));
    }

    Ok(ParsedSchema { pool, root_full_name })
}
```

#### 2.3 FieldInfo and MessageSchema

These structs are removed.  All sites that previously used `FieldInfo` are
migrated to use `FieldDescriptor` (for regular fields) and `ExtensionDescriptor`
(for extension fields) from `prost-reflect`.

The `proto_type` integer and `proto_label` integer constants modules are
removed.  Code that previously matched on `proto_type::INT32` etc. is migrated
to match on `Kind::Int32` etc.

#### 2.4 Field lookup at render time

The renderer currently calls `MessageSchema::fields.get(&field_number)` to
look up a `FieldInfo`.  This becomes a two-step lookup:

```rust
// Step 1: try regular field.
if let Some(field) = msg.get_field(field_number) {
    // regular field — use FieldDescriptor
}
// Step 2: try extension.
else if let Some(ext) = msg.get_extension(field_number) {
    // extension field — use ExtensionDescriptor
    // ext.full_name() → "[com.example.my_ext]" display name
}
// Step 3: unknown field.
else {
    // render by field number only
}
```

A small adapter type (internal, not public) can unify the two cases if the
renderer's code duplication becomes unwieldy:

```rust
enum FieldLookup<'a> {
    Regular(&'a FieldDescriptor),
    Extension(&'a ExtensionDescriptor),
}
```

This adapter provides the same accessors (name, kind, cardinality, is_packed,
display name for annotation) needed by the renderer, delegating to the
appropriate underlying descriptor type.

### 3. Rendering changes

#### 3.1 Field name display

For regular fields: `field.name()` (unchanged behaviour).

For extension fields: `format!("[{}]", ext.full_name())` — e.g.
`[com.example.my_extension]`.  This matches protoc's output format exactly.

The annotation field declaration for an extension field:

```
[com.example.my_ext]: 42  #@ int32 = 1000
```

The field number in the annotation (`= 1000`) is `ext.number()`.  The label
(`optional` / `repeated`) and type come from the `ExtensionDescriptor` the
same way as a regular field.

#### 3.2 Proto2 vs proto3 syntax

`FileDescriptor::syntax()` returns `Syntax::Proto2` or `Syntax::Proto3`.
This is made available via `ParsedSchema` or by threading `FileDescriptor`
references through the relevant code paths.  Immediate uses:

- UTF-8 validation mode for string fields (§2.7 of
  `docs/protoc-decode-anomalous-input.md`): currently all strings are
  treated as proto2 (invalid UTF-8 rendered as escaped bytes).  With syntax
  available, proto3 string fields could be flagged when annotations are
  enabled.
- Closed vs open enum semantics (§2.3): proto2 enums are closed; proto3
  enums are open.

These behavioural changes are deferred to follow-on specs; the syntax value
is made available here without changing current rendering behaviour.

### 4. Test migration

The unit tests in `schema.rs` currently construct `FieldDescriptorProto` /
`DescriptorProto` / `FileDescriptorSet` structs by hand.  These are migrated
to build `FileDescriptorSet` bytes via `prost::Message::encode` (as today) and
pass them to `DescriptorPool::decode()`, then assert on `MessageDescriptor` /
`FieldDescriptor` values instead of `FieldInfo`.

The two existing tests (`two_pass_enum_collection`,
`enum_named_float_not_mistaken_for_primitive`) are preserved as functional
equivalents.

### 5. Removal of `proto_type` and `proto_label` modules

`prototext-core/src/schema.rs` currently exports:

```rust
pub mod proto_type { pub const DOUBLE: i32 = 1; ... }
pub mod proto_label { pub const OPTIONAL: i32 = 1; ... }
```

These are used in 30+ match arms across `render_text/`, `encode_text/`, and
`decoder.rs`.  They are replaced by `Kind::*` and `Cardinality::*` throughout.

Since `Kind` and `Cardinality` are re-exported from `prost-reflect`, no new
constants module is needed.  A type alias in `schema.rs` makes the transition
ergonomic:

```rust
pub use prost_reflect::{Cardinality, Kind};
```

---

## Migration strategy

The migration touches many files.  The following order minimises the window
where the codebase is in a broken intermediate state:

1. Add `prost-reflect` to `Cargo.toml`; bump `prost` to `0.14`.
2. Rewrite `parse_schema` and `ParsedSchema` in `schema.rs` using
   `DescriptorPool`.  Keep `FieldInfo` and `MessageSchema` as a thin
   compatibility shim populated from `FieldDescriptor` — this makes the
   remaining files compile unchanged while the migration is in progress.
3. Migrate `decoder.rs` to use `FieldDescriptor` / `ExtensionDescriptor`
   directly (remove shim for that file).
4. Migrate `render_text/` files one by one.
5. Migrate `encode_text/` files.
6. Remove the shim (`FieldInfo`, `MessageSchema`), `proto_type`, and
   `proto_label` modules.
7. Add extension lookup (step 2 in §2.4).
8. Update tests.

The shim in step 2 ensures `cargo test` passes throughout the migration.

---

## Impact on existing tests

- All existing unit tests in `prototext-core` pass (field lookup, enum
  resolution, round-trips) — the functional behaviour is unchanged.
- The integration tests in `prototext/tests/roundtrip.rs` are unaffected:
  they call `parse_schema()` / `render_as_text()` / `render_as_bytes()`,
  none of which change signature.
- The `fixture_roundtrip_annotated` test continues to pass — rendering output
  is unchanged for all existing fixtures (no extension fields in current
  fixtures).

---

## Open questions

1. **FieldLookup adapter vs duplication.** The two-branch lookup (regular vs
   extension) could share code via a `FieldLookup` enum adapter or via
   trait objects.  Decision deferred to implementation; the spec does not
   mandate either approach.

2. **Thread-safe pool sharing.** `DescriptorPool` is `Clone` and internally
   reference-counted.  If multiple threads call `render_as_text` concurrently
   with the same `ParsedSchema`, the pool can be shared by value (cheap clone).
   Confirm this is sufficient or whether an `Arc<ParsedSchema>` is needed.

3. **`prost` version bump coordination.** Bumping `prost` from 0.13 to 0.14
   in `prototext-core` requires the same bump in `prototext` (the binary
   crate) and any downstream crates.  Confirm no breaking API changes affect
   the codebase (prost 0.14 changelog should be reviewed at implementation
   time).

---

## References

- `prototext-core/src/schema.rs` — current hand-rolled schema
- `prototext-core/src/serialize/render_text/` — renderer (consumes schema)
- `prototext-core/src/serialize/encode_text/mod.rs` — encoder (consumes schema)
- `prototext-core/src/decoder.rs` — wire decoder (consumes schema)
- `docs/protoc-decode-compatibility.md` — extension field divergence (D6)
- `docs/protoc-decode-anomalous-input.md` — proto2/proto3 UTF-8 and enum behavior
- `docs/specs/0010-protoc-compatibility.md` — protoc compatibility work
- prost-reflect 0.16.3 — https://docs.rs/prost-reflect/latest/prost_reflect/
