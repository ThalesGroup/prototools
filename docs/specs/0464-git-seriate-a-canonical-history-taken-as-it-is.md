<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0464 — git-seriate: a canonical history taken as it is

Status: draft
App: git-seriate
Refs: docs/specs/0450-git-seriate-a-history-from-snapshots.md (S2: the
        canonical form versions are compared by),
      docs/specs/0452-git-seriate-the-store-and-the-history.md (the
        history commits' block, `canon_rules`)

## Background

`settle` canonicalizes every distinct blob it sorts
(`Versions::canonical`, `git-seriate/src/history.rs`): it parses the
stored text, applies the rule sets, renders the result and keeps the
blob if the rendering is the same. A downstream front end profiled a
re-sort of a 1,130-snapshot history with nothing new (6.4 s): 78% is
there, though each history commit already says, in `canon_rules`, which
rule sets made it.

`canon_rules` is not enough to trust a blob, though: `canonical` also
re-renders, so a history written when the renderer wrote other text for
the same bytes (a prototext change) is brought to today's text. Taken as
it is, one version would be two blobs, two versions to the sort.

## Goals

- **G1.** A history commit written with today's rule sets and renderer is
  taken as canonical: its blobs not parsed, canonicalized nor rendered.
- **G2.** A check that canonicalizes everything, as `settle` does today,
  and reports what a commit's marker wrongly claims.

## Non-goals

- **N1.** A cache of canonical forms beside the repository. The block is
  the marker.

## Specification

- **S1. The renderer's version.** `Renderer` gets `fn version(&self) ->
  (String, u32)`: a name and a number, raised whenever the text it writes
  for some bytes changes. `FdpRenderer`'s is `("prototext", 1)`; the
  generic renderer's `("generic", 1)`.

- **S2. In the block.** History commits record it beside `canon_rules`:

  ```yaml
  canon_rules:
    fdp: 1
  renderer:
    prototext: 1
  ```

  A reader that doesn't know the key ignores it (the block is read by
  key), so the format stays 1.

- **S3. Trusted.** In `settle`, the items of a history commit whose
  `canon_rules` and `renderer` equal the current ones are canonical:
  their blob ids are their versions. Every other item is canonicalized as
  today: snapshot branches (raw), and history commits without `renderer`
  (written before this spec, or read through the legacy reader), or with
  other rule sets or another renderer.

- **S4. `settle --verify-canon`:** every item canonicalized, as before
  this spec; and each item of a trusted commit (S3) whose canonical blob
  is not its own reported, with its path, its commit and both ids. Exit 1
  if any is. With `--dry-run`, it writes nothing: a pure check.

- **S5. The renderer's version is held by a test:** prototext's rendering
  of a committed set of descriptors, compared with a committed copy; a
  change of text fails it, until the copy is refreshed and the version
  raised together.

## Alternatives considered

**Trusting `canon_rules` alone,** as asked. A renderer change would split
versions silently (Background).

**A per-blob mark (a git note).** Notes are refs of their own, to fetch
and to keep in step with the history; the commit already carries the
block.

## Test plan

1. `a_trusted_history_is_not_canonicalized` — a history settled, then
   re-sorted with nothing new: no item parsed (a counting renderer), the
   same history.
2. `another_renderer_version_is_canonicalized` — the same history, the
   renderer's version raised: every item canonicalized, the history
   rewritten with the new `renderer`.
3. `verify_canon_finds_a_false_marker` — a history commit's blob replaced
   by a non-canonical one under the same marker: `--verify-canon` names
   it, exit 1; plain `settle` doesn't see it.
4. `legacy_histories_are_canonicalized` — a commit without `renderer`.
5. The renderer test of S5.
6. Measured: the re-sort on a generated corpus of about 1,000 snapshots,
   before and after.

## Measured outcome

Filled in at implementation.
