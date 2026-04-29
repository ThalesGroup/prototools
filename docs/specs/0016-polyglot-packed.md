<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0016 — Polyglot mode: packed encoding

**Status:** draft
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

### 1. `re_syntax.py` — new module

Create `reproto/src/reproto/re_syntax.py`.

```python
def effective_syntax(fdp) -> str:
    """Return "proto2", "proto3", or "editions" from a FileDescriptorProto."""
    s = fdp.syntax          # "" for proto2, "proto3", or "editions"
    if s in ("", "proto2"):
        return "proto2"
    if s == "proto3":
        return "proto3"
    if s == "editions":
        return "editions"
    return "proto2"         # unknown: fall back to proto2


def packed_option(ctx: Context, is_packed: bool | None) -> str | None:
    """
    Return the string to emit for the packed option, or None to emit nothing.

    is_packed is the result of:
      True  if HasField("packed") and packed == True
      False if HasField("packed") and packed == False
      None  if not HasField("packed")  (option absent)

    Rules (findings doc Part IV):
      - None  → emit nothing (use syntax default in both proto2 and proto3)
      - True  → emit "true"
      - False → emit "false"

    ctx is accepted for future use (editions feature resolution).
    """
    if is_packed is None:
        return None
    return "true" if is_packed else "false"
```

### 2. `Options` — add `polyglot` flag

In `context.py`, add to the `Options` dataclass:

```python
polyglot: bool = False
```

### 3. `Context` — add `syntax` attribute

In `Context.__init__`, add:

```python
self.syntax: str = "proto2"   # overwritten per file in polyglot mode
```

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

### 5. `re_file.py` — set `ctx.syntax` per file

At the top of `ReFileDescriptorProto.render()`, before any output is
produced, add (under a `--polyglot` guard):

```python
if ctx.polyglot:
    from .re_syntax import effective_syntax
    ctx.syntax = effective_syntax(self.this)
```

This overwrites `ctx.syntax` for each file.  Rendering is single-threaded
and processes one file at a time, so this is safe.

### 6. `re_field.py` — syntax-aware packed rendering

The current code renders `packed` via the generic `render_options_from_message`
path, which emits whatever `FieldOptions.packed` says.

Under `--polyglot`, intercept packed rendering:

In `ReFieldDescriptorProto.render()`, after building `fo_msg` from the
serialized field options, clear the `packed` field from `fo_msg` before
passing it to `render_options_from_message`, and instead inject the correct
packed annotation computed by `packed_option()`.

Concretely, in the options block construction section:

```python
if ctx.polyglot:
    from .re_syntax import packed_option
    if self.this.options.HasField('packed'):
        is_packed = self.this.options.packed   # True or False
    else:
        is_packed = None                       # absent
    packed_str = packed_option(ctx, is_packed)
    # Clear packed from fo_msg so the generic path does not re-emit it
    fo_msg.ClearField('packed')
else:
    # Non-polyglot mode: fo_msg is passed to render_options_from_message
    # unchanged, which emits packed (and all other FieldOptions) exactly
    # as stored in the descriptor.  packed_str = None means we do not
    # inject any additional annotation.
    packed_str = None
```

Then, after `render_options_from_message` produces `option_blocks`, inject
the packed annotation if present:

```python
if packed_str is not None:
    opt_block.append(BlockLine(f'packed = {packed_str},', depth + 1))
```

Insert this **before** the `render_options_from_message` call so that
`packed` appears first in the options list (matching protoc convention).

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

1. Should `--polyglot` also fix the `syntax` line in the output (emit
   `syntax = "proto3";` instead of `syntax = "proto2";`)?  This is needed
   for full correctness but is out of scope here.  Without it, the packed
   roundtrip test still passes because `protoc` accepts `[packed = true]`
   in proto2 and produces the same descriptor.  Fixing the syntax line is
   deferred to the next spec in the 0015 series.
