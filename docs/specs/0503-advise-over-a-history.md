<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0503 — advise over a history: the lineages read as the consumer combines them

Status: draft (questions for the downstream below)
App: git-seriate-fdp
Refs: docs/specs/0499-lineages-a-producer-s-snapshots-sorted-apart.md
        (G5: no merged tree; N3: the combined tip, the consumer's, one
        include root per lineage, the order as precedence; S5: `advise`
        reads one lineage),
      docs/specs/0454-advise-what-a-history-of-descriptors-cannot-hold.md
        (what `advise` reads, S2, and reports)

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
  combines them, by a precedence the consumer gives.
- **G2.** Each problem named with its lineage.
- **G3.** The paths several lineages hold reported: which one wins,
  whether the others' versions differ once canonical, and, if they do,
  whether the winner's is the newest.
- **G4.** 0499 G5 kept: nothing merged in the history, the combination
  read only; what wins is the consumer's.

## Non-goals

- **N1. No precedence inferred** from contents, dates or names: given,
  or `advise` refuses a history of several lineages.
- **N2. No combined tree written,** in the history or elsewhere: the
  consumer's include roots stay the build (0499 N3).
- **N3. One lineage unchanged:** `--into H/L`, and a history of one
  lineage, read as today.

## Specification

- **S1. The precedence:** `--precedence L1,L2,…`, else git config
  `seriate.precedence` (the same list); every lineage of the history
  named once, else refused, naming those missing or unknown. A history of
  one lineage needs none.
- **S2. The combined reading:** each lineage read as today (its tip's
  files, their writers, its sources); a path taken from the first
  lineage of the precedence holding it, with that lineage's writer; the
  sources of all, each with its lineage. Then `advise` as today over
  that reading.
- **S3. Each problem's lineage:** every fixup, candidate and status line
  about a path names the lineage it was taken from (`[bundler]`), and
  `--json` gives it as `lineage`.
- **S4. Paths held by several lineages:** a section, `Held by several
  lineages`, one line per path: the winner, the others, and either
  `same once canonical`, or `differ`, with whether the winner's version
  is the newest (its writer's build time against the others').
  `--json`: `overlaps`, each `{path, winner, others, same, winner_newest}`.

## Questions for the downstream

- **Q1. The precedence's home:** an option, git config, or both (S1
  proposes both, the option winning)? A history pushed and cloned keeps
  no config: does the precedence travel with the build's own files
  instead?
- **Q2. Paths the same once canonical:** reported (S4 proposes yes, for
  a complete list), or left out, being harmless?
- **Q3. "Newest":** the build time of the commit that last wrote the
  path in each lineage (S4)? Or the newest source holding that version,
  as `advise` dates a file?
- **Q4. The other sections over a combination:** candidates and `--report`
  (0455) read per path, so they combine as S2 does. Does anything of
  yours read `advise`'s output per lineage, which S3's naming would
  change?

## Alternatives considered

- **A combined branch written by `settle`:** what 0499 G5 rules out; a
  merged tree in the history would be an inference git-seriate doesn't
  make.
- **`advise` run once per lineage, the consumer combining the outputs:**
  what can be done today, but a path two lineages hold is then seen by
  neither, the problem G3 is for.

## Test plan

1. `precedence_required_and_checked`: a history of two lineages refused
   without one; a precedence missing a lineage, or naming an unknown
   one, refused, naming them.
2. `a_path_taken_from_the_first_lineage`: a path two lineages hold, with
   different contents: the first's version advised, its lineage named;
   the overlap reported `differ`, the winner newest or not.
3. `the_same_once_canonical`: two lineages' versions differing only by
   what canonicalization drops: reported `same once canonical`.
4. `one_lineage_as_today`: `--into H/L` and a history of one lineage,
   output unchanged.

## Measured outcome

Filled in at implementation.
