<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0487 — retire reproto

Status: draft
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
    exists.
- **S3. Generators on protoback.** `wktRkyv`, `googleapisDb` and `customDb`
  run protoback (and `protoback instantiate`); `wktPrebuiltCheck` passing
  unchanged is G3's proof.
- **S4. Products on protoback:** the shells, the prototools bundle and its
  closure check, the tutorial, `demo/`, both talks, the workshop smoke
  test and image (D5), each checked as `ci` checks it today.
- **S5. Remove reproto:** `reproto/`, `bin/reproto*`, its Nix package,
  its `prototext-reproto` wheel, the website pages, the `reproto-alone`
  staging steps, its staged nixpkgs recipe, and what remains in the docs,
  which say "reproto, retired after 0.4.0" where history needs it. The
  four Python libraries stay (D4): their staged nixpkgs recipes stop
  reading reproto's source and version, and take the repository's
  tarball and their own sub-directories.
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
   reproto` outside `docs/` and `CHANGELOG.md` finds only history.

## Measured outcome

Filled in at implementation.
