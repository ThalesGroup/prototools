<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0412 — releases publish from CI

Status: implemented, except S5 (the maintainer's setup) and test plan 5 (at v0.3.2)
Implemented in: 2026-10-11
App: the release process (crates.io, PyPI)
Refs: docs/specs/0098-crates-io-publishing.md (S6: the `assemble` job and `publish.sh` this
      replaces); docs/specs/0405-prototools-in-nixpkgs-one-small-pr-at-a-time.md
      (S5: one workspace version; C3: the release order);
      docs/specs/0411-reproto-and-its-grammar-complete-on-pypi-and-staged-for-nixpkgs.md
      (the five PyPI packages)

## Background

**A release is published by hand.** On every push to main, CI's
`assemble` job (`.github/workflows/nix.yml`) collects the wheels and
`.crate` files into a `release` artifact with a generated `publish.sh`.
The maintainer downloads it and runs it with tokens stored on their
machine: `cargo publish` for seven crates, then `twine upload`. At
v0.3.1 that failed: no crates.io token was stored. Neither v0.3.0 nor
v0.3.1 reached a registry; both still serve 0.2.1.

**yb and base91 publish from CI.** A pushed `v*` tag starts a workflow that
builds and checks every package. Each upload job then runs in a GitHub
environment that requires the maintainer's approval. It authenticates with
trusted publishing (OIDC): no token is stored, anywhere.

## Goals

- **G1.** Pushing a release tag builds everything that is published. The
  maintainer approves the uploads, and CI performs them.
- **G2.** No registry token is stored: not in GitHub, not on the
  maintainer's machine.
- **G3.** An upload that fails halfway finishes when its job is re-run.
- **G4.** A tag that does not match the workspace version, or names a
  development version, publishes nothing.

## Non-goals

- **N1.** Publishing v0.3.1. Its tag predates the workflow, and a tag runs
  the workflow file as the tagged commit has it. The first release CI
  publishes is v0.3.2.
- **N2.** Source distributions on PyPI. The wheels cover the three
  platforms we build; nixpkgs builds from the GitHub tag.
- **N3.** Publishing protolens or the pyo3 crates on crates.io (not
  published today either).

## Specification

- **S1. `.github/workflows/publish.yaml`**, on a pushed `v*` tag, in
  douzebis/prototools only:
  - `check-version`: the tag is `v` followed by
    `workspace.package.version`, which has no `-dev` suffix.
  - `crates`: `scripts/check-crates` packages the seven crates with
    `cargo package --locked`, which compiles each `.crate` on its own, as
    crates.io will receive it. The `.crate` files are an artifact.
  - `wheels`, on the three runners: `nix-build -A pypi`, the five wheels
    each, as artifacts.
  - `publish-crates` (needs `crates`), in the `crates-io` environment:
    `rust-lang/crates-io-auth-action` exchanges the job's OIDC token for a
    short-lived crates.io token, and `scripts/publish-crates` publishes
    the seven crates in dependency order, skipping one already published
    at this version (`--no-verify`: `crates` verified them).
  - `publish-pypi` (needs `wheels` and `crates`: a release whose crates
    fail is published nowhere), in the `pypi` environment:
    `pypa/gh-action-pypi-publish` with `skip-existing`.
  - Third-party actions are pinned by commit, as in yb and base91.
- **S2. The release shell**: `nix-shell default.nix -A release-shell`,
  with cargo, rustc and curl, runs the two scripts in CI and locally.
- **S3. The crate list**, in dependency order, is
  `scripts/publishable-crates`, which both scripts source.
  `nix/crates-io.nix`, the packaging check on main, keeps its own copy,
  with a comment pointing at it.
- **S4. `nix.yml` stops assembling releases**: the `assemble` job and
  `publish.sh` go. The `pypi` and `crates-io` builds stay on main, as
  checks, without artifacts.
- **S5. Repository and registry setup**, done once by the maintainer and
  listed in `nixpkgs/README.md`:
  - **GitHub environments** `crates-io` and `pypi`: required reviewer the
    maintainer, deployments from `v*` tags only. They must exist before
    the first `v*` tag is pushed: GitHub creates an environment a job
    names on the fly, with no protection rule, so the upload would run
    unapproved.
  - **crates.io** trusted publisher (repository douzebis/prototools,
    workflow `publish.yaml`, environment `crates-io`) on each crate.
    Trusted publishing applies to crates that exist. Four of the seven do
    not yet (`prototext-schema`, `prototools-complete`, `fdp-scan`,
    `protoscan`), so their first release needs one token, once: run
    `scripts/publish-crates` locally with `CARGO_REGISTRY_TOKEN` set,
    before approving the job, which then skips all seven. Revoke the
    token afterwards, then add the trusted publishers.
  - **PyPI** trusted publisher on the four projects that exist, and a
    pending publisher for `tree-sitter-textproto`, which creates the
    project at its first upload.
- **S6. The release checklist** (`nixpkgs/README.md`) becomes: push the
  tag, inspect the artifacts, approve the two jobs.

## Alternatives considered

### Store tokens as repository secrets

One stored crates.io token and one PyPI token would do, but a leaked
secret publishes anything, until revoked. Trusted publishing ties each
upload to this repository, this workflow and this environment.

### Publish v0.3.1 by hand, then switch

It needs the same one-time token, and it leaves one more release made a
different way. v0.3.2 differs from v0.3.1 by this workflow alone.

### Publish from the `release` artifact of main

The artifact of the push to main is built from the same commit as the
tag. But a tag-triggered workflow is what environments can restrict to
`v*` tags, and what trusted publishing names.

## Test plan

1. `check-version` rejects a tag that differs from the version, and a
   `-dev` version (both checked locally on the script's logic).
2. `scripts/check-crates` packages the seven crates in the release shell.
3. `scripts/publish-crates`, given a version already on crates.io, skips
   every crate (run against 0.2.1, with no token, no upload attempted).
4. `actionlint`, if available, passes on `publish.yaml`.
5. At v0.3.2: the workflow builds, waits for approval, and publishes; the
   seven crates and 13 wheels are on crates.io and PyPI.

## Measured outcome

Implemented 2026-10-11, up to the first release.

- **S1:** `.github/workflows/publish.yaml`, with yb's and base91's job
  structure and pinned actions (plus `magic-nix-cache`, pinned to its v13
  commit). It runs in douzebis/prototools only, since the ThalesGroup
  mirror receives the same tags.
- **S2–S3:**
  - `release-shell` in `default.nix`: cargo, rustc, curl.
  - `scripts/publishable-crates` holds the crate list; `check-crates`
    and `publish-crates` source it.
  - Every crate depends on `workspace-hack = "0.1"`, cargo-hakari's
    crate, which is never published. On crates.io it resolves to hakari's
    empty stub, `workspace-hack` 0.1.0, so packaging and publishing work.
- **S4:** `nix.yml` keeps the `pypi` and `crates-io` builds on main, with
  no artifacts. The `assemble` job and `publish.sh` are gone.
- **Test plan:**
  1. The version check passes `v0.3.1` at 0.3.1. It fails `v0.3.2`, and
     a `0.3.2-dev` version.
  2. `scripts/check-crates --allow-dirty` packages and verifies the seven
     crates in the release shell.
  3. With the version forced to 0.2.1 and no token anywhere,
     `publish-crates` skips `prototext-core` and `prototext-graph` (on
     crates.io at 0.2.1). Then `cargo publish` of `prototext-schema`,
     which has no 0.2.1, stops: no token. Nothing was uploaded.
  4. `actionlint` passes on both workflows, `shellcheck` on both
     scripts.
- **Found on the way:**
  - crates.io and PyPI still serve 0.2.1: v0.3.0 and v0.3.1 were tagged,
    never uploaded.
  - Four crates are new to crates.io (S5).
