<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0497 — forget a snapshot; prune instead of drop

Status: implemented
Implemented in: 2026-11-27
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
  That `settle` re-sorts from the replacement's build time less the
  settling window, as for any new snapshot: for an old snapshot, nearly
  the whole history, once.
- **G3.** `drop` named for what it does.

## Non-goals

- **N1.** Replacing a snapshot in place, or automatically on a newer
  extractor's output.
- **N2.** Forgetting by content or by rule: by identity, or a glob of
  identities, only.

## Specification

- **S1. `git seriate forget IDENTITY… --reason TEXT [--into NAME]
  [--dry-run]`,** identities or globs, a pattern that matches no snapshot
  of the history refused, and so is forgetting all of them (delete the
  branch instead). The history is rewritten as `settle` rewrites it, with
  no new snapshot and without the forgotten ones, from the oldest of them:
  the commits below it are kept as they were; those above are rewritten
  as a re-sort rewrites them, their blocks stamped with today's rule sets
  and renderer (spec 0464), their trees rebuilt without the forgotten
  snapshots' items. The previous tip is saved as `NAME-N`. Then each
  forgotten snapshot's branch is deleted, so that `settle` doesn't put it
  back. `--dry-run` lists what it would do, and writes nothing.
- **S2. One record per decision,** `refs/seriate/forgotten/<n>`, `n`
  counting the decisions: a commit whose parents are the forgotten
  snapshots (each its branch's commit where it had one, else its history
  commit), which keeps them reachable; whose message gives the reason and
  the history; and whose tree is one file, `forgotten`, listing each
  identity and commit.
- **S3. Anchors:** a manual commit anchored to a forgotten snapshot is
  reported, as when an anchor is gone (spec 0452 S5 step 5), and replayed
  after the snapshot below it.
- **S4. What next:** the listing ends with "extract their replacements, if
  any, then settle", so that a history isn't shared half-repaired.
- **S5. `prune`** is `drop` renamed, unchanged: it deletes snapshot
  branches the history holds, listing first those whose raw form only the
  branch keeps. `drop` stays, hidden, warning, then pruning.

## Test plan

`git-seriate/tests/forget.rs`:
`a_forgotten_snapshot_leaves_the_history_and_the_store` (two snapshots
left, a file back to its version below, the previous tip saved, the branch
gone, the record's parent, message and list),
`a_forgotten_snapshot_is_added_again_the_normal_way` (and forgotten again,
a second record), `one_decision_one_record_for_every_snapshot_it_forgets`,
`a_dry_run_lists_and_writes_nothing`,
`what_matches_nothing_or_everything_is_refused` (and no `--reason`),
`prune_is_what_drop_was_and_drop_says_so`. Anchors: `settle`'s existing
tests of an anchor gone.

## Measured outcome

The tests above pass. To measure downstream: a repair forgetting one
archive's 1,116 snapshots by one glob, then extracting it again as about
20 atomic snapshots (spec 0495).
