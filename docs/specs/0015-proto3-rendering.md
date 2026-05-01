<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0015 — Proto3 rendering

**Status:** draft
**App:** reproto

---

## Problem

Reproto currently emits `syntax = "proto2";` for every output file,
regardless of what `FileDescriptorProto.syntax` says.  When the input was
originally written in proto3 this produces incorrect output:

- The `syntax` line is wrong.
- Proto3-specific constructs — absence of field labels, implicit presence,
  implicit defaults, implicit packed encoding — are rendered with proto2
  conventions that change their semantics or produce files that `protoc`
  rejects.

The goal of rendering is **roundtrip fidelity**: given a `.proto` file
compiled to `pb1` by `protoc`, reproto's output recompiled to `pb2` must
satisfy `pb1 == pb2` (byte-identical `FileDescriptorProto` serialization).
Achieving this requires matching the original syntax in the output.

---

## Goals

1. When `FileDescriptorProto.syntax == "proto3"`, render `syntax = "proto3";`
   and apply proto3-specific rendering rules to every construct in that file.
2. When `FileDescriptorProto.syntax == "proto2"` (or empty, which historically
   means proto2), preserve current proto2 rendering unchanged — zero
   regression.
3. Produce output that `protoc` accepts without errors or warnings for any
   well-formed descriptor.
4. Emit a `cli_warning` — never a hard error — for any descriptor construct
   that is inconsistent with the declared syntax (see §Inconsistency
   handling).  Provide a per-file syntax override mechanism for cases where
   the `syntax` field in the descriptor is known to be wrong.
5. Be architecturally ready for additional syntax families (editions, and
   anything after) without requiring a rewrite when they are added.

---

## Non-goals

- Supporting `edition = "2023"` or any other edition in this spec.
- Changing the rendering of proto2 files (zero regression).
- Altering the CLI interface beyond the per-file syntax override.
- Exhaustive edition semantics (covered in a future spec — see §Future).

---

## Background: proto2 vs proto3 — rendering differences

This section catalogues every construct that renders differently between the
two syntaxes.  It is the authoritative reference for the implementation.
Full research is in `docs/proto2-proto3-findings.md`.  All items
have been empirically verified.

### Syntax detection from `FileDescriptorProto`

`FileDescriptorProto` carries two fields that encode the syntax family:

- `syntax` (string): always present.  Values: `""` (legacy proto2 — no
  `syntax` statement in source), `"proto2"`, `"proto3"`, or `"editions"`.
- `edition` (enum `Edition`): present and non-zero only when
  `syntax == "editions"`.  Values include `EDITION_2023 = 1000`,
  `EDITION_2024 = 1001`, etc.

The `effective_syntax()` function therefore maps:

| `fdp.syntax` | `fdp.edition` | Result |
|--------------|---------------|--------|
| `""` | — | `"proto2"` |
| `"proto2"` | — | `"proto2"` |
| `"proto3"` | — | `"proto3"` |
| `"editions"` | `EDITION_2023` | `"editions"` (not yet supported) |
| anything else | — | `"proto2"` + `cli_warning` |

**Empirically confirmed (findings doc Part I):** protoc always sets
`syntax = ""` for proto2 files (both explicit and implicit).  The string
`"proto2"` never appears in a real descriptor.

### 1. Syntax line

| proto2 | proto3 |
|--------|--------|
| `syntax = "proto2";` | `syntax = "proto3";` |

### 2. Field labels

Proto2 requires an explicit label (`optional`, `required`, `repeated`) on
every non-oneof field.  Proto3 forbids `required` and makes singular fields
implicit (no label keyword).

| Situation | proto2 rendering | proto3 rendering |
|-----------|-----------------|-----------------|
| Singular field, not in oneof | `optional T f = N;` | `T f = N;` |
| Singular field, explicit presence (`proto3_optional=true`) | `optional T f = N;` | `optional T f = N;` |
| `required` field | `required T f = N;` | illegal — see §Inconsistencies |
| Repeated field | `repeated T f = N;` | `repeated T f = N;` |
| Field inside `oneof` | no label | no label (same) |

In the descriptor, proto3 implicit fields still have `label = LABEL_OPTIONAL`
— the absence of a label keyword is not a different enum value; it is
inferred from `syntax == "proto3"` AND `proto3_optional == false`.

