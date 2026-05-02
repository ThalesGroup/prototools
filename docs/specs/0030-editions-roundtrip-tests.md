<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0030 — Editions roundtrip tests

**Status:** implemented
**Implemented in:** 2026-05-02
**App:** reproto

---

## Purpose

Add automated end-to-end roundtrip tests for edition `.proto` files.
After specs 0028–0029, reproto fully renders edition files; this spec
closes the testing gap by:

1. Adding a `test_roundtrip_edition` parametrized test that exercises
   the `.proto → .pb → reproto → .pb` pipeline and asserts descriptor
   fidelity.
2. Adding an edition 2023 fixture that exercises all constructs from
   phases 1–4.
3. Discussing what a future edition 2024 (or later) fixture would
   require and what is needed to enable it.
4. Clarifying the proto-text comparison policy for edition roundtrips.

---

## Background

`test_roundtrip.py` already has `_run_roundtrip` which:

1. Compiles a `.proto` fixture with `protoc --descriptor_set_out`.
2. Runs reproto to regenerate the `.proto` from the `.pb`.
3. Recompiles the regenerated `.proto` with `protoc`.
4. Asserts `.pb` byte-identity (after clearing `source_code_info`).
5. Asserts normalized `.proto` text equality via `normalize_proto_batch`,
   which runs `uncomment` (tree-sitter comment stripping) followed by
   `buf format` (canonical whitespace and ordering).

Step 5 is fully applicable to edition files: `buf format` supports
editions syntax, and the normalize pipeline eliminates the surface
differences (comment headers, whitespace, option ordering) between
the source fixture and reproto's output.  Both `.pb` and `.proto`
comparisons should be run for edition roundtrips, exactly as for
proto2/proto3.

---

## Goals

1. Add `EDITION_FIXTURES` list and `test_roundtrip_edition` in
   `test_roundtrip.py`.
2. Add `editions_roundtrip.proto` fixture covering all phase 1–4
   constructs.
3. Add discussion of edition 2024 support and what would be needed.
4. No changes to production code — tests only.

---

## Non-goals

- Automated testing of edition 2024 or later (blocked on protoc support
  in the Nix shell; tracked as future work below).
- Skipping proto-text comparison for editions (both `.pb` and `.proto`
  comparisons are run, same as proto2/proto3).
- `--force-proto2-output` roundtrip testing of edition sources (the
  `.pb` will differ by `syntax` and `features` fields; this is correct
  behaviour covered separately).

---

## Specification

### 1. Fixture coding style

The fixture must follow the rules in
`reproto/src/reproto/tests/fixtures/STYLE.md` so that the proto-text
comparison passes.  That document covers definition order, custom option
name qualification, reserved statement formatting, bytes default escapes,
`json_name` suppression, and multi-option field formatting.

Two additional rules specific to edition fixtures:

- **Field-level feature overrides use inline composite form.**  Reproto
  emits `int32 x = 1 [features.field_presence = EXPLICIT];`, not a
  standalone `option features.field_presence = EXPLICIT;` line inside
  the message body.  Fixtures must match.

- **Message- and enum-level feature overrides use standalone `option`
  form.**  Reproto emits `option features.enum_type = CLOSED;` as a
  standalone statement, not inline.  Fixtures must match.

### 2. `editions_roundtrip.proto` fixture

Add `reproto/src/reproto/tests/fixtures/editions_roundtrip.proto`.

The file uses `edition = "2023"` and exercises:

- File-level `features` override (e.g. `option features.enum_type = CLOSED;`).
- Message with `features` override (e.g. `option features.field_presence = IMPLICIT;`).
- Field with `field_presence = IMPLICIT` (no label, proto3-like).
- Field with `field_presence = EXPLICIT` (explicit presence).
- Field with `field_presence = LEGACY_REQUIRED`.
- Repeated field with `repeated_field_encoding = EXPANDED`.
- Message field with `message_encoding = DELIMITED` (group-style).
- Field with `field_presence = EXPLICIT` and a `default` value.
- An enum with `features.enum_type = CLOSED`.
- A custom option extension (to verify extension options still render).
- `import weak` (allowed in editions).
- `extensions` range (allowed in editions).

The companion `weak_import_proto2_dep.proto` is already in the fixtures
directory and can be reused as the weak-import target (it is proto2,
which editions can import).

### 2. `EDITION_COMPANIONS` and `EDITION_FIXTURES`

In `test_roundtrip.py`:

```python
EDITION_FIXTURES: list[str] = [
    "editions_roundtrip.proto",
]
```

If the fixture imports a companion (e.g. for weak import), add it to
`FIXTURE_COMPANIONS`.

