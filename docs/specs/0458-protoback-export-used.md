<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0458 — protoback --export-used: export what other files use

Status: implemented
Implemented in: 2026-11-22
App: protoback
Refs: docs/specs/0434-type-names-as-protoc-resolves-them.md (the symbols a
        file sees, which this reads),
      docs/specs/0446-build-report.md (the warnings and their fields),
      docs/specs/0457-protoback-edition-names-from-the-input.md (editions
        2024 and 2026, their visibility defaults)

## Background

Editions 2024 and later give messages and enums a visibility: `export`
or `local`, set on the symbol or defaulted by the file's
`default_symbol_visibility` feature. Edition 2024's default is
`EXPORT_TOP_LEVEL` (top-level symbols exported, nested ones local), 2026's
`STRICT` (every symbol local, and a nested one can't be exported). Only
protoc enforces it: a symbol local to its file can't be used from another.
The descriptor pool doesn't check it, and it changes nothing in how a
message decodes.

A downstream repository compiles each `.proto` file protoback writes with
protoc. Three edition 2024 files, from newer builds, leave nested messages
and enums local; 168 files use one of them, most from older builds written
against versions of the three that exported them. protoc refuses each:

```
Symbol "a.Outer.Inner", defined in "a/outer.proto"  is not visible from
"b/user.proto". It defaulted to local from file-level 'option
features.default_symbol_visibility = 'EXPORT_TOP_LEVEL'; and cannot be
accessed outside its own file
```

The schema database is right as it is. What fails is the text, for a
reader who compiles it beside the database.

## Goals

- **G1.** An opt-in, `--export-used`: a symbol local to its file and used
  by another file of the output is written `export`.
- **G2.** Each symbol so changed is said where the change is: a comment in
  the file, a warning in the report.

## Non-goals

- **N1.** On by default. The text then says more than its source did; a
  reader of the files as they were written doesn't want that.
- **N2.** reproto. A protoback option, as `--target portable` is.
- **N3.** Symbols no other file of the output uses. A symbol only an
  absent file would use is not seen, and not changed.
- **N4.** A nested symbol under `STRICT`. protoc refuses `export` on it;
  making it exportable would mean changing the file's
  `default_symbol_visibility`, which changes every other symbol's. It is
  reported (S5), not changed.

## Specification

- **S1.** `--export-used`, off by default.

- **S2.** A symbol is *local* when its file is written as editions, its
  edition has visibility (2024 and later), and either it is set `local`,
  or it is unset and its file's default makes it so: `EXPORT_TOP_LEVEL`
  for a nested symbol, `LOCAL_ALL` and `STRICT` for any. The file's
  default is its own `features.default_symbol_visibility` if set, else
  its edition's default, from the input's `descriptor.proto`
  (`edition_defaults`, as phase 2 reads the other features). A symbol set
  `local` explicitly is treated as one defaulted to it: a file using it
  was compiled against a version that exported it.

- **S3.** A symbol is *used* by another file when that file, rendered,
  refers to it: a field's type, an extension's extendee, a method's input
  or output type. The references are the descriptors' fully qualified
  names, the symbol's file the one defining that name among the files
  rendered.

- **S4.** A local symbol used by another file is written `export`,
  preceded by a comment:

  ```proto
  message Outer {
    // WARNING[export-used]: exported: used by 'b/user.proto' (and 135 more); local in the source
    export message Inner { ... }
  }
  ```

  and reported, once per symbol: W31, `'a/outer.proto': a.Outer.Inner
  written export: used by 136 files (--export-used)`, with `file`,
  `symbol` and `users` (every using file, sorted). The descriptor written
  beside the text (`--emit-binary`) and the schema database follow the
  text: `visibility: VISIBILITY_EXPORT`, what protoc would build from it.

- **S5.** A nested symbol under `STRICT` used by another file is not
  changed: W32, `'a/outer.proto': a.Outer.Inner is used by 3 files, but
  STRICT visibility can't export a nested symbol (--export-used)`, with
  the same fields.

- **S6.** W31 and W32 are counted warnings, squashed like W28: one line
  per code on stderr unless `--detailed-warnings`, each occurrence in the
  report.

## Alternatives considered

**Exporting every nested symbol of an edition 2024 file.** It fixes the
case with no analysis, but it changes symbols nothing uses, and hides
which ones the output depends on.

**Setting the file's `default_symbol_visibility` to `EXPORT_ALL`.** One
line per file instead of one per symbol, and it covers `STRICT`; but it
exports every symbol of the file, and changes a feature the reader may be
looking at.

**Rewriting the using files instead** (dropping the reference). They
would compile and decode wrongly.

## Test plan

1. `export_used_exports_a_used_nested_symbol` — an edition 2024 file
   with `Outer.Inner` (unset) and `Outer.Other` (unset), another file
   using `Outer.Inner`: with the option, `export message Inner` with its
   comment, `Other` unchanged, one W31 with its `users`; without it, no
   change. The same with `Inner` set `local`.
2. `export_used_leaves_a_symbol_its_own_file_uses` — a nested symbol used
   only within its own file: unchanged.
3. `export_used_under_strict_reports` — an edition 2026 file, a nested
   symbol another file uses: unchanged, one W32; a top-level one: written
   `export`, one W31.
4. `export_used_binary_follows_the_text` — with `--emit-binary`, the
   symbol's descriptor has `visibility: VISIBILITY_EXPORT`.
5. The self-check (without the option): no change from this spec.

## Measured outcome

The four tests pass (`protoback/tests/export.rs`). On the Background's
shape (an edition 2024 file, a nested message and enum another file
uses), protoc 34.1 refuses the user file without the option and compiles
it with. The self-check against HEAD: no change from this spec (the two
cases that change are 0459's).

`default_symbol_visibility` is source-retained, so descriptors don't
carry it at file level: the default is in practice the edition's, from
the input's `descriptor.proto`. An input without one gets protoback's
built-in copy, which predates edition 2026: its files' default reads as
2024's `EXPORT_TOP_LEVEL`. The downstream inputs carry their own.

Not measured here: the downstream repository's 168 files.
