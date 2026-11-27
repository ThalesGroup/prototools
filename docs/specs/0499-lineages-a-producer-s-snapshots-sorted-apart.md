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

- **G1.** A lineage per producer: the snapshots one producer emitted
  sorted as a line of their own, the sort's two assumptions holding
  within it.
- **G2.** A snapshot's lineage decided once, at extraction, from what is
  known of how its copies were produced, never from how their contents
  differ from other copies; recorded in its block; redone, as a split
  is, by `forget` and `extract` again (specs 0495 S6, 0497).
- **G3.** Named by the producer, readable: `default` for the snapshots no
  producer is stated for, and the caller's names (`bundler`, say) for the
  others; a reader recognizes them in `git branch`.
- **G3a.** Nothing fitted to a corpus: no threshold, direction or
  difference measured on one history decides a lineage. A producer's
  output stays in its lineage when the producer changes what it drops or
  keeps.
- **G4.** The history laid out so that each lineage is a branch, and one
  hash pins every lineage's tip.
- **G5.** Combining the lineages' tips into one schema stays outside
  git-seriate: no merged tree, nothing inferred in a commit.

## Non-goals

- **N1.** A lineage inferred from contents: a test of how a snapshot's
  copies differ from its host's (fewer options, never more, once
  canonical) was drafted and dropped. It classified one history's sets
  correctly, but by properties of that history (which entries its
  bundler drops, which its rule sets remove, which direction the other
  sets differ in), each a threshold to retune when the corpus changes,
  and it would move a producer's output out of its lineage the day the
  producer kept one option more. Such differences are `advise`'s to
  report (S6), not a lineage's to decide.
- **N2.** A lineage derived after extraction, by `settle` or `advise`:
  `advise` may report a snapshot it suspects belongs to another lineage;
  it never moves one (0495 S8).
- **N3.** The combined tip: the consumer's (a directory built from the
  tips, one include root per lineage, the include order as precedence, as
  a downstream proposed).

## Specification

- **S1. A snapshot's lineage is stated, at extraction,** by whoever knows
  the producer: the caller, who knows what its inputs are, as it knows a
  group (`--as-one`, 0495 S3):
  - **`extract --lineage NAME FILE…`:** every snapshot of these inputs,
    the rest and its sets, in lineage `NAME`, for inputs a producer made
    whole (bundles shipped alone, say);
  - **`extract --lineages-from FILE INPUT…`:** a statement per snapshot,
    for hosts embedding a producer's output beside other sets: `FILE`
    holds one `<identity> <lineage>` per line, identities as extraction
    names them (`<input>!set@<start>`, deterministic for given bytes, so
    a first `--tar-out` run lists them); a snapshot not listed is
    `default`; a listed identity the run does not produce is refused,
    naming it, so that a statement cannot silently miss;
  - **nothing stated:** `default`.

  The caller decides from what it knows of the producer, outside
  git-seriate (a downstream recognizes its bundler's output format, the
  message enclosing the set's records); git-seriate holds no producer's
  knowledge. `--lineage` and `--lineages-from` conflict.

  Recorded in the block: `source.lineage: <name>`, with "stated by the
  caller" as its evidence. A snapshot of format 1, or with no `lineage`,
  is `default`.
- **S1a. Producer evidence, recorded, not acted on:** what extraction
  sees of how a copy was produced, recorded in the block (0494's
  evidence): how it was found (compiled into a module, a framed set, a
  file of its own) and the generator hints read (0492: Go's, C++'s,
  upb's, Python's). It is there for the caller and for `advise` to read;
  it decides no lineage. A rule mapping evidence to a lineage, if a
  producer ever leaves a mark of its own in the bytes, is a later spec,
  on a producer measured in more than one corpus.
- **S2. The layout:**
  - **snapshots under `refs/seriate/snapshots/<identity>`,** no longer
    branches: `git branch` shows histories only, and a snapshot is known
    by its namespace, not by a parentless tip;
  - **a history is a set of lineages, a branch each,
    `<history>/<lineage>`:** `main/default`, `main/bundler`. Git
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
  `NAME/default` (no lineage being stated for them, S1), the record written, the
  former branches kept under `refs/seriate/migrated/` until pruned.
  Commands refuse a repository not yet migrated, naming `migrate`;
  `import` and `--tar-out` write the new layout.
- **S5. What reads a history:** `advise`, `list`, `compact`, `forget` and
  `prune` read every lineage of a history, and name a commit's lineage
  where it matters (`forget` rewrites the lineage holding the snapshot).
- **S6. `advise` reports what contents suggest:** a snapshot whose copies
  sit inside another copy's span in its lineage (a straddle) and differ
  from that copy only by options or `json_name`s is reported, with the
  paths, as possibly another producer's output; so is a snapshot of a
  stated lineage whose copies equal `default`'s once canonical. The
  report names the remedy, `forget` and `extract` with a lineage stated;
  `advise` never moves a snapshot (N2).

## Open questions

- **Q1.** *Answered:* what tells a downstream's bundles apart at
  production. Not build metadata (none survives in a host), nor a path,
  nor a marker file (present in most bundles, not all: a fit again), but
  the bundler's output format: a message of its own enclosing the set's
  records, with its source list and metadata, known from its schema. It
  identifies all 10 bundles and none of the 21 other sets of one client,
  the 11 of another. The downstream's extractor states it per set
  (`--lineages-from`); bundles shipped alone, by their package
  (`--lineage`). One producer, one lineage, `bundler`.
- **Q2.** *Settled, one step:* a downstream pushing to a server that
  checks committers and signatures accepts new ref names, the commits
  being written as today; its users adapt their refspecs once.

## Test plan

1. A toy history as the downstream's case in small: an enum file, its
   full copy in executables, a bundler's copy dropping its options in a
   set embedded in a host holding the full one beside another set,
   extracted with `--lineages-from` naming the bundler's set only; that
   set's block `lineage: bundler`, stated
   by the caller, the other set and the host's rest `default`; settled, `main/default` and
   `main/bundler`, each one line, no straddle; the record's parents the
   two tips.
2. The same extracted with nothing stated: one lineage, `default`, its
   straddles as today, and `advise` reporting the set (S6) with the
   remedy.
2a. A bundler whose later version keeps every option: its copies stay in
   `bundler`, by the statement, whatever their contents.
2b. `extract --lineage bundler` on an input with no host: every snapshot
   in `bundler`.
2c. `--lineages-from` listing an identity the run does not produce:
   refused, naming it, nothing written; with `--lineage`: refused.
3. `migrate` on a history of today's layout: the same commits under the
   new names, the refusal before it, nothing lost.
4. A re-sort of one lineage: the record's new commit keeps the other
   lineage's tip as a parent, the previous record too.
5. Downstream, on the measured history, with the lineages stated by its
   extractor: the 375 straddles inside the bundles gone, the 35
   elsewhere unchanged.

## Measured outcome

Filled in at implementation.
