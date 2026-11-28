<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0002 — the histories and their records, pushed, carry no snapshot

App: git-seriate
Spec: docs/specs/0499-lineages-a-producer-s-snapshots-sorted-apart.md (S5a)

## The guarantee

Pushing a history's lineage branches (`<history>/<lineage>`), the
history's record (`refs/seriate/histories/<history>`) and the lineage
statements' record (`refs/seriate/lineages`) copies no snapshot commit:
nothing a downstream keeps local because it holds raw extracted bytes
reaches the remote through them. It holds whatever the history went
through (settles per lineage, moves between lineages, `forget`,
`compact`).

`refs/seriate/forgotten/<n>` is outside it: its parents are the forgotten
snapshots, so it stays local wherever snapshots do. So is
`refs/seriate/snapshots/`, by definition.

## Why it holds

- A history commit's only parent is the previous history commit of its
  lineage (`settle_with`, `compact_branch`): a snapshot's items are
  written into a new commit, never merged in.
- The history's record has as parents only its previous record, the
  lineages' tips (history commits) and the statements' record
  (`write_history_record`).
- The statements' record has as parent only its previous one, its tree
  naming identities as text, never as commits (`lineage::state`).

## Regression tests

- `git-seriate/tests/lineage.rs`:
  `pushing_the_histories_and_records_carries_no_snapshot`: a history
  settled into two lineages, a snapshot moved, another forgotten; the
  lineage branches and both records pushed to a bare repository, which
  holds none of the snapshot commits, those of the store and the
  forgotten one, and passes `git fsck --strict`.
- `git-seriate/tests/settle.rs`: `signs_like_ssh_keygen_with_a_key_file`
  verifies every commit both records reach: all signed, so none is a
  snapshot commit, which git-seriate writes unsigned.
