<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0027 — Editions rendering: phase 2 — thread ResolvedFeatures into rendering helpers

**Status:** implemented
**Implemented in:** 2026-05-02
**App:** reproto

---

## Purpose

Thread the `ResolvedFeatures` value (produced by the engine from spec 0026)
into the rendering helpers in `syntax.py` and their callers in `re_field.py`
and `re_descriptor.py`, so that edition files are rendered using per-element
resolved features rather than a file-level syntax string.

This is phase 2 of the strategy described in
`docs/specs/0025-editions-rendering-strategy.md`.

---

## Background

After spec 0026, reproto can resolve the effective `FeatureSet` for any element
in an edition file.  However, none of the rendering helpers (`field_label`,
`packed_option`, `allow_groups`, `is_synthetic_oneof`, `should_render_default`,
etc.) in `syntax.py` know about features — they read `ctx.target_syntax` and
make binary proto2/proto3 decisions.

For edition files, the per-element `ResolvedFeatures` must drive those same
decisions:

| Current decision point | Driven by | Edition equivalent |
|---|---|---|
| Field label (`optional`/`required`/`repeated`/`''`) | `ctx.target_syntax`, `field.label`, `field.proto3_optional` | `resolved.field_presence` |
| Packed encoding | `ctx.syntax`, `ctx.target_syntax`, `fo_msg.packed` | `resolved.repeated_field_encoding` |
| Group rendering | `ctx.target_syntax` (`allow_groups`) | `resolved.message_encoding` (`DELIMITED`) |
| Synthetic oneof suppression | `ctx.target_syntax == "proto3"`, `proto3_optional` | `resolved.field_presence == IMPLICIT` |
| Default value rendering | `ctx.target_syntax != "proto3"` | `resolved.field_presence != IMPLICIT` |

The design constraint from the strategy doc is:

> proto2 and proto3 rendering paths are **untouched** — they continue to pass
> `features=None` and nothing changes for them.

---

## Goals

1. Add an optional `features: ResolvedFeatures | None = None` parameter to
   the affected helpers in `syntax.py`.  When `features` is `None` the helpers
   behave exactly as today (backward-compatible).
2. In `re_field.py` (`ReFieldDescriptorProto.render`), resolve the element's
   `ResolvedFeatures` once at the top of the method and pass it to the
   affected helpers.
3. In `re_descriptor.py` (`ReDescriptorProto.render_oneofs` and the oneof
   inline blocks inside `render`), pass the resolved features when calling
   `field_label` and `is_synthetic_oneof`.
4. The rendering output for proto2 and proto3 files must be byte-for-byte
   identical to the current output (no regression).
5. Edition files must produce correct output for the constructs covered in this
   spec (field labels, packed, groups-as-delimited, synthetic oneofs, defaults).

---

## Non-goals

- Emitting `edition = "...";` file headers (phase 4).
- Emitting `features { ... }` option blocks (phase 3).
- Language-specific feature extensions (`pb.cpp` etc.).
- `allow_extend_block`, `allow_extension_ranges`, `allow_weak_import`,
  `allow_message_set_wire_format` — these are message/file-level constructs
  not controlled by per-element features; they remain syntax-string-based.
- Full roundtrip testing of edition files (deferred to phase 4 when the
  `edition = "...";` header and `features { }` blocks are also rendered).

---

## Specification

### 1. `resolve_field_features` helper in `re_field.py`

Add a private helper that resolves features for a field, given its containing
context:

```python
def _resolve_field_features(
    ctx: Context,
    fdp: FileDescriptorProto,
    msg_proto: DescriptorProto | None,
    field_proto: FieldDescriptorProto,
) -> ResolvedFeatures | None:
```

- Returns `None` if `ctx.syntax != "editions"` (avoids any work for
  proto2/proto3 files).
- Otherwise calls:

```python
file_fs   = fdp.options.features   if fdp.options.HasField('features')   else None
msg_fs    = msg_proto.options.features if (msg_proto is not None and
                msg_proto.options.HasField('features')) else None
field_fs  = field_proto.options.features if field_proto.options.HasField('features') else None
return resolve_features(ctx.edition_defaults, fdp.edition, file_fs, msg_fs, field_fs)
```

