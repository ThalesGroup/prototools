<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Changelog

## [Unreleased]

### Added

- seriation: a new crate, the sort that orders undated snapshots of files
  by their build times, their order hints and how the files in them
  evolve, as a library with no git and no file format. A caller passes
  each snapshot's files as version keys, a way to fetch a version's bytes
  and a comparator (`LinesChars`, line and character containment, is
  provided); `sort` returns the order and a report per phase.
  `seriation-gen` writes synthetic corpora with known true orders. The
  algorithm is described in `docs/seriation/` (spec 0451).
- fdp-scan: `scan_explained`, which returns what `scan` finds and the
  candidates it refused, each with its reason (an implausible name, a
  name-only record that does not stand alone, a defect); and
  `scan_explained_with`, which scans against the caller's own
  `descriptor.proto`, for descriptors written against a fuller one than
  the stock schema (spec 0448).
- prototext-graph: `EntryScore::undeclared`, counting the undeclared
  fields a scan read, and `EntryScore::ending`, saying how a scanned
  record ended. A struct literal of `EntryScore` must now name both.
- fdp-scan: `ScanOptions`, `scan_explained_options` and
  `scan_explained_with_options`. The one option,
  `increasing_field_numbers`, requires fields in number order, as protoc's
  C++ writer emits them, and reports in `Explained::order_cuts` each record
  it ends earlier than the scan without it. Off by default: Go's writers
  don't keep that order (spec 0449).
- prototext-graph: `ScoringOpts::increasing_field_numbers`,
  `EntryScore::order_break` and `Ending::OutOfOrder`, the walk's side of
  it. A struct literal of `EntryScore` must now name `order_break`, and a
  `match` on `Ending` the new variant.

### Changed

- fdp-scan, protoscan: a field the stock `descriptor.proto` does not
  declare, nested in one it does (an option it lacks), no longer cuts the
  descriptor before the field holding it; it is read and kept. Binaries
  built against a fuller `descriptor.proto` lost a third of their
  descriptors' contents this way. An undeclared top-level field still
  ends the descriptor (spec 0448).
- fdp-scan, protoscan: a descriptor ends before a `public_dependency` or
  `weak_dependency` index with no matching `dependency`, which protoc
  never writes; one left with its name only is refused, with the new
  reason `DanglingDependency`. Bytes after a descriptor that happen to
  read as such a field (Go's string data, random neighbors) are no longer
  taken for its own, and a Python module's options string in a `.pyc` is
  no longer reported as a descriptor (spec 0449).
- fdp-scan, protoscan: a `.proto` name is plausible only if each path
  component is made of `A-Z a-z 0-9 _ . + -`, which drops the Java
  codegen templates every `libprotoc` used to yield as five descriptors.
  A descriptor holding only its name, protoc's output for an empty file,
  is found when it stands alone.

- reproto and protoback say where they wrote the `.proto` files, and how
  many: `  protos:     DIR (N files)`, after the schema database's summary,
  or at the end of "Rendering proto files" without one. Nothing is printed
  in a dry run or under `--quiet`. With `--schema-db-out`, `-O` defaults
  to the database's stem followed by `/proto`, which nothing said.
- reproto reads descriptors in the text format with prototext's own
  parser, each file once, in import order, with the files it imports
  (specs 0426, 0427), instead of Python's `text_format`. Custom options
  printed by number, which `text_format` dropped, are kept; options
  printed by name resolve from the input; where the text leaves a value's
  framing open, the likely one is taken, with a warning naming the field.

### Removed

- reproto: the variant key `annotation_modules` (spec 0018) and the
  Python modules it imported. Declarations come from the input: a file
  declaring options is given as any dependency, on the command line or
  under `-I`. A variant that still has the key is read as before, the key
  ignored.

### Fixed

- reproto, protoback: `--emit-extension-ranges` no longer refuses a
  schema holding a message set (`option message_set_wire_format`), whose
  `extensions 4 to max` protoc writes up to 2^31 − 1: the scoring graph
  clamps extension ranges to the largest field number, 2^29 − 1, which is
  all a tag can carry. The descriptor set keeps the range as protoc wrote
  it (spec 0238 S3).
- reproto reads a directory input, or a directory under `-I`, in sorted
  path order, not in the order the filesystem lists it: which of two
  files holding the same descriptor is kept (W7), and the order files are
  processed in, no longer depend on how the directory was written
  (spec 0428).
- reproto adds the files of one rank to its descriptor pool in the order
  it found them, not in an order set by memory addresses: the schema
  database's file order, which of two files defining one symbol is kept
  (W3), and the order of the warnings no longer depend on the machine or
  the Python running it (spec 0428).
- reproto: an import it leaves out, its file missing or pruned, no
  longer shifts the `public` and `weak` imports after it onto other
  files; the file's options, which upb then refused, render again
  (spec 0424 S1).
- reproto: a custom option whose value is a message prints its map
  fields, entries in the order they were written, instead of crashing;
  its extensions as `[pkg.ext]`, and its groups by their type's name
  with their fields, which protoc could not read back (spec 0424 S1).
- reproto: custom options on a oneof or an extension range are no longer
  left out (spec 0424 S1).

## 0.3.6 — 2026-10-24

### Fixed

- reproto's completion scripts are printed as they are, whatever Click
  8.x prints after them; with Click 8.4, a test of 0.3.5 failed on the
  missing last newline, which made 0.3.5 fail to build in nixpkgs
  (spec 0423).

## 0.3.5 — 2026-10-24

### Added

- **reproto completes in zsh and fish**, as in bash (spec 0422): the
  descriptor files relative to `-I` and an `-I` blob's members included.
  reproto prints each shell's script, `_REPROTO_COMPLETE=bash_source`
  (or `zsh_source`, `fish_source`), so a pip install has them too; the
  Nix packages install all three.

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
