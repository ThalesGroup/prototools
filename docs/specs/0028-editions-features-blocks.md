<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0028 — Editions rendering: phase 3 — emit `features { }` blocks

**Status:** implemented
**Implemented in:** 2026-05-02
**App:** reproto

---

## Purpose

Emit `features { ... }` option blocks for every element that carries a
non-default `FeatureSet` override in an edition file.  After this phase,
reproto's edition output is structurally complete: it contains the correct
field labels, packed annotations, group blocks (from phase 2) **and** the
`features` options that record non-default choices.

This is phase 3 of the strategy described in
`docs/specs/0025-editions-rendering-strategy.md`.

---

## Background

After phase 2, reproto renders edition files as proto2 with correct field
labels and cardinality.  However, the `features { }` blocks that protoc
emits to record per-element overrides are absent.  As a result, recompiling
reproto's output produces a descriptor that differs from the original: all
`features` overrides are lost, so roundtrip fails for edition files.

Phase 3 closes this gap by rendering `features { }` for:

- The file (inside `FileOptions`).
- Each message (inside `MessageOptions`).
- Each field (inside `FieldOptions`, inline with other field options).
- Each enum (inside `EnumOptions`).
- Each enum value (inside `EnumValueOptions`).
- Each oneof (inside `OneofOptions`).

Only **explicit overrides** are emitted — fields whose resolved value equals
the edition default are omitted, exactly as protoc does.  This requires
calling `HasField` on the raw `FeatureSet` proto (which only returns true for
fields that were explicitly set), not comparing resolved values.

The `features` field is a built-in field of every `*Options` message.  It is
currently rendered by the generic `render_options_from_message` path alongside
other built-in options, but its output is wrong: it would serialize the entire
resolved `FeatureSet` including defaults, or it may render through `ReSimple`
in a way that doesn't match proto text syntax.  It must be **excluded** from
the generic path and handled by a dedicated renderer.

---

## Goals

1. Add `render_features_block(ctx, fs, edition_defaults, edition) -> Block`
   to `syntax.py` (or a new `features_render.py` module): given a raw
   `FeatureSet` proto, emit a `features { ... }` block containing only the
   explicitly-set fields, in proto text format.
2. Exclude the `features` field from the generic `render_options_from_message`
   path for edition files (add it to `exclude=`).
3. Hook `render_features_block` into:
   - `re_file.py`: in `render_file_options`, add `features { }` to `FileOptions`.
   - `re_descriptor.py`: in `render()`, add `features { }` to `MessageOptions`.
   - `re_field.py`: in `render()`, add `features { }` to `FieldOptions` (inline).
   - `re_enum.py`: in `render()`, add `features { }` to `EnumOptions`.
   - `re_enum_value.py` (or wherever enum value options are rendered): add to
     `EnumValueOptions`.
   - `re_descriptor.py` oneof rendering: add to `OneofOptions`.
4. The output for proto2 and proto3 files must be byte-for-byte identical to
   the current output (no regression).

---

## Non-goals

- Emitting `edition = "...";` file headers (phase 4).
- Language-specific feature extensions (`pb.cpp`, `pb.java`, `pb.go`).
  These are extensions on `FeatureSet` and are handled by the existing
  extension options path once the base `features` field is excluded.
- Removing the A1 warning (phase 4).
- Full roundtrip testing of edition files (deferred to phase 4).

---

## Specification

### 1. `render_features_block` in `syntax.py`

```python
def render_features_block(
    fs: FeatureSet,
    depth: int,
    inline: bool = False,
) -> Block:
```

**Input:** `fs` is the raw `FeatureSet` proto from the element's options
(e.g. `field_proto.options.features`).  Only fields that `fs.HasField(name)`
returns `True` for are emitted.

**Output:** A `Block` with the `features { ... }` text.  Two formats:

- `inline=False` (standalone option statement, used for file/message/enum):
  ```
  option features.field_presence = EXPLICIT;
  option features.message_encoding = DELIMITED;
  ```
  Each explicitly-set field is a separate `option features.<name> = <value>;`
  line, at `depth`.

- `inline=True` (composite, used for field options `[... features.X = Y ...]`):
  The field options block already uses the composite format.  Each feature
  override contributes a `features.<name> = <value>,` line to the composite
  block at `depth`.

