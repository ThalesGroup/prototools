<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0023 — Proto3: allow `extend *Options` blocks (custom options)

**Status:** implemented
**Implemented in:** 2026-05-01
**App:** reproto

---

## Problem

Spec 0015 §6 noted that proto3 allows `extend` blocks on the nine `*Options`
messages from `google/protobuf/descriptor.proto` (custom options), but the
`allow_extensions(ctx)` helper introduced in spec 0016 returns `False` for any
proto3 file.  As a result, reproto drops every `extend` block in a proto3 file
with a warning, including legitimate custom-option definitions such as:

```proto
extend google.protobuf.MethodOptions {
  google.longrunning.OperationInfo operation_info = 1049;
}
```

This is incorrect: the block is valid proto3 and must be emitted.

---

## Goals

1. `extend *Options` blocks in proto3 files are emitted normally (not dropped).
2. `extend UserMessage` blocks in proto3 files are still dropped with a
   `cli_warning` (unchanged behaviour).
3. `extensions N to M;` ranges in proto3 files are still dropped with a
   `cli_warning` (unchanged behaviour — extension ranges on user-defined
   messages remain proto2-only).
4. A new roundtrip fixture `custom_options_proto3.proto` exercises both a
   file-level and a message-level `extend *Options` block in proto3.

---

## Non-goals

- Editions support.
- Changing the `extensions N to M;` guard (proto2-only, unchanged).
- Adding support for `extend` on user-defined messages in proto3.

---

## Background

Proto3 allows `extend` only on the nine descriptor `*Options` messages:

| Message | FQN |
|---------|-----|
| `FileOptions` | `.google.protobuf.FileOptions` |
| `MessageOptions` | `.google.protobuf.MessageOptions` |
| `FieldOptions` | `.google.protobuf.FieldOptions` |
| `OneofOptions` | `.google.protobuf.OneofOptions` |
| `ExtensionRangeOptions` | `.google.protobuf.ExtensionRangeOptions` |
| `EnumOptions` | `.google.protobuf.EnumOptions` |
| `EnumValueOptions` | `.google.protobuf.EnumValueOptions` |
| `ServiceOptions` | `.google.protobuf.ServiceOptions` |
| `MethodOptions` | `.google.protobuf.MethodOptions` |

This is enforced by `protoc`: a proto3 file that tries to extend any other
message fails with a compile error.  Reproto only processes well-formed
descriptors, so it can treat any `extend` block whose extendee is one of these
nine FQNs as always valid in proto3.

Extension ranges (`extensions N to M;`) remain proto2-only even for the
`*Options` messages — `protoc` does not allow adding extension ranges to them
in proto3 source.

---

## Specification

### 1. `syntax.py` — replace `allow_extensions` with two helpers

Remove `allow_extensions(ctx)` and replace with:

```python
# The nine *Options FQNs that proto3 allows extending (custom options).
_DESCRIPTOR_OPTIONS_FQNS = frozenset({
    ".google.protobuf.FileOptions",
    ".google.protobuf.MessageOptions",
    ".google.protobuf.FieldOptions",
    ".google.protobuf.OneofOptions",
    ".google.protobuf.ExtensionRangeOptions",
    ".google.protobuf.EnumOptions",
    ".google.protobuf.EnumValueOptions",
    ".google.protobuf.ServiceOptions",
    ".google.protobuf.MethodOptions",
})

def allow_extend_block(ctx: Context, extendee: str) -> bool:
    """Return True iff an extend block for `extendee` is legal in target syntax.

    Proto2: always True.
    Proto3: True only when extendee is one of the nine descriptor *Options FQNs
            (custom options are the only proto3-legal extension target).
    """
    if ctx.target_syntax == "proto2":
        return True
    return extendee in _DESCRIPTOR_OPTIONS_FQNS

def allow_extension_ranges(ctx: Context) -> bool:
    """Return True iff `extensions N to M;` declarations are legal in target syntax."""
    return ctx.target_syntax == "proto2"
```

Keep `allow_extensions` as a thin alias for backwards compatibility with any
call site that uses it for extension-range decisions, redirecting to
`allow_extension_ranges`.  Remove the alias once all call sites are updated.

### 2. `re_file.py` — file-level extend blocks

In the file-level extensions rendering section, replace the single
`allow_extensions(ctx)` branch with per-extendee logic:

```python
from .syntax import allow_extend_block
for e in self.extension:
    extension_proto = cast(FieldDescriptorProto, e)
    if not allow_extend_block(ctx, extension_proto.extendee):
        cli_warning(
            f"'{self.name}': top-level extend block for "
            f"'{extension_proto.extendee}' is not valid in proto3; omitting"
        )
        # mark as excluded so extendee_short_names loop skips it
```

Concretely: collect only the extendees for which `allow_extend_block` returns
`True`; warn-and-skip the rest.

### 3. `re_descriptor.py` — message-level extend blocks

In `render_extensions`, replace the blanket `allow_extensions(ctx)` guard with
per-extendee filtering (same pattern as §2).

In `render_reserved`, the `allow_extensions` guard for `extension_range` is
renamed to `allow_extension_ranges` — semantics unchanged.

---

## Fixture: `custom_options_proto3.proto`

```proto
syntax = "proto3";
package mockup;

import "google/protobuf/descriptor.proto";

extend google.protobuf.FileOptions {
  string custom_file_opt = 50000;
}

message MyMessage {
  extend google.protobuf.FieldOptions {
    bool custom_field_opt = 50001;
  }
  string value = 1 [(mockup.MyMessage.custom_field_opt) = true];
}

option (mockup.custom_file_opt) = "hello";
```

Add `custom_options_proto3.proto` to `DEFAULT_FIXTURES` in `test_roundtrip.py`.

---

## Test coverage

- `test_roundtrip[custom_options_proto3.proto]`: full `.pb` roundtrip
  (byte-identical) and `.proto` text comparison after normalization.
- All existing `test_roundtrip[*]` tests must continue to pass (no regression).
- The `extensions_proto2.proto` fixture (user-defined message extensions in
  proto2) continues to pass unchanged.
