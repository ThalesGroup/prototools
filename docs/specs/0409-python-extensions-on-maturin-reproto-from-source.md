<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0409 — the Python extensions on maturin, reproto built from its source

Status: draft
App: prototext-codec, fdp-scan, prototext-graph (pyo3 extensions), reproto
Refs: docs/specs/0405-prototools-in-nixpkgs-one-small-pr-at-a-time.md
      (S9 step 3: one nixpkgs PR with the three libraries and reproto);
      docs/specs/0408-the-textproto-grammar-ships-its-parser.md (reproto's
      grammar binding, a prerequisite); docs/specs/0402-a-nixpkgs-friendly-build.md
      (S6: libpython linked only into stub generators and tests)

## Background

**The three pyo3 extensions** (`prototext-pyo3`, `fdp-scan-pyo3`,
`prototext-graph-pyo3`) declare hatchling as their build backend, but
hatchling does not compile Rust. Our Nix build compiles them in crane's
`workspaceBuild`, then `makePyo3Extension` copies each `.so` into its
package directory and runs hatchling around it, and `nix/pypi.nix`
assembles the wheels by hand. The old nixpkgs draft did the same with a
`cargo build` in `preBuild`: the complexity the review of
NixOS/nixpkgs#525997 objected to. nixpkgs builds pyo3 extensions with
maturin (`rustPlatform.maturinBuildHook`), which compiles the crate and
builds the wheel in one standard step.

Each package already has maturin's "mixed" layout: a Python package
(`prototext_codec_lib/__init__.py`) importing a native module nested in
it, next to its committed stub. Each `pyproject.toml` already names its
module for pyo3-stub-gen (`tool.maturin.module-name`).

**reproto** is built from `reprotoSrcFull`, not its source:
`patch/patch_reproto.sh` copies protobuf's well-known `.proto` files into
`src/resources/google/protobuf/`, compiles them to `.pb` with `protoc`,
and copies the `.pb` into `src/reproto/variants/google-protobuf/`. Both
directories are git-ignored, so a plain build of reproto's source lacks
them. It is the pattern prototext's fixtures had before spec 0405 S1.

## Goals

- **G1.** Each extension's nixpkgs recipe is the standard one:
  `buildPythonPackage` with `pyproject = true`,
  `rustPlatform.maturinBuildHook`, and the workspace's vendored Cargo
  dependencies.
- **G2.** reproto's recipe builds its own source, unmodified: no `protoc`
  step, no generated directories.
- **G3.** Our own build, the wheels, the image and the internal package
  keep working, with one way to build each extension (G1's), not two.
- **G4.** The stubs keep the rule of 2026-10-10: the generator never
  writes a committed file; CI compares and fails loudly.

## Non-goals

- **N1.** Changing any extension's Python API or reproto's behavior.
- **N2.** abi3 wheels. The wheels stay per-interpreter (cp313), as today.
- **N3.** The nixpkgs PR itself (spec 0405 S9 step 3), which follows a
  release carrying this spec.

## Specification

- **S1. maturin.** Each extension's `pyproject.toml` declares
  `build-backend = "maturin"` and the mixed layout:
  `tool.maturin.python-source = "."` and
  `module-name = "<pkg>.<pkg>"` (for example
  `prototext_codec_lib.prototext_codec_lib`), which places the compiled
  module inside the package, where `__init__.py` already imports it from.
  The committed stub is packaged as it is.
- **S2. Stubs stay generated aside.** With the module named `<pkg>.<pkg>`,
  pyo3-stub-gen would write `<pkg>/<pkg>.pyi`, the committed file. Each
  crate's `stub_info()` therefore redirects the generator's output
  (`StubInfo::python_root`, a public field) to a scratch directory, and
  the existing comparison in `workspaceBuild` reads from there. A stale
  committed stub still fails with the diff and the refresh command.
- **S3. Our build uses maturin too.** `makePyo3Extension` becomes
  `buildPythonPackage` with `maturinBuildHook`, as in nixpkgs, and
  `nix/pypi.nix` takes the wheels maturin produces instead of assembling
  them. Whether the extensions still come out of crane's shared
  `workspaceBuild` (for build time) or are built per package (as nixpkgs
  does) is settled at implementation by measuring `ci` both ways.
- **S4. reproto's resources are committed.** `src/resources/google/protobuf/*.proto`,
  the `.pb` compiled from them, and `src/reproto/variants/google-protobuf/**/*.pb`
  are committed, with their `.gitignore` entries removed.
  - **Licensing:** the `.proto` files are protobuf's own, and each says
    so: "Copyright 2008 Google Inc. All rights reserved", followed by the
    BSD-3-Clause terms. The descriptors compiled from them are derived
    from them and carry the same notice. Today they are copied in at
    build time, so the repository never contains them; once committed,
    it redistributes them. So:
    - `LICENSES/BSD-3-Clause.txt` is added (`reuse download
      BSD-3-Clause`);
    - `REUSE.toml` annotates them with Google's copyright and
      BSD-3-Clause, not MIT: Google's files cannot be relicensed.
  - **The same correction for two files already committed:**
    `prototext/fixtures/prebuilt/descriptor.pb` (spec 0405 S1) and
    `prototext-core/fixtures/descriptor.pb` are compiled from the same
    Google files but annotated MIT under the project's author. Their
    `.license` sidecars change to Google's copyright and BSD-3-Clause.
    The WKT scoring graph (`prototext/wkt/prebuilt/*.rkyv`), a structure
    computed from those descriptors rather than a translation of them,
    keeps its annotation.
  - **Freshness:** a check derivation in `ci` regenerates them with the
    pinned `protobuf` (`patch_reproto.sh`, unchanged) and fails with the
    diff and the refresh command when they differ.
  - `reprotoSrcFull` goes; reproto builds from its source.
- **S5. reproto's dependencies:** the grammar binding comes from spec
  0408's `tree-sitter-textproto` package; the extensions from S1.
- **S6. Tests:** the extensions' pytest suites (`fdp-scan-pyo3/tests`,
  `prototext-pyo3/tests`) and reproto's run against the packages built by
  S1–S5, as nixpkgs will run them (`pytestCheckHook`).
- **S7. The internal package** (spec 0405 C2) uses `prototext-codec` and
  reproto from our `default.nix`; their attribute names and Python
  modules are unchanged.

## Alternatives considered

### Keep hatchling, compile with crane

Works for us, but each nixpkgs recipe would need a hand-written Rust build
in `preBuild`, the shape that #525997's review rejected.

### setuptools-rust instead of maturin

Also builds pyo3 extensions, but maturin is what nixpkgs's hook and most
pyo3 projects use, and what the module names already assume.

### Generate reproto's resources at runtime

reproto could compile the well-known types when it first needs them, but
that needs `protoc` on the user's machine, which it does not need today.

## Test plan

1. Each extension builds with maturin, and `python -c "import <pkg>"`
   works on the result.
2. A deliberately stale committed stub fails `ci` with the diff; the
   generator leaves the committed stubs untouched.
3. The wheels in `pypi` install and import (`pip install` in a clean venv).
4. reproto builds from its unmodified source and `reproto-tests` pass.
5. reproto's resource check fails after a deliberate change to a committed
   `.pb`, and passes otherwise.
6. `reuse lint` passes with the Google files, and the two existing
   `descriptor.pb`, annotated as Google's under BSD-3-Clause.
7. `ci`, the image and its smoke test pass; the internal package builds.

## Measured outcome

Filled in at implementation.
