<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0503 — advise over a history: the lineages read as the consumer combines them

Status: draft (reviewed by the downstream, 2026-11-28)
App: git-seriate-fdp
Refs: docs/specs/0499-lineages-a-producer-s-snapshots-sorted-apart.md
        (G5: no merged tree; N3: the combined tip, the consumer's, one
        include root per lineage, the order as precedence; S5: `advise`
        reads one lineage),
      docs/specs/0454-advise-what-a-history-of-descriptors-cannot-hold.md
        (what `advise` reads, S2, and reports),
      docs/specs/0504-the-combined-tip-in-an-order-the-user-gives.md
        (the order, `--lineage`, and the combined tip)

## Background

Since 0499, a history is one branch per lineage, and the consumer builds
its schema from their tips: one include root per lineage, the include
order the precedence, so that a path several lineages hold is taken
from the first (N3). `advise --into H/L` reads one lineage: its fixups
and status say nothing of the others, and a path two lineages hold is
seen by none of them. A downstream building that way asks for `advise`
over the combination it builds.

`advise` reads, from one branch (0454 S2): the tip's files, each file's
last writer (commit, time, subject), and the sources the blocks name.
Read per lineage, those combine without writing anything.

## Goals

- **G1.** `advise` over a history, its lineages read as the consumer
  combines them, in the order it gives (0504).
- **G2.** Each problem named with its lineage.
- **G3.** The paths several lineages hold reported: which one wins,
  whether the others' versions differ once canonical, and, if they do,
  whether the winner's is the newest.
- **G4.** 0499 G5 kept: nothing merged in the history, the combination
  read only; what wins is the consumer's.

## Non-goals

- **N1. No order inferred** from contents, dates or names: 0504's, given
  with `--lineage`; none given, `default` alone (0504 G2), the others
  said left out.
- **N2. No combined tree written,** in the history or elsewhere: the
  consumer's include roots stay the build (0499 N3).
- **N3. One lineage unchanged:** `--into H/L`, and a history of one
  lineage, read as today.

## Specification

- **S1. The order:** `advise --into H --lineage NAME…`, 0504 S1; the
  lineages left out said, 0504 S3.
- **S2. The combined reading:** each lineage named read as today (its
  tip's files, their writers, its sources); a path taken from the first
  lineage named holding it, with that lineage's writer (0504 S2); the
  sources of all, each with its lineage. Then `advise` as today over that
  reading.
- **S3. Each problem's lineage:** every candidate and status line about
  a path names the lineage it was taken from (`[bundler]`), and `--json`
  gives it as `lineage`. In `fixups.yaml`, which protoback reads, the
  lineage only in a comment, never in a key or a pattern.
- **S4. Paths held by several lineages:** a path, or a descriptor's
  name, several named lineages hold, a `~N` copy counted under its name
  (as protoback shadows by name, 0504 S5). A section, `Held by several
  lineages`:
  - those that differ once canonical, one line each: the winner, the
    others, and whether the winner's version is the newest;
  - those the same once canonical, counted, one line per pair of
    lineages (`bundler and default: 312 the same once canonical`): a
    bundler's copies, its hosts' minus options, may be many and say
    nothing.

  `--json`: `overlaps`, every one, `{path, winner, others, same,
  winner_newest}`.
- **S4a. "Newest":** the newest source holding the version, as `advise`
  dates a file (0454): the commit that last wrote a path may be a manual
  one (a fix, a rendering, a replay after `compact`), which its own time
  would make the newest of all. A version no source holds is dated by
  its writer's time, said as such.

## Questions for the downstream

Answered by the downstream, 2026-11-28:
- the order's home: given per run in the consumer's own commands,
  recorded nowhere (0504 N1);
- paths the same once canonical: counted per pair of lineages in the
  text, every one in `--json` (S4);
- "newest": the newest source holding the version (S4a);
- nothing of theirs reads `advise` per lineage; `fixups.yaml`, read by
  protoback, carries the lineage only in comments (S3).

## Alternatives considered

- **A precedence required, from an option or git config:** this draft's
  first form; the downstream preferred an order given per run, `default`
  alone without one (0504).
- **A combined branch written by `settle`:** what 0499 G5 rules out; a
  merged tree in the history would be an inference git-seriate doesn't
  make.
- **`advise` run once per lineage, the consumer combining the outputs:**
  what can be done today, but a path two lineages hold is then seen by
  neither, the problem G3 is for.

## Test plan

1. `the_order_from_0504`: no `--lineage`, `default` alone and the others
   said left out; an unknown lineage refused.
2. `a_path_taken_from_the_first_lineage`: a path two lineages hold, with
   different contents: the first's version advised, its lineage named;
   the overlap reported `differ`, the winner newest or not.
3. `the_same_once_canonical`: two lineages' versions differing only by
   what canonicalization drops: counted for that pair in the text,
   listed `same: true` in `--json`.
3a. `a_tilde_copy_overlaps_its_name`: one lineage's `a/b~2.pb` and
   another's `a/b.pb`, one descriptor name: an overlap.
3b. `newest_by_source`: a winner last written by a manual commit, newer
   than every source: not newest by its source's date; a version no
   source holds dated by its writer, said as such.
4. `one_lineage_as_today`: `--into H/L` and a history of one lineage,
   output unchanged.

## Measured outcome

Filled in at implementation.
