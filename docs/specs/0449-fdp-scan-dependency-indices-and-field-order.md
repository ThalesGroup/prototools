<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0449 — fdp-scan: dependency indices checked, field order opt-in

Status: implemented
Implemented in: 2026-11-22
App: fdp-scan | prototext-graph
Refs: docs/specs/0448-fdp-scan-undeclared-fields-and-rejections.md (the
        `Reason` codes, extended here; its N2 "ambiguous and merged
        records" and N3 "a field-order option, later", answered here),
      docs/specs/0313-a-record-ends-at-its-last-clean-boundary.md (the
        clean-boundary snapshots a nested out-of-order field falls back
        to),
      docs/specs/0238-an-extension-range-is-what-makes-an-unknown-field-innocent.md
        (S12, the termination rules, which gain one)

## Background

**Options strings in Python bytecode read as descriptors.** Python's
generated `_pb2` modules store each file's options as a separately
serialized `FileOptions`, and a `.pyc` holds that string among marshal
bytes. A `FileOptions` with `java_package` (field 1) and
`java_multiple_files = true` (field 10, varint 1) reads, field by field,
as a `FileDescriptorProto` with `name: "com.example.foo.proto"` and
`public_dependency: 1`. It is clean, and it declares more than a name, so
0448 S6 does not refuse it. A downstream user found 20 among the 1 062
descriptors extracted from the `.pyc` files of three packages.

No genuine descriptor carries such an index. protoc's descriptor builder
rejects any `public_dependency` or `weak_dependency` index that is not
below the number of `dependency` entries ("Invalid public dependency
index.", "Invalid weak dependency index.": protobuf 34.1,
`src/google/protobuf/descriptor.cc`, `DescriptorBuilder::BuildFileImpl`).

**Random bytes after a record can pass as its fields** (0448 N2). With
random neighbors, 74–88 records per layout of a 7 554-descriptor private
corpus came back *ambiguous*: longer by bytes that happen to frame
declared top-level fields. A further 3–8 came back *merged*: such a field
claims a length that covers the next record. In real executables the same
happens: in `buf`, five records ran on into Go's adjacent string data,
whose `R` (`0x52`) frames a packed `public_dependency` of ASCII.

Order also tells such bytes apart. protoc's C++ writer emits every
message's fields in increasing number order, extensions interleaved: 0
exceptions among the 439 475 messages of googleapis and the 544 177 of the
private corpus, at every depth. Go writers do not keep this order (0448
N3), so an order check can only be an option.

## Goals

- **G1.** No reported record carries a dependency index with no matching
  `dependency`.
- **G2.** A caller that knows its input comes from protoc can require
  increasing field numbers.
- **G3.** A record the order check ends earlier than the scan without it
  is reported, so that a caller who turned it on for the wrong input sees
  what it lost.
- **G4.** Without the option, `Policy::Score` and every scan boundary
  that G1 doesn't shorten are as before.

## Non-goals

- **N1.** Order checking on by default. Go's descriptors would be cut
  short: reported (S7), but their fields still lost.
- **N2.** Detecting a protoc-written input automatically. The caller knows
  where its bytes came from; the scanner sees a buffer.
- **N3.** Other validity rules from `BuildFileImpl` (duplicate names,
  unresolved types). They need a pool of files, not one record.
- **N4.** Exposing either in the Python extension or the `protoscan` CLI
  (as 0448 N4).

## Specification

### The dependency cut (fdp-scan)

- **S1.** After a candidate is accepted by 0448's rules, its top-level
  fields are read: `dependency` entries (field 3) counted,
  `public_dependency` (10) and `weak_dependency` (11) indices collected,
  packed or not. The record ends before the first field holding an index
  not below the count of `dependency` entries before it, repeated until
  none dangles (a cut can only lower the count). A negative index is a
  ten-byte varint, past any count. A record left with its name only is
  refused, with the new reason `Reason::DanglingDependency`.

  `dangling_dependency_cut`, beside `declares_more_than_a_name`: knowledge
  about `FileDescriptorProto`, not about protobuf, so not in the walk. The
  field numbers are the stock ones, also under `scan_explained_with`.

- **S2.** A cut, not a refusal: protoc never writes a dangling index, so
  what the cut removes is never the record's own. Refusing would lose a
  genuine record to its neighbors; it did, on the public corpus, where a
  random `0x58` (a `weak_dependency` tag) followed two records.

### The order option (prototext-graph, `Policy::Scan` only)

- **S3.** `ScoringOpts::increasing_field_numbers` (default `false`). Under
  `Policy::Scan`, within each message, a field number lower than the
  previous field's is out of order. Equal numbers are not: a repeated
  field's elements repeat their number. A repeated field split around
  another (3, 4, 3) is out of order. An end-group tag and an out-of-range
  number take no part. Checked after 0238 S12, so a tag both rules end on
  keeps S12's reason. Ignored under `Policy::Score`.

- **S4.** At depth 0, an out-of-order field ends the record before it:
  `Ending::OutOfOrder`. 0448 S6 does not count it as standing alone.

- **S5.** Nested, an out-of-order field vetoes the frame, and the record
  falls back to its last clean boundary (`Ending::Abandoned`), before the
  top-level field that holds it: the parent's length bounds the nested
  message.

- **S6.** `EntryScore::order_break: Option<usize>`: the offset, from the
  start of the scored buffer, of the out-of-order tag that ended the
  record. At depth 0 it equals `termination`; nested, it lies past it. A
  nested frame walks a subslice of the buffer, so its offset is computed
  from the slices' addresses (`WalkState::offset_in_pb`).

### The scanner's options (fdp-scan)

- **S7.** `ScanOptions { increasing_field_numbers }` (`Default`: off),
  `scan_explained_options(data, &options)` with the stock schema and
  `scan_explained_with_options(data, graph, root, &options)` with the
  caller's. `scan_explained` and `scan_explained_with` are them with the
  default options, and keep their signatures.

  `Explained::order_cuts: Vec<OrderCut { start, end, at }>`: the records in
  `found` that the option ended earlier than the scan without it ends
  them, `at` the out-of-order tag. A record whose `order_break` is set is
  scored a second time without the option, and reported only if that
  record is longer: the order rule otherwise ends most records exactly
  where they end anyway, at random bytes that read as a lower field (over
  1 000 per noise layout of the private corpus). A cut is not a refusal:
  the record stays in `found`.

### The corpus (fdp-scan tests)

- **S8.** `generate.py`'s decoys gain the options string, serialized by
  Python's protobuf, alone and between random bytes. Both are refused as
  `DanglingDependency`.

- **S9.** The noise layouts are also scored with the option. Every record
  it shortens is in `order_cuts`. With it, no record is ambiguous, long,
  merged or spurious; the name-only record is missed as without it; the
  `file-option-5` mutations are short. Those are the generator's doing:
  Python serializes an unknown option after the known ones, where protoc,
  which knows it, writes it in order.

## Alternatives considered

**Refusing a record with a dangling index.** Built first. It refused two
genuine records of the public corpus whose neighbors began with a
`weak_dependency` tag (S2).

**Refusing only a `public_dependency` with no `dependency`,** the exact
shape of the `.pyc` false positive. A special case of S1, which costs
nothing more.

**Order checking at the top level only.** No safer: Go writers put some
top-level fields out of order too. And a merged record's junk field
covers the next record, which only the nested check reads.

**An order check in fdp-scan, on the recovered bytes.** It could refuse
an out-of-order record, but not end it at the right place: the boundary
is the walk's.

**Reporting every record the order rule ended** (`order_break` as the
walk sets it). Built first: over 1 000 reports per noise layout, almost
all at the record's true end. A report the caller must sift is no report.

## Test plan

prototext-graph (`score/tests.rs`):
`test_increasing_field_numbers_ends_a_record_before_a_lower_field`,
`test_increasing_field_numbers_abandons_at_a_nested_lower_field`,
`test_increasing_field_numbers_allows_repeated_numbers`,
`test_order_option_is_ignored_under_score`.

fdp-scan (`lib.rs`): `test_scan_refuses_a_dangling_public_dependency`,
`test_scan_accepts_a_public_dependency_with_its_import`,
`test_scan_cuts_a_record_before_a_dangling_weak_dependency`,
`test_scan_cuts_a_junk_index_after_a_record`,
`test_scan_reports_a_record_the_order_check_cut`,
`test_scan_reports_a_nested_order_cut_past_the_end`; (`tests/corpus.rs`)
`the_options_decoy_is_a_dangling_dependency`,
`the_order_option_on_random_neighbors`.

## Measured outcome

2026-11-22, the scanner at HEAD (`c7d69338`) against this change, with the
option off and on.

| Input | Off | On |
|---|---|---|
| googleapis, 7 771 records | identical; no refusal | no cut |
| 24 public executables, 1 219 records | 5 `buf` records that ran into Go's string data end exactly; no refusal | 128 cuts, all Go: 127 of `gh` 2.83.2's 255 records, 1 of `gh` 2.102's; none in a protoc-written record |
| Python protobuf 7.34.1's 110 `.pyc` | identical | no cut |
| private corpus, clean layouts | unchanged: 7 553 exact, 1 short (an undeclared field 171) | no cut |
| private corpus, noise layouts | ambiguous 74–88 → 13–14, merged 3–8 → 0, spurious 0–1 → 0 | ambiguous 12–13; 1 cut per layout |

The dependency cut does most of what the order option was for: the merged
records are gone without it. The ambiguous records left frame declared
fields in increasing order, which neither rule can see.

**Speed and memory:** within noise (hyperfine, 30 runs each, the host
idle). googleapis.desc: 341 ± 41 ms before, 334 ± 61 ms after, 330 ± 69
ms with the option; `gh` 2.83.2: 147 ± 11, 146 ± 11, 141 ± 22 ms. Peak RSS
23.7 MB and 56.4 MB, unchanged to within 0.2%. The second walk of S7 runs
on the records the order rule ends only.
