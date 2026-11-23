<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0469 — protoback --repair-skew: the output compiles despite version skew

Status: draft
App: protoback
Refs: docs/specs/0458-protoback-export-used.md (the model: an opt-in,
        each change commented and reported),
      docs/specs/0459-an-extend-block-of-orphans-is-an-orphan.md (an
        extend block of orphans),
      docs/specs/0434-type-names-as-protoc-resolves-them.md (the symbols
        a file sees),
      docs/specs/0455-advise-report-what-the-build-said-and-advise-did-not-foresee.md
        (where `advise --report` attaches a finding)

## Background

A downstream repository compiles each `.proto` file protoback writes
with protoc. After `--export-used` (0458), 24 of 16,166 fail: 15 only
inside an import of theirs, and 9 on their own content, all version skew
between builds months apart:

| In the file | Files | Today |
|---|---|---|
| a field whose type a written file defines, not imported | 5 | written; protoc: "seems to be defined in …, which is not imported" |
| a field whose type no file defines | 2 | written, W4 |
| an extension number its extendee doesn't declare | 1 | written |
| an open enum without a zero value, in a refused file | 1 | written |

protoback comments out an extension whose extendee is missing (and, since
0459, its block); it writes the rest as the descriptors say, which is
right by default.

## Goals

- **G1.** An opt-in, `--repair-skew`, after which those files compile:
  each repair the smallest that does, commented where it is and reported.
- **G2.** The repairs reported with what `advise --report` needs to
  attach them (0455).

## Non-goals

- **N1.** On by default: the text would no longer say what the
  descriptors say.
- **N2.** The open enum without a zero value: adding a value changes the
  enum; the file is refused anyway, and it is reported as today.
- **N3.** reproto: a protoback option, as `--export-used` is.
- **N4.** Ambiguous cases, repaired by guessing: a type two written files
  define stays as it is, reported (S5).

## Specification

- **S1. `--repair-skew`,** off by default.

- **S2. A missing import.** A field's type, an extendee, or a method's
  input or output type that resolves to a symbol defined in exactly one
  written file, which the file doesn't import, directly or publicly: the
  import is added, and the names are written as that file makes them
  visible (0434). Before it:

  ```proto
  // WARNING[repair-skew]: import added: defines a.Row, used here; not imported in the source
  import "a/row.proto";
  ```

  W33, `'b/user.proto': import "a/row.proto" added: a.Row is defined
  there (--repair-skew)`, with `file`, `import` and `symbols`. The
  descriptor written beside the text (`--emit-binary`) gets the
  dependency too.

- **S3. A type no file defines.** A field (or a method) whose type no
  written file defines (a W4 today) is commented out, an orphan, with a
  comment: `// WARNING[repair-skew]: commented out: .a.Gone is defined in
  no file`. W34, with `file`, `field` and `type`. A map field whose value
  type is missing, the same; a oneof left with no field, an orphan whole,
  as 0459 does an extend block; a message left with no field stays (an
  empty message compiles).

- **S4. An extension number its extendee doesn't declare.** An extension
  whose number is in none of its extendee's extension ranges is
  commented out, with `// WARNING[repair-skew]: commented out: 109825718
  is in no extension range of x.JobSpec`; its block, if empty, an orphan
  (0459). W35, with `file`, `extension`, `number` and `extendee`.

- **S5. Ambiguous.** A type that two or more written files define, which
  the file doesn't import: nothing changed; W36, with `file`, `symbol`
  and `defined_in` (each file).

- **S6. Reported** as W31 is (0458 S6): counted, squashed on stderr unless
  `--detailed-warnings`, every occurrence in the report. `advise --report`
  attaches W33 to W36 to the file's problem (0455), as it does a W4.

- **S7. The descriptors and the schema database** follow the text, as for
  0458: what protoc would build from it. A field commented out isn't in
  the descriptor written beside it.

## Alternatives considered

**Commenting out a field of a type defined but not imported** (S2's
case) instead of adding the import: it loses a field the descriptors
have, where an import keeps it, losslessly.

**Repairing in `advise`'s `fixups.yaml`:** the skew is within one file's
references, not between files `advise` decides among.

## Test plan

1. `repair_skew_adds_the_missing_import` — `b.proto` using `a.Row`,
   defined in `a/row.proto`, not imported: the import with its comment,
   one W33; protoc compiles both; without the option, as today.
2. `repair_skew_comments_out_a_field_of_no_type` — the field an orphan,
   W34; a oneof emptied that way an orphan whole; protoc compiles it.
3. `repair_skew_comments_out_an_extension_out_of_range` — W35; its block
   an orphan when empty; protoc compiles it.
4. `repair_skew_leaves_the_ambiguous` — two files define the type: W36,
   nothing changed.
5. `repair_skew_binary_follows_the_text` — with `--emit-binary`: the
   dependency added, the fields commented out absent.
6. The self-check (without the option): no change.

## Measured outcome

Filled in at implementation.
