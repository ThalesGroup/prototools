<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0462 — no other protobuf in our builds, and nixpkgs' default Python

Status: implemented
Implemented in: 2026-11-23
App: nix
Refs: docs/specs/0460-one-protobuf-release-for-every-descriptor-copy.md
        (the release, and the copy checks this extends to the builds),
      docs/specs/0461-the-protobuf-release-exported.md (the export a
        downstream build takes)

## Background

0460 holds every copy of `descriptor.proto` to one release, but not the
protoc a build runs. `nix/rust.nix`'s `protocArgs` still adds
`pkgs.protobuf`, the main pin's 34.1, to the Rust workspace's
derivations: `prototools-workspace`, its dependency cache
(`prototools-deps`), clippy, the tests and the descriptor-access check.
prototext's roundtrip tests run that protoc as their oracle
(`prototext/tests/roundtrip.rs`), so they compare against 34.1 while
everything they test is on 36.2. A downstream build found it in its
closure; nothing here would have.

`nix/protobuf.nix` builds the release's Python on `pkgs.python313`. At
the main pin that is `pkgs.python3` (3.13.15). Named by version, it
won't follow nixpkgs when the pin moves, and a downstream build passing
its own `pkgs` (0461) gets 3.13 even where its default is another. The
Python 3.14 a downstream build saw in its closure is not ours: it is in
the *build* closure of protoc 36.2, from the unstable pin, whose own
build tools (ninja, gtest, asciidoc) use that nixpkgs' Python 3.14.
protoc is substituted from the binary cache, and its runtime closure
(12 paths) holds no Python.

## Goals

- **G1.** Every derivation of ours that takes a protoc takes the
  release's.
- **G2.** `ci` fails when one of ours takes another protobuf, C++ or
  Python.
- **G3.** The release's Python is `pkgs.python3`, whatever that is.

## Non-goals

- **N1.** Other packages' own builds. nixpkgs' `buf` is built against
  its nixpkgs' protobuf (34.1 at the main pin), and protoc 36.2 is built
  with the unstable pin's Python 3.14; both are substituted, and their
  build inputs reach none of our outputs.
- **N2.** The demos and the grehack2026 game (`nix/grehack2026*.nix`,
  `nix/grpconf2026-demo.nix`, `grehack2026/game/default.nix`): not in
  `ci`, and not imported downstream. They move to `protoc` too, for one
  release in the repository, but the check doesn't cover them.

## Specification

- **S1.** `nix/rust.nix` takes `protoc` (0460's) and `protocArgs` adds
  it instead of `pkgs.protobuf`; so do the demos and the game (N2).
  After S1, `pkgs.protobuf` appears nowhere in the repository's Nix
  files (`pythonPkgs.protobuf` is the release's, 0461).

- **S2.** `nix/protobuf.nix` builds on `pkgs.python3`. The nixpkgs
  staging (0423) takes `pkgs.python3Packages`, as nixpkgs builds the
  recipes. At the main pin both are what they were (3.13.15): `ci`
  evaluates to the same derivations but for S1's.

- **S3.** `scripts/protobuf-release-check`, a step of the CI workflow
  (`nix.yml`) before the build: over `ci`'s build closure, read from its
  `.drv` files (`nix-store --query --graph`), each derivation that
  references a protobuf (`protobuf-<v>`, or `python3.<m>-protobuf-<v>`)
  other than the release's must be one of another package, listed with
  the reason (in `scripts/protobuf-release-check.py`):

  | Derivation | Why |
  |---|---|
  | `buf-<v>` | nixpkgs' buf, its own build (N1) |

  Any other fails the check, naming the derivation and the protobuf it
  takes. A listed package that no longer takes one fails it too, so the
  list doesn't outlive its entries.

## Alternatives considered

**The check as a derivation in `ci`.** It would have to read the build
closure, and `exportReferencesGraph` has Nix realise every path of the
closure it exports first, `unsafeDiscardOutputDependency` or not: 3 515
paths to fetch (14.4 GiB) and 134 to build, against one second for
`nix-store --query --graph`. Tried, and dropped.

**Checking runtime closures only.** The protoc a test runs is a build
input, not a runtime dependency: the roundtrip tests' 34.1 is in no
output's runtime closure.

**Matching our derivations by name** (`prototools-*`, `reproto-*`, …)
instead of listing the others. Every new derivation would have to fit
the naming; the list of foreign ones is short and changes when the pin
does.

**Keeping `python313`.** It is the default today, so nothing is gained
by naming it, and the next pin would leave it behind.

## Test plan

1. `scripts/protobuf-release-check` passes after S1; with `protocArgs` back on
   `pkgs.protobuf`, it fails naming `prototools-workspace` and
   `protobuf-34.1`.
2. With `buf` removed from the list, it fails naming `buf`.
3. The release's `python.version` is `pkgs.python3.version`; at the main
   pin, `ci`'s Python derivations are unchanged.
4. `ci`: prototext's roundtrip tests against protoc 36.2.

## Measured outcome

2026-11-23.

1. `scripts/protobuf-release-check` passes (about 12 s, evaluation
   included; the graph query itself under a second). With `protocArgs`
   back on `pkgs.protobuf` it fails, naming the six workspace
   derivations (`prototools-workspace`, `prototools-deps`, the tests'
   dependency cache, the tests, clippy, the descriptor-access check)
   and `protobuf-34.1`.
2. With `buf` removed from its list, it fails naming `buf-1.73.0`.
3. At the main pin `pkgs.python3` and `pkgs.python313` are one
   derivation (3.13.15), and so are their package sets: S2 changes no
   Python derivation.
4. `ci` passes; `rust-tests` references `protobuf-36.2` only.
