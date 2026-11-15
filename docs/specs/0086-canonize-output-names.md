<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0086 — Canonize output names via variant rewrite rules

**Status:** implemented
**Implemented in:** 2026-05-25
**App:** reproto

---

## Background

reproto processes descriptor sets whose file names and package namespaces
may differ from the canonical proto paths.  For example, the `proto2`
variant rewrites:

- `proto2/descriptor.proto` → `google/protobuf/descriptor.proto`
  (via `variant_import_rules` / `canonize_dependency`)
- `.proto2.DescriptorProto` → `.google.protobuf.DescriptorProto`
  (via `variant_ns_rules` / `apply_variant_namespace`)

These rewrites are already applied when rendering **`.proto` text output**:
import strings and type references in the emitted text are canonized.
However, the same rewrites are not applied to **binary descriptor output**
(`--emit-binary`, `--emit-descriptor` / `--build-schema-db`).

This creates a systematic inconsistency: the `.proto` text and the binary
descriptor describe the same schema but with different names.  A consumer
that round-trips through the binary output sees the original (pre-variant)
names, not the canonical ones.

Concretely, in `re_file.py`'s binary side-channel (spec 0076):

```python
fdp_out.CopyFrom(self.this)   # copies original name, dependency[], type names
```

After this copy, no variant rewrites are applied.  The resulting `fdp_out`
carries:

- `fdp_out.name` — original file name (e.g. `proto2/descriptor.proto`)
- `fdp_out.dependency[i]` — original import paths
- `fdp_out.message_type[*].field[*].type_name` — original type references
  (e.g. `.proto2.DescriptorProto`)
- `fdp_out.extension[*].extendee` — original extendee names
- similar in nested messages, enums, services

The rendered `.proto` text for the same file says
`import "google/protobuf/descriptor.proto"` and uses
`.google.protobuf.DescriptorProto`, but the binary says the opposite.

### Previously fixed (§86.1, §86.2)

The **on-disk output paths** for `.proto` and scoring-graph `.yaml` files
were canonized in the initial implementation:

- Phase 7 now writes `.proto` files to
  `out_repo / canonize_dependency(ctx, re_fdp.name)`.
- `_phase_emit_scoring_graphs` now writes `.yaml` files to the same
  canonized path.

The **pool lookup** (`ctx.pool.FindFileByName(re_fdp.name)`) correctly
uses the original name throughout — the pool was built from the original
descriptor set and is keyed by original names.

---

## Goals

1. *(done)* The `.proto` output file path is canonized.
2. *(done)* The scoring-graph YAML output path is canonized.
3. The binary `FileDescriptorProto` emitted by the binary side-channel
   carries canonized names:
   - `fdp_out.name` — canonized via `canonize_dependency`.
   - `fdp_out.dependency[i]` — each entry canonized via
     `canonize_dependency`.
   - All `type_name` / `extendee` string fields in fields, extensions,
     and service method I/O types — canonized via
     `apply_variant_namespace`.
4. The assembled `FileDescriptorSet` written by `--emit-descriptor` /
   `--build-schema-db` is self-consistent: every file's `name` and
   `dependency` entries use canonical names, and the `index.rkyv` is
   keyed by the same canonical names.
5. Pool lookups (keyed by original names) are not changed.
6. The name and type-reference fields in the binary output are consistent
   with the reconstructed `.proto` text: a consumer sees the same
   canonical names in both the text and binary forms.

---

## Non-goals

- Changing the scoring-graph YAML format.
- Canonizing `re_fdp.name` on the node itself (node identity in
  `ctx.nodes` stays keyed by the original name).
- Canonizing the descriptor path when `--keep-descriptor-path` is set —
  `canonize_dependency` already returns it unchanged in that case.
- Canonizing `options` extension field names (custom options use full
  FQDNs already handled by `canonize_opt_name`).

---

## Root cause

The binary side-channel in `re_file.py` calls `fdp_out.CopyFrom(self.this)`
and then selectively re-renders children (messages, enums, services,
extensions) via `render()`.  The re-render rebuilds the child list to
respect summoning/pruning, but does not apply variant rewrites to string
name fields.  The `re_field.py` side-channel similarly copies `self.this`
without applying namespace rewrites to `type_name`.

