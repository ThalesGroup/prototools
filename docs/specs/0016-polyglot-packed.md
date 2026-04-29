<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0016 — Polyglot mode: packed encoding

**Status:** implemented
**Implemented in:** 2026-04-29
**App:** reproto

---

## Problem

Reproto currently always emits `syntax = "proto2";` and renders fields with
proto2 conventions, regardless of the original file's syntax.  Spec 0015
defines the full scope of changes needed to support proto3.  This spec
implements the first concrete slice: a `--polyglot` feature flag that
enables syntax-aware rendering, starting with packed repeated field encoding.

In proto2, `[packed = true]` must be emitted explicitly for packed fields
because the default is unpacked.  In proto3, the default is packed, so a
field with `HasField("packed") == False` in the descriptor must emit
*nothing* — emitting `[packed = true]` would be redundant and may cause
roundtrip divergence.  The current code emits whatever the generic options
path produces, which is wrong for proto3.

---

## Goals

1. Add a `--polyglot` CLI flag to `reproto`.  When absent, behaviour is
   identical to today (proto2 only, no syntax inspection).
2. Add `polyglot: bool` to `Options` and `ctx.syntax: str` to `Context`.
3. In `--polyglot` mode, set `ctx.syntax` at the start of each file's
   rendering to `effective_syntax(fdp)`.
4. In `--polyglot` mode, make packed rendering syntax-aware: suppress
   `[packed = true]` for proto3 fields where `HasField("packed") == False`
   (i.e., the proto3 default applies).
5. Add a `re_syntax.py` module with `effective_syntax()` and
   `packed_option()` as the first two free functions.
6. Add proto3 packed fixture files (sourced from `docs/mockup/`) to the
   reproto test fixtures.
7. Add roundtrip regression tests that exercise packed encoding in both
   proto2 and proto3 under `--polyglot`, using the harness from spec 0017.

---

## Non-goals

- Any other proto3 rendering differences (field labels, defaults, synthetic
  oneofs, etc.) — deferred to later specs in the 0015 series.
- Editions support.
- Syntax-override YAML (`--syntax-overrides`) — deferred to spec 0015.
- Changing the behaviour of the existing test suite (all existing tests must
  continue to pass without `--polyglot`).
- Improving the `.pb` diff or `.proto` text comparison in the existing
  `test_roundtrip` — that is spec 0017's scope.

---

## Background

See `docs/proto2-proto3-findings.md` Parts IV and I for the empirical basis.
Key facts:

- `fdp.syntax == ""` for all proto2 files; `"proto3"` for proto3.
- For a proto3 `repeated int32` with no `[packed]` annotation:
  `HasField("packed") == False` in the descriptor, but the runtime encodes
  it as packed (confirmed via wire-level test in `docs/mockup/`).
- For a proto3 `repeated int32 [packed = true]`:
  `HasField("packed") == True, packed == True` — protoc preserves the
  explicit annotation.
- The rendering rule is therefore **syntax-independent at the descriptor
  level**: mirror `HasField("packed")` exactly.  The syntax only matters
  for interpreting what "unset" means at *runtime*, not for rendering.

The spec-0015 rendering rule (emit nothing when unset) is correct for both
syntaxes when outputting to the same syntax.  The issue is when reproto
incorrectly outputs a proto3 field as proto2: `HasField("packed") == False`
in proto3 means "packed by default", but in the emitted proto2 file it would
mean "unpacked by default" — a semantic mismatch.  `--polyglot` fixes this
by emitting the correct `syntax` line and suppressing the redundant explicit
annotation.

---

## Specification

### 1. `syntax.py` — new module

Create `reproto/src/reproto/syntax.py`.

```python
def fdp_syntax(fdp) -> str:
    """Return the syntax of a FileDescriptorProto as a non-empty string.

    fdp.syntax is "" for proto2 files (protoc omits the field); normalise
    that to "proto2".  All other values are returned as-is.
    """
    return fdp.syntax or "proto2"


def packed_option(
    source_syntax: str,
    target_syntax: str,
    has_field: bool,
    effective_packed: bool,
) -> str | None:
    """
    Return the string to emit for the packed option, or None to emit nothing.

    Args:
        source_syntax:   syntax of the input file ("proto2" or "proto3")
        target_syntax:   syntax reproto will emit ("proto2" or "proto3")
        has_field:       True if packed was explicitly set in the source .proto
        effective_packed: fo_msg.packed — the wire-level effective value
                         (includes proto3 defaults)

    Rules:
      - has_field=True  → emit the explicit value regardless of syntax
      - has_field=False, source==target → emit nothing (default preserved)
      - has_field=False, source=proto3, target=proto2 → emit "true" if
        effective_packed is True (preserve wire semantics across downconversion)
    """
    if has_field:
        return "true" if effective_packed else "false"
    if source_syntax == target_syntax:
        return None
    # Cross-syntax conversion: source=proto3 (packed by default), target=proto2
    # (unpacked by default) — must emit explicit annotation to preserve semantics.
    if effective_packed:
        return "true"
    return None
```

