<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0020 — Polyglot mode: proto3 inconsistency guards (required, groups, message_set_wire_format)

**Status:** implemented
**Implemented in:** 2026-04-30
**App:** reproto

---

## Problem

Specs 0016 and 0019 implemented the bulk of polyglot rendering.  Three items
from spec 0015's inconsistency table remain unguarded when
`ctx.target_syntax == "proto3"`:

1. **`required` fields** — `LABEL_REQUIRED` is proto2-only.  Reproto's
   `field_label()` helper already handles this correctly for the normal proto3
   path (it returns `'optional '` or `''`), but when a proto3 descriptor
   actually carries `LABEL_REQUIRED` — a structural inconsistency that can
   arise from hand-crafted `.pb` files or tool bugs — reproto silently emits
   an invalid proto3 field.  A `cli_warning` must be emitted and the label
   degraded to no label (implicit singular).

2. **Groups (`TYPE_GROUP`)** — the `group Foo { ... }` construct is
   proto2-only (and deprecated even there).  Reproto currently renders groups
   correctly for proto2 but has no guard for proto3.  When
   `ctx.target_syntax == "proto3"`, a group field must be degraded to a plain
   message field reference and a `cli_warning` emitted.

3. **`MessageOptions.message_set_wire_format`** — this option is proto2-only.
   It was previously listed as a variant orphan and silently suppressed in all
   output, causing roundtrip failures for proto2 descriptors that use it.
   That bug was fixed separately (removed from `variant_orphans` in both
   `context.py` and `google-protobuf.yaml`; `message_set_proto2.proto` fixture
   added and passing).  What remains is the proto3 guard: when
   `ctx.target_syntax == "proto3"`, the option must be omitted and a
   `cli_warning` emitted.

---

## Goals

1. In `field_label()` in `syntax.py`, when `ctx.target_syntax == "proto3"`
   and `field.label == LABEL_REQUIRED`, emit a `cli_warning` and return `''`
   (implicit singular — the safest proto3 degradation).

2. Add `allow_groups(ctx)` to `syntax.py`.  Returns `True` iff
   `ctx.target_syntax == "proto2"`.

3. In `re_field.py`, gate group rendering behind `allow_groups(ctx)`.  When
   `allow_groups` returns `False`:
   - Emit a `cli_warning` naming the field.
   - Render the field as a plain message field (`T name = N;`) using the
     group's type name and `self.name` (already lowercase in the descriptor).
   - Suppress the inline group body (the `{ ... }` block that follows).
   - Do **not** set `is_group = True` on the nested `ReDescriptorProto`, so
     it renders as a standalone message definition and the type reference
     remains valid.

4. Add `allow_message_set_wire_format(ctx)` to `syntax.py`.  Returns `True`
   iff `ctx.target_syntax == "proto2"`.

5. In the `MessageOptions` rendering path in `re_descriptor.py`, gate
   `message_set_wire_format` behind `allow_message_set_wire_format(ctx)`.
   When `False`, add it to the `exclude` set passed to
   `render_options_from_message` and emit a `cli_warning` if the field is set.

6. Add proto2 fixture `group_proto2.proto` exercising groups in both optional
   and repeated forms; add it to `DEFAULT_FIXTURES`.

7. All existing tests must continue to pass.

---

## Non-goals

- Editions support.
- Supporting `TYPE_GROUP` in proto3 output (the degradation to plain message
  field is intentional and lossy).
- Changing the rendering of any other proto2 construct.
- Implementing `allow_groups` / `allow_message_set_wire_format` for editions
  (deferred to a future editions spec).

---

## Background

### `required` fields

`LABEL_REQUIRED` in a proto3 descriptor is structurally impossible from a
well-formed `.proto` source — protoc rejects it.  It can appear in
hand-crafted `.pb` files.  The current `field_label()` code never encounters
this combination in practice, but the guard is cheap insurance and completes
the inconsistency table from spec 0015 §5.

The degradation is `''` (no label) rather than `'optional '` because implicit
singular is the natural proto3 equivalent of a required field with a defined
wire value.

### Groups

`TYPE_GROUP` is a proto2 construct that embeds a sub-message using the legacy
start/end group wire delimiters (wire types 3 and 4), distinct from the
length-prefixed wire type 2 used for regular embedded messages.  In the
descriptor, a group field carries `type == TYPE_GROUP`, and the group body is
a nested `DescriptorProto` whose name is the PascalCase form of the group
field name.

In proto2, reproto already renders the group inline:

```proto
optional group Foo = 1 {
  optional int32 x = 2;
}
```

The `is_group` flag on `ReDescriptorProto` is set during field initialization
(`__init_extra__` in `re_field.py`) to suppress the nested message from
appearing as a standalone message definition.

When `allow_groups(ctx)` is `False` (proto3 target), the degraded rendering
is:

```proto
Foo foo = 1;
```

i.e., `type_name field_name = number;` (no label, since proto3 implicit
singular has no label keyword).  The group body (`is_group` nested message)
must **not** be suppressed — it continues to render as a standalone message
definition so that the type reference remains valid.  This requires that
`is_group` is only set to `True` in `__init_extra__` when `allow_groups(ctx)`
is `True`.

