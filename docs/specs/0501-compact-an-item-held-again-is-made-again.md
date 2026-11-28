<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0501 — compact: an item held again is made again

Status: implemented
Implemented in: 2026-11-28
App: git-seriate
Refs: docs/specs/0452-git-seriate-the-store-and-the-history.md (S6,
        `compact`'s levels, which this amends),
      docs/specs/0450-git-seriate-a-history-from-snapshots.md (S4, what
        level 2 may lose: when a version appeared, never the tree)

## Background

At level 2, `compact` drops a commit before the cutoff when every change
it makes is made again by a later commit before the cutoff. A later
snapshot holding the same path, the same bytes, makes no change there:
its commit writes its items over a tree that already holds them. So it
never counts, and an old commit stays as long as one of its files holds a
version no later commit changes, even when every later snapshot holds
that file.

Found downstream, on a history whose base is 19 hand commits under its
snapshots: a dry run at level 2 drops 9 and keeps 10. Of the 1,172 files
the 10 kept still provide at the tip, 731 are items of a later snapshot,
identical; 3 of the 10 are covered entirely, and kept all the same. As
the downstream re-extracts the base's sources from newer builds, every
file a new snapshot holds should free the base commit that first wrote
it; today, none does.

Counting a held item as made again is within what level 2 promises
(0450 S4, 0452 S6): the tree at the cutoff unchanged, losing when a
version appeared, where it was ordered and which sources held it. The
version is then dated by the newest snapshot holding it, as `advise`
already dates a file (0452 S6, level 1).

## Goals

- **G1.** At level 2, a commit whose every path a later commit changes
  or holds is dropped; at level 3, a kept snapshot's items a later
  commit changes or holds are removed.
- **G2.** The tree at the cutoff unchanged, as at every level.
- **G3.** No path lost by two commits each dropped for the other: a
  snapshot dropped for changing nothing while an older commit is dropped
  for the snapshot holding its path.

## Non-goals

- **N1. No comparison of contents** beyond the history's: an item's bytes
  are its commit's tree's at its path, canonical (0464), never the raw
  form a store may still hold.
- **N2. Commits at and after the cutoff count for nothing,** as today:
  their trees are kept as they were, and the tree at the cutoff is built
  from the commits before it.
- **N3. Level 1 unchanged.**

## Specification

- **S1. What a commit holds:** its changes' paths, a deletion's
  included: the commit deleting a path holds it, and owns it when it is
  the last holder, a snapshot holding it before then owning nothing
  there. A snapshot commit's also every path of its `contents`, its
  items. A record-only commit
  (`contents` none) and a manual commit hold only their changes. An item
  needs no bytes compared: a snapshot commit writes its items over its
  parent's tree, so its tree holds each item's bytes at its path.
- **S2. A path's owner:** the last commit before the cutoff holding it,
  among those level 1 keeps (a second copy dropped at level 1 owns
  nothing; its first copy holds the same items).
- **S3. Level 2** drops a commit owning no path, its record with it if a
  snapshot commit, as today's level 2 drops a record. A commit owning a
  path is kept, even one changing nothing, which today's level 2 drops:
  a snapshot that changes nothing but owns a path keeps its record.
- **S4. The owner's rewrite:** a kept commit's changes are, in order, its
  own, and for each path it owns that the rebuilt tree lacks or holds
  other bytes for, that path with its original tree's bytes. A deletion
  is replayed only where the rebuilt tree holds the path: its writers
  dropped, there is nothing to delete, and the deletion owns nothing. A
  commit whose only owned paths are such deletions is dropped, decided
  before it is kept, so that one pass is final. The tree at the cutoff is
  then the old one, checked as today (nothing written otherwise).
  *Amended 2026-11-28:* a downstream's second pass dropped a commit the
  first had kept for a deletion it then didn't replay.
- **S5. Level 3** removes from a kept snapshot commit the items it holds
  but doesn't own, changed or only held: `reduced: true`, `contents` and
  `evidence` without them. Manual commits are never reduced, as today.
- **S6. The listing,** one line per commit: a drop for S3 says how many
  commits own its paths, the first two named (`drop 1a2b3c4d ('…'):
  every path it holds is held later, by 37 commits (5e6f7a8b, 9c0d1e2f,
  …)`), all of them with `--verbose`; a kept commit that gains changes by
  S4 says how many (`own 5e6f7a8b ('…'): 18 items it held become its
  changes`); a manual commit dropped by S4, that its deletions have
  nothing left to delete, and the commits owning its other paths if any.

## Alternatives considered

- **Keeping the oldest holder,** dropping later snapshots that change
  nothing (today's level 2): keeps the base commit for every file it
  wrote, the downstream's case. Same count of commits kept for a path
  held throughout, but the oldest evidence, not the newest, and no base
  ever freed by re-extraction.
- **Counting items without owners:** the cycle of G3. A base commit
  dropped because a snapshot holds its path, the snapshot dropped
  because it changes nothing; the rebuilt tree lacks the path, and
  `compact` refuses to write.

## Test plan

1. `a_base_commit_held_by_a_later_snapshot_is_dropped`: a manual commit
   writes `a` and `b`; a snapshot holds `a` and `b`, identical; level 2
   drops the manual commit, keeps the snapshot, whose rewritten commit
   adds `a` and `b`; the tree at the cutoff unchanged.
2. `a_partly_held_commit_is_kept`: the manual commit's `c` is held by no
   later commit: kept at level 2, `c` its only path owned.
3. `no_cycle`: a snapshot changing nothing, owning the paths an older
   commit wrote, is kept, not dropped with it; today's level 2 dropping
   it is the regression.
4. `held_after_the_cutoff_counts_for_nothing`: a path held only after
   the cutoff keeps its older owner.
5. `level_3_removes_held_items`: a kept snapshot's item held identical
   later is removed, `reduced: true`, `contents` without it.
6. `a_deletion_owns_its_path`: a snapshot holds `a`, a manual commit
   then deletes it: the manual commit owns `a` and is kept, the snapshot
   dropped if it owns nothing else; with the path's writers all dropped,
   a manual commit deleting only it is dropped, the tree at the cutoff
   unchanged.
7. `a_second_copy_owns_nothing`: a second copy dropped at level 1, last
   holder of a path; its first copy, or an older owner, keeps it.
8. The listing's lines (S6), by `--dry-run`, and with `--verbose`.
9. `one_pass_is_final_when_a_deletion_has_nothing_left`: a manual commit
   adding files a later snapshot holds and deleting one a dropped commit
   wrote is dropped in the same pass; a second pass drops nothing.

## Measured outcome

2026-11-28. `git-seriate/tests/compact.rs`: the eight tests of the plan
pass, beside the eleven of before. One of those changed, as S3 says it
would: `an_anchor_on_a_snapshot_no_longer_held_is_warned_about` relied
on level 2 dropping a snapshot that changes nothing (s2), which now owns
its path and stays; the anchor moved to s1, which 0501 drops.

The case S4 names, a deletion left with nothing to delete, could already
arise at today's level 2, its writer dropped for the deletion making its
change again. It is now handled as S4 says, and tested; what the code
before did with it was not checked.

The downstream's measurement, its base's commits kept before and after,
is to come.
