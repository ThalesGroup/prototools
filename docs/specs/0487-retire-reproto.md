<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0487 — retire reproto

Status: implemented (S1 to S6; S7 waits for the next release)
Implemented in: 2026-11-25
App: reproto, protoback, prototext, protolens, nix, release
Refs: docs/specs/0486-release-0.4.0.md (0.4.0, reproto's last release;
        S8, what this spec decides for nixpkgs and PyPI),
      docs/specs/0485-a-cargo-profile-argument-and-bounded-pytest.md (the
        quick builds, smaller once reproto is gone)

## Background

protoback is reproto's Rust successor, compared with it case by case in
`ci` (the protoback harness, 1 655 cases, two modes). reproto 0.4.0 is
its last release. Removing `reproto/` (251 files) is not a deletion:
an inventory of 2026-11-25 found the rest of the repository depending on
it in five ways.

- **Data protoback embeds, inside `reproto/`:** the WKT variant
  descriptors and `google-protobuf.yaml` (`protoback/src/variant.rs`),
  the profiles (`protoback/src/profile.rs`), and the script that
  generates the descriptors (`reproto/patch/patch_reproto.sh`, run by
  `reprotoWktGenerated`, checked by `reprotoWktCheck`).
- **A Rust crate inside `reproto/`:** `tree-sitter-textproto`, a workspace
  member, published on crates.io, used by protolens; its Nix checks
  (`treeSitterTextproto{Generated,Check,HighlightTest}`) read it there.
- **Generators that run reproto:** `wktRkyv`, which produces
  `prototext/wkt/prebuilt/` (checked by `wktPrebuiltCheck`) and `wktDb`;
  `googleapisDb` and `customDb` (full tests). protoback has the options
  they use (`--schema-db-out`, `--emit-extension-ranges`, `-I`, `-O`,
  `--builtin`, `--emit-scoring-yaml`, `instantiate`).
- **Tests:** reproto's pytest suite run against protoback
  (`reprotoTestsOnProtoback`, about 70 files) is protoback's main
  command-line suite; the harness takes reproto as its reference and reads
  its fixtures from `reproto/src/reproto/tests/fixtures`; reproto's own
  suites, its lint and a syntax canary go with it.
- **Publishing and products:** the `prototext-reproto` and
  `tree-sitter-textproto` Python wheels; the website's reproto pages
  (`deploy.yml`); the shells, the prototools bundle, the tutorial, both
  talks, the workshop smoke test and image, which run reproto; nixpkgs'
  `reproto` and its four Python libraries (`prototext-codec`, `fdp-scan`,
  `prototext-graph`, `tree-sitter-textproto`), which within this
  repository only reproto uses.

## Goals

- **G1. No `reproto/`,** and nothing that runs reproto, in the build, the
  tests or the products.
- **G2. protoback's own tests are its coverage:** its Rust unit and
  integration tests (74 in 11 binaries at 0.4.0). The differential harness
  and reproto's pytest suite go (D2, D3).
- **G3. Generated artifacts unchanged:** `prototext/wkt/prebuilt/` and the
  WKT variant descriptors come out byte-identical from protoback.
- **G4. Each phase passes its own checks, and the branch the full `ci`
  before it merges** (S6): two full runs in all, one after S1 and S2
  together, one after S5, both without the slowest members, which S2
  removes.

## Non-goals

- **N1. No change to protoback's behavior** for this: it already matches
  reproto on the harness.
- **N2. No new tests to replace the harness or the suite.** What they
  covered that protoback's own tests don't is a gap accepted here (D2,
  D3), to be filled case by case if a regression shows it.

## Specification

Phases, in order, each a commit or a few, each with its own checks
(Test plan). The tests go before the generators move (S2 before S3):
deleting them can't break a generator, and every later run is shorter.

