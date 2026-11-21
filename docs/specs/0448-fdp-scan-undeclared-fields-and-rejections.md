<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0448 — fdp-scan: undeclared fields, the caller's schema, reported rejections

Status: implemented
Implemented in: 2026-11-21
App: fdp-scan | prototext-graph
Refs: docs/specs/0239-the-schema-says-where-a-descriptor-ends.md (the
        accept rule on defect counters; its S4 "newer protoc" false
        negative, narrowed here; its S7 "a rejected candidate is
        invisible", answered by a second entry point),
      docs/specs/0313-a-record-ends-at-its-last-clean-boundary.md (the
        clean-boundary snapshots; its S4 name-only rule, narrowed here),
      docs/specs/0238-an-extension-range-is-what-makes-an-unknown-field-innocent.md
        (S12, the termination rules, now reported; S15,
        `Verdict::Extension`, generalized to nested undeclared fields)

## Background

A downstream user extracting descriptors from executables measured
`fdp-scan` against an older scanner of their own, on a large private
corpus (7 554 descriptors) and on a public corpus they built for the
purpose: the well-known types Python's protobuf carries, as they are and
mutated, in several layouts (records between NULs, back to back, framed as
a `FileDescriptorSet`, between random or hand-built bytes), plus decoys.

**Undeclared fields cut records short.** A descriptor carrying a field the
stock `descriptor.proto` does not declare, outside its extension ranges,
comes back ending before the top-level field that holds it: nested, the
field is an `unknowns`, so the next depth-0 boundary is not clean (spec
0313 S3). On the private corpus, written against a `descriptor.proto` with
more fields than the stock one, 2 361 of 7 554 (31%) were cut, every one by
a nested field (`FileOptions`, `MethodOptions`, `MessageOptions`); the
leftovers yielded 34 spurious records.

**`is_plausible_path` accepts code fragments.** Every `libprotoc` has 5
false positives: Java codegen templates such as
`\n$deprecation$private com.google.protobuf.MapFieldBuilder<…`, field 1,
length 36 (the `$`), a "name" ending in `.proto` once cut.

**A name-only record is never found** (spec 0313 S4), although protoc
writes exactly that for an empty proto2 file with no package.

**A refused candidate is invisible** (spec 0239 S7): a caller that reports
refused candidates beside what it extracts has to re-implement the rules.

## Goals

- **G1.** An undeclared field nested in a declared one never cuts a record.
- **G2.** A caller holding the `descriptor.proto` its descriptors were
  written against can scan with it.
- **G3.** A name is plausible only if made of the characters real `.proto`
  paths use.
- **G4.** A name-only record is found when it stands alone.
- **G5.** A second entry point returns what was refused and why; `scan`
  keeps its signature.
- **G6.** The public corpus is `fdp-scan`'s regression suite.

## Non-goals

- **N1.** Keeping an undeclared *top-level* field. See Alternatives: built,
  measured, and rejected.
- **N2.** Telling a record from declared fields that follow it by
  accident. Random bytes after a record that frame valid declared
  top-level fields are indistinguishable from the record's own; the suite
  counts them as *ambiguous*. Such bytes do reach the reported record as
  false declared data (a `weak_dependency` index, say), and when they run
  into the next record that record is *merged* and its inner anchors can
  surface as spurious records. A plausibility rule is the follow-up (N3).
- **N3.** A field-order option. protoc's C++ writer emits every message's
  fields in increasing number order (0 exceptions among 439 475 messages
  of googleapis and 544 177 of the private corpus, at every depth), which
  would refuse most of N2's junk; but Go writers do not (`buf` puts some
  top-level fields out of order, the xds/envoy descriptors in `gh` put
  custom options first: 133 of 1 219 records in the public executables
  measured). Hence an option for corpora known to come from protoc, later.
- **N4.** Exposing rejections or the caller's schema in the Python
  extension or the `protoscan` CLI. The Rust entry points first.
- **N5.** Any change to `Policy::Score`, the walk protolens and prototext
  drive. Every rule here is gated on `Policy::Scan` (spec 0313 N3).

## Specification

### The walk (prototext-graph, `Policy::Scan` only)

- **S1.** A field number a nested state does not declare, outside its
  extension ranges, gets the verdict `Extension`: read by its wire type,
  neither evidence for nor against the candidate. `EntryScore::undeclared`
  counts these; it is not an anomaly for spec 0313 S2's cleanliness, it is
  restored with a snapshot, and it is not in spec 0239 S4's accept rule.
  The parent's length prefix bounds the field, so the record's end cannot
  move. A declared field with the wrong wire type stays a defect: what is
  relaxed is the schema's coverage, not the wire format.

