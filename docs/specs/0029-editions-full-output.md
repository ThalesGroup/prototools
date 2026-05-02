<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0029 — Editions rendering: phase 4 — complete edition output

**Status:** implemented
**Implemented in:** 2026-05-02
**App:** reproto

---

## Purpose

Replace the proto2 stub output for edition files with correct, roundtrippable
edition `.proto` output: emit `edition = "2023";` instead of `syntax = "proto2";`,
remove the A1 warning, update `ctx.target_syntax`, extend the allow-guards
in `syntax.py` to cover editions, and add end-to-end roundtrip tests.

This is phase 4 of the strategy described in
`docs/specs/0025-editions-rendering-strategy.md`.

After this phase, reproto fully supports editions: an edition `.pb` file
compiled from a `.proto` source can be reconstructed by reproto into a
semantically equivalent edition `.proto`, and recompiling that output produces
a byte-identical descriptor.

---

## Background

After phases 1–3:

- The feature resolution engine is in place (`resolve_features`,
  `ctx.edition_defaults`).
- All per-element decisions (field labels, packed, groups, synthetic oneofs,
  default values) use `ResolvedFeatures` (phase 2).
- `features { }` blocks are emitted with only the explicit overrides
  (phase 3).

The remaining gap is the file header and a set of file-level guards that
still hard-code `ctx.target_syntax == "proto2"` for edition files.

Specifically:

| Current behavior | Required behavior |
|---|---|
| `syntax = "proto2";` emitted for edition files | `edition = "2023";` (or the actual edition) |
| A1 warning emitted for every edition file | A1 suppressed for fully-rendered edition files |
| `ctx.target_syntax = "proto2"` for edition files | `ctx.target_syntax = "editions"` |
| `allow_extension_ranges`, `allow_extend_block`, `allow_weak_import`, `allow_message_set_wire_format` use `ctx.target_syntax == "proto2"` | These must also return `True` when `ctx.target_syntax == "editions"` |
| `allow_extend_block` proto3 guard checks `_DESCRIPTOR_OPTIONS_FQNS` | Editions allow the same set as proto2 |

---

## Goals

1. In `re_file.py`, when `ctx.syntax == "editions"`, set
   `ctx.target_syntax = "editions"` and emit `edition = "<name>";` instead
   of `syntax = "...";`.
2. Suppress the A1 warning for edition files that complete rendering without
   falling back to proto2.
3. Update the `allow_*` guards in `syntax.py` to accept
   `ctx.target_syntax == "editions"` wherever `"proto2"` is accepted.
4. Update `_DESCRIPTOR_OPTIONS_FQNS` logic: in editions, `extend` is allowed
   for all types, not just the nine descriptor options (same as proto2).
5. Add edition roundtrip fixtures and a golden roundtrip test.
6. The output for proto2 and proto3 files must be byte-for-byte identical
   to the current output (no regression).

---

## Non-goals

- Language-specific feature extensions (`pb.cpp`, `pb.java`).
- Support for editions other than `"2023"` (they use the same structure;
  this is not excluded, just not explicitly tested).
- `--force-proto2-output` conversion from editions source (out of scope;
  the A2 warning path handles this already once `ctx.syntax == "editions"`
  is properly detected).

---

## Specification

### 1. `ctx.target_syntax` for edition files

In `re_file.py`, `render()`, update the syntax-to-target-syntax mapping:

```python
from .syntax import fdp_syntax
ctx.syntax = fdp_syntax(self.this)
if not ctx.force_proto2_output and ctx.syntax in ("proto2", "proto3", "editions"):
    ctx.target_syntax = ctx.syntax
else:
    ctx.target_syntax = "proto2"
```

When `ctx.force_proto2_output` is True and the source is editions, the
existing A2 path fires and `ctx.target_syntax = "proto2"` remains correct.

### 2. Edition file header in `re_file.py`

Replace the current header block:

```python
if ctx.syntax == "editions":
    out.append(report("A1", depth, file=self.name))
elif ctx.syntax != ctx.target_syntax:
    out.append(report("A2", depth, file=self.name, syntax=ctx.syntax))
out.append(BlockLine(f'syntax = "{ctx.target_syntax}";', depth))
```

With:

```python
if ctx.target_syntax == "editions":
    edition_name = _edition_name(self.this.edition)
    out.append(BlockLine(f'edition = "{edition_name}";', depth))
elif ctx.syntax != ctx.target_syntax:
    out.append(report("A2", depth, file=self.name, syntax=ctx.syntax))
    out.append(BlockLine(f'syntax = "{ctx.target_syntax}";', depth))
else:
    out.append(BlockLine(f'syntax = "{ctx.target_syntax}";', depth))
```

