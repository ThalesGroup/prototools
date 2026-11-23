<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0461 — the protobuf release, exported for downstream builds

Status: implemented
Implemented in: 2026-11-23
App: nix
Refs: docs/specs/0460-one-protobuf-release-for-every-descriptor-copy.md
        (the release, `nix/protobuf.nix`, and the checks it holds the
        copies to)

## Background

A downstream build imports this repository as
`import <source> { inherit pkgs pythonPkgs; }`, passing its own nixpkgs'
Python packages. Its Python protobuf is 7.34.1, so reproto runs on 34
inside that build while protoback, prototext and the descriptor pool are
on 36.2; and its own protoc, built from its nixpkgs' `protobuf`, is 34
too. Three releases where 0460 wants one, and 0460's checks only see
this repository's `ci`.

`nix/protobuf.nix` already returns what that build needs (`release`,
`protoc`, `python`, `pythonPkgs`), but `default.nix` doesn't export it,
and a `pythonPkgs` argument replaces 0460's without a word.

## Goals

- **G1.** `default.nix` exports the release, for a downstream build to
  take rather than re-derive.
- **G2.** A `pythonPkgs` passed in whose Python protobuf is not the
  release's fails evaluation, saying what to pass instead.

## Non-goals

- **N1.** A patched protoc, or protoc's source and recipe apart from the
  derivation. `protoc` is nixpkgs' `protobuf_<major>` derivation:
  `protoc.src` is the release's source and `overrideAttrs` adds a patch,
  as for any nixpkgs package. Exporting the recipe separately would copy
  nixpkgs'.
- **N2.** A flake output. The repository is imported as `default.nix`.

## Specification

- **S1.** `default.nix` exports `protobufRelease`: what `nix/protobuf.nix`
  returns, `{ release, protoc, python, pythonPkgs }`. `protoc` comes from
  this repository's nixos-unstable pin (`nix/unstable.json`), whatever
  `pkgs` the caller passes; `python` and `pythonPkgs` are the caller's
  `pkgs`' Python with protobuf moved to `7.<release>` (0460 S1).

- **S2.** `pythonPkgs`, when not passed, is `protobufRelease.pythonPkgs`:
  `nix/protobuf.nix` and the unstable pin are imported once, not once for
  the argument's default and once for the build.

- **S3.** A `pythonPkgs` passed in must have `protobuf.version ==
  "7.<release>"`, or evaluation fails: `pythonPkgs has Python protobuf
  7.34.1; this build needs 7.36.2 (spec 0460): pass
  (import <this repository> { inherit pkgs; }).protobufRelease.pythonPkgs,
  or leave pythonPkgs out`. The nixpkgs staging (0423) is unaffected: it
  takes nixpkgs' own Python packages, not this argument.

## Alternatives considered

**Exporting without S3.** A downstream build that keeps passing its own
`pythonPkgs` stays on three releases with no sign of it, the case that
prompted this.

**A warning instead of S3's failure.** Nix warnings scroll past in a
build log; reproto on another Python protobuf renders edition 2026 files
as `edition = "UNKNOWN"`, a wrong output, not a style issue.

## Test plan

1. `nix-instantiate --eval -A protobufRelease.release`: `"36.2"`;
   `protobufRelease.protoc.version` and
   `protobufRelease.pythonPkgs.protobuf.version`: `"36.2"`, `"7.36.2"`.
2. `import ./default.nix { pkgs = <the main pin>; }`, `pythonPkgs` left
   out: `ci` evaluates to the same derivation as before.
3. `import ./default.nix { inherit pkgs; pythonPkgs =
   pkgs.python313Packages; }`: evaluation fails with S3's message;
   with `pythonPkgs = …protobufRelease.pythonPkgs`: evaluates.

## Measured outcome

2026-11-23, the three items of the test plan:

1. `release`, `protoc.version`, `pythonPkgs.protobuf.version`:
   `"36.2"`, `"36.2"`, `"7.36.2"`.
2. `ci` evaluates to the derivation that passed before this change
   (`iqismmmq…-ci.drv`): nothing it builds changed.
3. With the main pin's `python313Packages` (Python protobuf 7.34.1),
   evaluation fails with S3's message; with
   `protobufRelease.pythonPkgs` it evaluates, to that same `ci`.

S2 is done by reading the argument through `@args`: a `let` binding named
`pythonPkgs` replaces the argument for the rest of `default.nix`, which
needed no other change.
