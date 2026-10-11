# nixpkgs staging

The files under `pkgs/` are prototools' nixpkgs packages, laid out as in the
nixpkgs tree (spec 0405):

| Here | In nixpkgs |
|---|---|
| `pkgs/by-name/pr/prototext/package.nix` | same path |
| `pkgs/by-name/pr/protoscan/package.nix` | same path (PR 2, spec 0407; not yet submitted) |
| `pkgs/development/python-modules/{prototext-codec,prototext-graph,fdp-scan,tree-sitter-textproto}/default.nix` | same paths, plus one `callPackage` line each in `pkgs/top-level/python-packages.nix` (PR 3, spec 0411; not yet submitted) |
| `pkgs/by-name/re/reproto/package.nix` | same path (PR 3) |
| `pkgs/by-name/pr/protolens/package.nix` | same path (PR 4, spec 0410; not yet submitted) |

They are what a nixpkgs PR submits, copied verbatim: nixfmt-formatted, with
no SPDX header (their licensing is declared in `REUSE.toml`).

`nix-build -A nixpkgs-staging` builds each staged recipe from the local
tree, through the same `callPackage`, with `src` and the vendored
dependencies taken from here instead of the tag, so its tests and version
check run on today's code. The Python recipes are called from the pinned
Python's package set, extended with the four staged libraries as
`python-packages.nix` will list them, so reproto's recipe finds them where
nixpkgs will. `nix-build -A nixpkgs-staging-check` checks the
formatting and the version: at a release the staged `version` equals the
workspace's (`Cargo.toml`, `workspace.package.version`); between releases
the workspace carries the next version with a `-dev` suffix and the staged
one, which nixpkgs ships, is at most the next release. The check is part
of `ci`; the builds, which compile everything again, run in their own
workflow (`.github/workflows/nixpkgs-staging.yml`) when the recipes or the
dependencies change, and at every release tag.

The packages follow one PR at a time, in the order of spec 0405 S9:
prototext, protoscan, the Python libraries with reproto, then protolens.

`pr-description.md` and `pr-briefing.md` are the notes for the current PR:
its description, and what to be ready to answer in review.

## Releasing

1. In `Cargo.toml`, set `workspace.package.version` and the `version` of
   each internal crate under `workspace.dependencies` to the release; set
   `version` in each `pyproject.toml`, and reproto's lower bounds on this
   repository's packages (evaluation fails if one disagrees); run
   `cargo update -w`, and `cargo update -p prototext-core` in
   `demo/bobapp` and `grehack2026/game`, which have lock files of their
   own. Add the release's section to `CHANGELOG.md`. Set the same
   `version` in the staged `package.nix` files (the check above requires
   it).
2. Commit, tag `vX.Y.Z` (annotated, `prototools X.Y.Z`), push the commit
   and the tag to douzebis/prototools; then push them to
   ThalesGroup/prototools (spec 0405 C3).
3. The tag starts `.github/workflows/publish.yaml` (spec 0412). Inspect
   its `crates` and `wheels-*` artifacts, then approve its two jobs: they
   upload the seven crates to crates.io and the wheels to PyPI, with
   trusted publishing. A job that fails halfway finishes when re-run.
4. nixpkgs, in a checkout of `master`:
   - copy the staged file;
   - `nix-update prototext` (sets `hash` and `cargoHash`);
   - build it, run `nixpkgs-review`, open the PR
     (`prototext: init at X.Y.Z`, then `prototext: A.B.C -> X.Y.Z`).
5. Copy the updated `package.nix` back here (with its `hash` and
   `cargoHash`), and commit.
6. Start the next cycle: as in step 1, with the next version and a `-dev`
   suffix (`0.3.1-dev`), which every `pyproject.toml` spells the PEP 440
   way (`0.3.1.dev0`); add an `[Unreleased]` section to `CHANGELOG.md`;
   commit.

### Publishing setup (once)

What `publish.yaml` relies on, outside the repository (spec 0412 S5):

- **GitHub environments** `crates-io` and `pypi` (repository settings,
  Environments): required reviewer the maintainer; deployment branches
  and tags restricted to the tag pattern `v*`.
- **crates.io**, each crate's Settings, Trusted Publishing: GitHub,
  repository `douzebis/prototools`, workflow `publish.yaml`, environment
  `crates-io`. A crate must exist first: a crate's first release is
  published with a token, once (`CARGO_REGISTRY_TOKEN=… scripts/publish-crates`
  in the release shell, from the tagged checkout, before approving the
  job, which then skips it); then revoke the token and add the trusted
  publisher.
- **PyPI**, each project's Publishing settings: GitHub, owner
  `douzebis`, repository `prototools`, workflow `publish.yaml`,
  environment `pypi`. A project not yet on PyPI gets a pending publisher
  (your account's Publishing page), which creates it at the first upload.
