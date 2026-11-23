<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0465 — git-seriate-fdp: no read-back at runtime

Status: implemented
Implemented in: 2026-11-23
App: git-seriate-fdp
Refs: docs/specs/0453-git-seriate-fdp-descriptors-from-executables.md (G3,
        which this amends)

## Background

0453 G3: "Items are stored as prototext text and read back to the same
bytes: checked on every write." `FdpRenderer::render` parses every text
it writes back to binary and compares. A downstream front end profiled
extraction of 3 archives (10.5 s, against 7.3 s for its Python tool,
which didn't read back): rendering is 36%, the read-back 22.7% of it;
the check has never refused an item on their corpora. `settle` pays it
too, for every blob it renders.

The renderer is lossless by design, and prototext's own round-trip tests
(`prototext/tests/roundtrip.rs`) check it on their corpus at every build.

## Goals

- **G1.** No read-back in `extract`, `add` or `settle`.
- **G2.** The guarantee kept where it costs nothing at runtime: in tests,
  and on demand.

## Non-goals

- **N1.** A faster read-back. Dropped, not optimized.

## Specification

- **S1.** `FdpRenderer::render` renders, and no longer parses its text
  back.

- **S2.** `seriate.checkRendering` (a boolean in the repository's or the
  user's git config, default false) restores the read-back: an item whose
  text doesn't read back is refused, as before.

- **S3.** A test reads back every item of git-seriate-fdp's corpora (the
  fdp-scan corpus's descriptors, the well-known types), with the stock
  schema and with another (prototext's frozen test set).

- **S2b.** The check is git-seriate's `Checked`, wrapping any renderer:
  every front end has it.

- **S4.** 0453 G3 becomes: "Items are stored as prototext text, which
  reads back to the same bytes: checked by the tests, and on every write
  with `seriate.checkRendering` (0465)."

## Alternatives considered

**Keeping the check where a write is new** (skipping it for a blob
already stored, or one unchanged). It keeps most of the cost in
`extract`, where every item is new.

## Test plan

1. `render_does_not_read_back` — a renderer counting parses: none in
   `render`.
2. `check_rendering_refuses` — with `seriate.checkRendering`, a renderer
   made lossy: the item refused, as before 0465.
3. S3's corpus test.
4. Measured: `extract` on the fdp-scan corpus and the well-known types,
   before and after.

## Measured outcome

2026-11-23, same host and method as 0463: `extract` of the 300
executables of 0464's measure into a repository, 0.60 s before, 0.36 s
after (−40%). The corpus test (S3) reads back over 100 descriptors with
each of two schemas.