The text rendering path already calls `canonize_dependency` (in
`re_file.py` for each import) and `apply_variant_namespace` (in
`utils.py`'s `shorten_type_name`) for every reference.  The binary
side-channel needs the same treatment.

---

## Specification

### §86.1 — Canonize `.proto` output path in phase 7 *(implemented)*

In `_phase7_output`, `canonical_name = canonize_dependency(ctx, re_fdp.name)`
is used to derive `res_path`.  The pool lookup uses `re_fdp.name` unchanged.

### §86.2 — Canonize scoring-graph YAML path *(implemented)*

In `_phase_emit_scoring_graphs`, `yaml_path` is derived from `canonical_name`
via `canonize_dependency`.

### §86.3 — Canonize binary FDP: file `name` and `dependency`

After `fdp_out.CopyFrom(self.this)` in `re_file.py`, apply:

```python
from .mappings import canonize_dependency
fdp_out.name = canonize_dependency(ctx, fdp_out.name)
for i, dep in enumerate(fdp_out.dependency):
    fdp_out.dependency[i] = canonize_dependency(ctx, dep)
```

This ensures the binary FDP's file name and import list match the
canonized names used in the `.proto` text output.

### §86.4 — Canonize binary FDP: type names and extendee

Type references in fields and extensions carry variant-specific FQDNs
(e.g. `.proto2.DescriptorProto`) that must be canonized to their
canonical forms (e.g. `.google.protobuf.DescriptorProto`).

The fix is to call `apply_variant_namespace` on every `type_name` and
`extendee` field in the binary side-channel, immediately after each
`CopyFrom`.

In `re_field.py` after `field_out.CopyFrom(self.this)`:

```python
from .mappings import apply_variant_namespace
from .fake_types import Ref
if field_out.type_name:
    field_out.type_name = str(apply_variant_namespace(ctx, Ref(field_out.type_name)))
if field_out.extendee:
    field_out.extendee = str(apply_variant_namespace(ctx, Ref(field_out.extendee)))
```

Note: file-level and message-level extensions do not need separate handling
in `re_file.py` or `re_descriptor.py` — they are re-rendered by calling
`ReFieldDescriptorProto.render()`, so the fix in `re_field.py` covers them.

In `re_method.py` after `method_out.CopyFrom(self.this)`, apply
`apply_variant_namespace` to `input_type` and `output_type`:

```python
from .mappings import apply_variant_namespace
from .fake_types import Ref
if method_out.input_type:
    method_out.input_type = str(apply_variant_namespace(ctx, Ref(method_out.input_type)))
if method_out.output_type:
    method_out.output_type = str(apply_variant_namespace(ctx, Ref(method_out.output_type)))
```

Note: `re_descriptor.py` and `re_service.py` do not need changes.
All field/extension `type_name`/`extendee` fixes are covered by `re_field.py`
(the message and file side-channels re-render fields by calling
`ReFieldDescriptorProto.render()`, which routes through the `re_field.py`
side-channel). Method `input_type`/`output_type` are the only type-name
fields not owned by `re_field.py`.

### §86.5 — `_phase_build_schema_db`: topo-sort consistency

Currently `fdp_by_name` is keyed by `fdp.name` (original) and the
`fdp.dependency` entries are also original, so the topo-sort is
internally consistent.  Once §86.3 is applied, both will use canonical
names and remain consistent.  No additional change is needed here beyond
§86.3.

### §86.6 — `canonize_dependency` and `apply_variant_namespace` are already correct

Both functions handle:

- Files/types with no matching rule: return the name unchanged (no
  regression for non-rewriting variants).
- `--keep-descriptor-path`: `canonize_dependency` returns the descriptor
  path unchanged.
- Multiple chained rules with `continue: true`.

---

## Example

With the `proto2` variant and `proto2/descriptor.proto`:

| Output | Before §86.3/§86.4 | After §86.3/§86.4 |
|---|---|---|
| `.proto` output path | `out/google/protobuf/descriptor.proto` (fixed by §86.1) | (unchanged) |
| `fdp_out.name` | `proto2/descriptor.proto` | `google/protobuf/descriptor.proto` |
| `fdp_out.dependency[i]` | `proto2/descriptor.proto` | `google/protobuf/descriptor.proto` |
| `field_out.type_name` | `.proto2.DescriptorProto` | `.google.protobuf.DescriptorProto` |
| `.proto` text import | `google/protobuf/descriptor.proto` (already correct) | (unchanged) |
| Pool lookup key | `proto2/descriptor.proto` (unchanged) | (unchanged) |

After §86.3/§86.4, the binary FDP is consistent with the `.proto` text.

---

## Files changed

- `reproto/src/reproto/phases.py` — §86.1, §86.2 (done)
- `reproto/src/reproto/re_file.py` — §86.3: canonize `fdp_out.name` and
  `fdp_out.dependency[]`
- `reproto/src/reproto/re_field.py` — §86.4: canonize `field_out.type_name`
  and `field_out.extendee` (covers fields and extensions at all nesting
  levels: file, message, nested message)
- `reproto/src/reproto/re_method.py` — §86.4: canonize `method_out.input_type`
  and `method_out.output_type`

---

## References

- Spec 0045 — `--emit-scoring-yaml` original specification
- Spec 0068 — FDS index (`index.rkyv`)
- Spec 0076 — binary output side-channel
- Spec 0080 — schema DB WKT completion
- `reproto/src/reproto/mappings.py` — `canonize_dependency()`,
  `apply_variant_namespace()`