`proto3_optional = true` means the source had an explicit `optional` keyword.
Such fields are placed in a synthetic oneof whose name starts with `_`.
Reproto suppresses the synthetic oneof at the oneof level and emits
`optional` at the field level (see §11 for the precise detection rule).

### 3. Default values

Proto2 allows `[default = <value>]` on optional/required scalar fields.
Proto3 forbids explicit defaults entirely (zero value is always the
implicit default).

| Situation | proto2 rendering | proto3 rendering |
|-----------|-----------------|-----------------|
| `default_value` set in descriptor | `[default = <val>]` | omit; emit `cli_warning` |

### 4. Packed repeated fields

Proto2: `packed` defaults to `false`; write `[packed = true]` to enable.
Proto3: `packed` defaults to `true` for all numeric and enum types; write
`[packed = false]` to opt out.

Rendering rule:

- `FieldOptions.packed` not set (`HasField == False`):
  - Proto2: emit nothing (unpacked is the default; redundant to state it).
  - Proto3: emit nothing (packed is the default; redundant to state it).
  In both cases the omission is correct for roundtrip fidelity, because
  protoc itself does not record a default-value option — it omits the field.
- `FieldOptions.packed = true` (explicitly set) → emit `[packed = true]`
  in both syntaxes.
- `FieldOptions.packed = false` (explicitly set) → emit `[packed = false]`
  in both syntaxes.

The user's concern — that a proto3 unset-packed field might need an explicit
`[packed = false]` — would only arise if we were outputting that field in a
proto2 file, since proto2 defaults to unpacked.  Because we now output proto3
files as proto3, the unset case means "packed" in proto3, which is correct.
The rule **emit nothing when unset** achieves roundtrip identity in both
syntaxes.

**Empirically confirmed (findings doc Part IV):** protoc never sets
`packed = true` implicitly.  Default-packed proto3 fields have
`HasField("packed") == False`.  An explicit `[packed = true]` in source
produces `HasField("packed") == True, packed == True`.

This rule is **identical for proto2 and proto3** — no syntax-specific logic
needed; it lives directly in `re_field.py`.

### 5. `required` fields

Proto2-only.  `label = LABEL_REQUIRED` in the descriptor.  See
§Inconsistency handling.

### 6. Extension fields and extension ranges

Proto3 restricts extensions as follows:

- `extensions N to M;` ranges on user-defined messages → proto2 only.
- `extend UserMessage { ... }` blocks → proto2 only.
- `extend SomeOptions { ... }` blocks where the extendee is one of the nine
  `*Options` messages from `google/protobuf/descriptor.proto`
  (`FileOptions`, `MessageOptions`, `FieldOptions`, `OneofOptions`,
  `ExtensionRangeOptions`, `EnumOptions`, `EnumValueOptions`,
  `ServiceOptions`, `MethodOptions`) → **allowed in proto3** (custom options).

In other words: proto3 files may define custom options via `extend *Options`,
but may not extend user-defined messages or declare extension ranges on them.

See §Inconsistency handling.

### 7. Groups (`TYPE_GROUP`)

Proto2-only (deprecated even there).  In editions, replaced by
`features.message_encoding = DELIMITED`.  See §Inconsistency handling.

### 8. `import weak`

`import weak` is proto2-specific.  Proto3 does not support it.
See §Inconsistency handling.

Reproto **already supports** rendering `import weak` (see `re_file.py`,
`weak_dependency` handling).  The only change required is to emit a
`cli_warning` and fall back to plain `import` when a weak import appears
in a proto3 file.

### 9. Enum `allow_alias`

Valid and identical in both syntaxes.  No rendering difference.

Reproto **already supports** `allow_alias`: it is rendered via the generic
`EnumOptions` rendering path, not as a special case.  No change required.

### 10. Enum zero-value constraint

Proto3 requires the first enum value to be 0.  Enforced at compile time;
no well-formed descriptor will violate it.  No rendering difference.

### 11. Oneof blocks and synthetic oneof suppression

Oneof syntax (`oneof name { ... }`) is identical in proto2 and proto3.

**Synthetic oneofs** are a proto3-only artefact.  When a proto3 field is
declared with an explicit `optional` keyword (e.g., `optional string foo
= 1;`), protoc creates:

1. A `OneofDescriptorProto` entry in `message.oneof_decl` whose `name`
   starts with `_` (e.g., `_foo`).
