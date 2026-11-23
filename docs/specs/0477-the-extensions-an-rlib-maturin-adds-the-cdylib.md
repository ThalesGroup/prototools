<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0477 — the Python extensions an rlib only, maturin adds the cdylib

Status: draft
App: prototext-pyo3 | fdp-scan-pyo3 | prototext-graph-pyo3 | nix
Refs: docs/specs/0409-python-extensions-on-maturin-reproto-from-source.md
        (S3, the wheels maturin builds),
      docs/specs/0423-nothing-uploaded-before-nixpkgs-builds.md (S4, S5,
        both pins)

## Background

`workspaceBuild` (`nix/rust.nix`) compiles the workspace once, then runs
maturin once per extension to build its wheel. The first maturin run
recompiles every dependency, about 130 crates and 50 s; the next two
recompile only their own crate. Two causes, both verified:

1. **Crate type.** The release profile uses thin LTO. maturin builds each
   extension as a `cdylib` alone, so Cargo compiles the shared
   dependencies as bitcode only. The build phase builds the same
   extensions as `["cdylib", "rlib"]`, because each stub generator
   (`[[bin]] …_post_build`) links the `rlib`. That forces machine code
   too. The two builds' dependencies differ, so maturin's are compiled
   again. Seen 2026-11-23 with `CARGO_TERM_VERBOSE=true`: maturin's
   `memchr` carries `-C linker-plugin-lto` and another
   `-C extra-filename`.
2. **An environment variable.** maturin sets
   `PYO3_ENVIRONMENT_SIGNATURE` (`cpython-3.13-64bit`) when it runs Cargo;
   the build phase doesn't. pyo3-build-config's build script declares
   that it depends on it, so Cargo reruns it and recompiles pyo3's family
   and everything above. Seen 2026-11-23 in a downstream build, with
   Cargo's fingerprint log: `EnvVarChanged { name:
   "PYO3_ENVIRONMENT_SIGNATURE", old_value: None, … }`, every other dirty
   unit a knock-on of it.

A downstream project has already tried cause 1's fix: 25 crates
recompiled became 8, and 242 s became 177 s on a loaded host. The 8 are
cause 2.

## Goals

- **G1.** maturin compiles only the extension crate itself for each
  wheel; no dependency is compiled twice.
- **G2.** The wheels, the nixpkgs recipes' builds and what they install
  are unchanged.

## Non-goals

- **N1.** Zero crates. The build phase compiles each extension as an
  `rlib` for its stub generator; maturin must compile it once more as the
  `cdylib` module and do the LTO link. Avoiding that would mean taking
  the stub generators out of the build phase.
- **N2.** The release profile, LTO included.
- **N3.** The nixpkgs recipes' files (`nixpkgs/pkgs/…`): they don't
  change.

## Specification

- **S1. Crate type.** Each extension's `[lib]` is `crate-type =
  ["rlib"]` (`prototext-pyo3`, `fdp-scan-pyo3`, `prototext-graph-pyo3`).
  maturin adds `cdylib` itself when it builds the wheel (maturin 1.12.6
  logs "Setting crate_type to cdylib").

- **S2. Bindings declared.** Each extension's `pyproject.toml` declares
  `[tool.maturin] bindings = "pyo3"`. Without a declared `cdylib`,
  maturin otherwise takes the crate's only binary (the stub generator)
  as the project and ships a wheel with no module. The downstream trial
  hit this; its import check caught it.

- **S3. The signature.** `workspaceBuild`'s environment sets
  `PYO3_ENVIRONMENT_SIGNATURE` to maturin's value for the target Python:
  `<implementation>-<major>.<minor>-<pointer width>bit`, derived from
  the Python and the host platform (not written out by hand), so that it
  follows a Python bump. It is set for the build phase and maturin alike.

- **S4. Where the module came from a plain `cargo build`.**
  `grpconf2026/artifacts.md` copies `target/release/libprototext_graph_lib.so`
  from `cargo build`, which no longer writes one. It uses `cargo rustc
  --release -p prototext-graph-pyo3 --lib --crate-type cdylib` instead.
  Nothing else in the tree loads a module from `target/` (checked:
  `nix/`, `scripts/`, the shells, the tests).

## Landing

On a branch of its own, merged only once all of these pass:

1. `nix-build -A nixpkgs-staging` (maturin 1.12.6) and
   `nix-build -A nixpkgs-staging-unstable` (maturin 1.15.0): the recipes,
   built by nixpkgs' `maturinBuildHook` from the changed files, each
   wheel imported (`pythonImportsCheck`) and tested.
2. The full `ci`, the wheels' import checks and tests among them.
3. The count in `workspaceBuild`, read from Cargo's fingerprint log:
   maturin compiles only each extension's crate.

## Test plan

1. The three landing builds above.
2. Each wheel holds its module: `unzip -l` lists
   `<libName>/<libName>.*.so`, and `import <libName>` works (the
   recipes' and `workspaceBuild`'s import checks).
3. The time `workspaceBuild` takes, before and after, on a quiet host
   (asked for first).

## Measured outcome

Filled in at implementation.
