<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0499 — lineages: a producer's snapshots, sorted apart

Status: draft
App: git-seriate, git-seriate-fdp
Refs: docs/specs/0495-extract-makes-atomic-snapshots.md
        (the atomic snapshots a lineage is a property of),
      docs/specs/0451-seriation-the-sort-as-a-library.md
        (phase 3's containment, phase 4's straddles),
      docs/specs/0452-git-seriate-the-store-and-the-history.md
        (snapshot branches and the history branch, which this re-lays),
      docs/specs/0497-forget-a-snapshot-and-prune-instead-of-drop.md
        (`refs/seriate/forgotten/`, the namespace this completes)

## Background

The sort reads a history as one line of versions: a copy contained in
another is older (phase 3), and a version appearing on both sides of
another is a straddle (phase 4). Two producers of one file break both
assumptions when one of them transforms it. Measured downstream: a schema
bundler that drops custom options embeds copies of files whose full copy
(A) is the same in older builds and in the bundle's host. With atomic
snapshots (spec 0495), each bundle is a snapshot of its own, built at its
host's revision, and its stripped copy (B) sits at the file's own path:

```
A A A A A A A A   B B B B B B B   A A A A ... A
older builds      the bundles     the host, later builds
```

Wherever the sort puts B on one line, it sits inside A's span: 375
straddles inside nine bundles, of a history's 410 (35 elsewhere, against
67 before the split). Placing B anywhere is wrong; B belongs to another
line, the bundler's.

## Goals

- **G1.** A lineage per producer transformation: snapshots whose copies a
  producer transforms the same way sorted as a line of their own, the
  sort's two assumptions holding within it.
- **G2.** A snapshot's lineage decided once, at extraction, from evidence
  `extract` holds, recorded in its block; redone, as a split is, by
  `forget` and `extract` again (specs 0495 S6, 0497).
- **G3.** Named by the transformation found: `full` (none), `options-
  dropped`; a reader recognizes them in `git branch`.
- **G4.** The history laid out so that each lineage is a branch, and one
  hash pins every lineage's tip.
- **G5.** Combining the lineages' tips into one schema stays outside
  git-seriate: no merged tree, nothing inferred in a commit.

## Non-goals

- **N1.** Transformations other than dropped options (stripped source
  info, renamed packages, …): each a lineage of its own when one is
  measured. The mirror case, a set carrying more options than its host,
  is one: phase 3 then reads the host as older, which costs little while
  they share a build time; an "options-added" lineage if it ever matters.
- **N2.** A lineage derived after extraction, by `settle` or `advise`:
  `advise` may report a snapshot it suspects belongs to another lineage;
  it never moves one (0495 S8).
- **N3.** The combined tip: the consumer's (a directory built from the
  tips, one include root per lineage, the include order as precedence, as
  a downstream proposed).

## Specification

- **S1. A snapshot's lineage, at extraction:** when an input splits
  (0495), each set's copies are compared with the copies of the same
  paths in its input's rest, the host's compiled copies, both
  canonicalized first by the history's rule sets (which drop, for
  example, a `json_name` equal to protoc's default):
  - **`options-dropped`:** every shared copy is the host's minus some
    entries and never more: options entries, custom or standard, on any
    element (the file, a message, a field, an enum, a value, a service, a
    method, a oneof), and `json_name`s; nothing else differing; and at
    least one copy strictly smaller. Options are compared as multisets of
    entries, their order set aside;
  - **`full`:** otherwise, and for every snapshot no host is compared
    with (the rest itself, an unsplit input, an `--as-one` group).

  The direction is what tells a lossy producer apart: measured
  downstream, a bundler's sets carry fewer entries than their host in
  every differing file and never more, while other embedded sets carry
  more, never fewer (their host's compiled copies being the leaner
  ones). A set equal to its host once canonical is `full`: nothing tells
  it apart.

  Recorded in the block: `source.lineage: <name>`, and the evidence, the
  shared paths compared and how many were strictly smaller. A snapshot
  of format 1, or with no `lineage`, is `full`.
