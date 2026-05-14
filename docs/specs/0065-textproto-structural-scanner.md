<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0065 — Textproto structural scanner via tree-sitter grammar

**Status:** implemented
**Implemented in:** 2026-05-14
**App:** reproto

---

## Background

Spec 0064 (multi-FDP input) requires splitting a multi-FDP
`FileDescriptorSet` in textproto format into individual per-FDP text
fragments, preserving the verbatim source text of each fragment so that
Phase 2's pool-aware `text_format.Parse` re-parse receives exactly what
was written — including comments, whitespace, and option values.

The approach chosen is to use the
[`PorterAtGoogle/tree-sitter-textproto`](https://github.com/PorterAtGoogle/tree-sitter-textproto)
grammar (ISC licence), fetched at build time from a pinned commit and
compiled as a plain C Python extension.  tree-sitter gives exact byte
offsets for every AST node, enabling verbatim slicing of the original
source with no reserialization.

---

## Why tree-sitter (not a hand-written scanner)

A hand-written brace-matching scanner (~40 lines) would work for the
common case, but requires manually reimplementing all lexical edge cases:
both `{}`/`<>` delimiter pairs, string escape sequences, comments,
extension field names `[pkg.Type]`, etc.  The textproto grammar is
already specified and implemented in `tree-sitter-textproto`; using it
means:

- No custom lexer to maintain or test for edge cases.
- Byte-precise `start_byte`/`end_byte` on every AST node — verbatim
  slicing is trivial.
- The grammar handles all delimiter forms, escape sequences, and comment
  stripping automatically.

---

## Why not use `tree-sitter-language-pack`

`tree-sitter-language-pack` 0.10.0 (already a project dependency) ships
170 pre-compiled `*.abi3.so` grammars but does **not** include
`textproto`.  Adding it to the pack would require upstreaming to a
third-party project.

Building the grammar locally as a C Python extension is simpler and
follows the same pattern that `tree-sitter-language-pack` uses
internally: each grammar is a `.so` exposing one function that returns a
`PyCapsule` named `"tree_sitter.Language"`.

---

## Source acquisition strategy

`grammar.js` and the supporting tree-sitter headers are **not committed**
to this repo.  They are fetched at Nix build time from the upstream repo
at a pinned commit SHA via `pkgs.fetchzip`.  `tree-sitter generate` is
then run at build time to produce `src/parser.c` from the grammar source.
This means the grammar is auditable and patchable — if a bug is found,
the fix goes into a fork and the `url` in `default.nix` is updated to
point at the fork until the fix is propagated upstream.

### Upstream pin

```
repo:   https://github.com/PorterAtGoogle/tree-sitter-textproto
commit: 568471b80fd8793d37ed01865d8c2208a9fefd1b  (latest, 2024-10-16)
sha256: 056h95fn779p73ik1gyr4n0y0r5w9pk09z25ly6mm42v5jlzq22l
```

### Disaster recovery / patching

A fork of the upstream repo is maintained in the project owner's GitHub
account.  Under normal circumstances the fork is not referenced in
`default.nix`.  Two scenarios where the fork becomes the active source:

- **Upstream disappears**: update `url` to the fork — one-line change,
  same SHA, same `sha256`, identical build output.
- **Bug fix needed**: apply the fix to the fork, update `url` to the
  fork and update the SHA and `sha256` to the patched commit.  Once the
  fix is merged upstream, revert to the upstream URL.

For grammar debugging, clone the fork locally and iterate there.  No
dev-shell grammar-hacking workflow is provided — rebuilding the `.so`
requires re-entering `nix-shell`.

### Dev-shell

The dev-shell depends directly on the `treeSitterTextproto` Nix
derivation (option C1).  The compiled `.so` is available in the Nix
store and exposed via `PYTHONPATH` pointing at the derivation output.
No shellHook build step is needed; the `.so` is never written to the
working tree.

---

## The C extension interface

`tree-sitter-language-pack`'s `get_language()` function works by:

1. Importing `bindings.<lang>` (a `.abi3.so`).
2. Calling `module.language()` which returns a `PyCapsule`.
3. Passing the capsule to `Language(capsule)`.

The capsule name must be `"tree_sitter.Language"` (verified from the
existing `proto.abi3.so`).

The local extension replicates this exactly: a ~25-line C shim
(`binding.c`) that wraps `tree_sitter_textproto()` (the symbol exported
by `src/parser.c`) in a `PyCapsule` and exposes it as `language()` on a
Python module named `textproto`.

---

## Repository layout

```
reproto/
  tree-sitter-textproto/
    binding.c        ← Python C extension shim (~25 lines, MIT)
    textproto.pyi    ← type stub for pyright
```

`grammar.js`, `src/parser.c`, and `src/tree_sitter/parser.h` are never
committed.  `parser.c` is generated at Nix build time from `grammar.js`
via `tree-sitter generate`.

The compiled `.so` is **not** committed and is **not** written to the
working tree.  It is produced by the `treeSitterTextproto` Nix
derivation and lives in the Nix store alongside the `.pyi` stub.  The
dev-shell exposes both via `PYTHONPATH` pointing at the derivation output.

---

## Build

### Nix derivations (`default.nix`)

```nix
treeSitterTextprotoSrc = pkgs.fetchzip {
  url    = "https://github.com/PorterAtGoogle/tree-sitter-textproto/"
           + "archive/568471b80fd8793d37ed01865d8c2208a9fefd1b.tar.gz";
  sha256 = "056h95fn779p73ik1gyr4n0y0r5w9pk09z25ly6mm42v5jlzq22l";
  stripRoot = true;
};

treeSitterTextproto = pkgs.stdenv.mkDerivation {
  name             = "tree-sitter-textproto";
  src              = ./reproto/tree-sitter-textproto;
  buildInputs      = [ pythonBin ];
  nativeBuildInputs = [ pkgs.tree-sitter pkgs.nodejs ];
  buildPhase  = ''
    cp ${treeSitterTextprotoSrc}/grammar.js .
    tree-sitter generate
    gcc -shared -fPIC \
      -o textproto$(python3-config --extension-suffix) \
      binding.c src/parser.c \
      -I src \
      $(python3-config --includes --ldflags)
  '';
  installPhase = ''
    mkdir -p $out
    cp textproto*.so $out/
  '';
};
```

### Dev-shell

The dev-shell lists `treeSitterTextproto` in `buildInputs` and adds
`${treeSitterTextproto}` to `PYTHONPATH`.  No shellHook build step.
The `.so` is never written to the working tree.

---

## Python module: `reproto.split_fdps`

### Purpose

`split_fdps.py` is the unified input pipeline for reproto's disk loader.
It absorbs format detection, proto type detection, `entry { }` wrapper
handling, and FDP splitting — replacing the current `decapsulate()`
function and the multi-FDP guard in `parse_qfile`.

### Interface

```python
FdpFragments = list[tuple[str, str]] | list[tuple[str, bytes]]

def split_fdps(
    contents: str | bytes,
    ext: str,
) -> FdpFragments:
    ...
```

- **`contents`**: raw file contents as read from disk (`str` for text
  extensions, `bytes` for binary extensions).
- **`ext`**: the file extension (e.g. `".textpb"`, `".pb"`), used as a
  tiebreaker when both text and binary parses succeed.
- **Returns**: a list of `(name, fragment)` pairs — one per FDP — where
  `name` is the FDP's `name` field and `fragment` is either a `str`
  (verbatim textproto interior) or `bytes` (serialised `FileDescriptorProto`).
- **Raises** `ValueError` if the input cannot be identified as a valid
  FDS or FDP in any supported format.

The list always contains at least one entry on success.  The caller
(`parse_qfile`) assembles one `QualFile` per entry.

### Format detection

Four encoding formats are recognised, tried in this order:

1. **`#@ prototext: protoc` header** — delegate to
   `_pt_codec.format_as_bytes`; the result is binary and re-enters
   format detection as `bytes`.
2. **Text parse succeeds** (`text_format.Parse` on the raw `str`) —
   candidate for a text FDS/FDP.
3. **Binary parse succeeds** (`ParseFromString` on the raw `bytes`) —
   candidate for a binary FDS/FDP.
4. When both text and binary parses succeed (ambiguous content), use the
   file extension as tiebreaker: text extensions → text path; binary
   extensions → binary path.
5. If neither succeeds, raise `ValueError`.

### Proto type detection

After format detection, determine whether the parsed proto is an FDS or
an FDP:

- **FDS**: presence of a `file` repeated field (or an `entry { file … }`
  wrapper) → extract and split into per-FDP fragments.
- **FDP**: presence of a `name` field at the top level → single-entry
  result.
- Otherwise: raise `ValueError`.

### Text path: structural scanning via tree-sitter

For text-format input, `split_fdps` uses the tree-sitter grammar to
locate FDP boundaries without any protobuf semantic interpretation.

The scanner handles both wrapper forms transparently:

```
entry { file { … } file { … } }   ← wrapper stripped, files split
file { … } file { … }              ← no wrapper, files split
file { … }                         ← single FDP, no splitting
```

`entry { }` decapsulation is performed structurally by the tree-sitter
scanner — the existing `decapsulate()` function in `load.py` is removed.

#### Grammar loading

The `Language` object is initialised once per process and cached in a
module-level variable.

#### Verbatim slicing

tree-sitter provides `node.start_byte` and `node.end_byte` on every AST
node.  For a `message_value` node `{ … }`:

- `src[msg_value.start_byte]` is `{` (or `<`)
- `src[msg_value.end_byte - 1]` is `}` (or `>`)
- `src[msg_value.start_byte + 1 : msg_value.end_byte - 1]` is the
  verbatim interior — no reconstruction, no reserialization.

#### AST navigation

The grammar's top-level `message` node is a sequence of `field` nodes.
Each `field` has:

- child 0: `field_name` (text `"entry"`, `"file"`, or other)
- a `message_value` child containing the body

For an `entry` wrapper, the scanner descends one level to find the inner
`file` fields.  For bare `file` fields, it operates at the top level.

Inside each FDP body, the `name` field value is extracted to produce the
tuple key.

#### Handling of `file: { }` vs `file { }` syntax

The textproto spec allows an optional `:` between field name and message
value.  The grammar normalises both to the same AST shape, so no special
Python handling is needed.

### Binary path

For binary-format input, splitting is trivial:

```python
fds = FileDescriptorSet()
fds.ParseFromString(data)
return [(fdp.name, fdp.SerializeToString()) for fdp in fds.file]
```

For a bare `FileDescriptorProto`, return `[(fdp.name, data)]`.

---

## Files added / changed

| File | Change |
|---|---|
| `reproto/tree-sitter-textproto/binding.c` | New C shim (~25 lines) |
| `reproto/tree-sitter-textproto/textproto.pyi` | Type stub for pyright |
| `reproto/src/reproto/split_fdps.py` | New unified input pipeline module |
| `reproto/src/reproto/tests/test_split_fdps.py` | New pytest tests |
| `default.nix` | Add `treeSitterTextprotoSrc` fetchzip, `treeSitterTextproto` derivation; add `.so` to `PYTHONPATH` via derivation output (C1) |

---

## REUSE / licence

`parser.c` and `parser.h` are fetched from upstream at build time and
never committed, so they need no SPDX headers in this repo.

`binding.c` and `textproto.pyi` are new files and receive the standard
MIT header via `reuse annotate`.

---

## Test plan

Tests live in `reproto/src/reproto/tests/test_split_fdps.py`.

| Case | Expected result |
|---|---|
| Text FDS, two `file { … }` blocks | 2-entry list, interiors verbatim |
| Text FDS, three `file { … }` blocks | 3-entry list |
| Text FDS with `entry { … }` wrapper | wrapper stripped, files split |
| `file: { … }` syntax (colon before brace) | same result as without colon |
| `file < … >` angle delimiters | correctly split |
| Single `file { … }` block | 1-entry list |
| Bare FDP (has `name`, no `file` field) | 1-entry list |
| Comment inside FDP body | comment preserved verbatim in interior |
| Nested message inside FDP body | verbatim interior includes nested block |
| String containing `{` inside FDP | correctly included in interior |
| Binary FDS, two FDPs | 2-entry list of `(name, bytes)` |
| Binary bare FDP | 1-entry list of `(name, bytes)` |
| `#@ prototext: protoc` encoded input | decoded, then treated as binary |
| Ambiguous content (parses as both) | extension used as tiebreaker |
| Unrecognisable input | `ValueError` raised |

---

## Implementation order

1. Add `treeSitterTextprotoSrc` and `treeSitterTextproto` to `default.nix`;
   add `.so` to `PYTHONPATH` via derivation output (C1). ✓ Done
2. Write `binding.c`, `textproto.pyi`; run `reuse annotate`. ✓ Done
3. Write `reproto/src/reproto/split_fdps.py`.
4. Write `reproto/src/reproto/tests/test_split_fdps.py`; run pytest.