The `fdp` and `msg_proto` are already available inside `render()` via the
parent chain (`self.parent` gives the containing `ReDescriptorProto` or
`ReFileDescriptorProto`, from which `this` gives the proto message).

Resolution is done once per `render()` call and the result is passed to all
affected helpers.

### 2. Changes to `syntax.py` helpers

All changes add an optional `features: ResolvedFeatures | None = None`
parameter.  When `features is None` the existing logic runs unchanged.

#### 2a. `field_label`

```python
def field_label(
    ctx: Context,
    field: FieldDescriptorProto,
    is_oneof: bool,
    features: ResolvedFeatures | None = None,
) -> str:
```

When `features is not None` (editions path):

- `LABEL_REPEATED` → `'repeated '` (unchanged — repeated is still a label
  field in editions).
- `field_presence == LEGACY_REQUIRED` → `'required '`
- `field_presence == IMPLICIT` → `''` (proto3-like implicit singular)
- `field_presence == EXPLICIT` → `'optional '`
- `is_oneof` → `''` (unchanged)

The existing proto2/proto3 branches run when `features is None`.

#### 2b. `packed_option`

```python
def packed_option(
    ctx: Context,
    has_field: bool,
    effective_packed: bool,
    features: ResolvedFeatures | None = None,
) -> str | None:
```

When `features is not None` (editions path):

- `repeated_field_encoding == PACKED` → `None` (packed is the default; emit
  nothing if it matches the edition default, which is PACKED for 2023).
- `repeated_field_encoding == EXPANDED` → `None` (the `features { }` block
  will carry this — emitting a legacy `[packed = false]` is wrong for editions
  and is deferred to phase 3).
- If `has_field` is True (an explicit legacy `packed` option is present in the
  descriptor) → emit it as today (legacy round-trip case).

This means that for edition files, `packed_option` returns `None` unless a
legacy `packed` field is set — the encoding intent will be expressed via
`features { }` in phase 3.

#### 2c. `allow_groups`

```python
def allow_groups(ctx: Context, features: ResolvedFeatures | None = None) -> bool:
```

When `features is not None`:

- `message_encoding == DELIMITED` → `True` (render as a group-style block).
- Otherwise → `False`.

The edition-correct way to render a `DELIMITED`-encoded message field is as
a group.  The existing proto2 check (`ctx.target_syntax == "proto2"`) runs
when `features is None`.

#### 2d. `is_synthetic_oneof`

```python
def is_synthetic_oneof(
    ctx: Context,
    oneof_name: str,
    members: list,
    features: ResolvedFeatures | None = None,
) -> bool:
```

When `features is not None` (editions path):

- A oneof is synthetic if it has exactly one member and that member's resolved
  `field_presence == IMPLICIT`.  The `proto3_optional` flag is not set in
  edition descriptors; `IMPLICIT` is the edition-correct signal.
- The `oneof_name.startswith('_')` heuristic is kept as an additional guard
  (protoc still generates underscore-prefixed names for synthetic oneofs in
  editions).

When `features is None` the existing proto3 detection runs unchanged.

Note: `is_synthetic_oneof` is called from `re_descriptor.py`, which iterates
over fields.  The per-field `features` must be resolved there too (see §3).

#### 2e. `should_render_default`

```python
def should_render_default(
    ctx: Context,
    field: FieldDescriptorProto,
    features: ResolvedFeatures | None = None,
) -> bool:
```

When `features is not None`:

- `field_presence == IMPLICIT` → `False` (implicit fields have no default).
- Otherwise → `True` if `field.HasField('default_value')`.

When `features is None` the existing logic (suppress for proto3) runs unchanged.

### 3. Changes to `re_field.py`

In `ReFieldDescriptorProto.render()`:

1. At the top, after the map-field early return, resolve features:

```python
features = _resolve_field_features(ctx, ..., self.this)
```

