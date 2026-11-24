<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0477 — the Python extensions an rlib only, maturin adds the cdylib

Status: implemented
Implemented in: 2026-11-23
App: prototext-pyo3 | fdp-scan-pyo3 | prototext-graph-pyo3 | nix
Refs: docs/specs/0409-python-extensions-on-maturin-reproto-from-source.md
        (S3, the wheels maturin builds),
      docs/specs/0423-nothing-uploaded-before-nixpkgs-builds.md (S4, S5,
        both pins)

## Background

`workspaceBuild` (`nix/rust.nix`) compiles the workspace once, then runs
maturin once per extension to build its wheel. The first maturin run
recompiles every dependency, about 130 crates and 50 s; the next two
recompile only their own crate. Three causes, all verified:

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

3. **A feature unified over the workspace, not in maturin's step.** The
   build phase resolves features over the whole workspace; maturin over
   one extension. protolens and prototools-affinity depend on the `nix`
   crate, which turns on libc's `extra_traits`: libc is `default,
   extra_traits, std` in the build phase, `default, std` for maturin's
   `fdp_scan_lib`. Every crate above libc in its tree is then a new unit
   (about 30). The hakari `workspace-hack` crate exists to unify such
   features, but `.config/hakari.toml` lists no platforms, and without
   one hakari leaves out platform-specific dependencies, libc
   (`cfg(unix)`) among them. Seen 2026-11-23: with causes 1 and 2 fixed,
   the first wheel still compiled 31 crates ("fingerprint error: failed
   to read", new units); their fingerprints show libc's two feature sets,
   every other crate's features and profile identical in both.

Both sides must also see the same `PYO3_PYTHON` string, not merely the
same interpreter: `…/bin/python` and `…/bin/python3` differ to Cargo
(found downstream, where a script gave maturin `-i python3`).
`workspaceBuild` passes maturin `-i` the very path it sets as
`PYO3_PYTHON`, and maturin passes it on unchanged.

A downstream project has already tried cause 1's fix: 25 crates
recompiled became 8, and 242 s became 177 s on a loaded host. The 8 are
cause 2.

## Goals

- **G1.** maturin compiles as little as can be had cleanly: the
  extension crate itself for each wheel, and once, for the first, the
  residual S8 names.
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
  --release -p prototext_graph_lib --lib --crate-type cdylib` instead.
  Nothing else in the tree loads a module from `target/` (checked:
  `nix/`, `scripts/`, the shells, the tests).