2. `FieldDescriptorProto.proto3_optional = true` on the field.
3. `FieldDescriptorProto.oneof_index` pointing to that synthetic oneof.

Detection rule: a oneof is **synthetic** if and only if all of the following
hold:
- `oneof.name` starts with `_`
- it contains exactly one field
- that field has `proto3_optional = true`

Rendering rule:
- Suppress the synthetic oneof entirely (do not emit `oneof _foo { ... }`).
- Render the field as `optional T foo = N;` (with the `optional` label,
  outside any oneof block).

This is already partially implemented in the current codebase (synthetic
oneofs are skipped during oneof rendering), but the `optional` label is not
yet emitted because reproto currently outputs everything as proto2 where
the field is rendered as a regular `optional` field.  The new proto3 path
must explicitly check `proto3_optional` to decide the label (see §2).

### 12. Map fields

Identical in both syntaxes.  No rendering difference.

### 13. `import` statements (non-weak)

No difference.

### 14. File options

`FileOptions` fields are syntax-neutral.  No rendering difference.

### 15. `MessageOptions.message_set_wire_format`

Proto2-specific legacy option.  Treat as inconsistency if present in a
proto3 descriptor.

### 16. `json_name` on fields

`FieldDescriptorProto.json_name` is always populated in both syntaxes.
`HasField("json_name")` is always `true` — cannot be used to detect custom
overrides.  Emit `[json_name = "..."]` only when the value differs from the
default camelCase derivation (split on `_`, lowercase first component,
capitalize subsequent components, join).

This fix is syntax-independent and is included here because proto3 files
are more likely to exercise it.

### 17. UTF-8 validation

Proto2: string fields not UTF-8 validated by default.
Proto3: string fields UTF-8 validated by default.

**Rendering implication**: none — this is a runtime semantic difference
with no syntax representation in proto2 or proto3 source.  The
`utf8_validation` feature only exists in editions.

### 18. Services and RPCs

No syntax difference.

### 19. Empirically resolved items

All items below have been verified.  Full details in `docs/proto2-proto3-findings.md`.

- **`FieldOptions.weak`** — syntax-neutral; accepted in proto2 and proto3
  (findings Part XI).
- **`FieldOptions.ctype`, `FieldOptions.jstype`** — syntax-neutral (findings
  Part XI).
- **`MessageOptions.no_standard_descriptor_accessor`** — syntax-neutral
  (findings Part XII).
- **`deprecated_legacy_json_field_conflicts`** — survives into descriptor in
  both proto2 and proto3; renders via generic options path unchanged (findings
  Part XVIII).
- **Extension range options** (`verification`, `declaration`) —
  `RETENTION_SOURCE`; not stored; no rendering needed (findings Part VII).
- **`import weak` in editions** — accepted; `weak_dependency` populated
  identically to proto2 (findings Part XVI).
- **`message_set_wire_format` in editions** — accepted by protoc 32.1;
  renders as proto2 (findings Part XVII).
- **Extension range `end` semantics** — exclusive upper bound; `to max`
  sentinels are `536870912` (normal) and `2147483647` (message_set)
  (findings Part XIX).

---

## Architecture: the `re_syntax.py` module

### Motivation

The differences between proto2 and proto3 affect a well-defined set of
rendering decisions.  The architecture must answer two questions:

1. **Flexibility**: will it accommodate editions without structural surgery?
2. **Readability**: does it avoid unwarranted code duplication?

**Editions require per-element resolution.**  In an edition file, different
fields in the same message may have different `field_presence` or
`repeated_field_encoding` values (set explicitly at the field level,
inherited from the message, or defaulting to the edition default).  A design
that computes a single syntax value per file is insufficient for editions: it
must inspect the resolved feature set at each element.

### Design: `re_syntax.py` — a module of stateless helper functions

`re_syntax.py` is a new module that centralises all syntax-dependent
rendering decisions.  Its purpose is to keep the renderers (`re_file.py`,
`re_field.py`, `re_descriptor.py`, etc.) free of `if syntax == "proto3":`
branches scattered throughout their code.  Instead, each renderer calls a
named function that expresses the intent — `field_label(...)`,
`allow_extensions(...)`, etc. — and the syntax-specific logic lives
exclusively in `re_syntax.py`.

