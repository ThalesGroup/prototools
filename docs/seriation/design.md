<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Seriation: ordering undated snapshots

The `seriation` crate orders a set of snapshots of files whose dates are
unreliable, by how the files in them evolve. This document is its design:
the problem, the data model, and the phases of the sort. The scores the
phases maximize are in [scores.md](scores.md), and the search that
maximizes them in [greedy-search.md](greedy-search.md). The engine around
it, which stores snapshots in git and rebuilds a history from the order,
is spec 0450.

## The problem

Snapshots arrive in no particular order. Each is the set of files a source
held at one point of an unknown history: the descriptors embedded in an
executable, the files of an archive, a directory. For each snapshot we
know:

- a **build time**, always, and usually right, but not always: two
  snapshots built from different revisions can carry each other's order
  inverted, and many snapshots can share one build time;
- sometimes an **order hint**, a monotonic revision number, which when
  correct is the ground truth, but which can be wrong: snapshots of
  different revisions sometimes share one.

The task is to recover the true order, as far as the evidence allows.

Two facts about how files evolve make that possible:

- **files mostly grow.** Given two versions of a file, one often contains
  the other, which says which is older. A comparator tells, for a pair of
  versions: equal, the first inside the second, the second inside the
  first, or neither. It abstains often and is sometimes wrong;
- **files rarely go back.** In the true order a file seldom returns to a
  version it has left: a sequence A, B, A of one file's versions is
  evidence that the order is wrong. Deliberate rollbacks exist, but are
  rare.

Build times agree with the true order more often the further apart they
are, so the comparator is only asked about pairs within a window of time,
which also keeps the cost linear in the number of snapshots.

## Terms

- **Snapshot** `s`: a load index (`sid`, fixed once loaded) and a position
  in the current order (`pos`, which the phases change).
- **File** `F`: a path. A **version** of `F` is an opaque key; two
  occurrences of `F` are the same version iff their keys are equal. The
  key is the canonical form's identity (spec 0450 S2), not the raw bytes'.
- **Instance**: the occurrence of a file in one snapshot, `(sid, version)`.
- `F_v`: the version of `F` in the snapshot at position `v`, or nil if `F`
  is absent from it.
- **Span** of a version `c` of `F`: `[lo, hi]`, its first and last
  position in the current order. A **real span** has `lo < hi`.

## Determinism

Given the same snapshots, the sort produces the same order, always. A
history is rebuilt from the order each time new snapshots arrive; a sort
that moved snapshots between runs with nothing new would rewrite history
for nothing. So:

- no randomness (no annealing, no random restarts);
- every tie is broken by a fixed rule: stable sorts, and positions in a
  fixed scan order ([greedy-search.md](greedy-search.md));
- snapshots are loaded in a fixed order: by name, bytewise, whatever order
  the caller lists them in.

## The phases

| Phase | Reads | Does |
|---|---|---|
| 0, load | identities, items | assigns load indices in name order; builds the file and instance tables, the version keys deduplicated |
| 1, build time | build times | stable sort by build time |
| 2, order hints (opt-in) | order hints | places the snapshots that have one in hint order, and makes them barriers for phases 3 and 4 |
| 3, containment | file contents, through the comparator | builds the edge graph, then moves snapshots to maximize the edge score |
| 4, interleaving | version keys | moves snapshots to minimize A…B…A patterns |

Only phase 3 reads contents. Each phase can be skipped; phase 0 cannot.

### Phase 1: build time

A stable sort of the order by build time. Snapshots with equal build times
keep their load order. This is near-correct whenever build times are, at
`O(N log N)`.

### Phase 2: order hints

Off by default, since hints can be wrong. When on, the snapshots whose
hint the sort is shown (**revealed** snapshots) are put in non-decreasing
hint order with few moves:

1. **Group** the revealed snapshots by hint value.
2. **Pack** each group of two or more into consecutive positions from its
   centroid (the mean of its members' positions, rounded down) less half
   its size, keeping its members' relative order.
3. Take the groups' order after packing, and compute the **longest
   increasing subsequence** of their hints. The groups in it are
   **settled**: already in the right relative order.
4. **Place** every other group, in hint order, between its nearest settled
   neighbors: its left anchor (the last position of the nearest settled
   group with a lower hint, or −1) and its right anchor (the first
   position of the nearest settled group with a higher hint, or `N`),
   centered on the position interpolated from the hints, rounded:

   ```
   target = left + (right − left) × (hint − left_hint) / (right_hint − left_hint)
   ```

   With no settled group below, `target = right × hint / right_hint`; with
   none above, the group goes right after its left anchor. The block is
   kept between the anchors. The group is then settled, and anchors the
   groups placed after it.

Snapshots without a hint aren't moved by this phase; they shift as the
others move.

The phase's positions then bind the later phases: a revealed snapshot may
not move past another revealed snapshot with a different hint. Snapshots
without a hint move freely. There is no placing without the barriers, nor
barriers without the placing: one setting, `order_hints`.

### Phase 3: containment

For each file, for each pair of its instances whose build times are
within the window (30 days by default), in load-index order:

- same version: an edge in both directions, without asking the comparator;
- otherwise the comparator: if one version is inside the other, an edge
  from the older snapshot to the newer; if it abstains, no edge.

The comparator is asked at most once per pair, and its answers are fixed
for the rest of the sort. Contents are fetched file by file, each distinct
version once, and dropped before the next file, so memory stays at about
one file's versions. Then the greedy search maximizes the edge score
([scores.md](scores.md)).

### Phase 4: interleaving

The greedy search minimizes the straddle count ([scores.md](scores.md)),
which reads only version keys.

## The comparator

The default comparator (`LinesChars`) works on text, line by line. It
splits both versions on `\n` (a final `\n` adds no empty line), and says
A is inside B when every line of A matches a line of B, in order, a line
matching when its characters are a subsequence of the other line's. So B
is "later" when it can be made from A by inserting lines and inserting
characters within lines; lines can't match across a newline. Characters
are UTF-8 sequences, matched whole; a truncated one is matched as far as
it goes. Both directions are tested, each in `O(|A| + |B|)` with neither
cursor backtracking; different bytes contained both ways count as equal.

A comparator for a file format replaces it. It sees canonical forms only:
normalizing is the canonicalizer's job (spec 0450 S2).

## Settings

One struct per run, passed in:

| Setting | Default | Meaning |
|---|---|---|
| `window` | 30 days | phase 3 compares pairs of instances within it |
| `nom_tries`, `max_tries` | 1, 20 | the greedy search's candidates ([greedy-search.md](greedy-search.md)) |
| `order_hints` | off | phase 2, and its barriers in phases 3 and 4 |
| `skip` | none | phases 1, 3 or 4 to skip, for experiments |
| `reveal_percent` | 100 | the self-test's share of hints shown |

## Self-test

Where every snapshot has a correct hint, the sort can be scored against
the truth. `reveal_percent` hides a share of the hints from the sort,
deterministically: an identity's hint is shown when its hash, modulo 100,
is below the percentage, the hash multiplying then adding each byte with
FNV-1a's offset basis and prime. Tau is always over every true hint,
hidden ones included ([scores.md](scores.md)).
This is how a change to a phase or a setting is judged: on corpora with
known orders, before any real one.

## Cost

Phases 3 and 4 cost, per move, `O(N · F)` in the worst case, `F` the
number of distinct files, because the search probes every destination.
Many snapshots of one build (equal build times, many small snapshots) are
where this shows: each move costs more as `N` grows. A probing window
limited by time and by edge partners is the known remedy, and an option
for later (spec 0450 N2).
