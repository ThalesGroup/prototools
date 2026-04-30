<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0019 — Polyglot mode: field labels, synthetic oneofs, default values, json_name, import weak, and extensions

**Status:** draft
**App:** reproto

---

## Problem

Spec 0016 added `--polyglot` support and fixed packed encoding.  Four further
rendering issues remain in `re_field.py` and `re_descriptor.py`:

1. **Field labels** — reproto currently emits `optional T f = N;` for every
   singular non-oneof field regardless of syntax.  In proto3, implicit
   singular fields must have *no* label keyword.  Emitting `optional` on
   such a field is illegal in proto3 (protoc rejects it).

2. **Synthetic oneofs** — when a proto3 source has `optional T f = N;`,
   protoc records a synthetic oneof `_f` in the descriptor.  Reproto must
   suppress this synthetic oneof block and instead emit `optional T f = N;`
   as a top-level field.  Currently the synthetic oneof block is already
   skipped during oneof rendering (partial fix), but the `optional` label is
   not emitted on the field itself because the proto2 path emits it
   unconditionally anyway.  Under `--polyglot`, the proto3 rendering path
   must explicitly check `proto3_optional` and produce the correct output.

3. **Default values in proto3** — reproto currently emits `[default = ...]`
   for any field whose `default_value` is set in the descriptor.  Proto3
   does not allow explicit defaults; emitting them produces a file protoc
   rejects.  In `--polyglot` mode with `ctx.target_syntax == "proto3"`, the
   default-value option must be suppressed and a `cli_warning` emitted.

4. **`json_name` over-emission** — `FieldDescriptorProto.json_name` is
   always populated by protoc (for both syntaxes) with the auto-derived
   camelCase value.  Reproto currently emits `[json_name = "..."]` whenever
   the field is set in the descriptor, which is always — producing spurious
   annotations on every field.  The option must be emitted only when the
   stored value differs from the auto-derived camelCase of the field name.
   This fix is syntax-independent (applies in both proto2 and proto3 modes).

5. **`import weak` in proto3** — `import weak` is proto2-specific.  Reproto
   already renders it correctly for proto2 (via `weak_dependency` indices in
   `FileDescriptorProto`).  In `--polyglot` mode with
   `ctx.target_syntax == "proto3"`, a weak import is an inconsistency: reproto
   must fall back to plain `import` and emit a `cli_warning`.

6. **Extension ranges and `extend` blocks in proto3** — extension ranges
   (`extensions N to M;`) and `extend Foo { ... }` blocks are proto2-only
   constructs (with the sole exception of extending `*Options` messages for
   custom options, which is handled separately).  In `--polyglot` mode with
   `ctx.target_syntax == "proto3"`, both must be omitted and a `cli_warning`
   emitted per occurrence.

Issues 1–3, 5–6 are proto3-specific; issue 4 is a correctness bug in both
syntaxes.  All are fully specified in spec 0015 §2, §11, §3, §16, §8, and §6
respectively, and require no new empirical research.

---

## Goals

1. In `--polyglot` mode with `ctx.target_syntax == "proto3"`, suppress the
   `optional` label on implicit singular fields
   (`label == LABEL_OPTIONAL` and `proto3_optional == False` and not in a
   real oneof).
2. In `--polyglot` mode with `ctx.target_syntax == "proto3"`, emit
   `optional` on fields with `proto3_optional == True` (these fields are
   rendered outside any oneof block).
3. In `--polyglot` mode with `ctx.target_syntax == "proto3"`, suppress the
   synthetic oneof block entirely and do not render its single member field
   inside the oneof (that field is rendered at message level instead — see
   goal 2).
4. Add two helper functions to `re_syntax.py` (created in spec 0016 as
   `syntax.py` — see §Note on module name): `field_label()` and
   `is_synthetic_oneof()`.
5. In `--polyglot` mode with `ctx.target_syntax == "proto3"`, suppress
   `[default = ...]` on any field whose `default_value` is set in the
   descriptor; emit a `cli_warning` per field.  Add a `should_render_default()`
   helper to `syntax.py`.
