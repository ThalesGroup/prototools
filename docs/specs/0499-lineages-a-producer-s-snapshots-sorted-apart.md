<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0499 — lineages: a producer's snapshots, sorted apart

Status: implemented
Implemented in: 2026-11-28
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
- **G2.** A snapshot's lineage stated by whoever knows its producer,
  after extraction, from the snapshots as extracted, never derived from
  how their contents differ from other copies; the statement kept beside
  the snapshots, not in them: a snapshot holds what the bytes show, a
  lineage is what the caller knows. Extraction stays one pass, and a
  lineage is corrected by stating it again, with no extraction.
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
- **N2.** A lineage derived by `settle` or `advise`: `advise` may report
  a snapshot it suspects belongs to another lineage; it never states one
  (0495 S8).
- **N4.** A lineage stated at extraction, in the snapshot's block: drafted
  and dropped. The caller can only recognize a producer's output among
  the snapshots once they are extracted, which forced two extraction
  passes, and correcting a lineage meant `forget` and `extract` again for
  what is no property of the bytes.
- **N3.** The combined tip: the consumer's (a directory built from the
  tips, one include root per lineage, the include order as precedence, as
  a downstream proposed).

## Specification

- **S1. Lineage statements, `git seriate lineage`:**
  - **`lineage set NAME IDENTITY…`**, and **`--from FILE`**, one
    `<identity> <lineage>` per line (`-` for standard input): the
    snapshots named go to lineage `NAME`. An identity neither the store
    nor any branch's history holds (a backup such as `H/L-N` included) is
    refused, naming it, nothing written, so that a statement cannot
    silently miss;
  - **`lineage unset IDENTITY…`:** back to `default`;
  - **`lineage list [NAME]`:** the statements in force;
  - **nothing stated:** `default`.
  - **Statements outlive their snapshots:** only `lineage` changes them.
    `prune`, `forget` and `compact` leave the statement of a snapshot they
    remove, a fact about its producer: the input extracted again goes back
    to its lineage. `lineage list` shows such a statement as any other.
    *Amended 2026-11-28,* from a downstream's `compact` (0501) dropping a
    set whose statement stayed.

  The caller decides from what it knows of the producer, reading the
  snapshots as extracted (each set's identity and range are in its
  record), outside git-seriate: a downstream recognizes its bundler's
  output format, the message enclosing a set's records; git-seriate holds
  no producer's knowledge. `NAME` is a ref component, `default` reserved.
- **S1b. The statements' record, `refs/seriate/lineages`:** a commit per
  `lineage` command, its tree one file, `lineages`, the whole mapping in
  force (`<identity> <lineage>`, sorted), its message the command and the
  identities changed, its parent the previous record. Pushed and fetched
  like the snapshots; one hash pins every statement as of a time.
- **S1a. Producer evidence, recorded, not acted on:** what extraction
  sees of how a copy was produced, recorded in the block (0494's
  evidence): how it was found (compiled into a module, a framed set, a
  file of its own) and the generator hints read (0492: Go's, C++'s,
  upb's, Python's). It is there for the caller and for `advise` to read;
  it decides no lineage. A caller's policy mapping evidence to lineages
  (by generator, say) is the caller's, applied with `lineage set`.
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
    parents are the previous record, every lineage's tip and the
    statements' record it routed by (S1b), so that one hash pins the
    history and the statements as of that settle, and keeps every tip
    reachable through a re-sort.
- **S3. `settle --into <history>`:** each candidate goes to the branch of
  its lineage as the statements stand, created on first need; each
  lineage sorted on its own, phase 3 comparing copies within it only; the
  record then written. A snapshot is never compared with another
  lineage's copies: containment across producers is the false signal
  this spec removes.
- **S3a. A lineage changed after a settle:** the next `settle` moves the
  snapshot: its former lineage rewritten without it, as `forget` rewrites
  (0497: from the oldest commit concerned, hand-made commits refused out
  of reach), and its new lineage sorting it in. `--dry-run` names each
  snapshot moved, from and to. Nothing moves before a `settle`.
- **S4. Migration, once, per repository:** `git seriate migrate`:
  snapshot branches to `refs/seriate/snapshots/`, then each history:
  - **`migrate HISTORY --into NEW`:** `NEW/default` written from
    `HISTORY` (no lineage being stated, S1) and `NEW`'s record, `HISTORY`
    left as it is, an ordinary branch no command reads as a history any
    more; `NEW` refused if it exists. Git holds no `NAME` beside
    `NAME/default`, so a history pushed to a server where its branch is
    protected (or the default branch) migrates under a new name, pushed
    beside the old one, which its owner retires when the new is checked;
  - **`migrate HISTORY`, in place:** `HISTORY` to `HISTORY/default`, the
    former branch kept under `refs/seriate/migrated/` until pruned.

  Commands refuse a repository whose snapshots are still branches,
  naming `migrate`; `import` and `--tar-out` write the new layout.
- **S5. What reads a history:** `advise`, `list`, `compact`, `forget` and
  `prune` read every lineage of a history, and name a commit's lineage
  where it matters (`forget` rewrites the lineage holding the snapshot).
