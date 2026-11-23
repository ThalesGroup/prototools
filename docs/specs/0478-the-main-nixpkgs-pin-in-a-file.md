<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0478 — the main nixpkgs pin in a file downstream can read

Status: draft
App: nix
Refs: docs/specs/0423-nothing-uploaded-before-nixpkgs-builds.md (S4, the
        nixos-unstable pin in `nix/unstable.json`)

## Background

`default.nix`'s main nixpkgs pin is the default of its `pkgs` argument:
a `fetchTarball` of nixos-26.05 at `7c8764b7`, written out in the
argument list. Nix exposes no function's default values, so nothing
outside the file can read it.

A downstream project builds this repository on the same machine, with its
own `pkgs`. With two nixpkgs commits, nearly every derivation's hash
differs, and the machine builds everything twice: two compilers, two
Pythons, this workspace twice. It has moved its pin to ours by hand, and
would rather read ours from our source, at the revision it depends on, so
that a bump of that revision moves its pin with it.

The nixos-unstable pin is already in a file, `nix/unstable.json`, which
`default.nix` reads.

## Goals

- **G1.** The main pin is readable from outside, with builtins alone
  (`fetchGit`, `readFile`, `fromJSON`), before any `pkgs` exists.
- **G2.** Nothing changes for any build: the same revision and hash give
  the same `pkgs`, so every derivation keeps its hash.

## Non-goals

- **N1.** Moving the pin. Its revision and hash stay as they are.
- **N2.** An update script for it (like `scripts/update-unstable-pin`):
  later, if wanted.

## Specification

- **S1. The file.** `nix/nixpkgs.json`, in `nix/unstable.json`'s format:

  ```json
  {
    "comment": "nixos-26.05 @ 2026-10-09, the main pin (spec 0478)",
    "rev": "7c8764b7c7b09b34f632464276218ef9090eaa11",
    "sha256": "01ybz131ld1aq6n302jq4wqpjd7d57wz9pxnjw4ly53dy6sn8s1i"
  }
  ```

- **S2. Read by `default.nix`.** The `pkgs` argument's default is
  `import (fetchTarball { url =
  "https://github.com/NixOS/nixpkgs/archive/${pin.rev}.tar.gz"; inherit
  (pin) sha256; }) {}`, `pin` read from `nix/nixpkgs.json`. Nothing else
  in the repository writes the main pin's revision out (a check:
  `grep 7c8764b7` finds only the JSON file and specs).

- **S3. What downstream may rely on.** The path `nix/nixpkgs.json` and
  its keys `rev` (a nixpkgs commit) and `sha256` (the unpacked tarball's
  hash, as `fetchTarball` takes it). They are kept, or a change is said
  in the changelog first. The `comment` is not part of it.

## Test plan

1. `nix-instantiate` of `ci`'s members before and after: the same `.drv`
   paths (G2).
2. The downstream reading: `builtins.fromJSON (builtins.readFile
   "${builtins.fetchGit { url = …; rev = …; }}/nix/nixpkgs.json")`
   gives `rev` and `sha256`.

## Measured outcome

Filled in at implementation.
