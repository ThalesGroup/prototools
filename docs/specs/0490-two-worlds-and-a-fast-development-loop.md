<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0490 — two worlds, a modest setting, and a fast development loop

Status: implemented
Implemented in: 2026-11-26
App: nix, scripts
Refs: docs/specs/0485-a-cargo-profile-argument-and-bounded-pytest.md
        (`cargoProfile`; its split gate, tests in `quick` under release,
        which this reverses; `scripts/ci`, which `check --release`
        replaces),
      docs/specs/0489-low-parallelism-by-default-for-modest-machines.md
        (the user shell's bound, which the modest setting generalizes),
      prototools' spec 0031 and notes of 2026-11-26 ("products apart from
        checks, and a fast development loop": the names, and points 2 to
        5 adopted; "spec 0490, our answers": `check`'s place, the demos)

## Background

The maintainer's model, 2026-11-26:

- **two worlds,** each a set of store paths: *quick* (`cargoProfile =
  "quick"`: no LTO, 16 codegen units) and *release* (thin LTO); paying
  for both, the store paths doubled, is accepted;
- **orthogonally, a modest setting,** parallelism minimized, for a modest
  machine;
- **the development cycle in the quick world,** for everything, always,
  but before a push: the CI in the release world then, or, optimistic,
  in the quick world.

Where the tree stands (at `5c207a81`):

- **The worlds exist for the products** (`default.nix`'s `cargoProfile`,
  spec 0485), **not for the tests:** `rustTests` builds in `quick`
  whatever the profile (0485's split gate), so the release world's CI
  tests other artifacts than it ships.
- **The dev shell is in the release world:** `dev-shell.nix` imports
  `default.nix` with its defaults; its hook runs `cargo build --release`
  for prototext, protolens and protoscan; `bin/`'s wrappers run
  `target/release/…`; its Python extensions (`prototextCodec`,
  `fdpScanLib`) are Nix's release builds, on `PYTHONPATH`, so `pytest` in
  the shell does not see a Rust edit until Nix rebuilds them.
- **No fast loop:** `scripts/ci --preflight` checks fmt, ruff, REUSE and
  the evaluation, no clippy, no tests; the rest goes through Nix, minutes
  for a one-line edit.
- **The modest bound exists for the user shell alone** (0489:
  `--max-jobs 1 --cores 2`, `PROTOTOOLS_BUILD_FLAGS`).

prototools proposes the same shape (their spec 0031), and asks for the
same command names in both repositories.

## Goals

- **G1. Two worlds, each whole:** a world's products, and its tests, in
  its own profile.
- **G2. The development cycle in the quick world:** the dev shell's tools,
  its Python extensions, its local builds; the loop's checks.
- **G3. A fast loop, `check`,** in the working tree, incremental; `check
  --nix`, the CI in the quick world; `check --release`, the CI in the
  release world, before a push.
- **G4. The modest setting, orthogonal:** one setting every entry point
  obeys, in either world; the user shell modest by default, the dev shell
  and `check` not (the dev cycle fast, every core).
- **G5. The demos in the release world:** some demonstrate the tools'
  performance (the maintainer, 2026-11-26), and what they show is what
  ships; the release world is built before a push anyway (prototools,
  agreeing).

## Non-goals

- **N1. The dependency layer's check pass** (`depsCache`'s
  `doCheck`, spec 0489's discussion): left as is.
- **N2. The GitHub workflow's matrix:** it runs `ci`, the release world,
  as now; `ci-quick` is for the maintainer's optimistic run.
- **N3. prototools' sixth part** (a `deploy` output their corporate CI
  builds): theirs alone.

## Specification

- **S1. Tests in their world:** `rustTests` builds at the world's
  profile; in the release world, with a dependency layer of its own for
  the tests (`depsCacheTests`, release). 0485's split gate goes.
- **S2. `ci-quick`:** an attribute of `default.nix`, `ci` at
  `cargoProfile = "quick"` (`default.nix` imported again with it), so
  that `nix-build -A ci-quick` is the quick world's CI.
- **S3. The dev shell in the quick world:** `dev-shell.nix` imports
  `default.nix` with `cargoProfile = "quick"`, so that its Nix inputs
  (tools, Python extensions) are the quick world's, `ci-quick`'s own; its
  hook's cargo builds use `--profile quick` (`target/quick/`), and the
  in-tree wrappers (`bin/`) run `target/quick/…` there. The demo shells
  stay in the release world (G5): their attributes are `default.nix`'s
  own, at its default.
- **S4. `scripts/check`:** one command, three modes, each stopping at the
  first failure, cheapest first, with `ci`'s options:
  - **`check`:** in the working tree: `cargo fmt --check`, `ruff`,
    `cargo clippy --profile quick --workspace --all-targets -- --deny
    warnings`, `cargo test --profile quick --workspace`, `pyright`,
    `pytest`; and the preflight's checks (REUSE, the evaluation, the
    protobuf-release check). No "touched crates" logic: cargo's
    incremental build decides what to recompile.
  - **`check --nix`:** `ci-quick`, on the tracked files (naming an
    untracked file the sandbox will not see).
  - **`check --release`:** what `scripts/ci` does now, `ci` and its
    steps in the workflow's order; `scripts/ci` goes, the workflow check
    (`ci-workflow-check`) reading `check --release --list-steps`.

  A check added to `ci` is added to `check` in the same change. The dev
  shell defines `check` as a function calling `scripts/check`, so a
  developer types `check`, `check --nix`, `check --release`; the script
  works outside the shell too (another shell, a tool's subprocess), as
  prototools' does.
- **S5. `scripts/deploy-extensions`:** builds the pyo3 extensions
  (`prototext-pyo3`, `fdp-scan-pyo3`, `prototext-graph-pyo3`) with cargo
  in the working tree, `quick` by default, and puts each module into its
  Python package's source directory, which the dev shell puts on
  `PYTHONPATH` before Nix's builds; `check` runs it before `pytest` when
  a Rust source is newer than a deployed module.
- **S6. The modest setting:** `PROTOTOOLS_MODEST=1` bounds every build an
  entry point starts: Nix's at `--max-jobs 1 --cores 2`, cargo's at
  `CARGO_BUILD_JOBS=2`. The user shell is modest by default (0489:
  `PROTOTOOLS_BUILD_FLAGS` still overrides its flags); the dev shell and
  `check` are not, `PROTOTOOLS_MODEST=1` asking for it.
- **S7. `BUILDING.md`**, a new file at the root, says it all in one place:
  the two worlds and which entry point is in which, using the tools
  (`nix-shell`, `result-user-shell`, `nix-build`), the modest machine,
  developing (the dev shell, `target/quick/`, the deployed extensions),
  checking a change (`check`'s modes, when to run each, its rules), and
  importing `default.nix` (`boundHint`). The README's "Building with Nix"
  shrinks to the two commands for using the tools and a link to it;
  `CONTRIBUTING.md`'s pull-request and testing sections point to `check`
  and to it, and lose their stale claims (`nix-build` as the gate,
  clippy's `--all-targets`, the tests in `prototext/tests/`).

## Open questions

- **Q1. The release world's tests, their cost:** compiled with thin LTO,
  the CI's tests take longer; to measure (`check --release` before and
  after), and to say in the measured outcome.

## Alternatives considered

- **The split gate kept** (0485): fewer compilations in the CI, but a
  release world's tests testing other artifacts than it ships; the
  maintainer's model puts each world's tests in it.
- **The dev shell's tools from cargo alone,** none from Nix: the tools on
  `PATH` would be the working tree's, but its Python extensions and its
  other inputs would stay the release world's; the quick world's `ci-quick`
  products, reused by the shell, keep one world.

## Test plan

1. `nix-instantiate -A ci`'s test derivations are at `release`;
   `-A ci-quick`'s at `quick`; the release products' store paths
   unchanged.
2. In the dev shell: `prototext` on `PATH` from `target/quick/`; the
   Python extensions' modules from their source directories after
   `scripts/deploy-extensions`; the demo shells' tools from the release
   world.
3. `check`: a formatting error, a clippy warning, a failing test, each
   stops it at its step; on a clean tree, it passes, in seconds when
   nothing changed.
4. `check --nix` builds `ci-quick`; `check --release`, `ci`, and
   `ci-workflow-check` passes.
5. `PROTOTOOLS_MODEST=1 check` runs cargo at two jobs, `check --nix` and
   `--release` Nix at `--max-jobs 1 --cores 2`; the user shell bounded
   without it.

## Measured outcome

Measured on 2026-11-26 on the maintainer's development VM, loaded by a
concurrent download (load average 7 to 10); Nix at `--max-jobs 2 --cores
6`, cargo at its default jobs.

| run | time | what it built |
|---|---|---|
| `check --preflight` | 100 s, then 9 s for the protobuf-release check unloaded | nothing |
| `check`, first run, `target/quick/` warm from earlier work | 429 s | clippy 2 min, tests 125 s, the suite via text 45 s, the extensions 12 s, pyright 40 s, pytest 1 s |
| entering the dev shell, first time | about 12 min | the quick world's dependency layer and workspace, the extensions, the shell's other inputs |
| `check --nix` (`ci-quick`), after that | 328 s | the quick world's checks and tests |
| `check --release` (`ci`), first run after S1 | 1,434 s, of which `ci` 1,400 s | the release world's checks and tests, with thin LTO |

Q1: the release world's tests are the cost. Each test target is its own
binary, optimized and linked with thin LTO, about a minute apiece on this
VM, where the quick world builds the same targets in seconds; `ci`
(release) took four times `ci-quick`. The decision that follows is the
GitHub workflow's (it builds `ci`, on three runners, on every pull request
and push to main): kept at `ci`, the release world's tests, for now; both
repositories are public, where the hosted runners' minutes are not
capped. Locally, `check --release` is optional: `check --nix` is the usual
check before a push.

Checked: test plan 1 (the release products' store paths unchanged from
the commit before; `ci-quick` the same derivation as `ci` at `quick`; each
world's tests on its own dependency layer), 2 for the dev shell
(`prototext` from `bin/` and `target/quick/`, `check` a function,
`target/python/` first on `PYTHONPATH`, pytest importing the deployed
modules), and 4 (`check --nix`, `check --release` with
`ci-workflow-check`, pass). Not yet: 2's demo shells, 3's failing steps,
5's modest setting.