6. In both proto2 and proto3 modes, emit `[json_name = "..."]` only when the
   stored value differs from the auto-derived camelCase of the field name.
   Add a `camel_case()` utility and a `should_render_json_name()` helper to
   `syntax.py`.
7. In `--polyglot` mode with `ctx.target_syntax == "proto3"`, render weak
   imports as plain `import` and emit a `cli_warning` per occurrence.  Add
   `allow_weak_import(target_syntax)` to `syntax.py`.
8. In `--polyglot` mode with `ctx.target_syntax == "proto3"`, omit
   `extensions N to M;` declarations and `extend Foo { ... }` blocks; emit
   a `cli_warning` per omitted declaration/block.  Add
   `allow_extensions(target_syntax)` to `syntax.py`.
9. Add fixture files and roundtrip tests for all six changes.
10. All existing tests (with and without `--polyglot`) must continue to pass.

> **Note on module name:** Spec 0016 created `reproto/src/reproto/syntax.py`
> with `fdp_syntax()` and `packed_option()`.  Spec 0015 §Architecture names
> this module `re_syntax.py`.  This spec adds to whichever file was actually
> created by spec 0016; if both exist, consolidate into `re_syntax.py` and
> update the import in `re_field.py`.  If only `syntax.py` exists, add the
> new functions there and rename it in a single commit.

---

## Non-goals

- Any proto3 rendering difference beyond field labels, synthetic oneofs,
  default values, `json_name`, `import weak`, and extensions (groups,
  `message_set_wire_format`) — deferred to later specs in the 0015 series.
- Editions support.
- Changes to proto2 rendering (zero regression).
- Adding a `--syntax-overrides` mechanism (deferred to spec 0015).

---

## Background

### Field labels in the descriptor (spec 0015 §2)

Proto3 singular fields always carry `label = LABEL_OPTIONAL` in the
descriptor, regardless of whether the source had an explicit `optional`
keyword.  The distinction between implicit and explicit presence is encoded
in `proto3_optional`:

| Source form | `label` | `proto3_optional` | synthetic oneof |
|---|---|---|---|
| `T f = N;` (implicit) | `LABEL_OPTIONAL` | `False` | none |
| `optional T f = N;` (explicit) | `LABEL_OPTIONAL` | `True` | `_f` created |
| `repeated T f = N;` | `LABEL_REPEATED` | `False` | none |
| field inside `oneof` | `LABEL_OPTIONAL` | `False` | none (real oneof) |

The rendering rules follow directly:

- Implicit (`proto3_optional == False`, not in real oneof): emit no label.
- Explicit (`proto3_optional == True`): emit `optional` (the field is
  rendered outside any oneof block).
- `LABEL_REPEATED`: emit `repeated` (same in proto2 and proto3).
- Field inside a real oneof: no label (same in proto2 and proto3).

`LABEL_REQUIRED` cannot appear in a well-formed proto3 descriptor;
inconsistency handling is out of scope for this spec (see spec 0015
§Inconsistency handling).

### Synthetic oneofs (spec 0015 §11)

Detection rule — a oneof is **synthetic** iff all three conditions hold:

1. `oneof.name` starts with `_`.
2. It contains exactly one field.
3. That field has `proto3_optional == True`.

A synthetic oneof must be:

- Suppressed at the oneof level (no `oneof _foo { ... }` block emitted).
- Its sole member field rendered at the enclosing message level as
  `optional T f = N;`.

Real oneofs (even those whose name happens to start with `_`) must never be
suppressed.  The three-condition rule is sufficient to distinguish them
because protoc guarantees that a real oneof always has more than one field or
its field has `proto3_optional == False`.

**Empirically confirmed** (mockup `f10_synthetic_oneof.proto` and
`f06_field_labels_proto3.proto` in `docs/mockup/`): protoc creates one
synthetic oneof per `optional` field, never merging two `optional` fields
into the same synthetic oneof.

### Default values (spec 0015 §3)

Proto2 allows `[default = <value>]` on optional/required scalar fields.
Proto3 forbids explicit defaults entirely (the zero value is always the
implicit default and is never stored in the descriptor).

