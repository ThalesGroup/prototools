<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0479 — nix: a local build is the build of its commit

Status: draft
App: nix
Refs: docs/specs/0478-the-main-nixpkgs-pin-in-a-file.md (the downstream
        that shares the machine and the pin)

## Background

The Nix sources are `lib.fileset` filesets (`workspaceSrc`, `depsSrc`,
`reprotoSrc`, the harness's, …). A fileset takes what is on disk, tracked
by git or not. A checkout holds files git ignores and no commit has:

- `ruff.toml`, `rust-toolchain.toml`: written by the dev-shell's hooks
  (`nix/shells.nix`, `_hook_python`, `_hook_rust`);
- `reproto/src/reproto/tests/fixtures/*.pb`: written by tests;
- `prototext-graph/Cargo.lock`: left by a Cargo run in that crate.

So a build from a checkout isn't the build of its commit. A downstream
project found it (2026-11-23): `prototools-workspace` from our checkout,
clean at `d29d318a`, is `c0c0mb1n….drv`; the same commit fetched with
`builtins.fetchGit` is `4dvs1gfc…`. These five files are the only
difference. Two of them can change the result, not only the hash: Cargo
reads `rust-toolchain.toml` and a crate's `Cargo.lock`. And where both
projects build, every crate of ours is compiled twice.

## Goals

- **G1.** A build from a git checkout evaluates to the same derivations
  as the build of its commit fetched with `builtins.fetchGit`, whatever
  untracked files lie around.
- **G2.** A source without `.git` (a fetched source, a tarball) builds
  as today.

## Non-goals

- **N1.** The nixpkgs recipes: they fetch their sources, which hold only
  tracked files already.
- **N2.** Stopping the dev-shell or the tests from writing those files:
  harmless once they no longer enter a source.

## Specification

- **S1. One helper.** `default.nix` defines, once, a function that takes
  a fileset and returns it intersected with `lib.fileset.gitTracked
  ./.` when `./.git` exists, unchanged otherwise. `gitTracked` counts
  staged files too, so a new file builds once it is `git add`ed.

- **S2. Every source of the repository** built with `lib.fileset.toSource`
  over this repository's files goes through it: in `default.nix`,
  `nix/rust.nix`, `nix/python.nix`, and the demos' (`nix/*-demo.nix`).
  A source made otherwise (`builtins.path`) is listed and either moved to
  a fileset or justified.

- **S3. What it costs.** `gitTracked` relies on `builtins.fetchGit`,
  which copies the tracked tree into the store at each evaluation before
  filtering: 22 MB today (1,634 files). Shallow clones work (nixpkgs
  passes `shallow = true`), so GitHub's default checkout does too.

- **S4. A consequence for working on the repository.** A new file is not
  in any build until `git add` (staging is enough). A build that fails
  for a missing file it just gained says so through the missing file.

## Test plan

1. G1: `nix-instantiate -A workspace-build` from the checkout, with the
   five files present, against the same commit fetched with
   `builtins.fetchGit`: the same `.drv` path. The same for `ci`.
2. G2: the same attribute evaluated from a copy without `.git`
   (`git archive`): it builds as before.
3. A git worktree (`.git` is a file there) evaluates.
4. An untracked new file under a crate: not in `workspaceSrc`; staged,
   it is.
5. The full `ci`.

## Measured outcome

Filled in at implementation.
