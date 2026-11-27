<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0497 — forget a snapshot; prune instead of drop

Status: draft
App: git-seriate
Refs: docs/specs/0452-git-seriate-the-store-and-the-history.md
        (drop, compact, the rebuild of a snapshot branch from its history
        commit),
      docs/specs/0495-extract-makes-atomic-snapshots.md
        (re-splitting, which goes through this),
      docs/specs/0496-fdp-scan-a-framed-descriptor-ends-at-its-frame.md
        (a fixed scanner, whose output replaces snapshots through this)

## Background

A stored snapshot can be wrong: extracted by a scanner since fixed (a
downstream's three snapshots read past their bundles' last records,
spec 0496), or split by a rule since improved (spec 0495). The store
refuses a second commit for an identity (spec 0452 S3), which is right:
nothing may replace a snapshot silently, and a different commit under a
known identity can as well be another input given the same name, or an
extractor that isn't deterministic. Nothing removes one either:

- `drop` deletes snapshot branches, but their commits stay in the
  history, and the next `settle` that re-sorts one rebuilds its branch
  from its history commit (0452 S5);
- `compact` drops records from the history, but chooses which itself,
  for compactness.

And `drop`'s name says the opposite of what it does: it frees stored
copies the history can rebuild; it removes nothing.

## Goals

- **G1.** Remove a named snapshot, from the history and the store, by an
  explicit decision, recorded with its reason.
- **G2.** Then add its replacement the normal way: `extract`, `settle`.
- **G3.** `drop` named for what it does.

## Non-goals

- **N1.** Replacing a snapshot in place, or automatically on a newer
  extractor's output.
- **N2.** Forgetting by content or by rule: by identity, or a glob of
  identities, only.

## Specification

- **S1. `git seriate forget IDENTITY… --reason TEXT [--into NAME]
  [--dry-run]`,** identities or globs: each snapshot's history commits
  dropped, the history rewritten from the first, the previous tip saved
  as `NAME-N` (as by `settle` and `compact`); its snapshot branch deleted,
  so that `settle` doesn't put it back; listing what it does, by
  `--dry-run` too.
- **S2. Kept reachable, with the reason:** each forgotten snapshot's
  commit under `refs/seriate/forgotten/<identity>/<n>` (escaped as spec
  0495 S4 escapes identities, `n` counting the times that identity was
  forgotten), a tag-like commit whose message gives the reason, the date
  and the history it left.
- **S3. Anchors:** a manual commit anchored to a forgotten snapshot is
  warned about, as by `compact`.
- **S4. `prune`** is `drop` renamed, unchanged: it deletes snapshot
  branches the history holds, listing first those whose raw form only the
  branch keeps. `drop` stays a deprecated alias, warning, for a while.

## Test plan

1. A history of three snapshots, the middle one forgotten: the history
   holds two, the tree at the tip without the middle one's own files,
   the previous tip saved, its branch gone, its commit and reason under
   `refs/seriate/forgotten/`.
2. Forgotten, then extracted again and settled: the new commit in its
   place, with no refusal.
3. `--dry-run`: the listing, nothing written.
4. A manual commit anchored to it: warned about.
5. `prune` as `drop` was; `drop` warning, and doing the same.

## Measured outcome

Filled in at implementation.
