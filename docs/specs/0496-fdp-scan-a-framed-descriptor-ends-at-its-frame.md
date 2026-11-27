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
- **S4. `extract` records it:** `frame-cut <path>: <origin>, <frame>
  frame, read on to <offset>`, in the snapshot's record, `<frame>`
  `protobuf` or `marshal`.
- **S5. A Python marshal frame,** *added 2026-11-27*: a compiled module
  (`*_pb2.pyc`) stores its file's descriptor as a marshal bytes object,
  the type `s` (`0x73`, or `0xf3` with marshal's reference flag) and a
  4-byte little-endian length right before it; the next constant is
  usually the module's name as a marshal short string, `z` (`0x7a`), which
  is the tag of field 15, `option_dependency`, so the walk took the
  module's name for one dependency more. A marshal header is a frame,
  trusted when inside and whole (S2), with no chaining: its length is 4
  bytes, and stray bytes before a raw descriptor don't declare one landing
  on one of its field boundaries.

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
`frame_at_reads_a_tag_and_a_length`,
`a_marshal_frame_ends_a_compiled_modules_descriptor` (with and without
the reference flag), `a_marshal_length_off_the_records_boundaries_is_no_frame`.
git-seriate-fdp: `a_framed_descriptor_ends_at_its_frame`, on two stock
descriptors.

## Measured outcome

Measured downstream, on two database clients embedding schema bundles,
a year apart, each extracted as its own input:

| | the newer client | the older one |
|---|---|---|
| ranges | 15,318, the same starts as before the fix | 12,728 |
| ranges changed | 11, exactly the over-read ones, now ending at their frames | |
| `frame-cut` entries | 11 | 10 |
| copies still read on | 0 | 0 |

The newer client's 15,307 other ranges kept their ends, the 2,542 framed
ones included. It has ten fewer paths: the closing file of nine bundles,
read on, had nine copies differing only by what followed them; cut, they
are the same bytes, and merge. Every record that had been read on sat in
a chain, so N3's limit didn't apply. The order check's tests unchanged.

S5, on the compiled modules of Python protobuf's own package (7.36.2,
CPython 3.13): 12 of its 13 `*_pb2` modules were read on past their
descriptor, each by exactly its own name (25 bytes for `any_pb2`: `z`, a
length, `google.protobuf.any_pb2`), and now end at their marshal frame;
the thirteenth, `descriptor_pb2`, is followed by a tuple, and unchanged.
Downstream, the same over-read was in 1,095 of a history's 1,106 compiled
modules, masked by a canonicalization rule dropping an
`option_dependency` equal to the module's name.
