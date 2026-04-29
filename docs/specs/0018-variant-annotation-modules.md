<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0018 — Variant YAML format and annotation_modules

**Status:** implemented
**Implemented in:** 2026-04-29
**App:** reproto

---

## Problem

Two related gaps:

1. The variant YAML format is not documented anywhere in the OSS repo.
   `variant.py` contains the parser; `google-protobuf.yaml` is the only
   example; but there is no normative description of the schema.

2. `reproto.py` contains `import_annotations()`, which unconditionally
   attempts to import a hardcoded list of internal Python packages absent
   from the OSS environment.  The function logs a `cli_warning` for each
   missing package, producing spurious noise on every OSS invocation.

   The right fix is to make the list of annotation modules part of the
   variant specification, defaulting to an empty list in the OSS variant.

---

## Goals

1. Document the complete variant YAML format in this spec (normative
   reference for implementors and variant authors).
2. Add `annotation_modules` as a new optional key to the variant YAML
   format.
3. Add `variant_annotation_modules: list[str]` to `Options` and `Context`,
   defaulting to `[]`.
4. Update `variant.py` to parse `annotation_modules`.
5. Remove the hardcoded package list from `import_annotations()` and drive
   it from `ctx.variant_annotation_modules` instead.
6. Update `google-protobuf.yaml` to include `annotation_modules: []`
   (explicit empty list, documents the field).
7. Suppress all import attempts (and all warnings) when the list is empty.

---

## Non-goals

- Changing the semantics of what annotation modules *do* once imported.
- Adding a CLI flag for annotation modules.
- Supporting filesystem-relative module paths (Python import semantics only).
- Documenting any CLI flags not directly related to variant loading.

---

## Specification

### 1. Variant YAML — complete format

A variant file is a YAML document.  All keys are optional; omitting a key
is equivalent to supplying its default value (shown in the example below).

```yaml
# -----------------------------------------------------------------------
# Variant metadata (informational only, not used by reproto at runtime)
# -----------------------------------------------------------------------
name: google.protobuf
description: >
  Standard OSS protobuf variant.  Input files are compiled against
  google/protobuf/descriptor.proto.

# -----------------------------------------------------------------------
# descriptor_proto
#   Path of the descriptor.proto that *input* files were compiled against.
#   Used to suppress the file from the output (reproto never writes it
#   unless --write_variant_descriptor is set).
#   Default: google/protobuf/descriptor.proto
# -----------------------------------------------------------------------
descriptor_proto: google/protobuf/descriptor.proto

# -----------------------------------------------------------------------
# well_known
#   Maps canonical well-known-type proto paths to variant-specific fallback
#   paths.  When reproto needs a well-known-type file that is absent from
#   the input descriptor set, it looks up the canonical path here and loads
#   the fallback from the variant's resource directory.  Paths not listed
#   fall back to the standard google/protobuf/ copies from the protoc
#   installation.
#   Default: {} (empty — use standard google/protobuf/ copies for all WKTs)
# -----------------------------------------------------------------------
well_known: {}

# -----------------------------------------------------------------------
# import_rewrites
#   Rules applied by canonize_dependency() to every import path found in
#   the descriptor set.  Rules are evaluated in order; the first matching
#   rule wins (there is no "continue" flag — first match stops).
#   Each rule has:
#     match:  string prefix to test against the import path
#     action: "rewrite" | "keep"
#     to:     replacement prefix (only for action: rewrite)
#   Default: [] (no rewrites — import paths are returned unchanged)
# -----------------------------------------------------------------------
import_rewrites: []

# -----------------------------------------------------------------------
# namespace_rewrites
#   Rules applied to every type-reference FQDN (package namespace).
#   Same rule structure and semantics as import_rewrites.
#   Default: [] (no rewrites)
# -----------------------------------------------------------------------
namespace_rewrites: []

# -----------------------------------------------------------------------
# orphans
#   Options listed here exist in the variant's descriptor schema but have
#   no equivalent in the standard google.protobuf namespace.  reproto
#   emits them as ORPHAN blocks (commented out with ///) rather than as
#   live code.
#
#   Note: the edition-related options (features, feature_support,
#   verification) are always treated as orphans regardless of what this
#   section says; they are merged in by variant.py unconditionally.
#
#   Format: mapping from message-kind name to list of field names.
#   Default: {} (no extra orphans beyond the hardwired edition ones)
# -----------------------------------------------------------------------
orphans:
  EnumOptions:            [features, feature_support]
  EnumValueOptions:       [features, feature_support]
  ExtensionRangeOptions:  [features, feature_support, verification]
  FieldOptions:           [features, feature_support]
  FileOptions:            [features, feature_support]
  MessageOptions:         [features, feature_support, map_entry, message_set_wire_format]
  MethodOptions:          [features, feature_support]
  ServiceOptions:         [features, feature_support]

# -----------------------------------------------------------------------
# annotation_modules  (NEW — spec 0018)
#   List of fully-qualified Python module paths to import at startup via
#   importlib.import_module().  Intended for *_pb2 modules that register
#   extension descriptors with the global proto pool so that custom options
#   in the input files are resolved correctly.
#   Modules that cannot be imported produce a cli_warning; they do not
#   abort execution.
#   When the list is empty (the default), no import is attempted and no
#   warning is emitted.
#   Default: [] (empty)
# -----------------------------------------------------------------------
annotation_modules: []
```