The module contains **stateless free functions** (no classes):

```python
# re_syntax.py

def effective_syntax(fdp: FileDescriptorProto, overrides: dict[str, str]) -> str:
    """
    Return "proto2", "proto3", or "editions" for the given file.
    overrides: map of proto file name → forced syntax (from --syntax-overrides).
    Emits cli_warning when the override differs from the detected syntax.
    """
    ...

def field_label(syntax: str, field: FieldDescriptorProto, is_oneof: bool) -> str:
    """
    Return the label prefix to emit before the field type (with trailing
    space), or '' if no label should be emitted.
    Handles proto3_optional, LABEL_REQUIRED inconsistency warning, etc.
    """
    ...

def should_render_default(syntax: str, field: FieldDescriptorProto) -> bool:
    """
    Return True iff [default = ...] should be rendered for this field.
    Emits cli_warning if default_value is set in a proto3 file.
    """
    ...

def allow_extensions(syntax: str) -> bool:
    """Return True iff extend blocks and extension ranges are legal."""
    ...

def allow_groups(syntax: str) -> bool:
    """Return True iff TYPE_GROUP fields are legal."""
    ...

def allow_weak_import(syntax: str) -> bool:
    """Return True iff import weak is legal."""
    ...

def allow_message_set_wire_format(syntax: str) -> bool:
    """Return True iff MessageOptions.message_set_wire_format is legal."""
    ...
```

When edition support is added, each function that needs to consult resolved
features gains a `features: FeatureSet | None = None` parameter and handles
the `"editions"` syntax branch.  The renderers that call these functions
change minimally — the signatures extend but the structure stays.

**Why not a class hierarchy?**
A `Proto2Syntax` / `Proto3Syntax` / `EditionSyntax` class hierarchy would
work for proto2/proto3 (both stateless) but breaks for editions: an
`EditionSyntax` object would need to carry **per-element** resolved feature
state, making it structurally incompatible with the stateless sibling
classes.  Free functions avoid this mismatch.

**Avoiding code duplication.**
Proto2 and proto3 share the vast majority of rendering logic.  The free
functions express the differences as short conditional branches.  The many
shared cases (packed mirroring, `json_name`, file options, oneof body, map
fields) remain unconditional in their call sites.

### Syntax propagation via `Context`

`effective_syntax(fdp, overrides)` is called once per file at the top of
`ReFileDescriptorProto.render()` and stored in `ctx.syntax`.

Using `Context` (rather than threading a new parameter through every
`render()` call) keeps the signatures of the existing rendering methods
unchanged.  The value is overwritten each time a new file starts rendering.
This is safe because rendering is single-threaded and files are rendered one
at a time — there is no risk of one file's syntax polluting another mid-render.

### Per-file syntax override

Some descriptors have an incorrect `syntax` field (hand-crafted `.pb`,
tool bugs).  The user must be able to override the detected syntax per file.

Proposed mechanism: a **YAML override file** passed via
`--syntax-overrides <path>`.  Format:

```yaml
# syntax-overrides.yaml
overrides:
  - file: "foo/bar.proto"
    syntax: proto2
  - file: "baz/qux.proto"
    syntax: proto3
```

`effective_syntax()` consults this map before reading `fdp.syntax`.  A
`cli_warning` is emitted for each file where the override differs from the
detected value.

Alternative: flags `--force-proto2 <glob>` / `--force-proto3 <glob>`.

> **Open question (deferred to review)**: YAML override file or flags?

---

## Specification

### New module: `re_syntax.py`

Contains all functions described in §Architecture.  No classes.  See above
for the full function list and docstrings.

### `Context` changes

- Add `syntax: str = "proto2"` — set by `effective_syntax()` at the start
  of each file's rendering.
- Add `syntax_overrides: dict[str, str]` — populated from
  `--syntax-overrides` at startup; default empty dict.

### `re_file.py` changes