### 2. `Options` — add `polyglot` flag

In `context.py`, add to the `Options` dataclass:

```python
polyglot: bool = False
```

### 3. `Context` — add `syntax` and `target_syntax` attributes

In `Context.__init__`, add:

```python
self.syntax: str = "proto2"         # input file syntax, set per file in polyglot mode
self.target_syntax: str = "proto2"  # output syntax; defaults to proto2 (non-polyglot)
```

In `--polyglot` mode, `ctx.target_syntax = ctx.syntax` when `ctx.syntax` is
`"proto2"` or `"proto3"` (same-syntax roundtrip).  For `"editions"` and any
unknown syntax, `ctx.target_syntax = "proto2"` (fallback; editions support is
out of scope for this spec).

Without `--polyglot`, `ctx.target_syntax = "proto2"` always (current
behaviour preserved).

### 4. `cli.py` — add `--polyglot` flag

Add a new Click option before the existing output options:

```python
@click.option(
    '--polyglot',
    is_flag=True,
    default=False,
    help='Enable syntax-aware rendering (proto2 and proto3). '
         'Without this flag, all output is proto2.',
)
```

Pass `polyglot=polyglot` in the `Options(...)` constructor call.

### 5. `re_file.py` — set `ctx.syntax` and `ctx.target_syntax` per file

At the top of `ReFileDescriptorProto.render()`, before any output is
produced, add:

```python
from .syntax import fdp_syntax
ctx.syntax = fdp_syntax(self.this)
if ctx.polyglot and ctx.syntax in ("proto2", "proto3"):
    ctx.target_syntax = ctx.syntax
else:
    ctx.target_syntax = "proto2"
```

This overwrites both attributes for each file.  Rendering is single-threaded
and processes one file at a time, so this is safe.

The `syntax = "...";` line in the output then uses `ctx.target_syntax`
instead of the hardcoded `"proto2"`.  The "Note: The original file used..."
comment is suppressed when `ctx.syntax == ctx.target_syntax`.

### 6. `re_field.py` — syntax-aware packed rendering

`fo_msg` is treated as **read-only** throughout.

In `ReFieldDescriptorProto.render()`, after building `fo_msg`, collect the
packed data directly from the original descriptor proto and pass an
`exclude={"packed"}` set to `render_options_from_message` so the generic
path skips `packed`:

```python
from .syntax import packed_option
has_packed = (self.this.HasField('options')
              and self.this.options.HasField('packed'))
effective_packed = fo_msg.packed  # wire-level value (includes proto3 defaults)
packed_str = packed_option(
    ctx.syntax, ctx.target_syntax, has_packed, effective_packed
)
```

Then pass `exclude={"packed"}` to `render_options_from_message` and inject
the result first in `opt_block`:

```python
if packed_str is not None:
    opt_block.append(BlockLine(f'packed = {packed_str},', depth + 1))
```

`render_options_from_message` in `base.py` gains an `exclude: set[str]`
parameter (default `set()`) that skips any built-in field whose name is in
the set.

### 7. Test fixtures

Copy two mockup files into `reproto/src/reproto/tests/fixtures/`:

- `packed_proto2.proto` — sourced from `docs/mockup/f07_packed_proto2.proto`
- `packed_proto3.proto` — sourced from `docs/mockup/f08_packed_proto3.proto`

Drop the `f07_`/`f08_` prefix.  Leave `package mockup;` as-is; the
roundtrip test does not check package names.

### 8. Roundtrip regression tests

In `test_roundtrip.py`, add a new parametrized test function:

```python
POLYGLOT_FIXTURES = [
    "packed_proto2.proto",
    "packed_proto3.proto",
]

@pytest.mark.parametrize("fixture_name", POLYGLOT_FIXTURES)
def test_roundtrip_polyglot(fixture_name: str, tmp_path):
    """
    Roundtrip test under --polyglot mode.  Uses the normalize() harness
    from spec 0017 for both .pb and .proto comparison.
    """
```

Factor the shared roundtrip logic into a helper so that both
`test_roundtrip` and `test_roundtrip_polyglot` call it.

---

## Test coverage

After this spec is implemented, running `pytest` must show:

- All existing `test_roundtrip[*]` tests still pass (no regression).
- `test_roundtrip_polyglot[packed_proto2.proto]` passes.
- `test_roundtrip_polyglot[packed_proto3.proto]` passes.

The proto3 test confirms that a proto3 file with a default-packed field
roundtrips byte-identically through reproto under `--polyglot`, and that
reproto reproduces the original source structure (no spurious
`[packed = true]`).

---

## Open questions

None.
