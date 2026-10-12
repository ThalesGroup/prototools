<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0411 — reproto and its grammar, complete on PyPI and staged for nixpkgs

Status: implemented
Implemented in: 2026-10-11
App: reproto, tree-sitter-textproto, the three pyo3 extensions
Refs: docs/specs/0405-prototools-in-nixpkgs-one-small-pr-at-a-time.md
      (S9 step 3: one nixpkgs PR with the libraries and reproto; S6–S7,
      the staging harness); docs/specs/0408-the-textproto-grammar-ships-its-parser.md
      (the grammar's Python package; this spec reverses its N2 for PyPI);
      docs/specs/0409-python-extensions-on-maturin-reproto-from-source.md
      (G1, which the staged recipes put to the test)

## Background

**reproto's PyPI wheel cannot work.** `nix/pypi.nix` assembles it by hand:
it zips `src/reproto/` with a METADATA that names no dependency. And
reproto's `pyproject.toml` declares only third-party packages, none of
what it imports from this repository: `prototext_codec_lib`,
`prototext_graph_lib`, `fdp_scan_lib`, and the textproto grammar
(`split_fdps.py`), which is not on PyPI at all. So
`pip install prototext-reproto` installs, and fails at import.

**The declared list is wrong both ways.** It also names `lark`, which
nothing imports, and our Nix build gives reproto
`tree-sitter-language-pack` as a runtime dependency, which only the tests
import.

**The grammar's module is a top-level `textproto`.** A generic name in
site-packages, which another package could share, and not tree-sitter's
convention: every official grammar binding, and every
`python3Packages.tree-sitter-*` grammar in nixpkgs, is the module
`tree_sitter_<lang>` with a `language()` function.

**Nothing has built the nixpkgs recipes yet.** Spec 0409 made each
extension buildable by nixpkgs's `maturinBuildHook` (its G1), but only our
crane build has exercised the pyproject files. PR 3 (spec 0405 S9) needs
five recipes: the three extensions, the grammar, and reproto.

## Goals

- **G1.** `pip install prototext-reproto` gives a working reproto: every
  dependency declared, every one on PyPI.
- **G2.** The grammar is published as a conventional tree-sitter binding.
- **G3.** PR 3's five recipes are staged and built by `nixpkgs-staging`,
  as the existing two are.
- **G4.** No new release step: the grammar's version and publishing follow
  the existing ones.

## Non-goals

- **N1.** abi3 wheels (as spec 0409 N2).
- **N2.** Publishing the grammar elsewhere (npm, crates.io).
- **N3.** Opening PR 3, which follows a release carrying this spec.
- **N4.** License metadata in the other packages' `pyproject.toml`; only
  the grammar, whose compiled parser carries a third party's ISC notice,
  gets it here.

## Specification

- **S1. The grammar's module is `tree_sitter_textproto`.** The package
  directory, its `__init__.py` re-exporting `language()`, the C extension
  `tree_sitter_textproto._binding` (`binding.c`'s module name and init
  function), its stub `_binding.pyi`, and `py.typed`, as tree-sitter's
  own bindings lay them out. reproto imports it under that name.
- **S2. The grammar declares its licenses.** `license = "ISC AND MIT"`
  (the parser, generated from Porter Matteo Haet's ISC grammar, and our
  MIT binding), and a `LICENSE` file beside `pyproject.toml` with both
  texts and both copyright lines, which the wheel carries, as ISC requires
  of copies.
- **S3. reproto declares what it imports.**
  - **Runtime:** click, google-re2, protobuf, pyvis, pyyaml, rapidfuzz,
    rich, `tree-sitter>=0.22` (the version that takes a grammar as a
    capsule), and this repository's four: `prototext-codec`,
    `prototext-graph`, `fdp-scan` and `tree-sitter-textproto`, each
    `>=` the workspace version.
  - `lark` goes. `tree-sitter-language-pack` moves to the tests'
    dependencies, in our Nix build too.
  - The workspace version check (`pyprojectVersion`, `nix/rust.nix`) also
    checks the four lower bounds, so a release cannot leave one behind.
- **S4. Every PyPI wheel is built by its own build backend.**
  - reproto's: setuptools, from its `pyproject.toml` (the reproto
    package's `dist` output). The hand-assembled wheel goes, and with it
    the last of `pypi.nix`'s wheel-zipping code.
  - The grammar's: setuptools, from its package's `dist` output. On
    Linux, setuptools tags it `linux_<arch>`, which PyPI refuses; `pypi`
    checks that its extension needs no glibc symbol newer than 2.34 and
    retags it `manylinux_2_34_<arch>`, the tag the extensions carry (spec
    0409 S3). On darwin, setuptools's tag is already the deployment
    target's (`macosx_14_0_arm64`).
- **S5. Publishing is unchanged.** The grammar's wheels join `pypi`'s
  output, which CI's `assemble` job collects with every wheel, and
  `publish.sh` uploads with `twine` like the others. Its version is the
  workspace's, already checked at evaluation (spec 0405 S5).
- **S6. PR 3's recipes are staged** in one directory,
  `by-name/re/reproto/` (amended 2026-10-12):
  - `package.nix`: `python3Packages.buildPythonApplication`, with its test
    suite;
  - `prototext-codec.nix`, `prototext-graph.nix`, `fdp-scan.nix`: the
    three extensions, with `rustPlatform.maturinBuildHook` and
    `buildAndTestSubdir`, each extension's pytest suite through
    `pytestCheckHook` where it has one;
  - `tree-sitter-textproto.nix`: the grammar, with setuptools.

  The four libraries are **private to reproto**:
  - `package.nix` builds them with `python3Packages.callPackage`, from
    reproto's own `src`, at its `version`, with the vendored Cargo
    dependencies it declares as `passthru.cargoDeps`;
  - they are not `python3Packages` entries.

  **Why:**
  - nixpkgs's merge bot lets a maintainer merge an update only for a
    package in `pkgs/by-name`, which `python-modules` is not. In
    `python3Packages`, every update of the four would need a committer.
  - r-ryantm updates packages one at a time, while reproto's `>=` bounds
    require the five at one version.
  - As one package, an update changes one version and two hashes, and
    `nix-update` makes all three.
  - Nothing else in nixpkgs uses the libraries; they are on PyPI for
    Python users.

  `nixpkgs-staging` calls the recipe with the pinned Python's package set,
  overriding its `src`, `version` and `passthru.cargoDeps` with the local
  tree, which the libraries follow. The Python recipes build from
  `stagingSrc` (the workspace, `reproto/` and the extensions' tests); the
  Rust ones keep `workspaceSrc`, so editing reproto does not rebuild them.
  The staging workflow also runs when a `pyproject.toml` changes.
  `nixpkgs-staging-check` formats every file of the directory and checks
  reproto's version. `nixpkgs/README.md` lists them.

## Alternatives considered

### Keep reproto off PyPI

Point pip users at Nix and stop uploading reproto. Simpler, but the three
extensions stay on PyPI for a tool that is no longer there, and publishing
the grammar costs only three more wheels in an existing pipeline.

### An sdist only for the grammar

pip would compile the parser at install time, so every pip user would
need a C compiler, for a 170 KB parser we already build on three
platforms.

### Keep the module name `textproto`

Publishing a name is permanent on PyPI. The convention costs four renamed
files now; a collision later would cost a breaking change.

### `auditwheel repair` for the grammar's Linux tag

It would also bundle any shared library the extension links, but the
parser links only libc. A symbol check and a retag do the part we need,
with no new tool in the build.

## Test plan

1. The grammar package builds and imports as `tree_sitter_textproto`;
   reproto's tests pass on it.
2. In a clean venv, with nothing but the `pypi` wheels as an index,
   `pip install prototext-reproto` resolves every dependency of this
   repository from them, and `reproto --version` runs.
3. The grammar's Linux wheel is tagged `manylinux_2_34_<arch>`, and the
   symbol check fails when given an extension that needs a newer glibc.
4. A lower bound left at an old version fails evaluation.
5. `nixpkgs-staging` builds all seven recipes; their tests and import
   checks pass. `nixpkgs-staging-check` passes.
6. `ci`, the image and its smoke test pass; `reuse lint` passes.

## Measured outcome

Implemented 2026-10-11.

- **S1–S2:** the grammar's module is `tree_sitter_textproto`, with
  `_binding` as its C extension. Its wheel carries `LICENSE` and declares
  `License-Expression: ISC AND MIT`.
- **S3:** reproto declares twelve runtime dependencies.
  - Test plan 4: `fdp-scan>=0.3.0` fails evaluation, naming the
    dependency and the bound it needs.
- **S4:**
  - `pypi` holds five wheels, each from its own build backend. The
    grammar's Linux wheel is retagged `manylinux_2_34_x86_64`.
  - Test plan 3: the grammar needs no symbol newer than `GLIBC_2.2.5`.
    With the ceiling lowered below that, the check fails. It keeps the
    extensions' tag: reproto needs those, at 2.34, anyway.
  - Test plan 2: in a fresh venv, with `PYTHONPATH` unset (the dev-shell's
    leaks into a venv and hides missing dependencies),
    `pip install --find-links <pypi> prototext-reproto==0.3.1.dev0`
    installs the five wheels and the third-party dependencies from PyPI.
    `reproto --version` runs, every module imports from the venv, and the
    grammar parses with tree-sitter 0.26.0.
  - The pin is needed only because the tree carries a pre-release
    version: a bare `prototext-reproto` resolves to the published 0.2.1.
- **S6:** `nixpkgs-staging` builds the seven recipes.
  - The extensions' suites pass (6 and 10 tests), reproto's too (284).
  - reproto passes nixpkgs's runtime-dependency check on its declared
    list, and its version check.
  - The extensions' recipes are the standard `maturinBuildHook` ones, so
    spec 0409's G1 holds.
- **Tests:**
  - `ci` passes;
  - the image passes 27 of 27 smoke checks. It shrank from 1,235 to
    996 MiB unpacked, since `tree-sitter-language-pack` and `lark` left
    reproto's runtime closure;
  - the dev-shell enters, and parses with the renamed grammar;
  - `reuse lint` passes.
- **S6, amended 2026-10-12:**
  - The libraries moved from `python-modules` into reproto's directory,
    private to it, and gained `__structuredAttrs`.
  - Starting from the recipe at 0.3.2, nix-update 1.16.0 (from nixpkgs
    master) found `v0.3.3` through the update script's version regex. It
    set the version, the source hash and `passthru.cargoDeps`'s hash to
    the values the release produced.
- **CI (the push of 2026-10-11):**
  - the Nix workflow passes on its three runners;
  - its release artifact holds 13 wheels, the grammar's among them as
    `manylinux_2_34_x86_64`, `manylinux_2_34_aarch64` and
    `macosx_14_0_arm64`;
  - `nixpkgs-staging` builds the seven recipes natively on each runner.
