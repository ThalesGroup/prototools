<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0456 — fdp-scan: an edition no writer sets

Status: implemented
Implemented in: 2026-11-22
App: fdp-scan
Refs: docs/specs/0448-fdp-scan-undeclared-fields-and-rejections.md (the
        accept rule, `NameOnly`, `Reason`),
      docs/specs/0449-fdp-scan-dependency-indices-and-field-order.md (S1:
        the dependency cut, which this follows)

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

## Goals

- **G1.** A record whose `edition` no writer sets ends before it; if
  nothing but its name is left, it is refused, with a reason of its own.

## Non-goals

- **N1.** Checking `edition` against `syntax` beyond S1, or any other
  field's consistency: the scanner finds records, the pool judges them.
- **N2.** Naming the editions: the range of S1 is numeric, so a later
  edition is accepted without a change here.

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
