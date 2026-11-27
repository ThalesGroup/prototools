<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0498 — fdp-scan reads containers as such

Status: draft
App: fdp-scan, git-seriate-fdp
Refs: docs/specs/0480-fdp-scan-inflates-zip-and-gzip-members.md
        (the members read today, one by one, and their origins),
      docs/specs/0494-extract-records-the-evidence.md
        (`buffers`: each buffer's type and a member's entry time),
      docs/specs/0495-extract-makes-atomic-snapshots.md
        (N5, the container split this enables; S5, a part's build time)

## Background

fdp-scan reads containers member by member (spec 0480): it finds a zip
member by its local header's signature (`PK\x03\x04`) and a gzip member by
its magic (`1f 8b 08`), anywhere in a buffer, inflates each, and scans it
in turn. That finds descriptors inside, and names where: `zip@<local
header>!<member>`. It doesn't know containers as such:

- **no archive:** a zip's members are found one by one; which archive each
  belongs to, its bounds, and what else it holds are not known, so a jar
  can't be told from a zip of descriptor files, or one archive from two
  side by side;
- **no member list:** the central directory, which names every member,
  with its offset, sizes, method and time, isn't read; a member whose local
  header is damaged, or whose sizes only its data descriptor gives, is
  read as well as its local header allows;
- **tar streams unread:** a release tarball, once its gzip inflated, is
  scanned as one raw buffer; its members, executables and their names,
  aren't known, and a descriptor's origin can't say which member holds it.

Without the kind of a container, `extract` can't choose its grain (spec
0495 N5): a jar is one build and its classes its members; a wheel is one;
a zip of loose descriptor files is one bundle; a release tarball's
executables are each one, or one build together.

## Goals

- **G1.** A zip known as an archive: its bounds and its members, from its
  central directory.
- **G2.** A tar stream known as one: its members, by name and range.
- **G3.** Each container's kind, from its members, structurally, as a
  packaging convention reveals it, nothing run.
- **G4.** Reported to the caller, as evidence (0494 `buffers`), the
  choice of grain left to `extract` (0495).
- **G5.** What reads today still reads: a container whose directory
  can't be read falls back to today's member-by-member scan.

## Non-goals

- **N1.** Other compressions (bzip2, xz, zstd) and other containers (`ar`
  and so `.deb`, OCI image layers): later, each when a corpus needs it.
- **N2.** Encrypted members, as today: skipped, reported.
- **N3.** Deciding a container's grain: 0495's, from the kind reported.

## Specification

- **S1. A zip's archive:** its end-of-central-directory record
  (`PK\x05\x06`, zip64's when the sizes say so), searched from the end of a
  buffer and anywhere in it, an archive embedded in an executable
  included; verified: its central directory, at the offset and of the size
  it declares, relative to where the archive starts, parses whole. The
  archive's bounds, then each member from its central directory entry:
  name, local header offset, compressed and uncompressed sizes, method,
  modification time. Members are read at the offsets the directory gives.
- **S2. Fallback:** a zip member found by its local header and claimed by
  no archive (a damaged or truncated archive, or a directory that doesn't
  verify) is read as today, and so marked.
- **S2a. A gzip is an encoding, unless what it holds is a container:**
  a gzip stream is an archive only when its content is itself one (a tar,
  S3); otherwise it's an encoding, read as any buffer, however many
  descriptors it holds, and is no container for 0495. Measured: on the
  open-source corpus's Go executables, about 120,000 descriptor
  occurrences sit in gzip streams each holding one descriptor, the old Go
  generator's encoding; downstream, configuration files are each one
  gzipped serialized message embedding many descriptors. Framed sets
  inside such a stream split by 0495's rule as in any buffer.
- **S3. A tar stream:** a buffer whose first block is a tar header (the
  `ustar` magic at offset 257, a checksum that verifies) is read as tar:
  each member's name and data range, regular files only; its members
  scanned as buffers of their own, so that a descriptor's origin names the
  member: `tar@<header offset>!<name>`, in the chain after the gzip layer
  that held it.
- **S4. Origins:** a zip member's layer stays `zip@<local header>!<name>`
  (0480 S6), so that origins read as before; a tar member's is new,
  `tar@<header>!<name>`, inside a tarball's gzip. Each archive reported
  once, with its bounds and members.
- **S5. Kinds,** from an archive's member list, the first that applies:

  | kind | recognized by | its likely grain (for 0495) |
  |---|---|---|
  | `jar` | `META-INF/MANIFEST.MF`, or `.class` members | one build |
  | `wheel` | a `*.dist-info/WHEEL` member | one build |
  | `apk`, `aar` | `AndroidManifest.xml` | one build |
  | `nupkg` | a `*.nuspec` member | one build |
  | `descriptor-zip` | every member a descriptor file (`.pb`, `.desc`, `.binpb`, `.protoset`) | one bundle |
  | `tarball` | a tar stream | each member, or one build (Q2) |
  | `zip` | none of the above | the member (Q3) |

  A kind is evidence, recorded, never acted on by fdp-scan.
- **S6. Reported to `extract`:** each archive with its kind, bounds and
  member count; each member with its entry time; recorded in the
  snapshot's `buffers` (0494 S2), by the chain its copies' origins begin
  with.

## Open questions

- **Q1.** An archive embedded in an executable away from its end (a Go
  `embed` of a zip, a resource section): found by its directory anywhere
  in the buffer, verified by the offsets; to measure how often that
  verification fails on real executables, and what the fallback finds
  then.
- **Q2.** A release tarball's executables: each a snapshot, or one build
  together? Kythe's are one build at one revision; a tarball bundling
  third-party tools is not. To measure on the open-source corpus's
  tarballs (store cpp-0008).
- **Q3.** A generic zip: per member, or whole? To decide on measured cases.

## Test plan

Synthetic archives, written by the tests, never by the tool under test:

1. A zip with a descriptor member and a text member, inside a buffer of
   other bytes: one archive, its bounds and two members from its
   directory; the descriptor's origin as today.
2. Two zips back to back: two archives, each its members.
3. A zip whose directory offset is wrong: the fallback reads its members
   as today, marked so.
4. A tar inside a gzip, two members, one holding a descriptor: its
   origin `gzip@N!tar@M!<name>!a..b`.
5. Each kind of S5 from a minimal member list.
6. A zip member's modification time reported, in a `buffers` entry.

## Measured outcome

Filled in at implementation: on the open-source corpus (stores raw-0007,
cpp-0008), the archives found, by kind, and how many fall back.