- **S5. A guard.** Each of S1, S3 and S7 can silently stop holding: a
  maturin that changes its signature convention, an extension declaring
  a `cdylib` again, a feature `workspace-hack` no longer unifies. So
  `workspaceBuild` fails when maturin's output shows it compiling any
  crate but the extension's own and S8's residual (`maturinResidual`,
  `nix/rust.nix`), listing them, naming this spec, and saying how to see
  why (Cargo's fingerprint log) and where to update the residual when a
  dependency change legitimately moves it. Failing is safe: the builds
  use pinned nixpkgs and a locked crate graph, so what maturin compiles
  changes only with a pin, a dependency or a manifest, which CI builds
  first. The nixpkgs recipes run no build phase before maturin, and need
  no such guard.

- **S6. The dev-shell** exports the same value, so that a local
  `cargo build` and `maturin` share pyo3's crates. The value is defined
  once (`nix/rust.nix`, `pyo3EnvironmentSignature`).

- **S7. The hakari platforms.** `.config/hakari.toml` lists the
  platforms nixpkgs builds: `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`,
  `aarch64-apple-darwin`; `workspace-hack` is regenerated (`cargo hakari
  generate`), so that it unifies libc's features, as the workspace build
  has them. A platform not listed only rebuilds more there; it is never
  wrong. `Cargo.lock` gains only `workspace-hack`'s dependency entries: no
  crate or version is added, so the vendored sources are unchanged.

  Unified in full, `workspace-hack` also brings platform-specific crates
  into each package's tree that it didn't have: on Linux `nix 0.29`,
  `rustix`, `linux-raw-sys` and `cfg_aliases` (prototext, protoscan, the
  three extensions; fewer for protoback and git-seriate, none for
  protolens), on Darwin `rustix` and `errno`. A nixpkgs recipe building
  one of ours compiles them for nothing: seconds per package. Excluding
  them (`[final-excludes]`) would avoid it, but `cargo hakari verify`
  then fails, which `hakari-check` runs in `ci`; full unification was
  chosen over weakening that check.

- **S8. The residual, accepted.** `tree-sitter-highlight` (protolens's)
  declares `crate-type = ["lib", "staticlib"]`, and Cargo builds a
  dependency's staticlib too, which needs its dependencies' machine code.
  So the workspace build compiles regex's tree (regex, memchr,
  aho-corasick, …) as machine code and bitcode; maturin's one extension,
  without tree-sitter-highlight, as bitcode only (`-C linker-plugin-lto`),
  the same features otherwise. Those crates and the crates above them in
  the first extension's tree, 22, are compiled again, once; seen
  2026-11-23 in the rustc commands of both (`cargo build -v`). Not
  removed: patching tree-sitter-highlight means vendoring a modified
  crate, and a separate build of the stub generators only moves the
  compile. About 130 crates became 22.

## Landing

On a branch of its own, merged only once all of these pass:

0. The regenerated `workspace-hack` reviewed (S7); `cargo hakari
   verify`, and `hakari-check` in `ci`.
1. `nix-build -A nixpkgs-staging` (maturin 1.12.6) and
   `nix-build -A nixpkgs-staging-unstable` (maturin 1.15.0): the recipes,
   built by nixpkgs' `maturinBuildHook` from the changed files, each
   wheel imported (`pythonImportsCheck`) and tested; and the recipes'
   `cargoHash`, if `Cargo.lock`'s change moves it.
2. The full `ci`, the wheels' import checks and tests among them.
3. The count in `workspaceBuild`, read from Cargo's fingerprint log:
   maturin compiles only each extension's crate.
4. The guard fires: with `PYO3_ENVIRONMENT_SIGNATURE` left out of a
   trial build, `workspaceBuild` fails with S5's message, listing pyo3's
   crates.

## Test plan

1. The three landing builds above.
2. Each wheel holds its module: `unzip -l` lists
   `<libName>/<libName>.*.so`, and `import <libName>` works (the
   recipes' and `workspaceBuild`'s import checks).
3. The time `workspaceBuild` takes, before and after, on a quiet host
   (asked for first).

## Measured outcome

2026-11-23, counted in `workspaceBuild`'s maturin steps:

| | first wheel | the other two |
|---|---|---|
| before | about 130 crates | their own crate |
| S1, S2, S3 | 31 (libc's features, then LTO) | their own crate |
| S7 too | 22 + its own (S8's residual) | their own crate |

The guard (S5): with the signature right, `workspaceBuild` passes and each
wheel holds its module (`<libName>/<libName>.cpython-313-…so`); with it
wrong (`PYO3_ENVIRONMENT_SIGNATURE=trial-mismatch`), it fails at the
first wheel, naming pyo3-ffi, pyo3 and numpy.

Landing: `cargo hakari verify` passes; `nixpkgs-staging-unstable`
(maturin 1.15.0) passes; the full `ci` passes; `nixpkgs-staging`
(maturin 1.12.6) built the three extensions' recipes, import checks and
tests passing, and failed on three reproto tests that assumed reproto's
own protobuf release, red on `main` already; fixed in 0f66c4cb, after
which it passes with this change (reproto 397 passed, 2 skipped; the
extensions' tests passing).

`workspaceBuild`'s time (test plan 3), 2026-11-24, on a quiet host:
`nix-build --check` of the derivation alone, before (`0f66c4cb`) and
after (`e7d71442`), both fetched with `builtins.fetchGit`, `--max-jobs 2
--cores 6`, alternating which goes first. One run of six, whose build
phase took 24 minutes instead of 3 (the host held it up; its maturin step
was normal), is left out.

| | before (3 runs) | after (2 runs) |
|---|---|---|
| build phase | 175 s (173–178) | 170 s (163–178) |
| maturin, first wheel | 64 s | 33 s |
| maturin, the other two | 15 s | 15 s |
| total | 259 s | 224 s |

About 35 s per build, 13%: the first wheel's maturin step halves. A
first measurement, the day before on a less quiet host, had shown the
build phase 20 s slower after; the quiet runs don't confirm it.
