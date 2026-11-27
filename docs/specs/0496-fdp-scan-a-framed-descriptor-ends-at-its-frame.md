<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0496 — fdp-scan: a framed descriptor ends at its frame

Status: implemented
Implemented in: 2026-11-27
App: fdp-scan, git-seriate-fdp
Refs: docs/specs/0239-the-schema-says-where-a-descriptor-ends.md
        (the walk handed the rest of the buffer),
      docs/specs/0449-fdp-scan-dependency-indices-and-field-order.md
        (the order check, the other place a record is ended early),
      docs/specs/0494-extract-records-the-evidence.md (the origins that
        measured it)

## Background

The scanner finds a `FileDescriptorProto` by its `name` field and reads
fields from there until one can't belong to a descriptor: handed the rest
of the buffer, the walk stops at the next record's `name`, a second
occurrence of a singular field. That delimits records of a set from one
another. It doesn't delimit the last one: a descriptor embedded in a
larger message is followed by that message's other fields, and they can
parse as valid fields of a `FileDescriptorProto`. Parsing cleanly doesn't
mark a record's end.

Measured downstream, with 0494's origins, on a database client embedding
schema bundles, each a serialized message: records framed as its field 1,
back to back, then the message's field 3, a list of source file names,
which reads as `dependency`. 2,553 ranges had a frame right before them;
in 2,542 the declared length equalled the range; the 11 others were each
bundle's last record, longer by exactly the enclosing message's field-3
strings (34 to 1,283 bytes). The over-read made a copy's content, and its
digest, depend on what follows it.

## Goals

- **G1.** A record framed in a set ends at its frame's declared length,
  not where the walk reads on to.
- **G2.** Never cut a genuine descriptor: a frame is trusted only when it
  holds up.
- **G3.** Nothing changes for records without a frame, which is how
  generated code stores them (the bytes and their size apart).
- **G4.** The cut is reported, as the order check's cuts are.

## Non-goals

- **N1.** Pruning after extraction (dependencies removed by their names):
  one producer's naming made a rule, and missing the next producer's
  field numbers.
- **N2.** Knowing the enclosing message's schema: only its frames are read.
- **N3.** A set of one record: its frame is chained to nothing (S2), so a
  single framed record read on past its end isn't cut. Where its producer
  writes fields in order, the order check (spec 0449) cuts it.

## Specification

- **S1. The frame:** from where the previous record found ended, a tag of
  wire type 2 (length-delimited) and a varint length, ending exactly where
  this record starts. Read forwards from there: read backwards from the
  record, a varint's continuation bytes can't be told from the bytes
  before it.
- **S2. Trusted when it holds up three ways:**
  - **chained:** it starts where the previous record ended (S1), as
    records of a set do; two stray bytes before a raw descriptor that read
    as a frame aren't chained to anything;
  - **inside:** its declared length ends before the walk's end;
  - **whole:** the prefix of exactly that length is accepted as a record
    on its own, ending there.
- **S3. The record ends at its frame,** in `Explained::found`, and the cut
  is listed in `Explained::frame_cuts`: its start, its end, and where the
  walk would have read on to. A record its frame cut reports no order cut.
- **S4. `extract` records it:** `frame-cut <path>: <origin>, read on to
  <offset>`, in the snapshot's record.

## Alternatives considered

- **Trusting any frame before a record:** over hundreds of thousands of
  descriptors, a tag and a length before a raw descriptor that land on one
  of its field boundaries would occur, and cut genuine records.
- **Cutting at a gap, or at a field number going down:** a set's records
  follow one another with no gap, and the gap after a set measured 115
  bytes to 2.2 KB, so gaps separate nothing; field order is producer
  knowledge, opt-in already (spec 0449).

## Test plan

fdp-scan: `a_chained_frame_ends_the_last_record_of_a_set`,
`a_set_of_one_record_is_not_cut`,
`bytes_that_read_as_a_frame_before_a_raw_record_are_no_frame`,
`a_set_its_frames_agree_with_reports_no_cut`,
`frame_at_reads_a_tag_and_a_length`. git-seriate-fdp:
`a_framed_descriptor_ends_at_its_frame`, on two stock descriptors.

## Measured outcome

The tests pass; the order check's tests unchanged. To measure on the
downstream's client: the 11 over-read records end at their frames, the
2,542 others unchanged.
