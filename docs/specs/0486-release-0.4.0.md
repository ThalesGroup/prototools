<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0486 — release 0.4.0

Status: draft
App: release
Refs: docs/specs/0412-releases-publish-from-ci.md (the tag, the two
        approved upload jobs, trusted publishing, new crates),
      docs/specs/0405-prototools-in-nixpkgs-one-small-pr-at-a-time.md (S9,
        one nixpkgs package per PR; C3, both remotes),
      nixpkgs/README.md (the release checklist, the staged recipes)

A plan, not a feature: what 0.4.0 publishes, in which order, who does each
step, and what is checked first.

## Background

Since 0.3.6 (2026-10-24), `main` gained new programs (`git-seriate`,
`git-seriate-fdp`, `protoback`), new APIs (`scan_inflating`, filtered and
not), and one behavior change (`protoscan --proto-out` writes `~2`): a
minor version, **0.4.0**.

The registries, read on 2026-11-25:

| where | what is there |
|---|---|
| crates.io | the 10 existing crates at 0.3.5; `prototools-affinity`, `seriation`, `git-seriate`, `git-seriate-fdp` reserved at 0.0.0 |
| PyPI | `prototext-reproto`, `prototext-codec`, `fdp-scan`, `prototext-graph`, `tree-sitter-textproto` at 0.3.5. (`reproto` on PyPI is an unrelated project.) |
| nixpkgs `master` | `prototext`, `protoscan`, `protolens`, `reproto` (and its four Python libraries) at 0.3.6 |