- **S1a. A lineage stated by the caller:** `extract --lineage NAME FILE…`
  for inputs the caller knows a producer made, such as schema bundles
  shipped alone, with no host to compare with; recorded as S1's, with
  "stated by the caller" as its evidence, and redone like any other, by
  `forget` and `extract`. S1's test is not run on an input with a stated
  lineage.
- **S2. The layout:**
  - **snapshots under `refs/seriate/snapshots/<identity>`,** no longer
    branches: `git branch` shows histories only, and a snapshot is known
    by its namespace, not by a parentless tip;
  - **a history is a set of lineages, a branch each,
    `<history>/<lineage>`:** `main/full`, `main/options-dropped`. Git
    keeps a ref and a directory of refs apart, so a history is never also
    a branch of its own name;
  - **the history's record, `refs/seriate/histories/<history>`:** a commit
    per settle whose tree lists the lineages and their tips, and whose
    parents are the previous record and every lineage's tip, so that one
    hash pins the history as of that settle and keeps every tip
    reachable through a re-sort.
- **S3. `settle --into <history>`:** each candidate goes to its lineage's
  branch, created on first need; each lineage sorted on its own, phase 3
  comparing copies within it only; the record then written. A snapshot
  is never compared with another lineage's copies: containment across
  producers is the false signal this spec removes.
- **S4. Migration, once, per repository:** `git seriate migrate`: snapshot
  branches to `refs/seriate/snapshots/`, each history `NAME` to
  `NAME/full` (its snapshots being `full`, S1), the record written, the
  former branches kept under `refs/seriate/migrated/` until pruned.
  Commands refuse a repository not yet migrated, naming `migrate`;
  `import` and `--tar-out` write the new layout.
- **S5. What reads a history:** `advise`, `list`, `compact`, `forget` and
  `prune` read every lineage of a history, and name a commit's lineage
  where it matters (`forget` rewrites the lineage holding the snapshot).

## Open questions

- **Q1.** *Answered by S1a:* a bundle shipped alone, with no host to
  compare with. A downstream holds six such archives, at several
  versions, which meet the full copies on one line: the conflict this
  work started from. A local test (it declares an extension none of its
  own files use) misfired downstream on single modules, so the caller,
  who knows what its archives hold, states the lineage.
- **Q2.** *Settled, `full`:* a set equal to its host once canonical
  (measured downstream: a small bundle of 3 files) sorts with its host
  either way.
- **Q3.** *Settled, one step:* a downstream pushing to a server that
  checks committers and signatures accepts new ref names, the commits
  being written as today; its users adapt their refspecs once.

## Test plan

1. A toy history as the downstream's case in small: an enum file, its
   full copy in executables, its options-dropped copy in a bundle set
   whose host holds the full one; the bundle's block `lineage:
   options-dropped`, the host's rest `full`; settled, `main/full` and
   `main/options-dropped`, each one line, no straddle; the record's
   parents the two tips.
2. A set whose copies equal its host's once canonical: `full`. A set
   carrying more options than its host in one file: `full`. A set
   differing from its host in a field other than options: `full`.
2a. A set differing by a standard option and a `json_name` not dropped
   by the rule sets, both fewer: `options-dropped`.
2b. `extract --lineage options-dropped` on an input with no host: its
   block says so, with "stated by the caller".
3. `migrate` on a history of today's layout: the same commits under the
   new names, the refusal before it, nothing lost.
4. A re-sort of one lineage: the record's new commit keeps the other
   lineage's tip as a parent, the previous record too.
5. Downstream, on the measured history: the 375 straddles inside the
   bundles gone, the 35 elsewhere unchanged. Their 31 sets: the 9
   straddling bundles `options-dropped`, the 22 others `full` (measured
   with a prototype of S1's test, before implementation).

## Measured outcome

Filled in at implementation.
