<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0414 — protolens on crates.io

Status: implemented, except the first publication (at the next release)
Implemented in: 2026-10-12
App: protolens, tree-sitter-textproto
Refs: docs/specs/0408-the-textproto-grammar-ships-its-parser.md (S5: protolens
      compiles the committed parser; N2, publishing the grammar, which spec
      0411 reversed for PyPI); docs/specs/0411-reproto-and-its-grammar-complete-on-pypi-and-staged-for-nixpkgs.md
      (the grammar's Python package, whose shape this follows);
      docs/specs/0412-releases-publish-from-ci.md (the publishing this joins)

## Background

protolens is the one prototools CLI that no registry carries: prototext
and protoscan are on crates.io, reproto is on PyPI. Spec 0412 left it out
(N3), and two things keep it out:

- **It cannot be packaged.** `build.rs` compiles
  `../reproto/tree-sitter-textproto/src/parser.c`, and `colorize.rs`
  embeds `../reproto/tree-sitter-textproto/highlights.scm`: both outside
  the crate (spec 0408 S5). `cargo package` ships only the crate's own
  directory, so a published protolens would not build.
- **Its name is taken.** `protolens` on crates.io is another project's
  (chunhuitrue/protolens, "TCP stream reassembly, application layer
  protocol parsing", 0.2.3, checked 2026-10-11).

The grammar has the same shape in Python since spec 0411: its own
package, `tree-sitter-textproto` on PyPI, which reproto depends on. That
name is free on crates.io.

## Goals

- **G1.** `cargo install <crate>` installs a working `protolens`.
- **G2.** The grammar is published once per ecosystem and consumed as a
  dependency, never copied: one committed parser, one freshness check
  (spec 0408 S2).
- **G3.** CI publishes it with the rest (spec 0412); no new release step.

## Non-goals

- **N1.** Renaming the program: the binary stays `protolens`, as do the
  nixpkgs package and its draft PR (NixOS/nixpkgs#572911).
- **N2.** Changing the grammar (specs 0116, 0121).

## Specification

- **S1. The grammar becomes a Rust crate too**, `tree-sitter-textproto`,
  in `reproto/tree-sitter-textproto/` beside its Python package, laid out
  as tree-sitter's own grammars are:
  - `Cargo.toml`, a workspace member;
  - `build.rs` compiling `src/parser.c` with `cc`;
  - `bindings/rust/lib.rs` exposing `LANGUAGE` (a
    `tree_sitter_language::LanguageFn`) and `HIGHLIGHTS_QUERY`;
  - its tests: the language loads, and a document parses without
    error.

  The package includes only the generated parser, `highlights.scm`,
  `grammar.js`, the bindings and `LICENSE`; `license = "ISC AND MIT"`, as
  on PyPI.
- **S2. protolens depends on it**, through `LANGUAGE` and
  `HIGHLIGHTS_QUERY`. Its `build.rs`, its `extern "C"` declaration and
  its `cc` build dependency go. Nothing outside protolens's directory
  remains in its build.
- **S3. protolens is published as `prototext-lens`** (see "The crate
  name"), with `[[bin]] name = "protolens"`, so the program keeps
  its name. Within the workspace, the package and the directory keep
  their names; only `[package] name` changes, and with it
  `cargo -p`'s argument.
- **S4. Publishing:**
  - `scripts/publishable-crates` and `nix/crates-io.nix` gain the grammar
    crate, before `prototext-core` (it depends on nothing of ours), and
    protolens last.
  - Both are new crates: their first release takes the one-time token,
    with the CI job and a local run alternating (spec 0412 S5); then
    they get trusted publishers.
- **S5. nixpkgs:** the protolens recipe builds `-p prototext-lens`. Its
  `cargoHash` changes at the next release anyway. The draft PR follows
  before it leaves draft.

## The crate name

**`prototext-lens`**, decided 2026-10-11. It groups protolens with
prototext's crates (`prototext-core`, `-graph`, `-schema`, three of which
it depends on), as a lens on prototext's decoding.

Rejected, both free on crates.io:
- **`prototools-protolens`:** unambiguous about the project, with
  `prototools-complete` as a precedent. But it is long and repetitive, and
  no other tool is published as `prototools-<program>`.
- **`protolens-tui`:** keeps the program's name in front, but says how it
  is used rather than what it is.

## Alternatives considered

### Copy the generated parser into protolens's directory

Packaging works with no new crate, but the 172 KB parser is then
committed twice, and the copy needs a freshness check of its own.

### Ask for the `protolens` name

crates.io transfers a name only from an inactive owner who agrees, and
the other project is maintained. Even then, `cargo install protolens`
would have meant theirs until now.

## Test plan

1. `scripts/check-crates` packages and verifies every crate, the grammar
   and protolens among them, each compiled on its own, as crates.io
   receives it.
2. From the packaged `.crate` files alone, outside the workspace,
   `cargo install` builds protolens, and `protolens --version` runs.
3. The grammar crate's tests pass; protolens's do, unchanged.
4. `ci`, `nixpkgs-staging`, the image and its smoke test pass.

## Measured outcome

Implemented 2026-10-12, up to the first release that publishes the two
crates.

- **S1:** `reproto/tree-sitter-textproto/` holds the crate:
  - `Cargo.toml`, `build.rs` and `bindings/rust/lib.rs`, beside the
    Python package;
  - two tests and a doc example, which pass;
  - its `.crate` holds the parser, its three headers, `highlights.scm`,
    `grammar.js`, the bindings and `LICENSE`, and nothing of the Python
    package.

  `workspaceSrc` gains the grammar's `LICENSE` and `grammar.js`, which the
  packaging check needs.
- **S2:**
  - protolens's `build.rs`, its `extern` block and its `cc` build
    dependency are gone; `colorize.rs` takes `LANGUAGE` and
    `HIGHLIGHTS_QUERY` from the crate.
  - Its 1,322 unit tests, 25 export and 4 script tests pass unchanged, in
    an empty environment.
- **S3:**
  - The package is `prototext-lens`, the binary still `protolens`.
  - The dev-shell, `bin/bench` and a test's comment use the new `-p`
    name.
  - `todo.md`, a working-notes file in `protolens/`, is excluded from the
    package.
  - The README's install section says `cargo install prototext-lens`,
    and why.
- **S4:** `scripts/publishable-crates` and `nix/crates-io.nix` list nine
  crates: `tree-sitter-textproto` first, `prototext-lens` last.
- **Test plan:**
  1. `scripts/check-crates` packages and verifies the nine crates, each
     compiled from its `.crate` against the packaged versions of the
     others.
  2. `cargo install` from the `.crate` files alone, outside the
     workspace, needs the 0.3.3 dependencies on crates.io, so it waits
     for the release. Test plan 1's verification compiles the same
     `.crate` the same way.
  3. As in S1 and S2.
  4. `ci` (rustfmt, clippy, the tests), `crates-io`, `nixpkgs-staging`
     (the protolens recipe built with `-p prototext-lens`, tests and
     version check passing), and the image, 27 of 27 smoke checks.
- **Found on the way:** the first `ci` run failed on rustfmt, for a test
  line in `bindings/rust/lib.rs` that `cargo fmt` had not been run on.
