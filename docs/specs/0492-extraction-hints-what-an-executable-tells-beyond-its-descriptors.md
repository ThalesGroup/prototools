<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0492 — extraction hints: what an executable tells beyond its descriptors

Status: implemented
Implemented in: 2026-11-27
App: git-seriate-fdp
Refs: docs/specs/0453-git-seriate-fdp-descriptors-from-executables.md
        (`extract`, which this complements),
      docs/specs/0480-fdp-scan-inflates-zip-and-gzip-members.md
        (origins, which the hints sit beside)

## Background

Two copies of one file name in one executable come from two producers, not
two versions of one line: measured on an open-source corpus of 2,896
executables, where in a Go executable two copies registered by the current
generator never share a name (protobuf-go's registry refuses it), and every
collision is between generators or registries. A history that reads two
producers as one line of versions sorts them wrongly. To tell producers
apart, and to place each copy in the repository it comes from, the
executable holds evidence beyond the descriptors, readable cheaply: strings
found by offset or by a byte search, nothing decoded.

## Goals

- **G1.** Read, per executable, which code registers each `.proto` it
  embeds, and which module or repository that code comes from, for Go and
  C++.
- **G2.** Cheap: a byte search and a few offsets, no disassembly, no debug
  information; in the order of the time to read the file.
- **G3.** Nothing run: the executable is data.

## Non-goals

- **N1.** A stable output format: this is the first reader, its JSON the
  input of experiments. A language-neutral record (registrant, generator,
  package and version, per descriptor) is to follow, once the experiments
  say what it needs.
- **N2.** Recording the hints in snapshots: a later spec, with the units of
  spec 0495.
- **N3.** Go's type metadata, DWARF, disassembly: not cheap.

## Specification

- **S1. `git seriate-fdp hints FILE…`** prints one JSON line per file:
  `{"file", "go", "cpp"}`, each part `null` when the file has none of its
  hints.
- **S2. Go** (`go`), from the runtime's function table (`pclntab`), which
  survives stripping, found by its magic (formats 1.2, 1.16, 1.18, 1.20)
  and a plausible header; from 1.16 on, its function-name and file-name
  tables alone are read, for 1.2 the whole file:
  - `pclntab`: the format;
  - `pb_go`: the generated `.pb.go` source paths, which name
    `module@version` from the module cache, or a GOPATH or `vendor/` path;
  - `proto_init`: `[package, name]` for each `<package>.file_<name>_proto_init`,
    the current generator's per-file init function: the Go package that
    registers the file (`%2e` in a package read as `.`);
  - `types`: `[package, type, methods]` for each pointer method
    `ProtoReflect` or `Descriptor`: the generated messages, per Go package.
- **S3. C++** (`cpp`), from symbols and strings:
  - `descriptor_tables`: the `.proto` paths from protobuf's
    `descriptor_table_<escaped>_2eproto` symbols (every character but a
    letter or a digit escaped `_XX`; an escape not of two hex digits, or
    not of a printable character, is no table);
  - `pb_cc`: `[file, repository]` for each `.pb.cc` named by a static
    initializer (`_GLOBAL__sub_I_…`) or a path, the repository being the
    Bazel external repository a path names (`…/external/<repo>/…`),
    `null` for none.

## Alternatives considered

- **Go's build info alone:** it lists the modules linked, not which module
  registers which file; and a file's `go_package` option can be stale (a
  generator overrides the import path without rewriting the option).
  Measured: the init function agrees with the build info in 98.3% of
  cases, the rest being stale `go_package` options, the hint right.
- **Decoding `pclntab` fully:** gives addresses, not needed for names.

## Test plan

1. `a_go120_pclntab_gives_its_two_tables`,
   `a_magic_with_an_implausible_header_is_not_a_pclntab`,
   `no_pclntab_no_go_hints`.
2. `the_go_hints_are_read_from_the_tables`: `.pb.go` paths, init functions
   and types, `%2e` unescaped.
3. `the_cpp_hints_are_read_from_symbols_and_strings`,
   `a_malformed_escape_is_no_table`.

## Measured outcome

On 2,896 Go and C++ executables of an open-source corpus: 2,595 with Go
hints, about 0.03 s of CPU each. The Go hints halve the descriptors left
unplaced by module (10.0% to 6.0%), and cut by 85% the unplaced files
named without a directory. On a C++ library of 310 table symbols: the 67
files it registers.