### `message_set_wire_format`

`MessageOptions.message_set_wire_format = true` enables the MessageSet wire
format, a legacy proto2 binary encoding.  It was previously a variant orphan,
silently suppressing it in all output and causing proto2 roundtrip failures.
That bug was fixed: `message_set_wire_format` was removed from `variant_orphans`
in `context.py` and `google-protobuf.yaml`, and a `message_set_proto2.proto`
fixture was added to `DEFAULT_FIXTURES` — all passing.

The remaining task is the proto3 inconsistency guard: when
`ctx.target_syntax == "proto3"`, this option must be excluded from
`MessageOptions` rendering and a `cli_warning` emitted.

---

## Specification

### 1. `syntax.py` — `field_label()` guard for `LABEL_REQUIRED` in proto3

In the existing `field_label()` function, inside the `ctx.target_syntax ==
"proto3"` branch, add a check before the `proto3_optional` test:

```python
if ctx.target_syntax == "proto3":
    if field.label == FieldDescriptorProto.LABEL_REQUIRED:
        cli_warning(
            f"field '{field.name}': 'required' label is not valid in proto3; "
            f"rendering as implicit singular"
        )
        return ''
    return 'optional ' if field.proto3_optional else ''
```

### 2. `syntax.py` — add `allow_groups(ctx)`

```python
def allow_groups(ctx: Context) -> bool:
    """Return True iff TYPE_GROUP fields may be rendered as groups."""
    return ctx.target_syntax == "proto2"
```

### 3. `syntax.py` — add `allow_message_set_wire_format(ctx)`

```python
def allow_message_set_wire_format(ctx: Context) -> bool:
    """Return True iff MessageOptions.message_set_wire_format may be rendered."""
    return ctx.target_syntax == "proto2"
```

### 4. `re_field.py` — gate group rendering behind `allow_groups(ctx)`

In `__init_extra__`, only set `grp.is_group = True` when `allow_groups(ctx)`
is `True`:

```python
case FieldDescriptorProto.TYPE_GROUP:
    grp = ReDescriptorProto.from_ref(ctx, Ref(self.type_name))
    from .syntax import allow_groups
    if allow_groups(ctx):
        grp.is_group = True
    self.targets.add(grp)
    self.type_descriptor = grp
```

In `render()`, gate the group type/name line:

```python
from .syntax import allow_groups
if self.type != FieldDescriptorProto.TYPE_GROUP or not allow_groups(ctx):
    if self.type == FieldDescriptorProto.TYPE_GROUP:
        cli_warning(
            f"field '{self.name}': groups are not valid in proto3; "
            f"rendering as plain message field"
        )
    ref = short_ref(ctx, self.type_descriptor, self.parent)
    string += f'{ref} {self.name}'
else:
    ref = short_ref(ctx, self.type_descriptor, self)
    string += f'group {ref}'
```

Gate the inline group body:

```python
if self.type != FieldDescriptorProto.TYPE_GROUP or not allow_groups(ctx):
    out.postpend(';')
else:
    ...  # group body rendering (unchanged)
```

### 5. `re_descriptor.py` — gate `message_set_wire_format` in `MessageOptions`

At the call site that renders `MessageOptions` (passing `ctx.mso_desc` as
`options_descriptor`), add an `exclude` set for the proto3 case:

```python
from .syntax import allow_message_set_wire_format
msf_exclude: set[str] = set()
if not allow_message_set_wire_format(ctx):
    if mo_msg.HasField('message_set_wire_format') and mo_msg.message_set_wire_format:
        cli_warning(
            f"message '{self.name}': 'message_set_wire_format' is not valid "
            f"in proto3; omitting"
        )
    msf_exclude = {'message_set_wire_format'}
option_blocks = self.render_options_from_message(
    ...,
    exclude=msf_exclude,
)
```

### 6. Fixture: `group_proto2.proto`

```proto
syntax = "proto2";
package mockup;

message WithGroups {
  optional group SimpleGroup = 1 {
    optional int32 value = 2;
  }
  repeated group RepeatedGroup = 3 {
    optional string name = 4;
    optional int32 count = 5;
  }
}
```

Add to `DEFAULT_FIXTURES` in `test_roundtrip.py`.

---

## Test coverage

After this spec is implemented, running `pytest` must show:

- All existing `test_roundtrip[*]` tests pass (no regression), including
  `test_roundtrip[message_set_proto2.proto]` (already passing).
- `test_roundtrip[group_proto2.proto]` passes:
  - Groups render correctly as `group Foo { ... }` in proto2.
  - `.pb` roundtrip is byte-identical.
- No new polyglot fixtures are required: the proto3 degradation paths
  (`required` → implicit, group → plain message field, `message_set_wire_format`
  → omit) are exercised indirectly via the `--force-proto2-output` arm of
  existing `test_roundtrip_polyglot` tests (crash-safety only; lossiness is
  expected and acceptable).

---

## Open questions

None.