2. Pass `features` to `field_label`, `packed_option`, `allow_groups`,
   `should_render_default`.

The parent message proto is obtained from `self.parent.this` when the parent
is a `ReDescriptorProto`, or `None` when the parent is a `ReFileDescriptorProto`
(top-level extension fields).

The `fdp` (file descriptor proto) is obtained by walking up the parent chain
to the `ReFileDescriptorProto`.

### 4. Changes to `re_descriptor.py`

`is_synthetic_oneof` is called in two places in `re_descriptor.py` —
`render_oneofs()` (the standalone method) and the inline oneof block inside
`render()`.  Both need access to per-field features.

Because `re_descriptor.py` iterates over `self.field`, the field's features
must be resolved inline.  Add a local helper or inline call:

```python
field_features = _resolve_field_features(ctx, fdp, self.this, field_proto)
if is_synthetic_oneof(ctx, oneof.name, members, features=field_features):
    ...
```

The `fdp` is obtained by walking `self.parent` up to `ReFileDescriptorProto`.

`field_label` is also called from `render_extensions` in `re_descriptor.py`
for extension fields inside `extend` blocks.  Extension fields in edition
files use the same resolution path: file-level + no message level (extensions
are not members of a message) + field-level.

### 5. `fdp` and `msg_proto` availability

Both `render()` methods have access to the parent chain via `self.parent`.
A small private helper in `re_field.py` (or a shared utility) can walk the
chain:

```python
def _get_file_and_msg(node):
    """Return (ReFileDescriptorProto, ReDescriptorProto | None) for a field node."""
    from .re_descriptor import ReDescriptorProto
    from .re_file import ReFileDescriptorProto
    parent = node.parent
    if isinstance(parent, ReFileDescriptorProto):
        return parent, None
    assert isinstance(parent, ReDescriptorProto)
    grandparent = parent.parent
    while not isinstance(grandparent, ReFileDescriptorProto):
        grandparent = grandparent.parent
    return grandparent, parent
```

### 6. No changes to `re_enum.py`

`ReEnumDescriptorProto.render()` does not call any of the affected helpers.
Enum-level features (`enum_type = CLOSED`) affect how enum values are handled
by the *consuming* code (e.g. the runtime), not how the enum is textually
rendered.  The `features { }` block for enums is deferred to phase 3.

### 7. `re_file.py`: no changes in this phase

`re_file.py` sets `ctx.syntax` and `ctx.target_syntax`.  For edition files,
`ctx.target_syntax` remains `"proto2"` until phase 4.  This means the file
header still emits `syntax = "proto2";` with the existing A1 warning — that
is intentional and unchanged.

---

## Decision: `ctx.target_syntax` for edition files

For edition files the current stub sets `ctx.target_syntax = "proto2"`.
The phase 2 design deliberately does **not** change this.  The per-element
`features` parameter carries the edition-specific decisions; `target_syntax`
continues to gate constructs that are truly file-level (extension ranges,
`extend` blocks, `message_set_wire_format`).  Changing `target_syntax` to
`"editions"` would require auditing every `ctx.target_syntax` comparison in
the codebase; that is deferred to phase 4 when the full edition output format
is ready.

---

## Testing

### Strategy

Two layers, same as spec 0026:

1. **Unit tests** — exercise each modified helper directly with synthetic
   `ResolvedFeatures` values (both `features=None` and `features=<value>`).
2. **Golden regression test** — compile an edition fixture with `protoc`,
   run reproto, compare output against a checked-in golden `.proto` file.

### Test file: `test_editions_rendering.py`

Location: `reproto/src/reproto/tests/test_editions_rendering.py`.