### 3. `test_roundtrip_edition`

```python
@pytest.mark.parametrize("fixture_name", EDITION_FIXTURES)
def test_roundtrip_edition(fixture_name: str, tmp_path: Path) -> None:
    """End-to-end roundtrip for edition .proto files.

    Same two-level check as for proto2/proto3: .pb descriptor
    byte-identity and normalized .proto text equality (uncomment
    + buf format).
    """
    orig_dir = tmp_path / "orig"
    new_dir = tmp_path / "new"
    orig_dir.mkdir()
    new_dir.mkdir()
    _, content = get_fixture_content(fixture_name)
    _run_roundtrip(fixture_name, content, orig_dir, new_dir)
```

`_run_roundtrip` with `compare_proto=False` already skips step 5.  The
`.pb` comparison in step 4 uses `pb_diff` / `normalize_proto_batch` and
clears `source_code_info` before comparing — confirmed sufficient by the
manual experiment that preceded this spec.

### 4. `_run_roundtrip` `.pb` comparison and `source_code_info`

The current `_run_roundtrip` compiles the fixture **without**
`--include_imports`, so `source_code_info` is absent from both the
original and recompiled `.pb`.  Raw byte comparison therefore works
for edition fixtures too, as verified by the manual experiment
preceding this spec.

No changes to `_run_roundtrip` are needed.

### 5. Edition 2024 and future editions

#### What exists today

The `Edition` enum in `descriptor.proto` defines `EDITION_2024` (value
1001) alongside `EDITION_2023` (1000).  The `render_features_block` and
`allow_*` guards in reproto are edition-year-agnostic: they key off
`ctx.target_syntax == "editions"` without checking the year.
`_edition_name` maps any `Edition` enum value to its string name
generically.

#### What is missing

- **protoc support in the Nix shell**: the current pinned protoc may
  not support `edition = "2024"`.  Check with `protoc --version` and
  the protoc changelog.  If `edition = "2024"` is not accepted,
  a fixture cannot be compiled and the test cannot run.
- **New feature defaults**: edition 2024 may introduce new defaults for
  existing `FeatureSet` fields (e.g. `json_format`, `utf8_validation`)
  or new fields entirely.  The `_FEATURE_FIELDS` list in `syntax.py`
  only covers the six fields present since edition 2023.  New fields
  would need to be added.
- **`build_edition_defaults`**: the edition defaults table is built from
  the descriptor's `FeatureSet.edition_defaults` annotations.  If the
  descriptor bundled in the variant does not include 2024 defaults,
  feature resolution will fall back to 2023 defaults for edition 2024
  files — potentially wrong.

#### Future test structure

When edition 2024 support is available:

```python
EDITION_FIXTURES: list[str] = [
    "editions_roundtrip.proto",        # 2023
    "editions_roundtrip_2024.proto",   # 2024 (future)
]
```

Each fixture is parametrized independently so a missing protoc does not
block the 2023 tests.  Use `pytest.mark.skipif` keyed on a
`protoc_supports_edition` helper if needed.

---

## Testing

| Test | What it covers |
|---|---|
| `test_roundtrip_edition[editions_roundtrip.proto]` | Full .pb + .proto roundtrip for edition 2023 |
| T-rt-2 regression: existing `test_roundtrip` fixtures | No proto2/proto3 regression |

---

## Modified files summary

| File | Change |
|---|---|
| `reproto/src/reproto/tests/test_roundtrip.py` | Add `EDITION_FIXTURES`, `test_roundtrip_edition`, companion entries |
| `reproto/src/reproto/tests/fixtures/editions_roundtrip.proto` | New fixture |
| `reproto/src/reproto/tests/fixtures/editions_custom_option_dep.proto` | New companion (imported custom option) |

No changes to production code.

---

## Open questions

All resolved during implementation:

1. **`--include_imports` for edition fixtures**: confirmed.  The fixture
   compiles without `--include_imports`; `source_code_info` is absent
   on both sides and raw byte comparison works.

2. **Weak import in the fixture**: `weak_import_proto2_dep.proto` is a
   suitable companion.  The fixture references `WeakDep` (short name,
   relative to package `mockup.editions`) in a field to avoid a protoc
   unused-import warning.

3. **Custom option in the fixture**: both strategies are used.  The
   fixture defines a local `extend google.protobuf.MessageOptions` block
   (inline) and imports `editions_custom_option_dep.proto` (companion)
   which defines a second `MessageOptions` extension.  The companion is
   proto2, imported by the editions fixture, verifying cross-syntax
   extension import.