`FieldDescriptorProto.default_value` is a `string` field that is absent
(`HasField("default_value") == False`) when no default was declared.  In a
well-formed proto3 descriptor this field is never set.  If it is set (e.g.
in a hand-crafted `.pb`), reproto must treat it as an inconsistency: emit a
`cli_warning` and omit the `[default = ...]` option from the output.

| Condition | proto2 rendering | proto3 rendering |
|---|---|---|
| `HasField("default_value") == False` | nothing | nothing |
| `HasField("default_value") == True` | `[default = <val>]` | omit + `cli_warning` |

The warning must include the file name and the fully-qualified field name.

**Empirically confirmed** (mockup `f11_default_values_proto2.proto`):
protoc never sets `default_value` in proto3 descriptors.

### `json_name` (spec 0015 §16)

`FieldDescriptorProto.json_name` is always set by protoc in both proto2 and
proto3.  `HasField("json_name")` is always `True` — it cannot be used to
detect a user-supplied override.

The auto-derived camelCase of a field name is computed by:

1. Split the name on `_`.
2. Keep the first component as-is (lowercased).
3. Capitalize the first letter of each subsequent component.
4. Join without separator.

Examples: `field_name` → `fieldName`, `already_camel` → `alreadyCamel`,
`x` → `x`, `under_score_heavy` → `underScoreHeavy`.

Emit `[json_name = "..."]` only when the stored `json_name` value differs
from the auto-derived camelCase of `field.name`.  When they are equal —
which is the common case — omit the option entirely, matching the source
proto and protoc's own behaviour.

This fix applies in both proto2 and proto3 modes (it is not gated on
`--polyglot`).

**Empirically confirmed** (mockup `f09_json_name.proto`): protoc stores the
user-supplied value when it differs from the auto-derived name, and the
auto-derived value when it is the same — so the stored value can always be
compared directly against the auto-derivation to decide whether to emit the
option.  One edge case: `same_as_auto` in the mockup has an explicit
`[json_name = "sameAsAuto"]` in source, but the stored value equals the
auto-derived value, so the option is correctly suppressed.

### `import weak` (spec 0015 §8)

`import weak` is a proto2-specific directive.  In the descriptor,
`FileDescriptorProto.dependency` lists all imports (both regular and weak)
by path.  `FileDescriptorProto.weak_dependency` contains the indices into
`dependency` that are weak.  Reproto already uses these fields to emit
`import weak "..."` for proto2 files.

In proto3, weak imports are illegal.  A proto3 descriptor with non-empty
`weak_dependency` is inconsistent.  The degraded rendering is: emit
`import "..."` (plain import, same path) and a `cli_warning` per weak
import.  The import itself is preserved so that type resolution in the
output is not broken.

**Empirically confirmed** (mockup `f14_weak_import_proto2.proto` and
`f14_weak_import_proto2_dep.proto`): `weak_dependency` contains the
0-based index of the weak import within `dependency`.

### Extensions and extension ranges (spec 0015 §6)

Proto2 supports two extension constructs:

1. **Extension ranges** — `extensions N to M;` inside a message body.
   These appear as `DescriptorProto.extension_range` entries in the
   descriptor.
2. **`extend` blocks** — `extend Foo { ... }` at file or message scope.
   File-level extensions appear in `FileDescriptorProto.extension`; nested
   extensions appear in `DescriptorProto.extension`.

Proto3 forbids both for user-defined message types.  (Extending `*Options`
messages for custom options is an exception, but such extensions appear in
`FileDescriptorProto.extension` just like any other extension — this spec
does not attempt to distinguish them.  Omitting all extension constructs in
proto3 output is the safe, conservative choice for roundtrip purposes.)

Degraded rendering in proto3 mode:

- Omit each `extensions N to M;` declaration; emit a `cli_warning` naming
  the message and the range.
- Omit each `extend Foo { ... }` block (file-level or nested); emit a
  `cli_warning` naming the extendee and the enclosing scope.

**Empirically confirmed** (mockup `f12_extensions_proto2.proto`): both
file-level and message-nested `extend` blocks appear in the descriptor as
expected.

---

## Specification

### 1. New functions in `syntax.py` (or `re_syntax.py`)

