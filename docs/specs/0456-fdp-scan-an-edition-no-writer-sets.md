<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0456 — fdp-scan: an edition, or a syntax, no writer sets

Status: implemented
Implemented in: 2026-11-22 (S1 to S3), 2026-11-28 (S4, S5)
App: fdp-scan, git-seriate-fdp
Refs: docs/specs/0448-fdp-scan-undeclared-fields-and-rejections.md (the
        accept rule, `NameOnly`, `Reason`),
      docs/specs/0449-fdp-scan-dependency-indices-and-field-order.md (S1:
        the dependency cut, which this follows),
      docs/specs/0496-fdp-scan-a-framed-descriptor-ends-at-its-frame.md
        (the frames S5 meets)

## Background

From a Python module's archive, `fdp-scan` found a record holding only:

```
name: "com.example.crawl.proto"  #@ string = 1
edition: EDITION_2_TEST_ONLY  #@ Edition(2) = 14
```

A string ending in `.proto` in field 1 (a Java package, most likely, as in
the options strings of 0449), then the bytes `0x70 0x02`, which a
`FileDescriptorProto` reads as `edition` (field 14) set to 2. 0449 S1
can't see it: it holds no dependency index. The descriptor pool refuses
it: `Setting edition requires that syntax="editions"`.

The pool's rule is also what writers do: a descriptor sets `edition` only
in an editions file, with `syntax: "editions"`, and only to a released
edition (`EDITION_2023` and later). The test-only values, the legacy
markers (`EDITION_PROTO2`, `EDITION_PROTO3`) and `EDITION_MAX` are never
written there.

Field 14's tag is the byte `0x70`, `p`, common after a record by chance;
so, as for 0449's indices, a genuine record can be followed by bytes the
walk reads as an `edition` of its own.

`syntax` (field 12, tag `0x62`, `b`) has the same exposure. Measured
downstream: a Go binary storing its descriptors back to back, unframed,
one followed by a stylesheet, its first bytes `b` and `o` (111) read as a
`syntax` of 111 bytes of CSS. The copy differed from the other builds'
by that field alone, and made a straddle.

## Goals

- **G1.** A record whose `edition` no writer sets ends before it; if
  nothing but its name is left, it is refused, with a reason of its own.
- **G2.** The same for a `syntax` no writer sets.
- **G3.** Where such a cut contradicts a frame declaring the record
  longer, the contradiction is said, not settled silently.

## Non-goals

- **N1.** Checking `edition` against `syntax` beyond S1, or any other
  field's consistency: the scanner finds records, the pool judges them.
- **N2.** Naming the editions: the range of S1 is numeric, so a later
  edition is accepted without a change here.
- **N3.** An `edition` outside the enum as a cut of its own, beyond S1:
  enum values are open in protobuf, and a newer protoc may write an
  edition this scanner's copy doesn't know.
- **N4.** Recording the value cuts outside a frame: there the record's
  end is the walk's judgment, made silently at every other byte that
  ends a record; a cut is one more way of finding it. Only a cut that
  contradicts something declared, a frame, is evidence a reader may want
  to check (S5).
- **N5.** Cutting on a value that merely looks implausible (text running
  on): a heuristic. Only values the field's definition excludes cut.

## Specification

- **S1.** After 0449 S1's cut, the record's top-level fields are read. An
  `edition` (field 14) is implausible when the record holds no `syntax`
  (field 12) equal to `"editions"`, or when its value is outside
  `EDITION_2023` (1000) to `EDITION_UNSTABLE` (9999). The record ends
  before the first implausible `edition`.

  `implausible_edition_cut`, beside `dangling_dependency_cut`; both read
  the record's top-level fields through one walk, `top_level_fields`. The
  field numbers are the stock ones, also under `scan_explained_with`.

