<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0485 — a cargo profile argument, pytest bounded by the build's cores, and a local CI that fails fast

Status: draft
App: nix, scripts
Refs: docs/specs/0487-retire-reproto.md (reproto and the harness gone,
        which changed the pytest background, 2026-11-25),
      docs/specs/0401-a-faster-nix-build.md (S1, workspaceBuild; S8, the
        tests' `quick` profile and their own dependency cache),
      docs/specs/0477-the-extensions-an-rlib-maturin-adds-the-cdylib.md
        (S5, the guard on what maturin compiles again),
      docs/specs/0479-a-local-build-is-the-build-of-its-commit.md (the
        sources a build reads)

## Background

A downstream project builds our tools with our Nix expressions, from our
source, at a revision its lock file names. Two things it can't control:

- **The profile.** What ships is built in `release` (`codegen-units = 1`,
  thin LTO): minutes for a one-line change, and LTO's memory on a laptop
  building from a cold store. Its own crates it builds in `quick` for
  development and low-memory builds, with `release` kept for its gate.
- **pytest's parallelism.** Until spec 0487, `reproto/pyproject.toml`
  set `addopts = "-n auto"`: one xdist worker per CPU, inside a Nix build
  too, whatever `--cores` said. With reproto gone, no `addopts` remains,
  and the four Nix pytest invocations (`prototextCodecTests`,
  `fdpScanTests`, `googleapisTests`, `customTests`) pass no `-n`: they run
  serially, pytest-xdist installed and unused. `googleapisTests` takes
  138 s that way (2026-11-25).

And one thing we don't control either, locally: **the order of the CI's
steps, and whether it stops at the first failure.** A local run of the
full CI is a command line typed by hand (or by an agent), each time from
memory of `.github/workflows/nix.yml`. On 2026-11-25 (spec 0487 S5), one
such run built the staged Python libraries first (about 15 minutes),
then failed on `rust-fmt`, a check of a few seconds that the edit had
made necessary (`include_bytes!` paths that now fit on one line), and,
its `set -e` defeated by the pipe into `tee`, went on building `ci` until
stopped by hand: about 15 minutes of builds and the restart lost, for
want of a `cargo fmt --check` before launching.

Read on 2026-11-25:

- **Rust test parallelism is already bounded.** crane's
  `configureCargoCommonVarsHook.sh` exports
  `CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-$NIX_BUILD_CORES}` and
  `RUST_TEST_THREADS=${RUST_TEST_THREADS:-$NIX_BUILD_CORES}`; `rustTests`
  and `descriptorAccessCheck` are crane derivations.
- **The profile is fixed in two derivations:** `depsCache` and
  `workspaceBuild` (`release`, crane's default), maturin's `--release`,
  and the copy-out path `target/release` (`nix/rust.nix`). The tests
  build in `quick` against a cache of their own, `depsCacheTests` (0401
  S8).

## Goals

- **G1. `cargoProfile ? "release"`** in `default.nix`, accepting
  `"quick"`, applied to what builds the tools: `depsCache`,
  `workspaceBuild`, maturin's wheels.
- **G2. The default changes nothing:** with `"release"`, every
  derivation is the one built today, byte for byte.
- **G3. pytest parallel, bounded by `NIX_BUILD_CORES`,** in every Nix
  pytest invocation.
- **G4. One command for a local full CI, cheap checks first, stopping at
  the first failure:** what a run checks, and in which order, is in the
  repository, not in whoever types the command.

## Non-goals

- **N1. No change to the tests' profile:** `quick` already (0401 S8).
- **N2. No other profile** than `release` and `quick`: a third would be
  untested.
- **N3. Nothing published changes:** releases, crates.io, PyPI and
  nixpkgs stay on `release`.

## Specification

- **S1. The argument** threads from `default.nix` to `nix/rust.nix`:
  `CARGO_PROFILE = cargoProfile` on `depsCache` and `workspaceBuild`;
  maturin `--profile ${cargoProfile}` in place of `--release`; the
  copy-out directory `target/${cargoProfile}` (and its cross form).
  Anything else naming `release` for these two is found and follows.
- **S2. One dependency cache under `quick`:** with `cargoProfile =
  "quick"`, the tests' cache and the build's would be the same profile:
  `depsCacheTests` is then `depsCache`, one dependency build fewer.
- **S3. The wheel guard** (0477 S5) under `quick`: maturin's residual is
  measured once under each profile; `maturinResidual` holds what each
  needs, extra entries being harmless.
- **S4. pytest:** each Nix pytest invocation passes
  `-n "$NIX_BUILD_CORES"`. A session fixture then runs once per worker
  (the full tests' instance generation, one `protoback instantiate` call):
  measured, and kept only if the suite is faster.
- **S5. The `ci` target** stays `release`: its end-to-end checks (the
  Python tests on the wheels, completions, closure) run on
  `workspaceBuild`'s artifacts, and are the gate for what ships.
- **S6. `scripts/ci`, the local full CI** (G4):
  - **A preflight first, in a minute or less, with no Nix build of the
    workspace:** `cargo fmt --all -- --check`; `ruff check` on the
    repository's Python; `nixfmt --check` on the staged recipes;
    `reuse lint`; `scripts/protobuf-release-check`; and `nix-instantiate
    -A ci` with import-from-derivation off. Each is the same check a `ci`
    member or a workflow step runs later, run where it costs nothing.
  - **Its tools are the dev shell's:** `scripts/ci` runs inside the
    repository's own nix-shell (`NIXSHELL_REPO` is the repository's root),
    and otherwise stops, saying to enter it. `nixfmt` joins the dev shell,
    which lacks it.
  - **Then the full run, in `nix.yml`'s order** (`rust-fmt`, `ci`,
    `pypi`, `crates-io`), and with `--staging`, the staged nixpkgs
    recipes after them.
  - **Stops at the first failing step,** whose name it prints, and exits
    non-zero: each step checked on its own, not through `set -e` across a
    pipe.
  - **Logs to `/tmp/<repo>-ci-<timestamp>.log`**, printing the path first,
    so the run can be followed with `tail -f`; Nix builds run with
    `--max-jobs 2 --cores 6` unless given otherwise (`--max-jobs`,
    `--cores`).
  - **`scripts/ci --preflight`** runs the preflight alone: what to run
    after an edit, before a commit.
- **S7. The workflow and the script agree, checked:** `nix.yml` keeps its
  steps, its per-step view and its per-runner `ci-target`; `scripts/ci`
  lists the same Nix steps in the same order. A cheap `ci` member compares
  the two lists, the `nix-build`/`nix-instantiate`/`scripts/` commands of
  `nix.yml`'s steps against the script's, and fails, naming the
  difference, when they part. The preflight stays local: GitHub runs
  without the dev shell, and its first steps are the cheap checks
  already.

## Alternatives considered

- **A `quick` variant of each tool, or an overlay.** Every consumer
  would have to know each attribute's name; one argument reaches all of
  them, and the default keeps today's.
- **Bounding `RUST_TEST_THREADS` ourselves.** crane does already.
- **`-n auto` removed from `pyproject.toml`.** It is the right default
  outside Nix, where a developer runs pytest by hand.

## Test plan

1. `nix-build -A ci`: unchanged with the default (G2), compared by
   derivation path.
2. `nix-build -A ci --arg cargoProfile '"quick"'`: passes; the wheel
   guard holds.
3. Once, our suites one test at a time: `RUST_TEST_THREADS=1` and
   `pytest -n 1`, to find any test that needs others beside it.
4. `--cores 1`: pytest runs one worker. `googleapis-tests` timed serial
   and with `--cores 6`.
5. `scripts/ci` on a tree with a formatting error in a Rust file: fails
   in the preflight, naming `cargo fmt`, within a minute and before any
   Nix build; likewise with a ruff error, a nixfmt error, and an
   unevaluable `ci`.
6. `scripts/ci` on a tree where a later step fails (`pypi`, say): stops
   there, exits non-zero, and runs nothing after it.
7. `scripts/ci` outside the dev shell: refuses, naming it. A step added to
   `nix.yml` alone: the S7 check fails, naming it.

## Measured outcome

Filled in at implementation.
