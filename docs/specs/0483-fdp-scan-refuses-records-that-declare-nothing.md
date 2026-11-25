<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0483 — fdp-scan refuses records that declare nothing

Status: implemented
Implemented in: 2026-11-25
App: fdp-scan
Refs: docs/specs/0448-fdp-scan-undeclared-fields-and-rejections.md (S5,
        plausible names; S6, a name-only record must stand alone; S7, the
        reasons `Explained` reports),
      docs/specs/0313-a-record-ends-at-its-last-clean-boundary.md (S3, a
        record kept as its clean prefix),
      docs/specs/0480-fdp-scan-inflates-zip-and-gzip-members.md (the
        inflating walk that exposed the case)

## Background

A downstream user measured `protoscan --inflate` on four packages of
144 MB to 4 GB (2026-11-25). It found no descriptor that the scan in the
clear misses, and 64 false ones, all in 7 gzip members: compiled HTML
templates whose bundles a binary embeds. Each holds one record per
`.proto` file its data types come from:

- field 1, the `.proto` path; field 2, its package; field 3, the Java
  outer class (`com.example.protos.pkg.Runtime`); field 7, short strings
  repeated (message names).

The scanner reads one as a `FileDescriptorProto` cut before field 7,
which doesn't parse as `FieldDescriptorProto` (0313 S3), and reports:

```
name: "x/y/proto/runtime.proto"
package: "y"
dependency: "com.example.protos.y.Runtime"
```

Each such name is a real descriptor's, found in the clear in the same
package at 1 332 bytes; the false one is 100. Extracted, it is stored as
`<name>~2.pb` beside the real one: a false version of the file, which
seriation reads as a change in history. Nothing makes the shape specific
to gzip members: in the clear, it would be accepted the same way.

## Goals

- **G1. These records are refused,** with a reason `Explained` reports
  (0448 S7).
- **G2. No real descriptor is refused.** Checked before deciding
  (2026-11-25) on 27 234 real descriptors: the 1 329 and 1 296 distinct
  ones of seriate-tuto's two corpora, the 8 125 of the protoback harness
  corpus's 248 descriptor sets (googleapis, the well-known types, the
  `imports` cases), and the 16 484 of a downstream user's history, a
  corpus unlike those (proto2, internal paths, several years of
  builds). S1 below holds on all of them; the rule first drafted beside
  it did not (Alternatives).

## Non-goals

- **N1. No check of what a dependency names:** whether the file it names
  is present is the consumer's concern.
- **N2. No rule on a dependency's form.** Real descriptors import files
  no `.proto` source wrote: in the downstream history, 15 real files
  import 43 that don't end in `.proto`, 42 schema files ending `.sdl`
  (a database's schema language, compiled to descriptors by its own
  compiler) and one `.protodevel`, an old extension for `.proto` files.
  protoc never sees those files, so nothing ever required otherwise.
- **N3. No change to the name rule** (0448 S5), which refuses a
  descriptor *named* `.sdl` or `.protodevel` when it is scanned. That is
  a gap in recall, not a false positive: those two are the extensions
  known to be real, should the rule ever widen. Not measured; a later
  spec.

## Specification

- **S1. A record that declares nothing must stand alone**
  (`Reason::DeclaresNothing`, `declares-nothing`): one holding only
  `name`, `package`, `dependency`, `public_dependency` and
  `weak_dependency` (fields 1, 2, 3, 10, 11), no message, enum, service,
  extension, option, `syntax` or `edition`, is accepted only if it
  stands alone (0448 S6: it ends at the buffer's end, at an
  undeclarable tag or at a second `name`), and was not shortened by a
  cut of 0449 S1 or 0456 S1. This extends 0448 S6's rule for a record
  holding a name only, which `NameOnly` keeps reporting. A real file that
  only imports exists: the downstream history holds 5, which re-export
  their imports; such a file is never followed by bytes that cut it.
- **S2. Order:** after the cuts of 0449 S1 and 0456 S1: a record those
  cut to a name alone keeps their reason.
- **S3. Everywhere:** in `scan`, `scan_explained`, and every buffer of
  `scan_inflating`. Its records in the clear are refused as well as in
  members.

## Alternatives considered

- **Dependencies held to the name rule** (0448 S5: ends in `.proto`, a
  plausible path), refusing the record whole. Drafted as this spec's S1,
  and dropped: it refuses all 64 false records, but also 15 real files of
  the downstream history (N2), the reverse of the false history it is
  meant to prevent. S1 alone refuses all 64.
- **A dependency held to the shape of a path only** (no empty, `.` or
  `..` component, not absolute, the name rule's characters), its
  extension unchecked. The false dependency, `com.example.protos.y.Runtime`,
  has a plausible shape: it would refuse none of the 64, and add a rule
  that catches nothing known.
- **"No `/` and not ending in `.proto`."** It refuses all 64 and none of
  the 15, every one of the 43 having a directory; but it is a guess on
  top of S1, which needs none.
- **Refusing only in members.** The shape is no less false in the clear;
  the walk merely met it first in gzip members.

## Test plan

1. `a_template_record_is_refused`: the shape above, followed by field 7's
   strings: refused, `declares-nothing`.
2. `a_cut_record_that_declares_nothing_is_refused`: `name` and `package`
   followed by bytes that cut the walk: refused, `declares-nothing`.
3. `an_imports_only_file_standing_alone_is_found`: `name`, `package` and
   two dependencies at the end of the buffer, and before a NUL: found.
4. `a_dependency_need_not_end_in_dot_proto`: a record importing a `.sdl`
   and a `.protodevel` file is found (N2).
5. The corpus (`fdp-scan/tests/corpus`) and the existing tests unchanged.
6. seriate-tuto's corpora and the harness corpus: the descriptors found
   unchanged (G2's check, redone on the implementation).

## Measured outcome

2026-11-25.

- **Real layouts unchanged:** protoscan from `main` (`e78842bb`, without
  0483) and with 0483, `--origin` with and without `--inflate`, on the
  four files of 0480's test 11 and the 248 descriptor sets of the harness
  corpus: 506 runs, identical output.
- **The public corpus** (`fdp-scan/tests/corpus`) scores as before.
- **Seven unit tests changed fixture, not intent.** They follow the
  smallest reported descriptor, a name and a package, with bytes that cut
  it, to test a recovery or a cut: under S1 that record is refused. They
  now use one that declares a message (`make_declaring_fdp`), or `syntax`
  where the test's schema declares no `message_type`; `make_fdp` keeps
  its uses where the record stands alone.
- **Not measured here:** the 64 template records and the 5 imports-only
  files, which are the downstream user's. Tests 1 and 2 rebuild the
  records' shape; whether the 5 files stand alone where they occur, the
  downstream user will check on their packages, before and after 0483.