where `_edition_name` maps the `Edition` enum integer to its string name
(`"2023"`, `"2024"`, etc.):

```python
def _edition_name(edition: int) -> str:
    """Map Edition enum value to the string used in .proto source."""
    from google.protobuf.descriptor_pb2 import Edition
    name = Edition.Name(edition)          # e.g. "EDITION_2023"
    if name.startswith("EDITION_"):
        return name[len("EDITION_"):]     # e.g. "2023"
    return name
```

### 3. A1 anomaly

A1 is now only emitted when the edition file falls back to proto2 output (i.e.
when `ctx.force_proto2_output` is True).  In normal rendering of an edition
file, A1 is never triggered.

The A1 anomaly entry in `anomalies.py` is updated to reflect the new
condition:

```
"A1": editions file rendered as proto2 due to --force-proto2-output
```

The stderr message becomes:
`"'{file}': editions file rendered as proto2 (--force-proto2-output)"`

### 4. `allow_*` guards in `syntax.py`

Every guard that currently reads `ctx.target_syntax == "proto2"` must also
accept `ctx.target_syntax == "editions"`:

```python
def allow_extension_ranges(ctx: Context) -> bool:
    return ctx.target_syntax in ("proto2", "editions")

def allow_extensions(ctx: Context) -> bool:
    return ctx.target_syntax in ("proto2", "editions")

def allow_weak_import(ctx: Context) -> bool:
    return ctx.target_syntax in ("proto2", "editions")

def allow_message_set_wire_format(ctx: Context) -> bool:
    return ctx.target_syntax in ("proto2", "editions")

def allow_groups(ctx: Context, features: ResolvedFeatures | None = None) -> bool:
    if features is not None:
        return features.message_encoding == MESSAGE_ENCODING_DELIMITED
    return ctx.target_syntax in ("proto2", "editions")
```

`allow_extend_block` in proto3 restricts extendees to `_DESCRIPTOR_OPTIONS_FQNS`.
For editions (same as proto2), all extendees are allowed:

```python
def allow_extend_block(ctx: Context, extendee: str) -> bool:
    if ctx.target_syntax in ("proto2", "editions"):
        return True
    # proto3: only descriptor *Options are allowed
    from .fake_types import Ref
    from .mappings import apply_variant_namespace
    canonical = str(apply_variant_namespace(ctx, Ref(extendee)))
    return canonical in _DESCRIPTOR_OPTIONS_FQNS
```

### 5. `render_file_options` exclusion of `features`

Phase 3 added `"features"` to the exclude set in options rendering.  In
phase 4, verify this exclude is in place before lifting the A1 guard, to
avoid double-rendering `features` once `ctx.target_syntax == "editions"`.

### 6. Roundtrip fixture

Add `editions_roundtrip.proto` to `reproto/src/reproto/tests/fixtures/`:

A handcrafted edition 2023 file that exercises all constructs that survived
phases 1–4:

- File-level `features` override.
- Message with `features` override.
- Field with `field_presence = IMPLICIT` (no label).
- Field with `field_presence = EXPLICIT` (optional).
- Field with `field_presence = LEGACY_REQUIRED` (required).
- Repeated field with `repeated_field_encoding = EXPANDED`.
- Message field with `message_encoding = DELIMITED` (group-style block).
- Field with `field_presence = EXPLICIT` and `default_value`.
- An enum with `features.enum_type = CLOSED`.
- A custom option extension (to verify extension options still render).
- `import weak` (allowed in editions).
- `extensions` range (allowed in editions).

Add the fixture to `DEFAULT_FIXTURES` in `test_roundtrip.py` (or a new
`EDITION_FIXTURES` list with its own parametrized test).

### 7. Golden roundtrip test

Add `test_roundtrip_edition` in `test_roundtrip.py`:

```python
@pytest.mark.parametrize("fixture_name", EDITION_FIXTURES)
def test_roundtrip_edition(fixture_name: str, tmp_path: Path) -> None:
    """End-to-end roundtrip: edition .proto → .pb → reproto → .pb; assert pb equal."""
```

The test:
1. Compiles the fixture with `protoc --include_imports`.
2. Runs reproto with `--use-variant descriptor`.
3. Recompiles reproto's output with `protoc`.
4. Asserts `pb1 == pb2` (byte-identical descriptor sets, minus `source_code_info`).