- Remove the "Note: The original file used …" comment workaround.
- At the top of `render()`: `ctx.syntax = effective_syntax(self.this, ctx.syntax_overrides)`.
- Emit `syntax = "{ctx.syntax}";`.
- When `ctx.syntax == "editions"`: emit `cli_warning` ("editions not yet
  supported; rendering as proto2") and proceed with proto2 rendering.
- Guard file-level extensions behind `allow_extensions(ctx.syntax)`.
- Guard `import weak` behind `allow_weak_import(ctx.syntax)`.

### `re_field.py` changes

`render(ctx, depth, is_oneof)` — signature unchanged:

1. Replace label block with `field_label(ctx.syntax, self.this, is_oneof)`.
2. Replace `_render_default_value` guard with
   `should_render_default(ctx.syntax, self.this)`.
3. Packed: always mirror `HasField("packed")` state directly
   (syntax-independent, see §4).
4. Guard group rendering behind `allow_groups(ctx.syntax)`; warn and degrade
   if false.

### `re_descriptor.py` changes

- Guard `extension_range` rendering behind `allow_extensions(ctx.syntax)`.
- Guard `render_extensions()` behind `allow_extensions(ctx.syntax)`.
- Guard `MessageOptions.message_set_wire_format` behind
  `allow_message_set_wire_format(ctx.syntax)`.
- Oneof rendering: skip any oneof that is synthetic (see §11 detection rule).

### `re_enum.py`, `re_enum_value.py`, `re_service.py`, `re_method.py`

No changes needed for proto2/proto3.  `ctx.syntax` is available if needed
for future items from §19.

### `re_field.py` — `json_name` fix (§16)

Emit `[json_name = "..."]` only when `field.json_name` differs from the
auto-derived camelCase of `field.name`.  Syntax-independent.

### Fixture updates

- Keep existing `field_comprehensive.proto` and `file_comprehensive.proto`
  as-is (proto2).
- Add:
  - `field_comprehensive_proto3.proto`
  - `file_comprehensive_proto3.proto`

### Test updates

Add roundtrip tests for each new proto3 fixture following the existing
pattern in `test_roundtrip.py`.

---

## Inconsistency handling

An *inconsistency* is a descriptor value that is illegal under the declared
syntax.  Policy: **warn and degrade gracefully; never crash**.

| Inconsistency | `cli_warning` (summary) | Degraded rendering |
|---------------|-------------------------|--------------------|
| Proto3, `required` field | "required fields are not valid in proto3" | render as `T f = N;` (no label) |
| Proto3, extension range | "extension ranges are not valid in proto3" | omit the `extensions` statement |
| Proto3, `extend UserMsg` block | "extend blocks on user-defined messages are not valid in proto3" | omit the block |
| Proto3, `extend *Options` block | *(not an inconsistency — allowed in proto3 for custom options)* | emit normally |
| Proto3, `import weak` | "import weak is not valid in proto3" | render as plain `import` |
| Proto3, `default_value` set | "explicit default values are not valid in proto3" | omit `[default = ...]` |
| Proto3, `TYPE_GROUP` field | "groups are not valid in proto3" | render as plain message field |
| Proto3, `message_set_wire_format` | "message_set_wire_format is not valid in proto3" | omit the option |
| Edition file (any) | "editions not yet supported; rendering as proto2" | full proto2 rendering |

Warnings must include file name and enclosing message/field name where
applicable.

The `--syntax-overrides` mechanism suppresses spurious inconsistency
warnings when the descriptor's `syntax` field is known to be wrong.

---

## Future: editions

`docs/proto2-proto3-findings.md` §III already documents the full edition
feature catalogue (8 core features, per-element inheritance, edition 2023
defaults, and the mapping to proto2/proto3 semantics).

Key architectural insight from that research: in an edition file, **two
fields in the same message may require different rendering rules** (e.g.,
one field has `field_presence = IMPLICIT`, another has `EXPLICIT`).  The
`re_syntax.py` free-function design accommodates this: when editions are
implemented, each function that needs to consult features gains a
`features: FeatureSet | None = None` parameter.  The caller resolves the
per-element feature set (by merging edition defaults → file overrides →
message overrides → field/enum overrides) and passes it in.

Implementation sequence (future spec):

1. Implement `FeatureSet` resolution in `re_syntax.py`.
2. Add `features` parameter to each `re_syntax` function.
3. Update `re_file.py` to detect `ctx.syntax == "editions"` and resolve
   file-level features.
4. Update each renderer to resolve per-element features and pass them to
   `re_syntax` functions.
5. Remove the edition fallback warning.

---

## Open questions

1. **Per-file override**: YAML file or flags?  Deferred to review.
2. **`json_name` fix scope**: include in this spec (low-risk) or separate?
3. **Empirical verification complete** — all items in §19 and §4 have been resolved.
   See `docs/proto2-proto3-findings.md` Parts I–XX.

---

## Annex A — Difficulty assessment: proto3 and editions support

### Proto2 → Proto2 + Proto3

**Moderate difficulty, well-bounded.**

The findings document provides a complete, empirically verified catalogue of
every rendering difference.  The changes are surgical:

- `re_file.py`: emit correct `syntax` line; guard `import weak`, `extend`,
  `extensions` for proto3.
- `re_field.py`: suppress label for non-optional proto3 fields; emit
  `optional` for `proto3_optional` fields; stop emitting `[default = ...]`;
  fix `json_name` detection.
- `re_descriptor.py`: suppress extension ranges; guard
  `message_set_wire_format`; suppress synthetic oneofs.
- New `re_syntax.py`: ~5 pure stateless functions, no state.

The spec is complete and every rendering decision is resolved.  No unknowns
remain.  The one non-trivial piece is **synthetic oneof suppression** (the
3-condition detection rule), but it is fully specified.  Everything else is
straightforward conditionals.

### Proto2 + Proto3 → Proto2 + Proto3 + Editions

**Significantly harder**, for structural reasons rather than sheer volume.

The core difficulty is that editions require **per-element feature
resolution**, not per-file.  Two fields in the same message can have
different `field_presence`, different `repeated_field_encoding`, etc.  The
resolution algorithm is:

1. Start from the edition defaults (a fixed table per edition, stored in
   `FeatureSetDefaults` inside `descriptor.proto`).
2. Apply file-level feature overrides (`FileOptions.features`).
3. Apply message-level overrides (`MessageOptions.features`).
4. Apply field/enum-level overrides (`FieldOptions.features`, etc.).

Only explicit overrides are stored in the descriptor — the defaults are not
propagated.  reproto must implement this resolution algorithm itself.

Concrete sub-problems:

- **Feature resolution engine**: ~20–50 lines once the defaults table is
  known; requires understanding `FeatureSet` and `FeatureSetDefaults`.
- **`message_encoding = DELIMITED`**: replaces `TYPE_GROUP` — different
  detection path and different rendering.
- **`field_presence = LEGACY_REQUIRED`**: replaces `LABEL_REQUIRED`.
- **`repeated_field_encoding`**: replaces the `packed` option entirely.
- **`enum_type = CLOSED/OPEN`**: must be resolved per-enum.
- **`FeatureSetDefaults` table**: must be extracted from the protobuf
  distribution once and embedded as a constant.  Not yet done empirically.

### Cross-file override contamination

Feature inheritance is **strictly within a single file**.  The hierarchy is:

```
edition defaults → file-level → message-level → field/enum-level
```

An imported file's feature settings have no effect on the importer.  This is
confirmed empirically: Part XIII of the findings shows `ClosedEnum` inheriting
`enum_type = CLOSED` from its own file's `FileOptions.features` only.  No
cross-file propagation occurs, and protoc does not store inherited values —
only explicit overrides appear in the descriptor.

### Resolution from `.pb` files

The `.pb` descriptor contains **all data needed** for feature resolution:

- `fdp.edition` — identifies the edition and its default table.
- `fdp.options.features` — file-level overrides.
- `msg.options.features` — message-level overrides.
- `field.options.features` — field-level overrides.

The `.pb`-first approach (which reproto uses) does not add difficulty — it is
arguably simpler than working from `.proto` source, since the descriptor
already contains the pre-parsed override values.  No `.proto` source file is
needed.

### Summary

| Concern | Assessment |
|---------|-----------|
| Proto3 support difficulty | Moderate — spec complete, no unknowns |
| Editions support difficulty | Higher — one concentrated hard part |
| Cross-file override contamination | Does not exist — file boundaries are hard |
| Data available in `.pb` for resolution | Complete — all explicit overrides stored |
| External dependency needed | `FeatureSetDefaults` table — extractable once from `descriptor.proto` |
| Resolution algorithm complexity | Simple 3-level merge once defaults are known |
| Main unknown for editions | Exact default values per edition (needs empirical extraction) |

**Recommended sequencing:** implement proto3 support first (this spec), then
editions as a follow-on spec.  The architecture already anticipates editions
correctly — free functions in `re_syntax.py` with a `features` parameter.
