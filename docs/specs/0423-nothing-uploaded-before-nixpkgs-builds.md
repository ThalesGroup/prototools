<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0423 — Nothing uploaded before nixpkgs builds

Status: draft
App: reproto (completion), release process, Nix packaging, CI
Refs: docs/specs/0412-releases-publish-from-ci.md (the publish workflow
      and its approvals);
      docs/specs/0405-prototools-in-nixpkgs-one-small-pr-at-a-time.md
      (the staged recipes);
      docs/specs/0411-reproto-and-its-grammar-complete-on-pypi-and-staged-for-nixpkgs.md
      (S6, reproto's recipe and its libraries);
      docs/specs/0421-reproto-keeps-editions-in-the-schema-database.md
      (the format change prototext 0.3.4 cannot read);
      docs/specs/0422-reproto-completes-in-bash-zsh-and-fish.md (the
      completion scripts this fixes)

## Background

v0.3.5 was tagged with `ci` and `nixpkgs-staging` green, and three
problems showed only afterwards:

1. **A new crate.** `prototext-upb` (spec 0415) had never been
   published. Trusted publishing cannot create a crate, so the upload
   stopped at it, after `tree-sitter-textproto` 0.3.5 was already on
   crates.io: a half-published release, finished by hand.
2. **Click 8.4.** reproto's completion classes dropped the script
   file's last newline, to make up for the one Click 8.3's `echo` adds.
   Click 8.4 adds none, so on nixpkgs master
   `test_S2_bash_source_is_completions_sh`, which compares byte for
   byte, failed. Users lose nothing (a script without its last newline
   sources the same); the test and the guess behind it were wrong.
3. **prototext in nixpkgs.** reproto's tests decode the databases it
   writes with the `prototext` nixpkgs provides. On master that is
   0.3.4, on prost-reflect, which panics on the editions database
   reproto 0.3.5 writes (spec 0421).

2 and 3 have one cause: `nixpkgs-staging` builds recipes headed for
nixpkgs-unstable against our stable pin (nixos-26.05, Click 8.3.1), and
gives reproto a prototext built from the same tree.

By then 0.3.5 was on crates.io and PyPI, so the fix needs a new
release anyway. A version that is tagged but never uploaded costs
nothing: 0.3.0 and 0.3.1 were, and the 0.3.2 entry of the CHANGELOG
says so. A version uploaded and then found unpackageable costs a
release.

## Goals

- **G1.** reproto's completion works, and its tests pass, whatever
  Click 8.x prints after a script.
- **G2.** The staged recipes are checked, reproducibly, against a
  nixpkgs close to the one the PRs build against.
- **G3.** Nothing is uploaded to crates.io or PyPI before the nixpkgs
  recipes build and pass their tests at the tag.
- **G4.** A crate crates.io does not have yet stops the release before
  any upload, saying what to do.
- **G5.** A written release procedure.

## Non-goals

- **N1.** Moving or deleting a tag. A tag whose release fails stays,
  unpublished; the next patch version replaces it.
- **N2.** Waiting for the nixpkgs PRs to be merged before uploading: the
  merge waits on a reviewer, and changes nothing that is uploaded.
- **N3.** Workarounds in the nixpkgs recipes for an upstream bug: no
  `patches`, no `disabledTests`. A `fetchpatch` fails to apply at the
  next bump, which r-ryantm cannot fix; a disabled test outlives its
  reason. The fix is a release.
- **N4.** Automating the nixpkgs PRs.
- **N5.** Moving the main pin off nixos-26.05: the demos and the
  workshop image want a stable one.

## Specification

### Completion (G1)

- **S1.** The completion classes of spec 0422 stay, Click's documented
  way to customize completion. Their `source()` returns the script as
  Click formats it, without removing anything.
- **S2.** The tests compare scripts with trailing newlines removed on
  both sides: `bash_source` against `completions.sh`, and `zsh_source`
  and `fish_source` as now.
- **S3.** Our zsh and fish scripts are compared with Click 8.4's
  templates, from which they derive; a fix 8.4 made to code we copied
  is taken, and named in the commit.

### The unstable pin (G2)

- **S4.** A second nixpkgs pin, nixos-unstable, in `nix/unstable.json`
  (`rev` and `sha256`), used only by the targets below. The main pin is
  unchanged (N5).
- **S5.** `nixpkgs-staging-unstable`: the targets of `nixpkgs-staging`,
  with this tree's source as there, built with that pin's `pkgs`.
  reproto's tests get the staged prototext, as the stacked PRs give it.
- **S6.** `nixpkgs-staging-unstable-reproto-alone`: reproto built with
  the pin's own prototext, as a reproto-only PR would be. Its failure is
  not a blocker: it says that prototext's bump must go under reproto's,
  in the same release (stacked PRs).
- **S7.** Triggers, all on `main`, all on GitHub Actions:
  - **Push.** The existing `nixpkgs-staging.yml`, which runs on a push
    to `main` touching the recipes, the manifests or the pin, gains a
    job building S5's and S6's targets against the pin, on Linux x86_64
    (the PRs' own CI covers the other platforms at release time):
    reproducible, so a failure is our change's.
  - **Weekly.** A new workflow, `nixpkgs-unstable.yml`, every Monday at
    06:00 UTC and on `workflow_dispatch`, on Linux x86_64, builds both
    targets against that day's nixos-unstable, not the pin: a failure
    is drift in nixpkgs, seen the week it happens, and the pin does not
    move until it is dealt with. It changes nothing in the repository.
    GitHub mails a failed scheduled run to whoever last changed its
    `cron` line; it disables schedules after 60 days without activity in
    the repository, after a warning.
  - **Release.** The pin is moved by hand (`scripts/update-unstable-pin`,
    which writes `nix/unstable.json` from the current nixos-unstable)
    and pushed, so the push trigger checks the release against it.
  - S6's target is reported, never enforced: its failure is a warning
    annotation on the run's page, not a red step.

### New crates (G4)

- **S8.** `scripts/check-new-crates` queries crates.io for each crate
  in `scripts/publishable-crates`, and fails if one does not exist,
  printing for each what to do when the release is approved: the
  manual first publish from the tag, the trusted-publishing settings
  (owner, repository, `publish.yaml`, `crates-io`), and re-running the
  workflow's failed jobs. The manual publish waits for the approval,
  like every other upload (G3).
- **S9.** The publish workflow's `crates` job runs it first. Both
  upload jobs need `crates`, so a new crate stops the release before
  any upload; re-running the failed jobs, once the crate exists, goes
  on to the approvals.

### The gate after the tag (G3)

- **S10.** The publish workflow is otherwise unchanged: it builds on the tag,
  then waits for approval of the `crates-io` and `pypi` deployments.
  The approval is the gate.
- **S11.** It is given only when the nixpkgs recipes, at the tag, with
  its real hashes, build and pass their tests against nixpkgs master:
  locally, then in the PRs' own CI.
- **S12.** If they fail, both deployments are rejected. The tag stays;
  the CHANGELOG entry of the next version says that this one was
  tagged and not published, with the reason; the fix goes into the
  next patch version.

### The procedure (G5)

- **S13.** `docs/releasing.md`, in this order:
  1. The unstable pin moved (`scripts/update-unstable-pin`) and pushed.
  2. `main` green: `ci`, `nixpkgs-staging` with its unstable job.
  3. `-reproto-alone`'s result decides whether the nixpkgs PRs are
     stacked.
  4. `scripts/check-new-crates`, to know whether step 9 will include a
     manual first publish.
  5. The release commit (versions, CHANGELOG heading, staged recipes'
     versions), pushed; CI green.
  6. The tag. The publish workflow builds, then waits.
  7. The staged recipes' hashes at the tag.
  8. The nixpkgs PRs updated, built and tested locally against master,
     pushed; their CI green.
  9. The approvals (a new crate published by hand first, S8), or S12.
  10. The hashes commit and the next `-dev` cycle on `main`.

## Alternatives considered

### Print the scripts without Click

`run()` could answer `…_source` itself, before Click starts. It removes
Click from printing, at the price of intercepting Click's own variable
by hand, which every later maintainer must discover; the bug was a
guess about Click's output, which S1 removes inside Click's documented
mechanism.

### Check every push against unpinned nixpkgs

Closest to the PRs, but not reproducible: a change in nixpkgs would
turn an unrelated commit red. The push check uses the pin, moved at
each release; only the weekly run, which judges nixpkgs rather than a
commit, goes unpinned. S11 checks against master itself, at the moment
it matters.

### A bot that moves the pin

The weekly run could open a PR moving the pin, or push it. A PR needs
the repository to let workflows create them; a push is a bot writing
to `main`. Moving the pin is one command at release time.

### Workarounds in the recipes

N3.

### Release candidates

A `v*` tag publishes, and the nixpkgs recipes take a release tag; an rc
would reach crates.io and PyPI as a pre-release, which is what the gate
avoids.

## Test plan

1. reproto's tests, S2, with Click 8.3.1 (our pin) and 8.4.2 (the
   unstable pin).
2. `nixpkgs-staging-unstable` passes on a current pin.
   `-reproto-alone` fails on
   `test_S1_prototext_decodes_delimited_and_packed` while the pin's
   prototext is older than 0.3.5, and passes once it is not.
3. `scripts/check-new-crates` passes now; with a made-up crate added to
   the list, it fails and prints the instructions.
4. `nixpkgs-unstable.yml`, run by hand, builds both targets against
   that day's nixos-unstable; `scripts/update-unstable-pin` moves the pin,
   and the push runs `nixpkgs-staging.yml` with it.
5. v0.3.6 is released by `docs/releasing.md`, the gate included.

## Measured outcome

Implemented 2026-10-24, for v0.3.6.

- **S1–S2:** the scripts are printed as is; the bash test ignores
  trailing newlines. With Click 8.3.1 the script now ends in one blank
  line more, which bash ignores.
- **S3:** Click 8.4.2's three templates are identical to 8.3.1's, from
  which ours derive: nothing to take. 8.4.2's other completion changes
  are the `echo` without newline (S1) and the escaping of multi-line
  help in fish's responses, which our fish script passes on unchanged.
- **S4–S6:** `nix/unstable.json` pins nixos-unstable 02c087f3 (Click
  8.4.2, prototext 0.3.3). `nixpkgs-staging` and `nixpkgs-staging-unstable`
  are one function of a nixpkgs (`mkStaging` in `default.nix`).
- **Plan 1, 2:** `nixpkgs-staging-unstable` builds, reproto's 293
  tests passing with Click 8.4.2; `-reproto-alone` fails on
  `test_S1_prototext_decodes_delimited_and_packed`, prototext 0.3.3
  panicking on the editions database: 0.3.6's nixpkgs PRs are stacked.
- **Plan 3:** `scripts/check-new-crates` passes on the ten crates, and
  fails with its instructions when a made-up crate is listed. The
  manual first publish moved from before the tag to the approval,
  which the first draft had wrong: before the tag, it would upload a
  crate ahead of the gate (G3).
- **Plan 4:** on 15c9c69, `nixpkgs-unstable.yml`, run by hand, and the
  new job of `nixpkgs-staging.yml` both passed: 293 reproto tests on
  today's nixos-unstable and on the pin; `-reproto-alone` failed on the
  editions test in both, as expected. That failure showed only in the
  step's log under `continue-on-error`; it is now a warning annotation.
- **Plan 5:** v0.3.6 is the procedure's first release.