**Convention:** every helper in `syntax.py` takes `ctx` as its first
argument and reads whatever it needs (`ctx.syntax`, `ctx.target_syntax`,
etc.) from it directly.  This keeps call sites simple and avoids the
parameter list growing as more context-dependent decisions are added.

`fdp_syntax()` is the sole exception: it is called to *populate* `ctx.syntax`
and therefore cannot receive `ctx`.

Add the following two functions:

```python
def field_label(ctx: Context, field, is_oneof: bool) -> str:
    """
    Return the label keyword to emit before the field type (with a trailing
    space), or '' if no label should be emitted.

    Args:
        ctx:      rendering context (reads ctx.target_syntax)
        field:    FieldDescriptorProto
        is_oneof: True if this field is rendered inside a real oneof block
                  (synthetic oneof members are passed is_oneof=False)

    Rules:
        - is_oneof                          → ''
        - field.label == LABEL_REPEATED     → 'repeated '
        - ctx.target_syntax == "proto2":
            field.label == LABEL_REQUIRED   → 'required '
            field.label == LABEL_OPTIONAL   → 'optional '
        - ctx.target_syntax == "proto3":
            field.proto3_optional           → 'optional '
            else                            → ''  (implicit singular)
    """


def is_synthetic_oneof(ctx: Context, oneof_name: str, members: list) -> bool:
    """
    Return True iff the given oneof is a proto3 synthetic oneof.

    Returns False immediately when ctx.target_syntax != "proto3", making
    the function safe to call unconditionally regardless of syntax.

    Detection rule (all conditions must hold):
        1. ctx.target_syntax == "proto3"
        2. oneof_name starts with '_'
        3. exactly one field is in members
        4. that field has proto3_optional == True
    """
```

### 2. `re_field.py` — use `field_label()`

Replace the existing label-emission logic with a call to `field_label()`.

Key detail: a field whose synthetic oneof is suppressed is rendered at
message level (not inside a `oneof` block), so `is_oneof` must be `False`
for such a field.  The caller (`re_descriptor.py`, see §3) is responsible
for passing the correct `is_oneof` value.

The `render(ctx, depth, is_oneof)` signature is unchanged.  Inside:

```python
from .syntax import field_label

label_str = field_label(ctx, self.this, is_oneof)
# emit: f"{label_str}{type_str} {field.name} = {field.number}{opts};"
```

The existing branch that hardcodes `optional`/`required`/`repeated` is
removed and replaced by `label_str`.

The `packed_option()` call is similarly updated to the `ctx`-first
convention: `packed_option(ctx, has_packed, effective_packed)`.

### 3. `re_descriptor.py` — synthetic oneof suppression

This is the most structurally significant change in this spec.

#### 3a. Identify synthetic oneofs

At the top of `ReDescriptorProto.render()`, before iterating over fields and
oneofs, build two sets:

```python
# Map from oneof_index → list of fields in that oneof
from collections import defaultdict
oneof_fields: dict[int, list] = defaultdict(list)
for f in self.this.field:
    if f.HasField('oneof_index') or f.oneof_index >= 0:
        # Note: oneof_index is always present when the field is in a oneof;
        # use HasField only if the proto uses proto3 optional detection.
        # Safe fallback: check oneof_index against the oneof_decl length.
        pass  # filled in implementation

synthetic_oneof_indices: set[int] = set()
for idx, oneof in enumerate(self.this.oneof_decl):
    members = [f for f in self.this.field
               if f.HasField('oneof_index') and f.oneof_index == idx]
    if is_synthetic_oneof(oneof, members):
        synthetic_oneof_indices.add(idx)
```

> **Implementation note on `HasField('oneof_index')`:** In proto3,
> `oneof_index` is a scalar `int32` with no `HasField` support in the Python
> API — presence is inferred from whether the field is a member of a oneof at
> all (i.e., `field.HasField('oneof_index')` raises `ValueError` for scalar
> fields).  The correct check in prost-reflect / protobuf Python is:
> `f.WhichOneof('oneof_index') is not None` or simply iterate
> `message.oneofs[idx].fields`.  Implementors should use whichever API the
> existing codebase already uses for oneof membership detection.