- **S5a. The records' commits:** the statements' record (S1b) and the
  history's (S2) are written as history commits are: the committer and
  signing of 0452 S5 step 6, and the user running the command as their
  author (0471). Neither reaches a snapshot commit: the history's record
  has as parents only its previous record, lineage tips (history
  commits, whose parents are history commits) and a statements' record;
  the statements' record only its previous one, its tree naming
  identities as text. So pushing the histories and the two records
  carries no snapshot; `refs/seriate/forgotten/` (0497) does, its
  parents being snapshots, and stays local where snapshots do. A
  guarantee: an invariant of `docs/invariants/`, with its regression test
  (a push of the histories and records to a bare repository holds no
  snapshot commit), at implementation.
- **S6. `advise` reports what contents suggest:** a snapshot whose copies
  sit inside another copy's span in its lineage (a straddle) and differ
  from that copy only by options or `json_name`s is reported, with the
  paths, as possibly another producer's output; so is a snapshot of a
  stated lineage whose copies equal `default`'s once canonical. The
  report names the remedy, `lineage set` then `settle`; `advise` never
  states a lineage (N2).

## Open questions

- **Q1.** *Answered:* what tells a downstream's bundles apart at
  production. Not build metadata (none survives in a host), nor a path,
  nor a marker file (present in most bundles, not all: a fit again), but
  the bundler's output format: a message of its own enclosing the set's
  records, with its source list and metadata, known from its schema. It
  identifies all 10 bundles and none of the 21 other sets of one client,
  the 11 of another. The downstream's extractor states it per set
  (`lineage set --from`), bundles shipped alone by their package. One
  producer, one lineage, `bundler`.
- **Q2.** *Settled, one step:* a downstream pushing to a server that
  checks committers and signatures accepts new ref names, the commits
  being written as today; its users adapt their refspecs once.

## Test plan

1. A toy history as the downstream's case in small: an enum file, its
   full copy in executables, a bundler's copy dropping its options in a
   set embedded in a host holding the full one beside another set;
   extracted, then `lineage set bundler` on the bundler's set only;
   settled, `main/default` (the other set and the host's rest) and
   `main/bundler`, each one line, no straddle; the record's parents the
   two tips and the statements' record.
2. The same with nothing stated: one lineage, `default`, its straddles as
   today, and `advise` reporting the set (S6) with the remedy; then
   `lineage set` and `settle`: the set moved (S3a), `--dry-run` naming
   it, the straddles gone.
2a. A bundler whose later version keeps every option: its copies stay in
   `bundler`, by the statement, whatever their contents.
2b. `lineage set` naming an identity neither the store nor a history
   holds: refused, naming it, nothing written; `lineage set default …`: refused; `unset`: back
   to `default`, moved at the next settle.
2c. The statements' record: one commit per command, the whole mapping in
   its tree, the previous record its parent.
3. `migrate` on a history of today's layout: the same commits under the
   new names, the refusal before it, nothing lost; `--into NEW`: `NEW/default`
   with the history's commits, `HISTORY` unchanged, `NEW` refused when it
   exists.
3a. S5a: the records' commits signed and authored as history commits;
   the histories and the records pushed to a bare repository, no snapshot
   commit in it.
4. A re-sort of one lineage: the record's new commit keeps the other
   lineage's tip as a parent, the previous record too.
5. Downstream, on the measured history, with the lineages stated by its
   extractor: the 375 straddles inside the bundles gone, the 35
   elsewhere unchanged.

## Measured outcome

Implemented in four steps, each its tests: the layout and `migrate` of
snapshots; `lineage set`, `unset`, `list` and their record; histories as
a branch per lineage, their record, the moves (S3a), `migrate` of
histories in place and `--into`; `advise`'s report (S6) and invariant
0002 (`docs/invariants/0002-pushed-histories-carry-no-snapshot.md`),
with its regression test.

Decided at implementation, beyond the text above:

- **HEAD, unborn and naming the history** (a fresh `git init -b main`),
  is pointed at the lineage `settle` writes: git holds no `H` beside
  `H/default`, so HEAD would name a branch that can never be.
- **A lineage every snapshot of which moves out, or is forgotten,** is
  emptied: its branch deleted, its tip saved as `H/L-N`, gone from the
  record. Forgetting every snapshot of the history stays refused.
- **`--into H/L`** names the history `H`; `advise` reads one lineage,
  `H` when it has one, else `H/L`, the refusal naming them.
- **One signer per command:** an encrypted key's passphrase is asked
  once, the records signed with the history commits.
- **On the toy histories, the sort spares a straddle when it can:** a
  stripped copy is moved out of the full copy's span unless other files
  forbid it, as a bundle's other files do. S6's test needed three files
  making every order straddle one.

Measured downstream (test plan item 5), on a copy of a history of 59
snapshots migrated with `--into`, the nine bundle sets stated by hand:

| | snapshots | files | straddles |
|---|---|---|---|
| before, one lineage | 59 | 18,315 | 410 (375 in the bundle sets, 35 elsewhere) |
| `default` | 50 | 18,291 | 17, on 6 files |
| `bundler` | 9 | 330 | 0 |

The 375 are gone, the nine sets mutually consistent. Elsewhere fell from
35 to 17: copies of one executable that had sat among the bundles' no
longer straddle. Before anything was stated, `advise` (S6) named all
nine bundle sets (15 to 51 paths each) and 14 other snapshots (1 to 3
paths each); after the move, nothing on `bundler`, 13 on `default`, not
yet looked at downstream.

Changed from the downstream's review: the statements' record's message
names `--from FILE` or `--from -`, never the file's path, since the
record is shared.
