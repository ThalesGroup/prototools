<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0457 — protoback: edition names from the input, and no file lost without a warning

Status: implemented
Implemented in: 2026-11-22
App: protoback
Refs: docs/specs/0429-protoback-rendering.md (S16: a render failure is a W6),
      docs/specs/0446-build-report.md (W6's fields),
      docs/specs/0455-advise-report-what-the-build-said-and-advise-did-not-foresee.md
        (what `advise --report` does with the W6)

## Background

A downstream repository holds two descriptors of edition 2026
(`syntax: "editions"`, `edition: 1002`), compiled against a
`descriptor.proto` whose `Edition` enum declares `EDITION_2026 = 1002`.
protoback writes neither: no warning, nothing in the report, exit status
0. On one alone, it writes only its dependency.

The edition line is rendered by `edition_name` (`render/pyfmt.rs`), the
names Python protobuf 7.34.1 knows, which stop at `EDITION_2024`. On any
other value it returns `None`, and `render_body` returns at its `?`
before setting `failed`, which is what makes phase 7 report the file
(W6). reproto, on the same file, raises in `Edition.Name` and reports W6
("internal error"): the silence is protoback's alone.

Edition 2026 was released with protobuf v36.0 (2026-08-20); the
`descriptor.proto` vendored in `prototext-upb` (v36.2) declares it, and
the descriptor pool accepts these files. It changes no grammar. Its new
defaults (`enforce_naming_style`, `default_symbol_visibility`,
`enforce_proto_limits`) are read, like every feature default, from the
input's `descriptor.proto` (phase 2's edition defaults), and visibility
is rendered wherever the descriptor sets it, whatever the default. So an
edition 2026 file renders as a 2024 one does, but for its edition line.

## Goals

- **G1.** Edition 2026 files rendered, with `edition = "2026";`.
- **G2.** An edition named from the input's own `descriptor.proto`, so
  that a later edition needs no change here.
- **G3.** No summoned file left unwritten without a warning.

## Non-goals

- **N1.** Checking what edition 2026 forbids (naming style, visibility,
  limits): the input was compiled, so it holds; protoc checks the output.
- **N2.** reproto: its Python protobuf names what it names. protoback
  departs from it here on purpose; the harness corpus holds no edition it
  can't name, so the comparison is unchanged.

## Specification

- **S1.** Phase 2 reads, beside the feature defaults, the `Edition` enum
  of the input's `descriptor.proto` (the variant's): each value's number
  and name.

  The file's edition is read from its bytes. Descriptors are decoded
  against the vendored `descriptor.proto`, where `Edition` is a closed
  enum: a value it doesn't declare is kept as an unknown field, and
  `edition` reads unset (0, `UNKNOWN`). The encoding keeps the unknown
  field, so the value is read back from it (field 14, the last one
  winning) when `edition` is unset.

- **S2.** The edition line names the file's edition from S1, without the
  `EDITION_` prefix; failing that, from the vendored `descriptor.proto`'s
  `Edition` enum; failing that, from `edition_name`'s table.

  An edition the vendored `descriptor.proto` lacks is also one the
  descriptor pool can't load: the file is refused (W19) and rendered as
  any refused file is, its file options as an anomaly, with the edition
  line the input names.

- **S3.** An edition none of them names fails the file: W6, with the
  reason `edition N has no name in its descriptor.proto`, in the report
  with `file` and `reason` (0446 S3). The file is not written.

- **S4.** A render that returns no output always sets `failed`, so
  phase 7 reports it (W6). Where the render has its own reason (S3), the
  W6 carries it rather than `internal error`.

## Alternatives considered

**Rendering an unknown edition by its number** (`edition = "1003";`).
protoc refuses it, but the file looks written; a W6 says what happened
where the build is read.

**Refreshing the vendored `descriptor.proto` instead.** It is already
the latest release's (v36.2), which declares `EDITION_2026`: that is why
the pool accepts these files. The drop was protoback's own table. Each
new edition reaches inputs before a refresh does; S1 covers the gap, and
S3 says what it can't.

**Adding `1002 => "2026"` to the table only.** It fixes this edition and
leaves the next one to drop files the same way; S2 keeps the table as the
last fallback.

## Test plan

1. `edition_2026_renders` — a file of `syntax: "editions"`, `edition:
   1002`, one message with one field, against a `descriptor.proto`
   declaring `EDITION_2026`: written, with `edition = "2026";`.
2. `edition_unnamed_is_a_w6` — `edition: 1003`, declared nowhere: W6 with
   S3's reason, in the report with `file`; exit status 0, the file not
   written, the other input file written (S4).
3. `edition_named_by_the_inputs_descriptor` — `edition: 1003`, and an
   input `descriptor.proto` whose `Edition` declares `EDITION_2027 =
   1003`: written, with `edition = "2027";`.

## Measured outcome

The three tests pass. `scripts/protoback-selfcheck` against HEAD
(`326e09c1`), both modes: 1 669 cases and 1 655 graph dumps, 0 differ.

A text input (`.textpb` and the like) is parsed against the input's
`descriptor.proto` (spec 0426), and an input without one gets the
built-in variant's, which predates edition 2026: `edition: 1002` in text
is refused there (W15), as before. Binary inputs aren't parsed against
it, and the downstream inputs are binary.
