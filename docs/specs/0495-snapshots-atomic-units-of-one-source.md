<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0495 — snapshots: atomic units of one source

Status: draft
App: git-seriate, git-seriate-fdp
Refs: docs/specs/0452-git-seriate-the-store-and-the-history.md
        (the snapshot and its block, S4, whose format this changes),
      docs/specs/0494-extract-records-every-origin-and-each-member-s-type.md
        (the origins and types this splits by),
      docs/specs/0492-extraction-hints-what-an-executable-tells-beyond-its-descriptors.md
        (the Go and C++ hints, a later split)

## Background

The sort assumes that a snapshot is the state of one source repository at
one revision: every file in it as of one point of that source's history.
An executable often isn't. It can carry descriptor sets built elsewhere
and earlier, each a state of its own: a schema bundle embedded in a
database client, the libraries of a Python wheel, the members of a release
tarball. Mixed into one snapshot, they describe no single moment, and the
sort reads the mixture as history. Measured downstream: about 150 of a
history's 160 straddles traced to one executable holding an embedded
bundle's copies beside its own, the bundle's copy at the plain path and
the executable's at `~2`.

## Goals

- **G1.** A snapshot is atomic relative to its source repository: one
  source, one revision, as far as `extract` can tell.
- **G2.** Where it can't tell, the snapshot stays as today, and records
  that it wasn't split.
- **G3.** The nesting recorded once per snapshot, in its block, instead of
  once per file.

## Non-goals

- **N1.** Lineages (the producers' transformations): a later spec, built
  on units.
- **N2.** Splitting a C++ build's scattered descriptors: one per
  translation unit, they are the executable's own build, atomic as such.
- **N3.** Splitting by Go module (spec 0492's hints): a later step, once
  the container split stands.

## Specification

- **S1. The units,** in this order of certainty:
  1. **each container member holding descriptors** (a zip member, a gzip
     member, a tar member within one), by the origin chain (0494):
     mechanical;
  2. **an embedded descriptor set in raw bytes,** by contiguity (Q1):
     after measurement;
  3. **the rest:** the input's own descriptors, one unit.
- **S2. A unit's source:** identity `<input's identity>!<chain>`; the
  input's group, so that what to extract again stays known; its build
  time the member's own where the container has one (a zip entry's time),
  else the input's, an upper bound; which of the two, recorded.
- **S3. The block's format 2:** `source.provenance`, outermost first, one
  entry per layer (`{artifact, sha256}`, `{member, type}`, …); the
  per-file `origin` entries of 0494 lose their chain prefix, keeping each
  file's range within the unit. A writing command refuses a repository of
  a format it doesn't know (spec 0452 S4), so a format-1 build cannot
  write format 2 by mistake.
- **S4. `~N`** within a unit only (two copies of one path in one unit
  being a real anomaly), kept by the lowest name among copies equal once
  canonical (a later spec).

## Open questions

- **Q1.** Contiguity: how to find an embedded set's bounds in raw bytes.
  fdp-scan's order cuts (spec 0449) may mark them; to measure, on a
  synthetic bundle in a synthetic executable, then on real ones.
- **Q2.** Migration: a format-1 repository read as before; written in
  format 2 only through a new history (`settle --into`), or migrated in
  place?
- **Q3.** Phase 1 orders by build time; a unit inheriting its input's
  time, an upper bound, sorts too late. Does an upper bound need its own
  treatment in the sort?

## Test plan

1. An input with descriptors of its own and in a zip member: two
   snapshots, identities `<input>` and `<input>!zip@N!<member>`, each with
   its provenance.
2. A tarball of two executables: two units, one per tar member.
3. A format-2 block written and read back; a format-1 build refusing it.
4. An input with nothing to split: one snapshot, as today, recorded as
   not split.

## Measured outcome

Filled in at implementation.