Step 4 is the correctness criterion: roundtrip fidelity means the descriptor
is fully preserved, not just the `.proto` text.

### 8. `fdp_syntax` — no change needed

Empirically confirmed (protoc 27+): edition files have `fdp.syntax == "editions"`
(not the empty string).  The current `fdp_syntax` implementation,
`return fdp.syntax or "proto2"`, already returns `"editions"` for edition
files and `"proto2"` for proto2 files where `fdp.syntax == ""`.

No change to `fdp_syntax` is required for protoc 27+.  If support for older
protoc versions is ever needed, the `fdp.edition` fallback can be added then.

---

## Decision: `ctx.target_syntax = "editions"`

Setting `ctx.target_syntax = "editions"` (rather than keeping `"proto2"`)
is the right call for phase 4 because:

- The `allow_*` guards in `syntax.py` become logically correct: edition files
  allow the same constructs as proto2, and the guards express this directly.
- The `features` exclusion added in phase 3 is gated on
  `ctx.syntax == "editions"` at the call sites, not on `target_syntax`, so
  changing `target_syntax` does not affect it.
- The existing A2 downconversion path continues to work: when
  `--force-proto2-output` is set, `target_syntax = "proto2"` regardless of
  source syntax.
- The risk of audit is low: `ctx.target_syntax` comparisons are concentrated
  in `syntax.py` (already updated above) and the few call sites in
  `re_file.py`.  A targeted grep confirms the scope.

---

## Testing

| Test | What it covers |
|---|---|
| T-rt-1: `test_roundtrip_edition` | Full roundtrip: pb1 == pb2 for edition fixture |
| T-rt-2: proto2/proto3 regression | `test_roundtrip` still passes for all existing fixtures |
| T-rt-3: `fdp_syntax` editions | `fdp.syntax = ""`, `fdp.edition = 1000` → returns `"editions"` |
| T-rt-4: `fdp_syntax` proto2 | Both empty → returns `"proto2"` |
| T-rt-5: `allow_extension_ranges` editions | `ctx.target_syntax = "editions"` → `True` |
| T-rt-6: `allow_weak_import` editions | `ctx.target_syntax = "editions"` → `True` |
| T-rt-7: `allow_extend_block` editions | All extendees → `True` |
| T-rt-8: `edition = "2023";` header | reproto output starts with `edition = "2023";` |
| T-rt-9: A1 suppressed | No A1 warning in normal edition rendering |
| T-rt-10: A1 with `--force-proto2-output` | A1 warning emitted; output is proto2 |

---

## Modified files summary

| File | Change |
|---|---|
| `reproto/src/reproto/syntax.py` | Update all `allow_*` guards; update `fdp_syntax` |
| `reproto/src/reproto/re_file.py` | Emit `edition = "...";`; update `ctx.target_syntax`; suppress A1 |
| `reproto/src/reproto/anomalies.py` | Update A1 message/condition |
| `reproto/src/reproto/tests/fixtures/editions_roundtrip.proto` | New fixture |
| `reproto/src/reproto/tests/test_roundtrip.py` | Add `test_roundtrip_edition` |
| `reproto/src/reproto/tests/test_editions_rendering.py` | Add T-rt-3–T-rt-10 |

No changes to `re_descriptor.py`, `re_field.py`, `re_enum.py`,
`feature_resolution.py`, `base.py`, or `context.py` in this phase.

---

## Open questions

1. **`source_code_info` in roundtrip comparison**: protoc strips
   `source_code_info` from the roundtripped `.pb` (reproto does not emit
   comments from source_code_info back into the reconstructed file).  The
   roundtrip test should compare with `source_code_info` cleared on both
   sides, or use `pb_diff` / `pb_diff_fields` from `proto_normalize.py`.
   Confirm the right comparison strategy before writing the test.

2. **`fdp.syntax = ""` vs `fdp.edition != 0`**: Empirically confirmed —
   protoc 27+ sets `fdp.syntax = "editions"` for edition files.  The current
   `fdp_syntax` already handles this correctly.  No change needed.

3. **`--force-proto2-output` + editions**: when `--force-proto2-output` is
   set and the source is an edition file, `ctx.target_syntax = "proto2"`.
   Phase 3's `render_features_block` is gated on
   `ctx.target_syntax == "editions"`, so no `features { }` blocks appear in
   the proto2 output — correct.  The A1 anomaly (updated in §3) is emitted
   to signal the downconversion.