- **S2.** A cut, not a refusal (0449 S2's argument): the field may be the
  next bytes' rather than the record's. A record left with its name only
  is refused with `Reason::ImplausibleEdition`, listed by `scan_explained`
  as the other reasons are. `git-seriate-fdp extract` names it
  `implausible-edition`.

- **S3.** The corpus: `generate.py`'s decoys gain the found record's
  shape, a `.proto` string in field 1 followed by `0x70 0x02`, alone and
  between random bytes. Both are refused as `ImplausibleEdition`. The
  corpus has no editions record (its records are the well-known types);
  a genuine one followed by `0x70 0x02`, found exact, is a unit test.

- **S4. A `syntax` no writer sets.** Before S1's cut, which reads the
  `syntax` it leaves, the record's top-level fields are read: a `syntax`
  (field 12) is
  implausible when its value is none of `proto2`, `proto3`, `editions`,
  the values `descriptor.proto` documents and protoc's descriptor
  builder accepts ("Unrecognized syntax" otherwise). The record ends
  before the first implausible `syntax`; left with its name only, it is
  refused with `Reason::ImplausibleSyntax`, which `extract` names
  `implausible-syntax`. `implausible_syntax_cut`, beside the edition's.

- **S5. A cut inside a frame.** When S4 cuts a record a frame before it
  declares longer (a protobuf frame chained to the previous record, or a
  marshal header: 0496), the frame says the bytes are the record's and
  the value says no record holds them. The cut is kept, the record
  ending before the `syntax`, and the contradiction listed in
  `Explained::syntax_conflicts` (start, end, the frame's end, the frame).
  `extract` records it: `syntax-cut <path>: <origin>, inside a <frame>
  frame ending at <offset>`.

## Alternatives considered

**Refusing a record holding only `name` and `edition`**, the exact shape
found. A special case of S1, which also covers an implausible `edition`
after a genuine record's fields.

**Refusing the record instead of cutting it.** It would lose a genuine
record followed by a chance `p` and a small byte.

**Accepting only the editions the scan's schema names.** A record of a
later edition than the schema knows would be cut, for no gain: the range
already excludes every value writers don't set.

## Test plan

1. `implausible_edition_is_cut` — unit: a name and `edition: 2` refused
   `ImplausibleEdition`; an editions record (`syntax: "editions"`,
   `edition: 1001`) kept whole; the same followed by `0x70 0x02` cut
   before them; `edition: 1000` without `syntax` cut.
2. The corpus test (0448 S9): the new decoys refused, no target changed.
3. The public executables of 0448 S10 and the private corpus: the
   records found before and after, compared (Measured outcome).
4. S4, units: `a_syntax_no_writer_sets_ends_the_record_before_it` (a
   record followed by a stylesheet, cut exact, nothing recorded),
   `a_syntax_a_writer_sets_is_kept` (the three values),
   `a_name_left_alone_by_the_syntax_cut_is_refused`; the first and the
   third fail without the cut, as does item 5's first.
5. S5: `a_syntax_cut_inside_a_frame_is_a_conflict_reported` (fdp-scan);
   `a_syntax_cut_inside_a_frame_is_recorded` (git-seriate-fdp, the
   record entry).

## Measured outcome

2026-11-22, the scanner at HEAD (`326e09c1`) against this change, default
options, on 0449's inputs.

| Input | Records | Changed |
|---|---|---|
| googleapis | 7 771 | none |
| 24 public executables | 1 219 | 3 `buf` records (`field_mask.proto`, in three binaries) end 2 bytes earlier, before `pa` of Go's string data: now exact |
| Python protobuf 7.34.1's 110 `.pyc` | 24 | none |
| private corpus, clean layouts | | none |
| private corpus, noise layouts | 60 432 in all | 35 records end 2 to 5 bytes earlier, every one now exact: long 13–14 per layout → 1–3 |

No refusal was added or removed on any of them: the `ImplausibleEdition`
refusals are the corpus decoys'. Of the long records 0449 left (13–14
per noise layout, each framing declared fields), most were a random `p`
read as `edition`.

Speed not measured: the cut walks the top-level fields of each record
accepted, once, as the dependency cut does.

S4 and S5, 2026-11-28: the units of the test plan, items 4 and 5. The
downstream case of the Background (one copy, one straddle) to be
measured downstream.