#### 3b. Skip synthetic oneof blocks

When iterating `self.this.oneof_decl` to emit `oneof` blocks, skip any
index in `synthetic_oneof_indices`.

#### 3c. Render synthetic oneof member fields at message level

The current code likely separates "oneof fields" from "non-oneof fields"
when iterating `self.this.field`.  Fields whose `oneof_index` is in
`synthetic_oneof_indices` must be treated as **non-oneof fields** for
rendering purposes:

- They are rendered in the main field list, not inside a `oneof` block.
- `is_oneof=False` is passed to their `render()` call.
- They carry `proto3_optional == True`, so `field_label()` returns
  `'optional '` for them.

Fields whose `oneof_index` is in a *real* oneof index (not in
`synthetic_oneof_indices`) continue to be rendered inside the `oneof` block
with `is_oneof=True`.

### 4. `syntax.py` — `should_render_default()` and `json_name` helpers

Add three more functions to `syntax.py`:

```python
def should_render_default(target_syntax: str, field) -> bool:
    """
    Return True iff [default = ...] should be rendered for this field.

    Args:
        target_syntax: "proto2" or "proto3" (ctx.target_syntax)
        field:         FieldDescriptorProto

    Emits a cli_warning if default_value is set in a proto3 file.
    The caller is responsible for including file/field context in the warning.
    """
    has_default = field.HasField('default_value')
    if not has_default:
        return False
    if target_syntax == "proto3":
        return False   # caller must emit cli_warning
    return True


def _camel_case(name: str) -> str:
    """
    Derive the default JSON name (camelCase) for a proto field name.

    Rules: split on '_', keep first component as-is, capitalize the
    first letter of each subsequent non-empty component, join.

    Examples:
        'field_name'        → 'fieldName'
        'already_camel'     → 'alreadyCamel'
        'x'                 → 'x'
        'under_score_heavy' → 'underScoreHeavy'
        '__foo'             → 'Foo'   (leading underscores produce empty
                                       first component, next is capitalized)
    """
    parts = name.split('_')
    if not parts:
        return name
    return parts[0] + ''.join(p.capitalize() for p in parts[1:] if p)


def should_render_json_name(field) -> bool:
    """
    Return True iff [json_name = "..."] should be emitted for this field.

    Emit only when the stored json_name differs from the auto-derived
    camelCase of field.name.  This is syntax-independent.
    """
    return field.json_name != _camel_case(field.name)
```

`should_render_default()` returns `False` for proto3; the caller in
`re_field.py` must separately emit the `cli_warning` when
`target_syntax == "proto3"` and `field.HasField('default_value')` is `True`.

### 5. `re_field.py` — default value and `json_name` gates

#### 5a. Default value

Replace the existing guard around `[default = ...]` emission:

```python
from .syntax import should_render_default

if not should_render_default(ctx.target_syntax, self.this):
    if (ctx.target_syntax == "proto3"
            and self.this.HasField('default_value')):
        cli_warning(
            f"{ctx.current_file}: field '{self.this.name}': "
            f"explicit default values are not valid in proto3; omitting"
        )
else:
    # existing default-value rendering code unchanged
    ...
```

#### 5b. `json_name`

Replace the existing `json_name` emission guard (or add one if missing):

```python
from .syntax import should_render_json_name

if should_render_json_name(self.this):
    opt_block.append(BlockLine(f'json_name = "{self.this.json_name}",', depth + 1))
```

Remove any unconditional `json_name` emission.

### 6. `syntax.py` — `allow_weak_import()` and `allow_extensions()`

Add two more predicate functions:

```python
def allow_weak_import(target_syntax: str) -> bool:
    """Return True iff import weak is legal in this syntax."""
    return target_syntax == "proto2"


def allow_extensions(target_syntax: str) -> bool:
    """Return True iff extension ranges and extend blocks are legal."""
    return target_syntax == "proto2"
```

### 7. `re_file.py` — `import weak` degradation

Reproto already iterates `fdp.dependency` and uses `fdp.weak_dependency`
to decide whether to emit `import weak "..."` or `import "..."`.  Add a
guard around the `weak` keyword:

```python
from .syntax import allow_weak_import

for i, dep in enumerate(self.this.dependency):
    is_weak = i in weak_set   # weak_set built from weak_dependency indices
    if is_weak and not allow_weak_import(ctx.target_syntax):
        cli_warning(
            f"{ctx.current_file}: 'import weak' is not valid in proto3; "
            f"rendering as plain import: \"{dep}\""
        )
        is_weak = False
    keyword = "weak " if is_weak else ""
    lines.append(f'import {keyword}"{dep}";')
```

The import path is always emitted; only the `weak` keyword is suppressed.

### 8. `re_descriptor.py` — extension range and `extend` block guards

#### 8a. Extension ranges (`extensions N to M;`)

When iterating `self.this.extension_range` to emit `extensions` statements,
skip ranges and warn when extensions are not allowed:

```python
from .syntax import allow_extensions

if not allow_extensions(ctx.target_syntax):
    for er in self.this.extension_range:
        cli_warning(
            f"{ctx.current_file}: message '{self.this.name}': "
            f"extension range [{er.start}, {er.end}) is not valid in "
            f"proto3; omitting"
        )
else:
    # existing extension_range rendering unchanged
    ...
```

#### 8b. File-level `extend` blocks (`FileDescriptorProto.extension`)

File-level extensions are rendered in `re_file.py` (or wherever
`fdp.extension` is iterated).  Wrap that iteration:

```python
if not allow_extensions(ctx.target_syntax):
    for ext in self.this.extension:
        cli_warning(
            f"{ctx.current_file}: top-level extend block for "
            f"'{ext.extendee}' is not valid in proto3; omitting"
        )
else:
    # existing file-level extension rendering unchanged
    ...
```

#### 8c. Message-nested `extend` blocks (`DescriptorProto.extension`)

Same pattern as 8b, applied wherever `msg.extension` is iterated:

```python
if not allow_extensions(ctx.target_syntax):
    for ext in self.this.extension:
        cli_warning(
            f"{ctx.current_file}: message '{self.this.name}': "
            f"nested extend block for '{ext.extendee}' is not valid "
            f"in proto3; omitting"
        )
else:
    # existing nested extension rendering unchanged
    ...
```

### 9. Test fixtures

Copy six mockup files into `reproto/src/reproto/tests/fixtures/`:

- `field_labels_proto3.proto` — sourced from `docs/mockup/f06_field_labels_proto3.proto`
- `synthetic_oneof.proto` — sourced from `docs/mockup/f10_synthetic_oneof.proto`
- `default_values_proto2.proto` — sourced from `docs/mockup/f11_default_values_proto2.proto`
- `json_name.proto` — sourced from `docs/mockup/f09_json_name.proto`
- `weak_import_proto2.proto` — sourced from `docs/mockup/f14_weak_import_proto2.proto`
  (also copy `f14_weak_import_proto2_dep.proto` → `weak_import_proto2_dep.proto` since the
  import path inside the fixture references the dependency by name)
- `extensions_proto2.proto` — sourced from `docs/mockup/f12_extensions_proto2.proto`

Drop the `f06_`/`f09_`/`f10_`/`f11_`/`f12_`/`f14_` prefixes.  Leave
`package mockup;` as-is.  Update the `import weak "..."` path inside
`weak_import_proto2.proto` to reference `weak_import_proto2_dep.proto`.

### 10. Roundtrip regression tests

Proto2 fixtures go into the existing non-polyglot `DEFAULT_FIXTURES` list.
Proto3 polyglot fixtures are split into two categories based on whether a
lossless roundtrip is possible **without** `--polyglot`:

- **Strict** (`POLYGLOT_FIXTURES_STRICT`): fixtures where the only
  no-polyglot `.pb` difference is `syntax`.  The existing
  `differing <= {"syntax"}` assertion applies.