| Test | What it covers |
|---|---|
| T1 — `field_label` proto2/proto3 unchanged | Pass `features=None`; assert existing behaviour for all label combinations |
| T2 — `field_label` editions EXPLICIT | `features.field_presence = EXPLICIT` → `'optional '` |
| T3 — `field_label` editions IMPLICIT | `features.field_presence = IMPLICIT` → `''` |
| T4 — `field_label` editions LEGACY_REQUIRED | `features.field_presence = LEGACY_REQUIRED` → `'required '` |
| T5 — `packed_option` editions PACKED default | `features.repeated_field_encoding = PACKED`, `has_field=False` → `None` |
| T6 — `packed_option` editions EXPANDED | `features.repeated_field_encoding = EXPANDED`, `has_field=False` → `None` (deferred to phase 3) |
| T7 — `allow_groups` editions DELIMITED | `features.message_encoding = DELIMITED` → `True` |
| T8 — `allow_groups` editions LENGTH_PREFIXED | `features.message_encoding = LENGTH_PREFIXED` → `False` |
| T9 — `is_synthetic_oneof` editions IMPLICIT | Single member with `IMPLICIT` presence → `True` |
| T10 — `is_synthetic_oneof` editions EXPLICIT | Single member with `EXPLICIT` presence → `False` |
| T11 — `should_render_default` editions IMPLICIT | `field_presence = IMPLICIT` → `False` even if `default_value` is set |
| T12 — `should_render_default` editions EXPLICIT | `field_presence = EXPLICIT`, `default_value` set → `True` |
| T13 — proto2/proto3 regression | Run reproto on the existing proto2 and proto3 test fixtures; assert output identical to current golden |

### Golden fixture: `editions_rendering.proto`

Location: `reproto/src/reproto/tests/fixtures/editions_rendering.proto`.

A handcrafted edition 2023 file that exercises every decision point:

- A field with `field_presence = IMPLICIT` (synthetic-oneof-like, no label).
- A field with no override (inherits `EXPLICIT` from edition default, renders
  as `optional`).
- A field with `field_presence = LEGACY_REQUIRED` (renders as `required`).
- A repeated field with `repeated_field_encoding = EXPANDED`.
- A message field with `message_encoding = DELIMITED` (renders as a group
  block).
- A field with `field_presence = EXPLICIT` and a `default_value` set.

### Golden file: `editions_rendering.golden.proto`

Location: `reproto/src/reproto/tests/fixtures/editions_rendering.golden.proto`.

The expected `.proto` output produced by reproto when processing the compiled
fixture.  Committed to Git; any intentional change requires a deliberate
golden update.

Note: because phase 2 does not yet emit `edition = "...";` or `features { }`
blocks, the golden output will be proto2-syntax with the A1 warning comment.
The field labels, packed annotations, and group blocks must be correct.

---

## Modified files summary

| File | Change |
|---|---|
| `reproto/src/reproto/syntax.py` | Add `features` param to `field_label`, `packed_option`, `allow_groups`, `is_synthetic_oneof`, `should_render_default` |
| `reproto/src/reproto/re_field.py` | Add `_resolve_field_features`, `_get_file_and_msg`; pass `features` to helpers in `render()` |
| `reproto/src/reproto/re_descriptor.py` | Resolve per-field features for `is_synthetic_oneof` and `field_label` calls in oneof-rendering code |
| `reproto/src/reproto/tests/test_editions_rendering.py` | New test file (unit tests T1–T13) |
| `reproto/src/reproto/tests/fixtures/editions_rendering.proto` | New fixture |
| `reproto/src/reproto/tests/fixtures/editions_rendering.golden.proto` | New golden file |

No changes to `re_enum.py`, `re_file.py`, `re_service.py`, `context.py`,
`cli.py`, or `feature_resolution.py` in this phase.

---

## Open questions

1. **Extension fields at message level**: an extension field declared inside a
   `message` body (not a top-level `extend`) has a containing message.  Should
   `_resolve_field_features` include the message-level features for such fields?
   Current assumption: yes (same chain as regular fields).

2. **`packed_option` for `EXPANDED`**: the spec defers emitting `[packed = false]`
   for `EXPANDED` fields to phase 3 (`features { }` blocks).  Is this correct,
   or does roundtrip fidelity require the legacy annotation in the interim?
   Needs verification against `protoc` output for `EXPANDED` fields compiled
   from an edition source.
