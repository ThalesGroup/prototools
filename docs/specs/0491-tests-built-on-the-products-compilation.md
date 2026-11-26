<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0491 — the tests built on the products' compilation

Status: implemented
Implemented in: 2026-11-26
App: nix
Refs: docs/specs/0490-two-worlds-and-a-fast-development-loop.md
        (each world's tests in it; the release tests' cost, its Q1),
      docs/specs/0477-the-extensions-an-rlib-maturin-adds-the-cdylib.md (the same flags for
        a reused compilation, and a guard that fails when it is redone),
      docs/specs/0374-a-workshop-image-for-every-laptop.md S2 (no installed target in a product's
        closure),
      prototools' note of 2026-11-26, "tests built on the products'
        compilation" (the idea)

## Background

Spec 0490 put each world's tests in that world. Its measurement: `ci` in
the release world takes 1,400 s against `ci-quick`'s 328 s, the release
tests being the difference.

`rustTests` and `workspaceBuild` both start from `depsCache`, in sandboxes
of their own, so every workspace crate is compiled twice in `ci`: once
for the products, and once again by `cargo test`, with the same source,
profile and flags. `cargo test` compiles each library twice by nature:

1. **normally,** for its dependents, the binaries and the integration
   tests: exactly what the products' build compiled;
2. **with `cfg(test)` and the test harness,** for its unit tests: test-only.

Part 1 can be reused. Checked in the working tree on 2026-11-26: after
touching `prototext-core/src/lib.rs`, `cargo build --profile quick
--no-default-features --workspace` recompiled 18 units, and the `cargo
test --no-run` that followed recompiled none of them. It compiled only
the 14 examples and the 48 `cfg(test)` units and test binaries.

## Goals

- **G1.** `ci` compiles each workspace crate's non-test code once: the
  tests reuse the products' compilation.
- **G2.** A drift that makes the tests compile that code again fails `ci`,
  naming what was compiled again.
- **G3.** Less CPU for `ci` in both worlds, without a longer wall time.

## Non-goals

- **N1.** Running unit tests against the shipped library: the `cfg(test)`
  compilation is a different library by construction.
- **N2.** Fewer thin-LTO links: each test binary is still linked, with the
  world's LTO.
- **N3.** `descriptorAccessCheck`, clippy, and the dev loop (`check`), which
  shares one `target/` already.

## Specification

- **S1. `workspaceArtifacts`:** the workspace's `cargo build` (the flags of
  the former `workspaceBuild`: `workspaceArgs`, `protocArgs`, the
  profile), from `depsCache`, with its target installed (crane's
  incremental `target.tar.zst`, `.prev` to `depsCache`).
- **S2. `workspaceBuild` and `rustTests`** both start from it.
  `workspaceBuild` keeps `doInstallCargoArtifacts = false` and finds its
  `cargo build` fresh; nothing copies from `workspaceArtifacts`, so no
  product's closure holds the installed target (0374 S2; the closure check
  in `ci` verifies it). The tests no longer wait for the wheels.
- **S3. The guards,** one function (`reuseGuard`, `nix/rust.nix`): cargo's
  JSON messages read as they come, and the first unit compiled where it
  should have been found fresh stops cargo, named, with the command that
  shows why. Stopping at the first keeps a drift's failure to seconds,
  where reading the report at the end cost a whole recompilation first.
  - **`rustTests`** first runs `cargo test --no-run`: it may compile only
    test code, `profile.test` units and examples (which `cargo build`
    never builds); every other unit, the dependencies' included, comes
    fresh from the layers below.
  - **`workspaceBuild`'s** `cargo build` must compile nothing at all.
    (prototools' wheel guards maturin's phase only, and noted that a
    redone build phase would pass unseen: this closes it here.)

## Alternatives considered

- **`workspaceBuild` installing its target, the tests on it:** the
  simplest change, but the installed target, with `.prev` to the layers
  below, would enter the closure of every product copied from
  `workspaceBuild` (0374 S2); and the tests would wait for the stubs and
  the wheels.
- **The split gate (tests in `quick` under release, 0485):** cheaper still,
  but the release world's tests would not test the release world; 0490
  rejected it.

## Test plan

1. Evaluation: `rust-tests` depends on `workspaceArtifacts`, not on
   `depsCache` directly; `ci`, `ci-quick` and `prototools` evaluate.
2. `check --nix` (`ci-quick`): passes, the guard included, and the
   closure check.
3. The guards: a flag given to one side alone (`RUSTFLAGS = "--cfg
   reuse_drift"`, in `rustTests`, then in `workspaceBuild`) fails each.
4. The measurement, in the release world, on a warm `depsCache`, the
   machine otherwise idle, with the maintainer's go-ahead:
   - **CPU:** each Rust derivation of `ci` built one at a time
     (`--max-jobs 1 --cores 6`, `nix-build --check` for those already
     built). Before: `workspaceBuild` plus `rustTests`. After:
     `workspaceArtifacts`, `workspaceBuild` and `rustTests`. Each
     build's user and system CPU time comes from its cgroup or from the
     daemon's log.
   - **Wall time:** `nix-build -A ci --max-jobs 2 --cores 6` from a warm
     `depsCache`, before and after.

## Measured outcome

Quick world, 2026-11-26, the maintainer's VM (12 CPUs) otherwise idle:
`nix-build -A ci-quick --max-jobs 2 --cores 6`, the workspace's
derivations salted to rebuild from the same warm `depsCache`, the rest
cached, before (the commit before) and after, alternated:

| run | before | after |
|---|---|---|
| 1 | 381 s | 349 s |
| 2 | 382 s | 346 s |

Wall time 8% shorter, not merely even: before, the two compilations of
the workspace competed for the same 12 CPUs. Both guards pass in `ci` and
`ci-quick`: the tests compile only test code and examples, and
`workspaceBuild` compiles nothing, so one compilation of the workspace's
non-test code is gone from every run.

Test plan 3, quick world: the drift in `rustTests`, then in
`workspaceBuild`, fails each guard after 6 s, at the first dependency
recompiled (`cfg_if`).
