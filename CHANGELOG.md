<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Changelog

## [Unreleased]

### Changed

- **prototext and protolens read schemas through upb**, Google's C
  protobuf runtime, instead of prost-reflect (specs 0415, 0417):
  editions schemas load, where prost-reflect panicked on them. A schema
  file that cannot be loaded is skipped and reported, instead of ending
  the run. protoscan reads descriptor names without prost (spec 0418);
  no tool depends on prost any more. Outputs are byte-identical on our
  golden corpora; protolens starts 4 times faster on googleapis, in a
  quarter of the memory.
- **A schema database keeps editions files as editions** (spec 0421),
  and so do the `.proto` files written beside it. `--schema-db-out`
  converted them to proto2, which cannot express every editions
  feature: a delimited message field that is not a group decoded as a
  type mismatch. prototext and protolens 0.3.4 and older, on
  prost-reflect, panic loading such a database: to build one for them,
  pass `--force-proto2-for-editions`, as for any reader without
  editions support, such as prost-reflect.

### Fixed

- A group's header is named by its type only when protoc does so
  (spec 0415): an editions delimited field that is not group-like goes
  by its own name, as in protoc's text format.

## 0.3.4 — 2026-10-15

protolens only: every other tool is unchanged, republished under the
shared version.

### Changed

- **protolens's editor key opens your editor** (spec 0420): `VISUAL`,
  then `EDITOR`, then `nvim` if installed, else `vi`, run as git runs
  it, with your own configuration. Neovim is reused across presses and
  positioned at the line and column, as before; Vim at the line and
  column; any other editor at the line. A Ctrl-C in the editor no longer
  ends protolens.

### Removed

- protolens's bundled Neovim configuration and `PROTOLENS_NVIM_CONFIG`:
  protolens no longer replaces your Neovim configuration with its own.
  Language support for `.proto` files is your editor setup's, as for any
  other file. The Nix package bundles no Neovim or buf.

## 0.3.3 — 2026-10-12

### Added

- **protolens on crates.io** (spec 0414), as `prototext-lens`, since the
  `protolens` crate belongs to an unrelated project:
  `cargo install prototext-lens` installs the `protolens` program. The
  textproto grammar it uses is published too, as the `tree-sitter-textproto`
  crate, as it is on PyPI.

### Fixed

- `reproto-instantiate-schema --version` crashed with `RuntimeError:
  'reproto' is not installed`; it prints its version, like `reproto`
  (spec 0413). The build now checks `--version` on every installed
  program, not only the main one.

### Removed

- The `prototext-gen-man`, `reproto-gen-man` and
  `reproto-instantiate-schema-gen-man` programs, which existed only to
  write man pages, and took `--version` for a directory to write them
  in. The man pages come from `PROTOTEXT_GEN_MAN=<dir> prototext` and
  `REPROTO_GEN_MAN=<dir> reproto` instead, as protoscan's and protolens's
  do (spec 0413).

## 0.3.2 — 2026-10-11

### Changed

- **Releases are published from CI** (spec 0412): a release tag builds
  the crates and wheels, and, once the maintainer approves, uploads them
  to crates.io and PyPI with trusted publishing.
- This is the first release on crates.io and PyPI since 0.2.1: 0.3.0 and
  0.3.1 were tagged but never uploaded, so their changes, below, reach
  the registries with this one.

## 0.3.1 — 2026-10-11

### Changed

- **protoscan is a Rust binary** (spec 0407), with the same command line
  and output, built on a new scanner crate, `fdp-scan`, which the
  `fdp_scan_lib` Python extension now wraps. It gains bash completion that
  handles `:`, `=` and quotes (spec 0406), zsh and fish completions, a man
  page and `--version`. Install it with `cargo install protoscan`; the
  Python package `protoscan` is no longer published.
- **The PyPI wheels are built by maturin and setuptools** from each
  package's `pyproject.toml` (specs 0409, 0411), instead of being
  assembled by hand: they now carry their type stubs and complete
  metadata. The macOS wheels are tagged `macosx_14_0`, the macOS version
  they are built for; they had claimed `macosx_11_0`.
- **`prototext-reproto` declares its dependencies** (spec 0411): the
  three extensions, `tree-sitter`, and the textproto grammar, now
  published as `tree-sitter-textproto` (module `tree_sitter_textproto`).
  `pip install prototext-reproto` gives a working reproto; until now the
  wheel installed and failed at import. `lark`, which it never used, is
  no longer required.

- **protolens's editor key works with whatever is installed** (spec
  0410). Without Neovim, it says `nvim not found on PATH: install Neovim
  to use the editor key`. Without buf, Neovim still opens the file,
  highlighted, and shows one notice that go-to-definition is unavailable,
  instead of failing to start buf's language server on every `.proto`.

### Deprecated

- **protoscan `--proto_out`**: use `--proto-out`, spelled like every other
  long option. The old spelling still works, prints a warning, and will be
  removed in a later release.

### Fixed

- The Python extensions' type stubs name their real module: the
  `prototext-codec` stub declared `register_schema` as returning an
  undefined `prototextSchemaHandle`, and the `prototext-graph` stub lacked
  `build_fds_index`'s `ext_to_file` parameter.

## 0.3.0 — 2026-10-10

The first release with one version for every tool, crate and Python
package, from github.com/douzebis/prototools.

### New

- **protolens**, an interactive terminal browser for binary protobufs:
  type inference with per-range heat cues, the raw bytes behind every line,
  overrides, search, export, and guided scripts.
- **prototext**: `is-canonical`; whole-file scoring; a truncated message
  says which fields it is missing, and a truncated field keeps its declared
  type.
- **reproto**: reads a blob as a root, and applies the FDP plugin at every
  parse.
- **protoscan**: asks the schema where a descriptor ends.

### Changed

- Bash completion for prototext and protolens no longer depends on bash's
  word splitting: arguments containing `:` or `=`, quoted paths and escaped
  spaces complete correctly, and a completion never corrupts the line.
- Scoring: a bool is any varint, and a packing mismatch scores net zero.
- The builds need no `protoc` or `reproto`: the descriptors and the WKT
  scoring graph they used to generate are committed.

## 0.2.1 — 2026-06-19

prototext, protoscan and reproto, as published on crates.io and PyPI.