- **Lossy** (`POLYGLOT_FIXTURES_LOSSY`): fixtures that use proto3-only
  descriptor fields (`proto3_optional`, synthetic `oneof_decl`, `oneof_index`)
  which are structurally impossible to reproduce from proto2 source.  The
  no-polyglot roundtrip still runs for crash-safety, but the field-diff
  assertion is widened to `differing <= PROTO3_ONLY_FIELDS` where:

  ```python
  PROTO3_ONLY_FIELDS = {"syntax", "proto3_optional", "oneof_index", "oneof_decl", "name"}
  ```

  `"name"` appears in the set because `pb_diff_fields` traverses into
  missing `oneof_decl` sub-messages and surfaces their `name` child field;
  it is an artifact of the diff algorithm, not a top-level field change.

Fixture assignments:

- `packed_proto2.proto` → `POLYGLOT_FIXTURES_STRICT` (spec 0016)
- `packed_proto3.proto` → `POLYGLOT_FIXTURES_STRICT` (spec 0016)
- `json_name.proto` → `POLYGLOT_FIXTURES_STRICT` (proto3, but no synthetic oneofs)
- `field_labels_proto3.proto` → `POLYGLOT_FIXTURES_LOSSY` (has synthetic oneofs)
- `synthetic_oneof.proto` → `POLYGLOT_FIXTURES_LOSSY` (has synthetic oneofs)
- `default_values_proto2.proto` → `DEFAULT_FIXTURES` (non-polyglot)
- `weak_import_proto2.proto` → `DEFAULT_FIXTURES` (non-polyglot)
- `extensions_proto2.proto` → `DEFAULT_FIXTURES` (non-polyglot)

`test_roundtrip_polyglot` is updated to accept both lists.  For strict
fixtures it behaves exactly as before.  For lossy fixtures it substitutes
`PROTO3_ONLY_FIELDS` for `{"syntax"}` in the no-polyglot assertion.  The
with-`--polyglot` pass (full `.pb` + `.proto` comparison) is identical for
both categories.

---

## Test coverage

After this spec is implemented, running `pytest` must show:

- All existing `test_roundtrip[*]` tests pass (no regression).
- `test_roundtrip_polyglot[packed_proto2.proto]` and
  `test_roundtrip_polyglot[packed_proto3.proto]` pass (spec 0016 regression,
  strict category).
- `test_roundtrip_polyglot[json_name.proto]` passes (strict category).
- `test_roundtrip_polyglot[field_labels_proto3.proto]` passes (lossy category):
  - no-polyglot roundtrip completes without crash; field diff is within
    `PROTO3_ONLY_FIELDS`.
  - with `--polyglot`: implicit singular fields render with no label;
    `optional` fields render with `optional` label outside any oneof;
    `repeated` fields render with `repeated` label; fields inside a real
    `oneof` render with no label inside the oneof block.
- `test_roundtrip_polyglot[synthetic_oneof.proto]` passes (lossy category):
  - no-polyglot roundtrip completes without crash; field diff is within
    `PROTO3_ONLY_FIELDS`.
  - with `--polyglot`: synthetic oneofs (`_opt_scalar`, `_opt_string`) are
    suppressed; their member fields render at message level as
    `optional int32 opt_scalar = 1;` etc.; the real `oneof real_choice { ... }`
    block is preserved.
- `test_roundtrip[default_values_proto2.proto]` passes:
  - all `[default = ...]` annotations are reproduced exactly.
  - the field with no default (`no_def`) produces no default option.
- `test_roundtrip_polyglot[json_name.proto]` passes:
  - `field_name`, `already_camel`, `under_score_heavy` — no `[json_name]`
    emitted (value equals auto-derived camelCase).
  - `custom` — `[json_name = "My"]` emitted (differs from auto).
  - `same_as_auto` — no `[json_name]` emitted (stored value equals
    auto-derived value, even though it was explicit in source).
- `test_roundtrip[weak_import_proto2.proto]` passes:
  - `import weak "weak_import_proto2_dep.proto";` is reproduced exactly.
- `test_roundtrip[extensions_proto2.proto]` passes:
  - `extensions 100 to 199;` inside `Extendable` is reproduced.
  - file-level `extend Extendable { ... }` block is reproduced.
  - message-nested `extend Extendable { ... }` inside `Holder` is reproduced.

---

## Open questions

None.
