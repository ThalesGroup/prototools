<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0408 — the textproto grammar ships its parser

Status: draft
App: tree-sitter-textproto (used by reproto and protolens)
Refs: docs/specs/0405-prototools-in-nixpkgs-one-small-pr-at-a-time.md
      (S9 steps 3 and 4: both reproto's and protolens's nixpkgs recipes
      need this first); docs/specs/0121-tree-sitter-textproto-field-no-vendoring.md
      (why the grammar is ours: a locally modified grammar.js);
      docs/specs/0116-tree-sitter-textproto-highlight-captures.md
      (highlights.scm and its regression test)

## Background

`reproto/tree-sitter-textproto/` holds our textproto grammar: `grammar.js`
(locally modified, spec 0121), `highlights.scm`, a highlight test, and
`binding.c`, the Python binding. Nothing generated is committed. Four Nix
derivations turn it into what the tools use:

- `treeSitterTextprotoGenerated` runs `tree-sitter generate`, which needs
  the tree-sitter CLI and Node.js, to produce `src/parser.c` and
  `src/tree_sitter/*.h`;
- `treeSitterTextproto` compiles `binding.c` and `parser.c` with
  `python3-config` into the `textproto` Python extension reproto imports
  (`reproto/src/reproto/split_fdps.py`);
- `treeSitterTextprotoRustLib` compiles `parser.c` into a static library,
  which protolens's `build.rs` links from a path given in
  `TREE_SITTER_TEXTPROTO_LIB_DIR`, next to the highlights query it embeds
  from `TREE_SITTER_TEXTPROTO_QUERIES_DIR`;
- `treeSitterTextprotoHighlightTest` regenerates the parser and runs
  `tree-sitter test`.

No nixpkgs recipe can reproduce that: it would need the tree-sitter CLI,
Node.js, and two environment variables pointing into another derivation.
tree-sitter grammars conventionally commit their generated `src/parser.c`
(every grammar packaged in nixpkgs does), so that consumers need only a C
compiler.

The three tree-sitter versions involved work together today: the CLI that
generates the parser (0.26.8, from the nixpkgs pin), the Python runtime
reproto uses (0.25.2), and protolens's Rust crate (0.26.11).

## Goals

- **G1.** The generated parser is committed, so building the grammar
  needs a C compiler and nothing else: no tree-sitter CLI, no Node.js, no
  environment variable.
- **G2.** The committed parser cannot drift from `grammar.js`: CI
  regenerates it and fails, loudly and with the refresh command, when they
  differ. Refreshing is an explicit copy, never done by a build (the rule
  the stubs, fixtures and WKT graph follow).
- **G3.** Each consumer builds the grammar with its own ordinary build:
  reproto's Python binding through a Python build backend, protolens
  through Cargo's `build.rs`.

## Non-goals

- **N1.** Changing the grammar or its queries (specs 0116, 0121).
- **N2.** Publishing the grammar on its own (npm, crates.io, PyPI).
- **N3.** The packaging of reproto and protolens themselves: specs 0409
  and 0410 build on this one.

## Specification

- **S1. Commit the generated sources** that `tree-sitter generate`
  writes: `src/parser.c`, `src/tree_sitter/parser.h` and the other
  headers it writes, `src/grammar.json` and `src/node-types.json`, under
  `reproto/tree-sitter-textproto/src/`. Their licensing goes in
  `REUSE.toml`, as generated from our MIT `grammar.js`.
- **S2. A freshness check** in `ci`: a derivation runs the pinned
  `tree-sitter generate` on `grammar.js` and compares its output with the
  committed `src/`. When they differ, it prints the diff and the refresh
  command. `treeSitterTextprotoGenerated` becomes that check's input, and
  nothing else consumes it.
- **S3. The highlight test** runs against the committed parser rather
  than a regenerated one.
- **S4. The Python binding** is built by a Python build backend from
  `binding.c` and the committed `parser.c`. ("Binding" is tree-sitter's
  term for the glue that hands a grammar to a language's tree-sitter
  runtime: `binding.c` is about thirty lines exposing
  `tree_sitter_textproto()` as a `textproto` module, which the Python
  `tree_sitter` library parses with. Rust needs none: it declares the C
  function itself.) Two shapes are possible:
  - **(a)** a small Python package of its own, `tree-sitter-textproto`
    (setuptools, one C extension), the shape of the tree-sitter grammars
    nixpkgs already packages as Python modules; reproto depends on it;
  - **(b)** reproto's own `pyproject.toml` builds the extension.

  This spec chooses **(a)**: the grammar is a separate component with
  its own consumers, reproto's build stays pure Python, and in nixpkgs it
  is a small, conventional `python3Packages` entry. The `treeSitterTextproto`
  derivation is replaced by building that package.
- **S5. protolens's `build.rs`** compiles `src/parser.c` with the `cc`
  crate (a build dependency already in the lock file through tree-sitter)
  and embeds `highlights.scm` by a path relative to the repository. The
  two environment variables, `treeSitterTextprotoRustLib`, and their
  plumbing in `nix/rust.nix` and `nix/shells.nix` go.
- **S6. The internal package** (spec 0405 C2) is unaffected: it uses
  reproto and protolens, not the grammar directly.

## Alternatives considered

### Keep generating at build time

What we do today. It ties every consumer to the tree-sitter CLI, Node.js
and a Nix derivation, which is what makes reproto's and protolens's
recipes impossible to write plainly.

### Option (b) of S4: the extension inside reproto's build

One package fewer, but reproto's `pyproject.toml` gains a C build, its
wheel stops being pure Python, and the grammar's other consumer
(protolens) still needs the sources elsewhere.

## Test plan

1. The freshness check passes on the committed sources, and fails with
   the diff after a deliberate change to `grammar.js` without refreshing.
2. The highlight test (spec 0116) passes.
3. reproto's tests pass with the Python binding from S4 (`reproto-tests`,
   including `split_fdps`).
4. protolens builds and its tests pass without the two environment
   variables (`env -u TREE_SITTER_TEXTPROTO_LIB_DIR -u
   TREE_SITTER_TEXTPROTO_QUERIES_DIR cargo test -p protolens`).
5. `ci`, the image and its smoke test pass.

## Measured outcome

Filled in at implementation.