- **S1. Move the grammar, change nothing.** `reproto/tree-sitter-textproto`
  to `tree-sitter-textproto/` at the top level, its Rust crate and its
  Python binding together (D4). Every path that reads it follows:
  `Cargo.toml`, its own manifest's path to `workspace-hack`, the Nix
  filesets and checks (`stagingSrc` included), `nix/crates-io.nix`,
  `nix/python.nix`, `REUSE.toml`, its staged nixpkgs recipe.
  - **The data protoback embeds moves in S5, not here.** reproto reads the
    same variant and profile files as its own package data, so moving them
    while reproto exists would break it until then. In S5 they move to
    `protoback/data/` as a rename, with `patch_reproto.sh` and the fixtures
    `customDb` compiles (to `tests/custom/`), in the commit that deletes the
    rest of `reproto/`.
- **S2. The tests that need reproto go** (D2, D3): reproto's test package
  (`reproto/src/reproto/tests/`, its pytest configuration with it); its
  suites in `ci` (`reprotoTests`, `reprotoTestsViaText`,
  `reprotoTestsOnProtoback`) and its syntax canary
  (`unknownFieldsStubCheck`); the differential harness, in `ci` and
  `full-tests` (`protobackHarness`, `protobackDumpHarness`, their `-full`
  versions) and as scripts (`scripts/protoback-harness`, and
  `scripts/protoback-selfcheck`, the harness with protoback on both
  sides, which has no other way to run).
  - **The fixtures `customDb` compiles move now,** to
    `tests/custom/fixtures/`, since their directory goes: `customDb` reads
    them there, and they leave the workspace source, where no Rust code
    read them.
  - **`scripts/bench-protoback` stays,** on a corpus the harness prepared
    before; a new one can no longer be prepared.
  - **pythonLint and pythonRuff stay until S5,** linting reproto while it
    exists. In S5 they move to the Python that remains (the extensions' and
    full tests, protoback's plugin adapter, the scripts), rather than go.
- **S3. Generators on protoback.** `wktRkyv`, `googleapisDb` and `customDb`
  run protoback (and `protoback instantiate`); `wktPrebuiltCheck` passing
  unchanged is G3's proof.
- **S4. Products on protoback:** the shells, the prototools bundle and its
  closure check, the tutorial, `demo/`, both talks, the workshop smoke
  test and image (D5), each checked as `ci` checks it today.
- **S5. Remove reproto:** first the moves, then the deletion.
  - **The moves:** the variant (`google-protobuf.yaml` and its `.pb`) to
    `protoback/data/variants/`, the profiles to `protoback/data/profiles/`,
    and `patch_reproto.sh` to `protoback/data/compile-wkt.sh` with its
    helper; `reprotoWktCheck` becomes `wktVariantCheck`
    (`wkt-variant-check`).
  - **The deletion:** `reproto/`, `bin/reproto*`, its Nix package,
  its `prototext-reproto` wheel, the website pages, the `reproto-alone`
  staging steps, its staged nixpkgs recipe, `scripts/reproto-phase-times`,
  and `tests/harness/` (a test of a `bin/proto-gen` long gone, run by
  nothing). The docs say "reproto, retired after 0.4.0" where history
  needs it. The four Python libraries stay (D4): their staged nixpkgs
  recipes stop reading reproto's source and version, and take the
  repository's tarball and their own sub-directories.
  - **What stays as history:** provenance in protoback's comments
    ("reproto's `variant.py`"), the fixtures' `reproto.test` package
    names, `attic/`, `grpconf2026/artifacts.md`, the 0.4.0 PR notes under
    `nixpkgs/`, and the specs.
- **S6. The merge:** the full `ci` on the branch, the staged nixpkgs
  recipes built, then `main`.
- **S7. After the next release:** nixpkgs and PyPI as D6 decides.

## Decisions

Decided by the maintainer, 2026-11-25:

- **D1. Where things move:** `tree-sitter-textproto/` at the top level for
  the grammar, crate and Python binding (S1); in S5, `protoback/data/` for
  what protoback embeds and generates, and `tests/custom/` for the fixtures
  the full tests compile.
- **D2. The differential harness is dropped** (S2), with nothing in its place.
  Considered: frozen reproto 0.4.0 outputs as goldens (an independent
  reference, but generated data to keep and size to measure), and
  protoback against a pinned release of itself (regressions only).
- **D3. reproto's pytest suite is deleted** (S2), the part run against
  protoback included. Considered: keeping its command-line tests to run
  on protoback alone.
- **D4. The four Python libraries stay,** published as libraries in their
  own right: `prototext-codec`, `fdp-scan`, `prototext-graph`,
  `tree-sitter-textproto`, with their own tests (`prototextCodecTests`,
  `fdpScanTests`, the grammar's highlight test).
- **D5. The talks and the workshop are rewritten for protoback,** and stay
  built and checked in `ci`.
- **D6. nixpkgs and PyPI** (S7): in nixpkgs, `reproto` stays at 0.4.0 until the
  next release, then goes by PR, protoback replacing it, and the four
  libraries' recipes stop inheriting from it; on PyPI, `prototext-reproto`
  0.4.0 is its last release, which the next release's notes say.

## Alternatives considered

- **Deleting `reproto/` in one commit.** The data protoback embeds and the
  grammar crate would go with it: the build breaks until everything is
  moved, and the moves are hidden in a deletion's diff.
- **Keeping reproto as a test-only reference, unpublished.** Its 251
  files, its Python and its suites would stay, the cost this removes.

## Test plan

1. **S1 and S2, one full `ci`:** it touches the workspace layout, which everything
   builds from. Every binary that doesn't link the grammar is identical to
   `main`'s, protolens differing only in the source paths it embeds; the
   grammar's staged Python recipe builds.
2. **S2:** `ci` evaluates, without a member that runs reproto; `rust-tests`
   passes.
3. **S3:** `wkt-prebuilt-check` passes against the committed `prebuilt/`
   unchanged, and the WKT descriptors' check against the committed copies:
   byte-identical outputs, so nothing built from them changes; `googleapisDb`
   and `customDb` build with protoback.
4. **S4:** the tutorial, both talks and the workshop smoke test run with
   protoback and no reproto on the `PATH`; the shells build; the bundle's
   closure check passes.
5. **S5 and S6, the full `ci`,** and the staged nixpkgs recipes; `git grep
   reproto` outside `docs/` and `CHANGELOG.md` finds only history (S5's
   list).

## Measured outcome

2026-11-25, on branch `retire-reproto`:

- **S1 and S2:** the full `ci` passed in about 35 minutes for `ci` itself
  (about an hour before, with the harness and reproto's suites), rebuilt
  from scratch after a garbage collection; the grammar's staged Python
  recipe builds.
- **S3:** `wkt-prebuilt-check` passes with protoback generating:
  `prototext/wkt/prebuilt/` byte-identical; `wkt-db` built by reproto
  (`main`) and by protoback identical file for file, but for its
  setup-hook's own store path. `custom-tests` (25) and `googleapis-tests`
  (88) pass on the databases protoback builds.
- **S4:** the bundle's and the image's closure checks, the three shells,
  and the workshop image's smoke test pass with protoback. The client
  still embeds `descriptor.proto` (spec 0388 S13), which only reproto
  needed: protoback, given the client's descriptors without it, builds the
  same `life.proto` with no warning.
- **S5:** `wkt-variant-check` passes on the moved descriptors. The
  retargeted pyright found one error in a test annotation nothing linted
  before (`prototext-pyo3/tests/test_descriptor_text.py`: warnings are
  `(code, message)` pairs since 2026-11-17), fixed.
  `protobuf-release-check` failed on its allowance for `buf`, which only
  reproto's tests brought into `ci`: removed. The full `ci`, `pypi`,
  `crates-io` and the four staged Python libraries pass.
- **S6:** merged into `main` on 2026-11-25.
- **S7:** open, at the next release.