**Value rendering:** Feature fields are enum-typed.  The value must be emitted
as the enum value **name**, not the integer.  Use `feature_value_name` from
`feature_resolution.py` with `ctx.edition_defaults` to map integer → name.
`ctx` is passed as a parameter for this purpose.

Revised signature:

```python
def render_features_block(
    ctx: Context,
    fs: FeatureSet,
    depth: int,
    inline: bool = False,
) -> Block:
```

**The six standard RETENTION_RUNTIME fields** (`field_presence`, `enum_type`,
`repeated_field_encoding`, `utf8_validation`, `message_encoding`,
`json_format`) are rendered in this fixed order when set.  Unknown or
RETENTION_SOURCE fields are skipped.

**When `ctx.target_syntax != "editions"`:** return an empty `Block` immediately.
This guards both proto2/proto3 files AND edition files rendered with
`--force-proto2-output` (where `ctx.target_syntax = "proto2"` even though
`ctx.syntax = "editions"`).  Emitting `features { }` blocks in proto2 output
would be invalid proto2 syntax.

**When `fs` is absent:** callers check `HasField('features')` before calling;
the function may also guard internally and return empty if `fs` is the default
(zero-value) `FeatureSet`.

### 2. Exclude `features` from generic rendering

In every call to `render_options_from_message` / `render_options` for edition
files, add `"features"` to the `exclude` set.

The guard must be syntax-aware: for proto2/proto3 files `features` is either
absent or irrelevant and the exclude is a no-op (it's not set in the
descriptor anyway), so the exclude can be applied unconditionally without
risking proto2/proto3 regressions.

In practice, add `"features"` to every `exclude=` argument passed to
`render_options_from_message` in `re_field.py`, `re_descriptor.py`,
`re_enum.py`, and `re_file.py`'s options helpers.

### 3. Hook sites

#### 3a. File-level: `re_file.py` `render_file_options`

After existing options are rendered, if `ctx.syntax == "editions"` and
`self.this.options.HasField('features')`, call `render_features_block` with
`inline=False` and `depth=depth`.  Append to `out`.

#### 3b. Message-level: `re_descriptor.py` `render`

After existing message options are rendered (around line 471), if
`ctx.syntax == "editions"` and `self.this.options.HasField('features')`,
call `render_features_block` with `inline=False` and `depth=depth+1`.

#### 3c. Field-level: `re_field.py` `render`

Fields use composite (inline) option format.  The `features` override
contributes lines to the composite option block alongside `default`, `packed`,
`json_name`, etc.

In the `render()` method, after resolving `field_features`, if
`self.this.options.HasField('features')`, generate the `features.<name> = <val>,`
lines and append them to `opt_block` (before calling `format_composite_options`).

Use `inline=True` with `render_features_block`.

#### 3d. Enum-level: `re_enum.py` `render`

After existing enum options are rendered, if `ctx.syntax == "editions"` and
`self.this.options.HasField('features')`, call `render_features_block` with
`inline=False` and `depth=depth+1`.

#### 3e. Enum value-level

Enum value options use composite format (same as field options).  If the enum
value's options have a `features` override, emit `features.<name> = <val>,`
into the composite block.

Locate the enum value rendering in `re_enum.py` or `re_simple.py` and apply
the same pattern as 3c.

#### 3f. Oneof-level: `re_descriptor.py` oneof rendering

Oneofs may carry `features` overrides in editions (e.g. `features.field_presence`
on a oneof).  After rendering the `oneof <name> {` line, if
`ctx.syntax == "editions"` and `oneof.options.HasField('features')`, emit
`option features.<name> = <val>;` lines at `depth+2`.

### 4. Ordering within options

For consistency with protoc output, `features` is emitted **first** among all
options at each element, before other built-in and extension options.  This
matches the order protoc produces when serializing `FileOptions`,
`MessageOptions`, etc.

### 5. No changes to `feature_resolution.py`

`render_features_block` uses `feature_value_name(ctx.edition_defaults, fname, value)`
which already exists.  No new resolution logic is needed.

---

## Anomaly

No new anomaly codes are introduced.  If `feature_value_name` returns `None`
for an unknown value (e.g. a future edition field not in the table), emit the
integer as a fallback: `features.<name> = <int>,`.

---

## Testing

### Strategy

Same two-layer approach as previous phases:

