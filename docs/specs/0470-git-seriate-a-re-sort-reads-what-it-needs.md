<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0470 — git-seriate: a re-sort reads only what it needs

Status: implemented
Implemented in: 2026-11-23
App: git-seriate | seriation
Refs: docs/specs/0464-git-seriate-a-canonical-history-taken-as-it-is.md
        (a trusted history commit's items, taken from its tree)

## Background

A downstream front end profiled `settle --no-cutoff --dry-run` on its
largest history (1,145 commits, all marked by 0464, nothing new), on a
busy host, so in shares:

| | share |
|---|---|
| `history::history_items`, of which `history::files` | 61%, 55% |
| of which object lookup, pack decode, zlib inflate | 54%, 36% inflate |
| `seriation::sort` | 34% |
| `Versions::fetch` (blobs for the comparator) | 19% |
| `alloc::fmt::format_inner` | 12.5% |

`history_items` (0464 S3) wants the items a history commit's block lists:
its snapshot's, a few hundred at most. It gets them by listing the
commit's whole tree (`files`), about 15,000 files there, every subtree
decoded again for every commit though consecutive commits share nearly
all of them, and a `format!` per entry; then it keeps the few it wants.

`build_edges` (`seriation/src/state.rs`) fetches every distinct version of
a file before comparing, though it compares only the pairs within the
window: a version with no pair inside it is read for nothing.

## Goals

- **G1.** A history commit's items are read by their paths, not by
  listing its tree; a subtree is decoded once per command.
- **G2.** A version is fetched when a comparison first needs it.

## Non-goals

- **N1.** The sort itself (34%): 0450 N2's probing window stays deferred.
- **N2.** `compact`'s marking (0464): a one-off, the downstream finds its
  cost acceptable; a mark without rewriting would need a store beside the
  history.

## Specification

- **S1. Items by path.** `history_items` looks each path the block lists
  up in the commit's tree, component by component, through a cache of
  decoded trees by id, shared by every lookup of the command (the
  `Versions` it builds, or the settle's). A path the tree lacks is refused
  as today. `files` stays for what needs a whole tree, untouched.

- **S2. No string built per entry.** The lookup compares a component's
  bytes with each entry's name; no path is formatted.

- **S3. Fetched on first need.** `build_edges` fetches a version the
  first time a comparison within the window names it, and keeps it for
  the file's other comparisons, as today; a version no comparison names
  isn't fetched. The edges and the counts are unchanged.

## Alternatives considered

**Memoizing a subtree's file list,** as the downstream proposed: it still
lists every file of the first commit and of each changed subtree, where
S1 reads only the paths wanted.

**Following each commit by a tree diff from the previous:** correct for a
linear walk, but `settle` reads the history commits it re-sorts in no
particular need of order, and a diff decodes the changed subtrees of both
sides.

## Test plan

1. `items_by_path_are_the_trees_files` — on a history of nested paths,
   `history_items` gives what listing the tree gave, for every commit; a
   path the tree lacks refused, as before.
2. `a_subtree_is_decoded_once` — a counting object reader: each tree id
   decoded once per settle.
3. `versions_out_of_the_window_are_not_fetched` — a counting `Contents`,
   two versions of a file built beyond the window: neither fetched; the
   sort's result unchanged.
4. The existing histories (`tests/histories/*.tsv`): unchanged.
5. Measured: the re-sort of 0464's measure, before and after; the
   downstream's figure is theirs to take.

## Measured outcome

2026-11-23. The tests pass (109 in seriation, git-seriate and
git-seriate-fdp, under `env -i`), the committed histories unchanged.

Not shown on our corpora. A re-sort with nothing new of a generated
history of 400 snapshots of about 1,000 files among 3,024: 40.3 s before,
40.9 s after, the sort taking nearly all of it (the untangling search
21%, the comparator 13%), `history::files` 0.16% before. This history has
4 directories of about 750 files each, every commit changing files in
all: its commits share few subtrees (1,770 distinct trees of the 2,000 a
full listing decodes), where the downstream's share nearly all of theirs
(a handful of 15,000 files changed per commit). The gain is theirs to
measure, as 0464's was.
