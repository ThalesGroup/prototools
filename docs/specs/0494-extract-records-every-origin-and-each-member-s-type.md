<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0494 — extract records every origin, and each member's type

Status: draft
App: git-seriate-fdp
Refs: docs/specs/0480-fdp-scan-inflates-zip-and-gzip-members.md
        (origins: S6's text, S8's record entries),
      docs/specs/0495-snapshots-atomic-units-of-one-source.md
        (the units these records let a reader find, then `extract` make)

## Background

A downstream surveying its history found, by hand, two producers in one
executable: an embedded schema bundle's copies beside the executable's own,
told apart only by tracing contents. What would have shown it directly is
where each descriptor was found. `extract --inflate` records an origin only
for a descriptor found inside a container member (`origin <path>:
zip@…!…`); one found in the input itself records none. Nor is the kind of
each buffer scanned recorded: an ELF library, a `.pyc`, a jar's class
file, a text bundle.

## Goals

- **G1.** Every descriptor's origin in the snapshot's record, the input's
  own included.
- **G2.** Each buffer's type, by its magic bytes, for the input and each
  member holding descriptors.
- **G3.** The block's format unchanged: these are record entries, shown,
  never interpreted (spec 0452 S4).

## Non-goals

- **N1.** Splitting an input into snapshots: spec 0495, which reads these.
- **N2.** Recording spec 0492's hints: with 0495's units.

## Specification

- **S1.** A descriptor found in the input itself records `origin <path>:
  START..END`, as one found in a member records its chain then range.
  Without `--inflate` too.
- **S2.** `type: <kind>` for the input, and `type <chain>: <kind>` for each
  member holding descriptors, `<chain>` as its descriptors' origins begin.
  `<kind>` by magic bytes: `elf`, `mach-o`, `pe`, `wasm`, `zip`, `gzip`,
  `class`, `pyc`, `text` (no control byte), else `other`.

## Open questions

- **Q1.** The record's size: an executable of thousands of descriptors
  gains thousands of entries. Acceptable as record entries, or a reason to
  go straight to 0495's per-unit provenance?

## Test plan

1. An input with descriptors of its own and in a gzip member: every
   descriptor has an origin; `type: other`, `type gzip@N: other`.
2. Each `<kind>` from a minimal buffer of its magic.
3. A record entry's text stable: the same input gives the same block.

## Measured outcome

Filled in at implementation.
