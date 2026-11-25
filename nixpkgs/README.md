# nixpkgs staging

The files under `pkgs/` are prototools' nixpkgs packages, laid out as in the
nixpkgs tree (spec 0405):

| Here | In nixpkgs |
|---|---|
| `pkgs/by-name/pr/prototext/package.nix` | same path (PR 1: NixOS/nixpkgs#572580, merged 2026-10-11 at 0.3.0; NixOS/nixpkgs#573094 updates it to 0.3.3, by hand: the recipe changed, spec 0413) |
| `pkgs/by-name/pr/protoscan/package.nix` | same path (PR 2, spec 0407: NixOS/nixpkgs#572904, merged) |
| `pkgs/by-name/re/reproto/package.nix` | same path: reproto (spec 0411 S6; NixOS/nixpkgs#572923) |
| `pkgs/development/python-modules/{fdp-scan,prototext-codec,prototext-graph,tree-sitter-textproto}/default.nix` | same paths: reproto's four libraries, `python3Packages` entries that take their source, version and vendored crates from reproto, so that one update moves all five (spec 0411 S6, as amended in review; NixOS/nixpkgs#572923) |
| `pkgs/by-name/pr/protolens/package.nix` | same path (spec 0410: NixOS/nixpkgs#572911, merged at 0.3.4) |
| `pkgs/by-name/pr/protoback/package.nix` | same path (spec 0486 S4: `protoback: init at 0.4.0`, to open) |
| `pkgs/by-name/gi/git-seriate/package.nix` | same path, `git-seriate` and `git-seriate-fdp` (spec 0486 S4: `git-seriate: init at 0.4.0`, to open) |

They are what a nixpkgs PR submits, copied verbatim: nixfmt-formatted, with
no SPDX header (their licensing is declared in `REUSE.toml`).

`nix-build -A nixpkgs-staging` builds each staged recipe from the local
tree, through the same `callPackage`, with `src` and the vendored
dependencies taken from here instead of the tag, so its tests and version
check run on today's code. reproto's recipe is called with the pinned
Python's package set, as nixpkgs passes `python3Packages`, extended with
the four staged libraries, which read their source, version and vendored
crates from that staged reproto, as in nixpkgs. `nix-build -A nixpkgs-staging-check` checks the
formatting and the version: at a release the staged `version` equals the
workspace's (`Cargo.toml`, `workspace.package.version`); between releases
the workspace carries the next version with a `-dev` suffix and the staged
one, which nixpkgs ships, is at most the next release. The check is part
of `ci`; the builds, which compile everything again, run in their own
workflow (`.github/workflows/nixpkgs-staging.yml`) when the recipes or the
dependencies change, and at every release tag.

The packages follow one PR at a time, in the order of spec 0405 S9:
prototext, protoscan, protolens, then reproto with its four libraries.

`pr-description-<package>.md` and `pr-briefing-<package>.md` are the notes
for a PR being prepared: its description, and what to be ready to answer in
review (protoback and git-seriate, for 0.4.0). `pr-description.md` and
`pr-briefing.md` are prototext's first PR's, kept as the model.

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
   Approve them the day they wait: GitHub expires an approval wait after
   30 days, without notice, and the run can't be re-run after that. v0.3.6's
   expired so (spec 0486): to run a release's upload again, delete its
   tag on douzebis and push the same tag object back, which starts the
   tag's own workflow; approve only the job that is missing.
4. nixpkgs, in a checkout of `master`:
   - copy the staged file;
   - `nix-update prototext` (sets `hash` and `cargoHash`);
   - build that package alone (`nix-build -A prototext`), its tests and
     version check included, then open the PR (`prototext: init at
     X.Y.Z`, then `prototext: A.B.C -> X.Y.Z`). Not `nixpkgs-review` or
     any evaluation of the whole of nixpkgs on the development host: it
     runs out of memory there (spec 0486 S6).
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
  and tags restricted to the tag pattern `v*`, as a **tag** rule (a
  branch rule `v*` never matches a tag). They must exist before the first
  tag: a job naming a missing environment creates it unprotected.
- **crates.io**, each crate's Settings, Trusted Publishing: GitHub,
  repository `douzebis/prototools`, workflow `publish.yaml`, environment
  `crates-io`. A crate must exist first, and crates.io has no pending
  publisher, so a new crate's first release takes a token restricted to
  its name with the `publish-new` scope. crates.io refuses that token on
  a crate that has a trusted publisher, so the job and a local run
  alternate: approve the job, and when it stops at a new crate, run
  `scripts/publish-crates` in the release shell from the tagged checkout,
  with the token in `CARGO_REGISTRY_TOKEN` (`read -rs`, then `export`).
  When it stops at an existing crate, re-run the job. Each run skips what
  is published. Then revoke the token, and add the new crates' trusted
  publishers (spec 0412 S5).
- **PyPI**, each project's Publishing settings: GitHub, owner
  `douzebis`, repository `prototools`, workflow `publish.yaml`,
  environment `pypi`. A project not yet on PyPI gets a pending publisher
  (your account's Publishing page), which creates it at the first upload.
