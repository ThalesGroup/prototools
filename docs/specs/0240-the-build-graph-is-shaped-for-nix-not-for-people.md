<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0240 — the build graph is shaped for Nix, not for people

Status: draft
App: nix (default.nix, nix/rust.nix, nix/python.nix)
Refs: docs/specs/0239-….md (the change that exposed this: adding one
        leaf-crate dependency forced a schema-graph decision across six
        derivations); memory `crane-recompilation-investigation.md`
        (CLOSED — the `depsCache` stub fix; do not reopen it, this is a
        different problem)

## Background

Placeholder. This spec exists to record that the build wants a redesign,
not yet to say what the redesign is. Fill it in before implementing
anything.

What is known today, and worth not re-deriving:

**Cold builds compile the whole workspace at least four times.**
`rustClippy`, `rustTests`, `prototextBare` and `protolens` each build
`--no-default-features --workspace` off `depsCache`, which is
`buildDepsOnly` and therefore dummy-sourced — so no workspace member's
artifacts survive into any of them. `prototext` (full) and the three
PyO3 extension stages then add scoped rebuilds on top. Nix parallelizes
the four, so the wall-clock cost is less than 4x, but the CPU cost is
not.

**Feature unification makes the workspace one unit.** Spec 0239 added
`prototext` as a dependency of the leaf crate `fdp_scan_lib`. Because
every derivation builds `--workspace`, Cargo unified `prototext/wkt-db`
on for all of them, and each then had to be told where the WKT scoring
graph comes from. One leaf edge, six derivations to reason about. A
`--workspace` build has no notion of "this crate is not part of that
stage".

**Failure is slow to surface.** `rustFmt` is cheap and early, but
`rustClippy` and `rustTests` are full workspace compiles that start from
dummy sources. A type error in one crate is found after the same work
that a successful build would have done.

**Debug builds are not reachable through Nix.** Every derivation is
`cargoWithProfile` / `--release`. The dev-shell convention (memory
`Build conventions`) is `cargo build --release` because `./target/release`
is on PATH. There is no fast-feedback path.

## Goals

- **G1.** Fewer whole-workspace compilations on a cold build.
- **G2.** Fail fast — cheap, broad checks before expensive narrow ones.
- **G3.** A debug/dev target with `cargo`-speed feedback, alongside the
  release path, which keeps today's semantics unchanged.

## Non-goals

- **N1.** Reopening the Crane recompilation investigation. It is closed
  (`59a2012`, stubbed `patchPhase` in `depsCache`) and this is a
  different problem: not spurious external-dep rebuilds, but the number
  of legitimate workspace builds.

## Specification

Not written. Ideas to weigh, none of them decided:

- Per-crate or per-layer `cargoArtifacts` chains instead of six
  independent `--workspace` builds off one dummy-sourced `depsCache`, so
  that later stages inherit real artifacts.
- Scoped builds (`-p` / `--exclude`) so a leaf crate's features do not
  propagate. The comment at `nix/rust.nix` on `prototextBare` warns that
  a scoped invocation recomputes profile `unit_for` hashes and can force
  an external-dep recompile; that warning predates the `depsCache` fix
  and should be re-measured rather than trusted.
- Ordering `rustFmt` → `rustClippy` → `rustTests` as a chain rather than
  three parallel roots, trading some parallelism for early failure.
- A `dev` attribute building `--profile dev`, and whether it can share
  any artifacts with the release chain at all.

## Alternatives considered

Not written.

## Test plan

Not written. Whatever lands must be measured against a recorded cold
`nix-build -A ci` baseline (wall clock and CPU), not argued.

## Measured outcome

Not implemented.