#### Hardwired edition orphans

Regardless of the `orphans` section in the variant file, `variant.py`
always merges the following entries (they cannot be removed by a variant):

| Message kind            | Always-orphan fields                          |
|-------------------------|-----------------------------------------------|
| `EnumOptions`           | `features`, `feature_support`                 |
| `EnumValueOptions`      | `features`, `feature_support`                 |
| `ExtensionRangeOptions` | `features`, `feature_support`, `verification` |
| `FieldOptions`          | `features`, `feature_support`                 |
| `FileOptions`           | `features`, `feature_support`                 |
| `MessageOptions`        | `features`, `feature_support`                 |
| `MethodOptions`         | `features`, `feature_support`                 |
| `ServiceOptions`        | `features`, `feature_support`                 |

#### Unknown keys

Unknown YAML keys are silently ignored.  This allows future keys to be
added without breaking older reproto versions that load the same file.

---

### 2. `Options` / `Context` — new field

In `context.py`, add to `Options`:

```python
variant_annotation_modules: list[str] = field(default_factory=list)
```

`Context` exposes it as-is (same pattern as other `variant_*` fields).

---

### 3. `variant.py` — parse `annotation_modules`

In `_parse()`, add:

```python
'variant_annotation_modules': list(raw.get('annotation_modules') or []),
```

Validate that each element is a string; raise `ValueError` if not.

---

### 4. `import_annotations()` in `reproto.py`

Change the signature and body:

```python
def import_annotations(modules: list[str]) -> None:
    """Import annotation modules declared by the active variant.

    Does nothing when the list is empty.  Logs a warning for each module
    that cannot be imported, but continues execution.
    """
    for full_module_name in modules:
        try:
            importlib.import_module(full_module_name)
            cli_info(f"Module '{full_module_name}' imported successfully.")
        except ModuleNotFoundError:
            cli_warning(f"Module '{full_module_name}' not found.")
```

The call site in `reproto()` becomes:

```python
import_annotations(ctx.variant_annotation_modules)
```

Remove the `TYPE_CHECKING` block that imported the hardcoded internal modules.

---

### 5. `google-protobuf.yaml` — add `annotation_modules: []`

Append to the existing file:

```yaml
annotation_modules: []
```

---

## Test coverage

- All existing roundtrip tests must pass unchanged (empty list → no
  warnings, no behavioural change).
- No new test fixtures are required.

---

## Open questions

None.
