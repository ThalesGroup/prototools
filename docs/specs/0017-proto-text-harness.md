<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0017 — Proto text comparison harness

**Status:** implemented
**Implemented in:** 2026-04-29
**App:** reproto

---

## Problem

The existing `test_roundtrip` tests compare reproto's `.pb` output against
the reference `.pb` byte-for-byte.  When this comparison fails, the error
message is opaque: raw binary differs with no indication of which field
changed.  There is also no check that the reproto-generated `.proto` text is
structurally equivalent to the original fixture source.

---

## Goals

1. Add a `normalize_proto` function that strips comments and canonicalizes
   formatting from a `.proto` source string, producing a stable string for
   equality comparison.
2. Normalize in two phases: **uncomment first** (tree-sitter, syntax-aware),
   **then format** (buf).  Buf runs on comment-free text, giving it a clean
   input and making its output deterministic regardless of where comments
   appeared in the original.
3. Batch all `buf format` calls in a single subprocess invocation — one call
   per test run over all files that need normalization, not one call per file.
4. Apply the `.proto` text comparison to **all current fixtures** in
   `test_roundtrip`.  This validates that all existing reproto output is
   structurally equivalent to its fixture source.
5. Improve `.pb` failure messages: when a `.pb` comparison fails, decode
   both sides via `prototext --decode` and include the text diff in the
   assertion error.

---

## Non-goals

- Changes to reproto rendering logic — that is spec 0016's scope.
- Editions support in the harness (buf handles it transparently).
- Normalizing `.proto` text for any purpose other than test comparison.

---

## Background

**Phase order — uncomment then buf, not buf then uncomment:**

If buf runs first, it may reposition comments (buf preserves comments but
reattaches them to adjacent nodes, with placement that is implementation-
defined and may change across buf versions).  Running tree-sitter first
removes all comments before buf sees the file, so buf's output is purely
structural and version-stable.

**buf batching:**

`buf format --write` applied to a directory formats all `.proto` files in
that directory in a single subprocess call.  By writing all inputs to a
shared tmpdir with a minimal `buf.yaml`, all files are formatted in one
invocation.

**tree-sitter grammar from nixpkgs:**

`pythonPkgs.tree-sitter` (v0.24.0) and
`pkgs.tree-sitter-grammars.tree-sitter-proto` (v0.25.3) are both in
nixpkgs — no PyPI package is needed.  The compiled grammar `.so` path is
injected via the `PROTO_GRAMMAR_SO` environment variable set in
`default.nix`.

**Existing fixtures have comments; reproto output does not:**

All current fixture `.proto` files contain at minimum an SPDX header and
often inline documentation comments.  Reproto emits no comments.  The
`normalize_proto` pipeline — uncomment both sides, then buf-format both —
makes this difference irrelevant: both sides are comment-free before
comparison.

---

## Specification

### 1. New module: `proto_normalize.py`

Create `reproto/src/reproto/tests/proto_normalize.py`.  This module is
test-only and is not part of the installable reproto package.

#### 1a. `uncomment(text: str) -> str`

Strip all `//` and `/* */` comments using tree-sitter.  The tree-sitter
proto grammar is syntax-aware: `//` inside a string literal is part of a
`string_lit` node, never a `comment` node, so there are no false positives.

The grammar `.so` is loaded once at module import time from
`os.environ["PROTO_GRAMMAR_SO"]`.

```python
import os
import re
from tree_sitter import Language, Parser

def _make_parser() -> Parser:
    so = os.environ["PROTO_GRAMMAR_SO"]
    lang = Language(so, "proto")
    p = Parser()
    p.set_language(lang)
    return p

_PARSER = _make_parser()

def uncomment(text: str) -> str:
    """Remove all comment nodes from proto source text."""
    src = text.encode()
    tree = _PARSER.parse(src)
    ranges: list[tuple[int, int]] = []
    stack = [tree.root_node]
    while stack:
        node = stack.pop()
        if node.type == "comment":
            ranges.append((node.start_byte, node.end_byte))
        else:
            stack.extend(node.children)
    out = bytearray(src)
    for start, end in sorted(ranges, reverse=True):
        del out[start:end]
    # Clean up residual blank lines and trailing whitespace
    lines = out.decode().splitlines()
    lines = [line.rstrip() for line in lines]
    result = re.sub(r'\n{3,}', '\n\n', '\n'.join(lines))
    return result.strip() + '\n'
```

#### 1b. `buf_format_batch(texts: dict[str, str]) -> dict[str, str]`

Format multiple proto source strings through `buf format` in a single
subprocess call.  Input is a mapping of `label → text`; output is the same
mapping with each value buf-formatted.

Labels must be valid filename stems (no path separators).  Each text is
written to `<label>.proto` in a shared tmpdir containing a minimal `buf.yaml`.
`buf format --write` is then run on the directory, overwriting files in place.
Results are read back.