- **nixpkgs, how they got there:** five PRs by the maintainer, all
  reviewed and approved by the same nixpkgs reviewer (#572580, #572904,
  #572911, #572923, #573094); and two bumps by r-ryantm, merged
  (#578425 protoscan 0.3.2 → 0.3.6, #577946 prototext 0.3.3 → 0.3.4).
- **Each Rust package is its own recipe:** its own `version`, source
  hash, `cargoHash` and `passthru.updateScript` (`nix-update-script`,
  v-prefixed tags). They share a release number by our practice, not by
  construction; reproto's four Python libraries do follow reproto by
  construction (`inherit (reproto) src version cargoDeps`).
- **Neither protoback nor git-seriate has a staged recipe** in
  `nixpkgs/`.
- **0.3.6 reached nixpkgs, which builds from the tag, and no registry.**
  The v0.3.6 run of `publish.yaml` (2026-10-24) built its crates and
  wheels; its two upload jobs waited for the maintainer's approval in the
  `crates-io` and `pypi` environments until GitHub expired them, 30 days
  later (2026-11-23), with no step run. crates.io and PyPI both stop at
  0.3.5, uploaded that same day by the v0.3.5 run. 0.3.6 fixed a reproto
  test only: on crates.io, 0.3.5 is the same code and is left as it is;
  on PyPI, `prototext-reproto` 0.3.6 is a real change and is missing. A
  run can only be re-run within the 30 days, and the workflow starts only
  on a pushed tag.

## Goals

- **G1. 0.4.0 published:** the tag on both remotes, the crates on
  crates.io (the four reserved ones included), the wheels on PyPI.
- **G2. Two nixpkgs PRs, one package each** (0405 S9): `protoback: init
  at 0.4.0`, `git-seriate: init at 0.4.0`, conforming to the approved
  PRs' recipes and descriptions.
- **G3. The existing nixpkgs packages at 0.4.0.**
- **G4. PyPI holds 0.3.6,** after the fact and before 0.4.0, as nixpkgs
  ships it. crates.io stays at 0.3.5 for 0.3.6: the same code.

## Non-goals

- **N1. No build changes:** 0485 (the profile argument) comes after.
- **N2. No `.sdl`/`.protodevel` names:** 0484 is tabled.
- **N3. reproto's removal** is its own spec, after the release (S8).

## Specification

- **S1. Before the tag, checked:**
  - the four reserved crates each have their trusted publisher
    (repository douzebis/prototools, workflow `publish.yaml`, environment
    `crates-io`), in the crates.io UI: the public API doesn't show it. If
    one hasn't, it is set now, since the crate exists (0412 S5);
  - `scripts/publishable-crates` lists all 14, in dependency order;
  - the GitHub environments `crates-io` and `pypi` still require the
    maintainer's approval.
- **S1a. 0.3.6 to PyPI, by pushing its tag again:** `v0.3.6` deleted on
  douzebis and the same annotated tag object pushed back (`48082568`, on
  `9f73fb5e`). The push starts `v0.3.6`'s own `publish.yaml`, as on
  2026-10-24: its ref is the tag, which the environments' `v*` policy
  accepts, and its version check passes. The `pypi` job is approved and
  the `crates-io` job rejected, each environment's approval being its
  own. It uploads the five 0.3.6 wheels: `prototext-reproto`, and the four
  libraries at an unchanged 0.3.5 code, which do no harm. Nothing else
  changes: the same tag object and commit, so the same tarball and
  nixpkgs' hash; ThalesGroup's copy, where the workflow doesn't run, is
  left alone. Done 2026-11-25, run 45591346267.
- **S1b. Approvals expire:** a release's upload jobs are approved the
  day they wait, as `nixpkgs/README.md` § Releasing now says: GitHub
  expires an approval wait after 30 days, without notice.
- **S2. The version, everywhere** (`nixpkgs/README.md` § Releasing, step
  1): `Cargo.toml`'s workspace and internal crate versions, every
  `pyproject.toml`, reproto's lower bounds, `cargo update -w` and the two
  separate lock files, the `CHANGELOG.md` section, the staged recipes'
  `version`. Then `ci`, as for any change.
- **S3. The tag and the uploads** (0412): `v0.4.0`, annotated,
  `prototools 0.4.0`, pushed to douzebis then ThalesGroup; `publish.yaml`'s
  artifacts inspected, its two jobs approved by the maintainer.
- **S4. The two new recipes,** staged in `nixpkgs/pkgs/by-name/` and built
  by `nixpkgs-staging` before any PR, in the approved recipes' form:
  `rustPlatform.buildRustPackage (finalAttrs: …)`, `__structuredAttrs`,
  `fetchFromGitHub` with `tag = "v${finalAttrs.version}"`, `-p` build and
  test flags, completions and man pages through `installShellFiles` when
  the build can run the host's binaries, `versionCheckHook`,
  `nix-update-script` with the v-tag regex, `meta` as theirs (homepage,
  changelog at the tag, MIT, maintainer douzebis, `mainProgram`, unix),
  nixfmt, no SPDX header.
  - **protoback:** `-p protoback`; it embeds files from reproto's tree, read
    from the same source tarball. Not on crates.io (`publish = false`),
    which nixpkgs doesn't need.
  - **git-seriate:** one package for the two git subcommands,
    `-p git-seriate -p git-seriate-fdp`, `mainProgram = "git-seriate"`;
    its tests need `git` and `openssh` (`nativeCheckInputs`), or they
    skip.
- **S5. The review comments of the five approved PRs** are read in full and
  their points listed in each new PR's `pr-briefing.md`, beside its
  `pr-description.md` written in the approved descriptions' form; each
  recipe is checked against that list before the PR opens.
- **S6. Building for nixpkgs on this host:** the package alone, from a
  nixpkgs checkout of `master`, `nix-update` for the hashes. Never
  `nixpkgs-review` or any whole-nixpkgs evaluation: it runs this host out
  of memory. The README's checklist says otherwise; it is corrected.
- **S7. The existing packages (G3):** left to r-ryantm, which bumped two of
  them already, through each recipe's `updateScript`; reproto's four
  libraries follow reproto in the same PR. By hand only if the bot hasn't
  opened them within a week, or fails.
- **S8. reproto after the release:** `main` without reproto means no
  release after 0.4.0 has a `reproto/` directory, so r-ryantm's next
  reproto update would fail. The removal spec decides nixpkgs' side (keep
  reproto at 0.4.0, or remove it there by PR), PyPI's
  (`prototext-reproto` ends at 0.4.0), and where protoback's embedded files move.
- **S9. Who does what:** the maintainer creates nothing on registries the
  check of S1 doesn't call for, approves the two upload jobs, opens the
  nixpkgs PRs and asks for review; everything else is prepared and shown
  before it goes out.

## Alternatives considered

- **One nixpkgs PR for both packages.** 0405 S9's practice, and the
  reviewer's, is one package a PR.
- **Bumping the existing packages by hand, now.** Four more PRs for the
  reviewer, where the bot's bumps need none of our time; kept as the
  fallback (S7).
- **A `workflow_dispatch` trigger with a tag input.** It can't work
  without loosening the environments: a dispatched run's ref is the
  branch it is dispatched on, which their `v*`-tags-only policy refuses,
  and dispatching on the tag runs that tag's workflow file, which has no
  such trigger.
- **Uploading 0.3.6 by hand,** with tokens, from a local build at the tag.
  It bypasses the reviewed pipeline and trusted publishing.
- **0485 in this release.** Its pytest bound concerns reproto, about to be
  removed, and its profile change touches what ships on release day.

## Open questions

1. The quick-build change (0485) touches only our `default.nix`; nixpkgs'
   recipes don't read it, so r-ryantm has nothing to do there. To confirm
   once 0485 is settled.

## Test plan

1. S1's checks, recorded.
1a. The run of the re-pushed `v0.3.6`: its version check passes; after
    approval, PyPI holds 0.3.6 and crates.io still 0.3.5.
2. `ci` on the version bump.
3. `nixpkgs-staging` builds protoback and git-seriate from the local tree.
4. In a nixpkgs checkout, each new package builds alone, its tests and
   version check included.

## Measured outcome

Progress, 2026-11-25:

- **S1, checked:** both environments require the maintainer's approval
  and deploy from `v*` tags only; `scripts/publishable-crates` lists the
  14 crates; the maintainer confirmed the four reserved crates' trusted
  publishers.
- **S1a, done:** `v0.3.6` pushed again on douzebis, the same tag object;
  run 45591346267: `crates` and the wheels built, `pypi` approved and
  `crates-io` rejected. PyPI holds the five packages at 0.3.6
  (`prototext-reproto` its one wheel, the four libraries three wheels
  each, with attestations); crates.io still 0.3.5.
- **S4, staged and built:** `nix-build -A nixpkgs-staging-rust.<name>`
  builds one staged application through nixpkgs' `callPackage`.
  - protoback: 74 tests in 11 binaries, none skipped; completions for
    three shells, 2 man pages, version check. Its plugin tests run their
    plugins with `python3` and don't skip without it: `python3` is in its
    `nativeCheckInputs`.
  - git-seriate: 96 tests in 16 binaries, none skipped; both programs'
    completions for three shells, 16 man pages, version check.
- **S5, done:** the reviewer's comments on the five PRs read in full; their
  points open each `pr-briefing-<package>.md`, beside its
  `pr-description-<package>.md`.
