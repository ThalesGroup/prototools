<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0475 — reproto, protoback: a refused file keeps its options

Status: draft
App: reproto | protoback
Refs: docs/specs/0438-warning-codes-and-fail-on.md (A3, W19, W22),
      docs/specs/0473-protoback-repairs-reach-the-pool-and-the-schema-database.md
        (repaired files no longer refused)

## Background

When the descriptor pool refuses a file (W19), both tools write it
without its file options:

```
// OMITTED[render]: file options could not be rendered (the descriptor pool refused it: couldn't resolve name '.p.M.T')
```

(`render_file_options` in reproto, `file_options` in protoback: an early
return on a refused file.) Every file option is lost, and for an editions
file its file-level features with them: `features.enum_type = CLOSED`
dropped makes its enums open (protoc then refuses an enum whose first
value isn't zero), `field_presence = IMPLICIT` dropped makes presence
explicit, `repeated_field_encoding = EXPANDED` dropped packs repeated
fields. Nothing but that comment says so. A downstream corpus has 34
refused files: 28 lose options such as `java_package`, 6 editions files
their features, 5 of which still compile, with other semantics.

Reading a file's options needs no type of the file resolved: standard
options and features are fields of `descriptor.proto`'s `FileOptions`,
which the options pool holds; a custom option whose extension isn't
available is written as an orphan, by number, with W22, as in any file.

## Goals

- **G1.** A refused file's file options are rendered as any file's are.
- **G2.** No option dropped without a warning `--fail-on` can catch.

## Non-goals

- **N1.** Accepting the file: it stays refused, W19 as today (0473 makes
  a repaired file accepted under `--repair-skew`).
- **N2.** Other elements' options: they are rendered today; only file
  options returned early.

## Specification

- **S1.** The file options of a file the pool refused are rendered from
  its descriptor, as for any file: its features, its standard options,
  its custom options resolved against the options pool; an option whose
  extension isn't there, an orphan with W22 (0438). No `OMITTED[render]`
  for the refusal; W19 still reported, its comment kept where it is.

- **S2.** An option still impossible to render (the options can't be
  parsed at all) is A3 with W6, as an internal failure is today: never
  silent.

- **S3. Both tools** write the same, byte for byte (the differential
  harness compares them).

## Test plan

1. `a_refused_file_keeps_its_features` — an edition 2023 file with
   `option features.enum_type = CLOSED;` and `java_package`, an enum
   starting at 1, and a field of an undefined nested type: refused (W19),
   written with both options; protoc compiles the written file once the
   field is fixed (`--repair-skew`).
2. `a_refused_files_custom_option_is_an_orphan` — a custom file option
   whose extension is defined in the refused file itself: written as an
   orphan, W22.
3. Both tools; the harness, both modes (the refused cases of the corpus
   now carry their options).

## Measured outcome

Filled in at implementation.