- **S2.** At depth 0 an undeclared field number still ends the record
  (spec 0238 S12 rule 1). Unchanged; N1 says why.

- **S3.** `EntryScore::ending` reports how the record ended: the end of
  the buffer, an out-of-range field number (a NUL is one), an undeclared
  field, a repeated singular (the next record's first field), or
  `Abandoned` to its last clean boundary (spec 0313 S3). Always
  `EndOfBuffer` under `Policy::Score`.

### The scanner (fdp-scan)

- **S4.** `scan_explained_with(data, graph, root)` scans against the
  caller's schema: `graph` built from their `descriptor.proto` (`reproto
  --schema-db-out --emit-extension-ranges`), `root` its
  `FileDescriptorProto`. The fields the stock schema lacks are then
  declared: checked by type rather than read blind, and a top-level one no
  longer ends the record. `scan_explained` is it with the embedded WKT
  graph and `google.protobuf.FileDescriptorProto`; S1 still applies, to
  whatever the given schema lacks.

- **S5.** `is_plausible_path`: as before (ends in `.proto`, not absolute,
  no empty, `.` or `..` component), and every component made of
  `[A-Za-z0-9_.+-]` only. Every path in googleapis (7 817 files) and in the
  private corpus fits, even `[A-Za-z0-9_./-]`. protoc accepts any name
  (`café.proto`, `a b.proto`, `x$y.proto` all compile): the rule is about
  which names are worth believing. A real file outside the set is reported
  by S7, not lost silently.

- **S6.** A record holding only `name` is accepted when it stands alone:
  its `ending` (S3) is the end of the buffer, an out-of-range field number,
  or a repeated singular. Not when abandoned, nor at an undeclared field.
  The `.proto` Java package names spec 0313 S4 exists for are field 1 of
  `FileOptions`, followed by more `FileOptions` fields, which either fail
  to read as a descriptor's or are undeclared in one. Read from the walk
  rather than from the bytes after the record, so the judgment lives once.

- **S7.** `scan_explained(data) -> Explained`: the records `scan` returns
  (`scan` is its `found`), plus the refused candidates in discovery order,
  each with its start, the name it claims and a reason: `ImplausibleName`
  (S5), `NameOnly` (S6), or `Defect` (vetoed, `unknowns`, `mismatches`, or
  nothing clean at all). Only anchors whose name ends in `.proto` are
  listed: every `0x0A` byte is an anchor.

### The corpus (fdp-scan tests)

- **S8.** The user's generator and its output are imported under
  `fdp-scan/tests/corpus/`: `generate.py`, and per layout a `.bin` and a
  `.truth.json` (`[name, start, end]` per true record), with `corpus.json`
  naming the protobuf version. Committed as generated, so that both sides
  score the same bytes. A Nix check regenerates the corpus with the pinned
  Python protobuf and fails on any difference.

- **S9.** An integration test, `fdp-scan/tests/corpus.rs`, scores `scan` on
  each layout against its truth. A true record is *exact* (found at its
  start, with its end), *ambiguous* (found at its start, ending late only by
  bytes framing declared top-level fields of `FileDescriptorProto`, never
  field 1), *short*, *long*, *merged* (inside a range found from an earlier
  start), or *missed*. Targets:

  1. records between NULs, back to back, as a descriptor set, and between
     hand-built neighbors: every record exact, except those carrying an
     undeclared top-level field (`#top-level-171`), which end exactly
     before it (S2);
  2. a record followed by an undeclared top-level field and random bytes
     (`trailer`): exact, but for the name-only record (4);
  3. between random bytes: exact or ambiguous;
  4. the name-only record: exact wherever it stands alone, missed before
     random bytes or an undeclared field (S6);
  5. no spurious record in any layout, decoys included.

- **S10.** The public executables the user listed (`buf`, `gh`, protobuf
  32.1 and 34.1), and others: what `scan` found before this spec, minus the
  libprotoc templates. Too large to commit; checked by hand (Measured
  outcome).

- **S11.** Spec 0239 and 0313's googleapis guarantees hold: every record
  found, each boundary matching the `FileDescriptorSet` framing, each
  record clean at its own end.

## Alternatives considered

**Keeping undeclared top-level fields, provisionally.** Built. An entry
reading one became *provisional*: no snapshot after it, kept only if the
record then ended credibly (a NUL, the next record's first field with its
declared wire type, the end of the buffer), with field-number order
enforced after it. On the public corpus it nearly worked. On the private
corpus, with random bytes between records, 1 124 to 2 976 records of 7 554
came back merged (2 to 5 before): a junk undeclared length-delimited field
can claim any length (`ba a5 24 | c8 bc 75`, 1.9 MB), its payload is
opaque, and it swallowed up to 879 records before anything looked wrong.
What it was for, the one top-level undeclared field in the private corpus,
turned out to be a 71-byte field made of a neighboring blob that the user's older
scanner had run on into. A caller with fields the stock schema lacks
passes their schema instead (S4), which checks those fields rather than
trusting their framing.

**At most one undeclared field per record**, to curb the above. A guess
about random bytes, not a property of records; superseded by the order
rule, then by dropping the whole provisional design.

**Scoring against a caller-supplied schema *instead* of S1.** Fixes a
caller who has the schema, and nobody else: the cut still happens to any
binary whose `descriptor.proto` nobody has. Hence both.

**Banning only space and `$` in names.** The two characters the libprotoc
templates showed. A positive set covers the next template too and costs
nothing on any corpus measured.

**Deciding "stands alone" from the byte after the record** (end of buffer,
`00`, `0A`). Built first; it repeated the walk's own termination judgment
by hand. S3 reports it instead.

**Generating the layouts in Rust from the bare records** (20 KB instead of
the corpus's size). The user's harness and ours would then score
different bytes, and the shared numbers are what makes a regression on
either side visible to the other.

## Test plan

1. `corpus_layouts_meet_their_targets` (S9) — the acceptance test.
2. `fdp-scan-corpus-check` (S8) — the Nix check.
3. `test_scan_undeclared_nested_field_is_kept`,
   `test_scan_declared_field_with_wrong_wire_type_is_still_a_defect`,
   `test_scan_reports_how_a_record_ended` (prototext-graph; S1, S3), and
   spec 0238's test 9, still ending at an undeclared top-level field (S2).
4. `an_undeclared_nested_field_is_kept`,
   `an_undeclared_top_level_field_ends_the_record` (S1, S2).
5. `a_callers_schema_declares_what_the_stock_one_lacks`,
   `a_callers_schema_checks_the_fields_it_adds` (S4), on a vendor schema
   built in the test.
6. `names_outside_the_set_are_implausible`,
   `a_codegen_template_is_reported_as_an_implausible_name` (S5, S7).
7. `a_standalone_name_only_record_is_found`,
   `a_java_package_name_is_not_a_descriptor` (S6).
8. `a_defect_is_reported`, `scan_is_scan_explained_found` (S7).
9. The `#[ignore]`d googleapis tests (S11), with `PROTOSCAN_CORPUS_DESC`.

Speed and peak memory are compared with the code before this spec on
`googleapis.desc` and a large executable (Measured outcome).

## Measured outcome

On the same corpora (scratch harness, `fdp-scan` before and after this
spec):

| corpus, layouts | before: exact / short / spurious | after |
|---|---|---|
| private, between NULs, back to back, descriptor set, crafted | 5 192 / 2 361 / 34 | 7 553 / 1 / 0 |
| private, random neighbors (3 seeds) | 5 133–5 141 / 2 359–2 361 / 35–36 | 7 460–7 476 / 1 / 0–1 |
| public, clean layouts | 25 / 26 / 0 | 40 / 12 / 0 |

The one private record still short is the junk field 171 above; the 12
public ones are the `#top-level-171` mutations (S2). With the private
`descriptor.proto` as the caller's schema (S4), the counts are the same
within one record. With random neighbors the private corpus has 74–88
ambiguous and 3–8 merged records (before: 49–57 and 2–5), and one spurious
record caused by a merge (N2).

On 24 public executables (`buf`, `gh` ×2, protobuf 32.1 and 34.1,
`grpcurl`, `protoc-gen-go`, containerd ×4, Python protobuf's extension):
1 219 records, every boundary as before; the only records dropped are the
10 libprotoc templates.

Speed and memory, one release binary linking either version, idle
12-core host, 15 runs after 3 warm-up (`hyperfine`), peak RSS over 5 runs
(`time -f %M`):

| input | before | after | peak RSS, before / after |
|---|---|---|---|
| `googleapis.desc`, 21.3 MB, 7 771 records | 171.4 ± 2.1 ms | 168.4 ± 13.8 ms | 25.3 / 25.2 MB |
| `gh` 2.83.2, 55.0 MB, 255 records | 80.6 ± 9.3 ms | 75.0 ± 15.6 ms | 57.9 / 57.9 MB |

No difference beyond noise: the new rules act only on candidates the walk
already reads, and the memory is the input, read whole, plus 3–4 MB. Both
versions report the same 7 771 records on `googleapis.desc`.
