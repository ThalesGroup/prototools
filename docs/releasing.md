<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Releasing prototools

One version for every crate and Python package (spec 0405), released from
`main` by a `vX.Y.Z` tag. The tag starts `.github/workflows/publish.yaml`,
which builds everything and then waits for approval before uploading to
crates.io and PyPI (spec 0412).

**The rule** (spec 0423): nothing is uploaded before the nixpkgs recipes
build and pass their tests at the tag. A tag that fails that check is
never published; the next patch version replaces it. Tags are never moved
or deleted.

## Before the tag

1. **Move the nixos-unstable pin**, and push:

   ```bash
   scripts/update-unstable-pin
   git commit -m "nix: the nixos-unstable pin at <date>" nix/unstable.json
   ```

   `nixpkgs-staging.yml` then builds the staged recipes against it. If the
   weekly `nixpkgs-unstable.yml` run is red, deal with that first.

2. **`main` green**: `Nix` (`ci`) and `nixpkgs staging`, both jobs.

3. **Stacked or not.** The `nixpkgs staging` run reports whether reproto
   builds with nixos-unstable's own prototext ("Build reproto with the
   pin's own prototext"). If it fails, reproto's tests need this release's
   prototext: the nixpkgs PRs will be stacked, prototext's bump under
   reproto's.

4. **New crates.** `scripts/check-new-crates`. A crate crates.io does not
   have yet will stop the publish workflow before any upload; it is
   published by hand at step 9.

## The release

5. **The release commit**, `release: vX.Y.Z`, as the previous ones:
   - `workspace.package.version` and the internal crates' requirements
     in `Cargo.toml`, with the three lock files (`Cargo.lock`,
     `demo/bobapp/Cargo.lock`, `grehack2026/game/Cargo.lock`);
   - every `pyproject.toml`, and reproto's lower bounds;
   - `CHANGELOG.md`: the `## X.Y.Z — <date>` heading under
     `[Unreleased]`;
   - the staged recipes' versions, in `nixpkgs/pkgs/by-name/`; their
     hashes follow the tag.

   Check the lock files with `cargo metadata --locked` in each of the
   three directories. Push to both remotes; wait for CI.

6. **The tag**, annotated, on the release commit, pushed to both remotes:

   ```bash
   git tag -a vX.Y.Z -m "prototools X.Y.Z"
   git push origin vX.Y.Z && git push thales vX.Y.Z
   ```

   The publish workflow builds the crates and wheels, then waits.

## The gate

7. **The hashes at the tag.** Compute the source and vendored-crates
   hashes (one each, shared by the four staged recipes), put them in the
   staged recipes, and check each fetch against its hash.

8. **The nixpkgs PRs**, in `~/code/nixpkgs`, from current master: the
   recipes at the tag, built and tested locally
   (`nix-build -A <package>`, with `--max-jobs 2 --cores 6`), then pushed;
   stacked if step 3 said so. Their CI must be green.

9. **Approve**, or not:
   - **Green:** for a new crate (step 4), publish it by hand from the tag
     and add its trusted publisher, as `scripts/check-new-crates` prints,
     then re-run the workflow's failed jobs. Approve the `crates-io` and
     `pypi` deployments.
   - **Red:** reject both deployments. The tag stays, unpublished. Fix,
     and release the next patch version; its CHANGELOG entry says this
     one was tagged and not published, and why.

## After

10. **On `main`**: the staged recipes' hashes
    (`nixpkgs: the staged recipes at vX.Y.Z, with their hashes`), then the
    next cycle (`chore: start the X.Y.(Z+1)-dev cycle`): `-dev` in
    `Cargo.toml` and the lock files, `.dev0` in the `pyproject.toml`
    files.
