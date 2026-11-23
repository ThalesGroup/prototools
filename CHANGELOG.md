<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Changelog

## [Unreleased]

### Added

- Nix: `default.nix` exports `protobufRelease` (`release`, `protoc`,
  `python`, `pythonPkgs`), the release of spec 0460, for a downstream
  build to take rather than re-derive. A `pythonPkgs` passed in whose
  Python protobuf is not that release's now fails evaluation, saying what
  to pass; left out, it is `protobufRelease.pythonPkgs`, as before
  (spec 0461).
- Nix: `scripts/protobuf-release-check`, a step of CI: no derivation of
  ours takes another protobuf than spec 0460's release. The Rust
  workspace, the demos and the game took nixpkgs' protobuf 34.1, so
  prototext's roundtrip tests compared against protoc 34.1; they take
  the release's protoc now. The release's Python is nixpkgs' default
  `python3` (3.13.15 at the pin, unchanged), no longer `python313` by
  name (spec 0462).
- Docs: a tutorial for `git seriate` and `git seriate-fdp`
  (`docs/git-seriate/tutorial.md`), and their entry in the README.
- Nix: `git-seriate` and `git-seriate-fdp` packages, with shell
  completions (bash, zsh, fish) and man pages, one per command, named so
  that `git help seriate` finds them; both in `ci` and in the `prototools`
  bundle. crates.io: `seriation`, `git-seriate` and `git-seriate-fdp` are
  publishable crates, released by the publish workflow like the others.
- git-seriate-fdp: a new crate and binary, `git seriate-fdp`, git-seriate
  for the protobuf descriptors embedded in executables. `extract FILE…`
  adds each executable's descriptors as a snapshot (fdp-scan's refusals
  recorded with their codes); they are stored as prototext's annotated
  text, every rendering read back before it is kept; the rule set `fdp`
  drops a `json_name` equal to protoc's default and orders fields by
  number, so that one file written by different code generators is one
  version. The descriptor schema is a repository setting,
  `seriate.fdpSchema` (spec 0453).
- git-seriate-fdp: `advise`, which reads a history of descriptors and
  writes a `fixups.yaml` for protoback settling its conflicts (copies,
  versions, moves, reused extension numbers), a `STATUS.md`, the problems
  left (version skew a descriptor pool refuses, stale importers, missing
  imports, manual files, ties, out-of-order sources), and the groups whose
  sources are candidates to fix them; `--check` tells whether the files
  are current, `--report` reads a protoback report back; a front end words
  the candidates through `advise::Wording` (spec 0454).
- git-seriate: `cli::run`, the command line as a library entry point for
  front ends with their own plugins and commands; `plugins::Composed`,
  rule sets applied in order.
- prototext-core: `helpers::json_name`, protoc's default JSON name, which
  protoback now uses too.
- git-seriate: a new crate and binary, `git seriate`, storing snapshots of
  files as git branches, each a single commit holding the files and a
  `format: 1` block saying where they come from. `add DIR`, `list`,
  `drop` and `import`; the store refuses paths git or a checkout can't
  hold, renames clashing ones `~N`, and checks every tree before writing
  it. `settle` rebuilds the recent history in the order `seriation`
  finds: the tail past a cutoff (by default the oldest new snapshot's
  build time less 60 days) re-sorted, the unchanged prefix kept as it is,
  manual commits that change no item path replayed after the snapshot
  they followed (recorded in a `Snapshot-Anchor:` trailer), the previous
  tip saved as `NAME-N`, commits signed with SSH when git is configured
  to; `--dry-run` writes nothing. `compact --cutoff DATE` garbage-collects
  the history before a date: commits that change nothing (`--level 1`, the
  default, lossless), then those whose changes are all made again later
  (`2`), then the items of snapshot commits changed again later (`3`,
  marked `reduced: true`), the tree at the cutoff always unchanged. A
  `LegacyReader` plugin reads blocks of an older format, for `settle` and
  `compact`, which writes them in `format: 1` (spec 0452).
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
- git-seriate-fdp: `advise --report FILE --strict` exits 1 when the
  build found something the analysis didn't foresee (a finding attached
  to no problem, an idle directive), listed under `Not foreseen:`; a build
  applying `advise`'s `fixups.yaml` checks it this way rather than with
  protoback's `--fail-on W24`. `advise::Options` has a new field,
  `strict` (spec 0455).
- protoback: `--export-used`, off by default: a message or enum local to
  its file (edition 2024 and later) that another written file uses is
  written `export`, so that protoc compiles the output, each one with a
  comment and a W31 in the report; a nested one under edition 2026's
  `STRICT`, which can't be exported, is reported as W32. The descriptor
  written beside the text follows it (spec 0458).

### Changed

- One protobuf release, v36.2, behind every copy of `descriptor.proto`
  and the other well-known types: the vendored upb, the built-in
  descriptors reproto and protoback fill in, prototext's prebuilt
  fixtures and WKT graph, fdp-scan's corpus, the protoc the builds run
  and Python protobuf (7.36.2), all named once in `nix/protobuf.nix`.
  `ci` fails when a copy is from another release (`upb-release-check`,
  `descriptor-copies-check`). Both tools' built-in `descriptor.proto` was
  protobuf 34.1's: a rendered `google/protobuf/descriptor.proto` now has
  35.x and 36.x's additions, a text input may hold `EDITION_2026`, and an
  input without its own `descriptor.proto` gets edition 2026's defaults
  (spec 0460).

- git-seriate-fdp: `advise --report` reads W24, each attached to the
  `stale-importer` problem of its importer and move; and a file refused
  because its import was, with what is said of it, is attached under the
  root refusal instead of standing alone as a `build` problem (spec
  0455).
- fdp-scan, protoscan: a descriptor ends before an `edition` no writer
  sets (one without `syntax: "editions"`, or outside `EDITION_2023` to
  `EDITION_UNSTABLE`); one left with its name only is refused, with the
  new reason `ImplausibleEdition` (`implausible-edition` in `git
  seriate-fdp extract`'s records). A `match` on `Reason` must name it.
  Bytes after a descriptor that read as such a field (a `p` and a small
  byte) are no longer taken for its own (spec 0456).

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

- reproto, protoback: an `extend` block whose every field is commented
  out (its extendee or type in no file) was written with its `extend`
  line and brace as code, an empty block protoc refuses before naming the
  missing import; the block is now commented out whole (spec 0459).

- protoback: an editions file of an edition its own table didn't name,
  edition 2026 among them, was left unwritten with no warning, nothing in
  the report and exit status 0. The edition is now named from the input's
  `descriptor.proto`, then the vendored one; an edition none names is a
  W6 with its reason, and a file not written always has a warning (spec
  0457).

- git-seriate: `list`, `drop` and `import` now read snapshot branches in
  an older format through the front end's legacy reader, as `settle` and
  `compact` do; they used to see none of them, so `drop` couldn't delete
  what `settle` had merged, and `import` copied nothing (reported by the
  downstream implementation).
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