1. **Unit tests** in `test_editions_rendering.py` — extend with T14–T18.
2. **Golden update** — update `editions_rendering.golden.proto` to include
   `features` blocks; the fixture already has fields with overrides.

### New unit tests (T14–T18)

| Test | What it covers |
|---|---|
| T14 — `render_features_block` inline empty | `HasField` returns False for all fields → empty Block |
| T15 — `render_features_block` inline one field | `field_presence = EXPLICIT` set → one `features.field_presence = EXPLICIT,` line |
| T16 — `render_features_block` standalone one field | `inline=False`, `message_encoding = DELIMITED` → `option features.message_encoding = DELIMITED;` |
| T17 — `render_features_block` multiple fields | Two fields set → two lines in declaration order |
| T18 — proto2/proto3 guard | `ctx.syntax = "proto2"` → empty Block returned |

### Updated golden

`editions_rendering.golden.proto` is regenerated after the implementation
to include the `features` options.  The `AllFeatures` message fields should
each carry their explicit override:

```proto
// (with edition = "proto2" header still, until phase 4)
message AllFeatures {
  string implicit_field = 1 [features.field_presence = IMPLICIT];
  optional string explicit_field = 2 [features.field_presence = EXPLICIT];
  required string required_field = 3 [features.field_presence = LEGACY_REQUIRED];
  repeated int32 expanded_ids = 4 [features.repeated_field_encoding = EXPANDED];
  optional Inner delimited_field = 5 [features.message_encoding = DELIMITED];
  optional int32 with_default = 6 [default = 42, features.field_presence = EXPLICIT];
}
```

Note: `features.field_presence = EXPLICIT` on `explicit_field` and
`with_default` — although EXPLICIT is the edition 2023 default, these fields
carry an **explicit override** in the descriptor (`HasField` returns True).
The rendered output must include it to preserve roundtrip fidelity.

---

## Modified files summary

| File | Change |
|---|---|
| `reproto/src/reproto/syntax.py` | Add `render_features_block` |
| `reproto/src/reproto/re_file.py` | Hook 3a; add `"features"` to exclude |
| `reproto/src/reproto/re_descriptor.py` | Hooks 3b, 3f; add `"features"` to exclude |
| `reproto/src/reproto/re_field.py` | Hook 3c; add `"features"` to exclude |
| `reproto/src/reproto/re_enum.py` | Hook 3d, 3e; add `"features"` to exclude |
| `reproto/src/reproto/tests/test_editions_rendering.py` | Add T14–T18 |
| `reproto/src/reproto/tests/fixtures/editions_rendering.golden.proto` | Regenerate |

No changes to `context.py`, `feature_resolution.py`, `base.py`, `cli.py`,
or `re_service.py`.

---

## Implementation note: phases 3 and 4 must be committed together

`render_features_block` is gated on `ctx.target_syntax == "editions"`.
Phase 4 is what sets `ctx.target_syntax = "editions"` for edition files.
Therefore phases 3 and 4 cannot be released independently — they must be
implemented in a single commit so that the guard fires correctly.

Corollary: with `--force-proto2-output`, `ctx.target_syntax = "proto2"`,
so `render_features_block` returns an empty block immediately, and no
`features` blocks appear in the proto2 output (correct behaviour).

---

## Empirical findings (from experiment before implementation)

1. **`features` already suppressed by generic path.**  `canonize_opt_name`
   in `mappings.py` explicitly returns `''` for `features` and
   `feature_support` field names.  `dump_option` short-circuits on empty
   name and returns an empty block.  No `exclude=` entry is needed in the
   generic path — the suppression is already in place.

2. **`ReMessage.render()` produces proto-text format** (`field: VALUE`, colon
   syntax), not proto-source format (`features.field = VALUE`, dot syntax).
   The generic path cannot produce the correct output even if the suppression
   were removed.  A dedicated renderer is required.

3. **`fdp.syntax == "editions"` confirmed** for edition files compiled with
   protoc 27+.  `fdp.edition == 1000` for edition 2023.

---

## Open questions

1. **Oneof `features` in the descriptor**: do `OneofDescriptorProto.options`
   carry `features` in practice in edition 2023 files?  Need to verify with
   a compiled fixture before implementing 3f.

2. **Enum value `features`**: same question for `EnumValueDescriptorProto`.
   The spec handles it, but if no edition fixture exercises it the golden test
   won't cover it.  A supplementary fixture may be needed.