```python
import subprocess
import tempfile
from pathlib import Path

def buf_format_batch(texts: dict[str, str]) -> dict[str, str]:
    """Run buf format on multiple proto texts in a single subprocess."""
    with tempfile.TemporaryDirectory() as td:
        root = Path(td)
        (root / "buf.yaml").write_text("version: v2\n")
        paths: dict[str, Path] = {}
        for label, text in texts.items():
            p = root / f"{label}.proto"
            p.write_text(text)
            paths[label] = p
        subprocess.run(
            ["buf", "format", "--write", str(root)],
            check=True,
        )
        return {label: paths[label].read_text() for label in texts}
```

#### 1c. `normalize_proto_batch(texts: dict[str, str]) -> dict[str, str]`

Uncomment all texts (pure Python, no subprocess), then format all in one
buf call.

```python
def normalize_proto_batch(texts: dict[str, str]) -> dict[str, str]:
    """Uncomment then buf-format multiple proto source strings."""
    uncommented = {k: uncomment(v) for k, v in texts.items()}
    return buf_format_batch(uncommented)
```

#### 1d. `normalize_proto(text: str) -> str`

Single-file convenience wrapper.

```python
def normalize_proto(text: str) -> str:
    """Uncomment then buf-format a single proto source string."""
    return normalize_proto_batch({"f": text})["f"]
```

#### 1e. `pb_diff(pb1: bytes, pb2: bytes) -> str`

Decode both `.pb` blobs via `prototext --decode` and return a unified diff.
Returns an empty string if the blobs are equal.  `prototext` is at
`bin/prototext` in the repo; its path is resolved relative to this file.

```python
import difflib

_PROTOTEXT = Path(__file__).parents[5] / "bin" / "prototext"

def pb_diff(pb1: bytes, pb2: bytes) -> str:
    """Decode both .pb blobs via prototext and return a unified diff."""
    if pb1 == pb2:
        return ""

    def decode(data: bytes) -> str:
        result = subprocess.run(
            [str(_PROTOTEXT), "--decode",
             "--type", "google.protobuf.FileDescriptorSet"],
            input=data,
            capture_output=True,
        )
        return result.stdout.decode(errors="replace")

    left  = decode(pb1).splitlines(keepends=True)
    right = decode(pb2).splitlines(keepends=True)
    return "".join(difflib.unified_diff(
        left, right, fromfile="expected", tofile="actual"
    ))
```

Note: `prototext --decode` without `--descriptor` uses the built-in
well-known descriptor for `google.protobuf.FileDescriptorSet`, which is
sufficient here.

### 2. Integration into `test_roundtrip`

Replace step 4 in `test_roundtrip` (the `filecmp.cmp` block and the
`protoc --decode` fallback) with:

```python
# Step 4: Compare descriptor sets
pb1 = orig_pb.read_bytes()
pb2 = new_pb.read_bytes()
assert pb1 == pb2, (
    f"Descriptor sets differ for {fixture_name}:\n"
    + pb_diff(pb1, pb2)
)
```

Add a step 5 after the `.pb` comparison:

```python
# Step 5: Compare .proto text (comments and whitespace stripped)
normalized = normalize_proto_batch({
    "fixture": content,           # original fixture text (already loaded)
    "output":  new_proto.read_text(encoding="utf-8"),
})
assert normalized["fixture"] == normalized["output"], (
    f".proto text differs after normalization for {fixture_name}"
)
```

`content` is already in scope from the `get_fixture_content` call at the
top of the test.

Import `pb_diff` and `normalize_proto_batch` at the top of
`test_roundtrip.py`:

```python
from reproto.tests.proto_normalize import normalize_proto_batch, pb_diff
```

### 3. `conftest.py` — dependency checks

Add `buf` and `PROTO_GRAMMAR_SO` checks to `check_dependencies`:

```python
if not shutil.which("buf"):
    pytest.skip("buf not found", allow_module_level=True)

if not os.environ.get("PROTO_GRAMMAR_SO"):
    pytest.skip(
        "PROTO_GRAMMAR_SO not set (add tree-sitter-proto grammar to env)",
        allow_module_level=True,
    )
```

### 4. `default.nix` changes

Add to the test environment (`reprotoTests`, `pythonLint`, and the dev-shell):

```nix
pythonPkgs.tree-sitter                               # Python bindings
pkgs.tree-sitter-grammars.tree-sitter-proto          # compiled grammar .so
pkgs.buf                                             # buf format
```

Expose the grammar path as an environment variable in `reprotoTests`:

```nix
PROTO_GRAMMAR_SO =
  "${pkgs.tree-sitter-grammars.tree-sitter-proto}/parser";
```

Add to the dev-shell `shellHook`:

```bash
export PROTO_GRAMMAR_SO="${pkgs.tree-sitter-grammars.tree-sitter-proto}/parser"
```

---

## Implementation steps

Work through the following steps in order.  Each step has a stated
verification to run before proceeding.

### Step A — add `proto_normalize.py` with `uncomment` only

Create `reproto/src/reproto/tests/proto_normalize.py` containing only
`_make_parser()`, `_PARSER`, and `uncomment()` (§1a above).

**Verify:**

```
PROTO_GRAMMAR_SO=... python - <<'EOF'
from reproto.tests.proto_normalize import uncomment
src = '''
// header comment
syntax = "proto2"; // inline
/* block */ package foo; // trailing
message M {
  // field comment
  optional string s = 1; // with "// not a comment" in a string default
}
EOF
print(uncomment(src))
EOF
```

Expected: no `//` or `/* */` tokens remain; the `"// not a comment"` string
literal is preserved intact; blank lines are collapsed; no trailing whitespace.

### Step B — add `buf_format_batch` and `normalize_proto_batch`

Add §1b, §1c, §1d to `proto_normalize.py`.

**Verify:**

```
PROTO_GRAMMAR_SO=... python - <<'EOF'
from reproto.tests.proto_normalize import normalize_proto_batch
texts = {
    "a": '// comment\nsyntax = "proto2";\npackage foo;\nmessage  M { optional int32 x=1; }\n',
    "b": 'syntax="proto2";package foo;message M{optional int32 x =1;}',
}
result = normalize_proto_batch(texts)
assert result["a"] == result["b"], f"mismatch:\n{result['a']!r}\n{result['b']!r}"
print("OK:", result["a"])
EOF
```

Expected: both normalize to the same canonical string (identical formatting,
no comments).  This validates that `uncomment` + buf together collapse both
stylistic and comment differences.

### Step C — add `pb_diff`

Add §1e to `proto_normalize.py`.  Verify the `_PROTOTEXT` path resolves
correctly by checking `_PROTOTEXT.exists()` in a Python one-liner inside
the nix-shell:

```
python -c "from reproto.tests.proto_normalize import _PROTOTEXT; print(_PROTOTEXT, _PROTOTEXT.exists())"
```

Expected: prints the absolute path and `True`.

### Step D — add dependency checks to `conftest.py`

Add the `buf` and `PROTO_GRAMMAR_SO` checks (§3).  Run the existing test
suite with the new env var set to confirm it still passes:

```
PROTO_GRAMMAR_SO=... pytest reproto/src/reproto/tests/test_roundtrip.py -v
```

Expected: all existing `test_roundtrip[*]` tests pass (proto_normalize is
not yet imported, so this only tests that the skip logic doesn't fire).

### Step E — integrate into `test_roundtrip`: `.pb` diff first

Replace the `filecmp.cmp` block in `test_roundtrip` with the `pb_diff`-based
assertion (§2, step 4 only — do not add step 5 yet).

**Verify:**

```
PROTO_GRAMMAR_SO=... pytest reproto/src/reproto/tests/test_roundtrip.py -v
```

Expected: all `test_roundtrip[*]` tests pass.  The `.pb` comparison logic
is now using `pb_diff` but all fixtures currently pass, so no diff output
should appear.

To confirm the diff path works, temporarily corrupt one `.pb` output in the
test (or add a short-lived test that asserts two different byte strings
differ and shows output).  Remove any such temporary change before step F.

### Step F — integrate into `test_roundtrip`: `.proto` text comparison

Add step 5 (the `normalize_proto_batch` assertion) to `test_roundtrip` (§2).

**Verify:**

```
PROTO_GRAMMAR_SO=... pytest reproto/src/reproto/tests/test_roundtrip.py -v
```

Expected: all `test_roundtrip[*]` tests pass, including the new `.proto`
text assertion.  If any fixture fails here it means reproto's output for
that fixture is structurally divergent — investigate and fix before
proceeding to spec 0016.

### Step G — update `default.nix`

Add `tree-sitter`, `tree-sitter-grammars.tree-sitter-proto`, and `buf` to
the appropriate `buildInputs` / `nativeBuildInputs` / `propagatedBuildInputs`
in `default.nix` (§4), and expose `PROTO_GRAMMAR_SO`.

**Verify:**

```
nix-build -A reprotoTests 2>&1 | tee /tmp/reproto-tests.log
```

Expected: build succeeds; all `test_roundtrip[*]` tests pass inside the
Nix sandbox.

---

## Test coverage

After all steps are complete:

- All existing `test_roundtrip[*]` tests pass with both the `.pb` and
  `.proto` text assertions.
- `.pb` failure messages include a prototext diff when they occur.
- `normalize_proto` / `normalize_proto_batch` are available for use by
  spec 0016's `test_roundtrip_polyglot`.

---

## Open questions

None.
