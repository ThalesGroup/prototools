<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0451 — seriation: the sort, as a library

Status: implemented
Implemented in: 2026-11-22
App: seriation
Refs: docs/specs/0450-git-seriate-a-history-from-snapshots.md (the engine
        this crate is the first piece of; S1 its place, S2 the
        comparator and canonicalizer it is handed, S5 the order of work),
      docs/seriation/design.md, docs/seriation/scores.md,
      docs/seriation/greedy-search.md (the algorithm, as this crate
        implements it)

## Background

Spec 0450 splits the engine into three crates. This is the first: the
sort, which orders snapshots of files from their build times, their order
hints and how the files in them evolve. An implementation of it exists,
inside a tool that also reads git and one text comparator directly, with
its settings in thread-local globals (0450 Background). This crate is that
algorithm with no git, no file format and no globals.

## Goals

- **G1.** A library that orders snapshots given their metadata, the
  version key of each of their files, a comparator, and a way to fetch a
  version's contents. Nothing else.
- **G2.** On the same inputs and settings, the order the existing
  implementation gives (0450 G6).
- **G3.** A synthetic generator of corpora with known true orders, for
  the self-test and for G2.
- **G4.** `docs/seriation/` describes what this crate does. Where the
  implementation departs from it, the documents change with the code.

## Non-goals

- **N1.** The probing window (0450 N2).
- **N2.** Any comparator but the text one; the descriptor comparator is
  the protobuf layer's (0450 S5).
- **N3.** Parallelism. Phase 3's comparisons could run in parallel later,
  behind the same deterministic result.
- **N4.** Reading or writing git, or printing anything: the caller does
  both (`git-seriate`).

## Specification

### S1 — The interface (`seriation/src/lib.rs`)

```rust
pub struct Snapshot {
    pub identity: String,           // unique; the load order (S3)
    pub build_time: i64,            // seconds since the epoch
    pub order_hint: Option<u64>,
    pub items: Vec<(FileId, VersionId)>,
}

pub trait Contents {
    fn fetch(&mut self, file: FileId, version: VersionId) -> Result<Vec<u8>, BoxError>;
}

pub trait Comparator {
    fn compare(&self, a: &[u8], b: &[u8]) -> Containment;
}

pub enum Containment { Equal, FirstInSecond, SecondInFirst, Incomparable }

pub fn sort(snapshots: &[Snapshot], contents: &mut dyn Contents,
            comparator: &dyn Comparator, settings: &Settings)
    -> Result<Sorted, Error>;

pub struct Sorted { pub order: Vec<usize>, pub phases: Vec<PhaseReport> }
```

- `FileId` and `VersionId` are `u32`s the caller assigns: the caller owns
  paths and blob identities; the crate only compares keys.
- `Contents` is called file by file, each distinct version once per file;
  nothing fetched is kept past that file.
- `PhaseReport`, one per phase, skipped ones included: the moves, phase
  3's comparisons and signals, and every score known after the phase (the
  edge score once the graph exists, the interleaving score after phases 3
  and 4, tau always). *Amended 2026-11-22:* phases 3 and 4 also list
  their moves, in the order made (`moved: Vec<Move>`): the snapshot
  moved, its position before and after, the score it gained, and its
  neighbors right after the move, so that a caller can tell what the
  search did without replaying it. The order is unchanged.
- `LinesChars` is the text comparator of design.md.
- The crate prints and logs nothing.
- Refused with an `Error`, never a panic: contradictory settings, two
  snapshots with one identity, a snapshot listing a file twice, a file in
  more than 65 535 snapshots (the interleaving score's slots are `u16`),
  more than `u32::MAX` snapshots.

### S2 — Settings

`window` (30 days), `nom_tries` (1, at least 1), `max_tries` (20, at
least `nom_tries`), `order_hints` (off), `skip` (phases 1, 3, 4; none),
`reveal_percent` (100, at most 100). Passed in, never read from globals.

`order_hints` is phase 2 *and* its barriers in phases 3 and 4, one
setting: the existing implementation runs the placement only with the
barriers on, and the barriers have nothing to hold without it.

### S3 — What decides the order

Every rule that chooses between moves is the existing implementation's,
since G2 depends on them all (greedy-search.md): load order by identity,
bytewise; the build-time sort stable; phase 2's packing and placement as
design.md gives them, centroids rounded down; candidates by rank, ties by
position; destinations leftwards from `from − 1`, then rightwards, each
direction stopping at a barrier; the first improving move kept unless a
later one is strictly better; the stopping rule on `nom_tries`.

Changed where the order cannot tell:

- an edge move is probed from the moved snapshot's own edges, not by
  searching every file for it;
- the full recomputation of the score at each iteration is a debug
  assertion, not production work; the interleaving state is not rebuilt
  after each phase 3 move just to report it;
- the text comparator matches a truncated UTF-8 sequence as far as it
  goes, where the existing one indexes past the needle.

An order hint of 0 is a hint: `Option<u64>` says "unknown", and the
store's 0-for-unknown (0450 S3) is the caller's to translate.

### S4 — The self-test

`reveal_percent` shows the sort a share of the order hints. An identity's
hint is shown when its hash, modulo 100, is below the percentage; the
hash multiplies then adds each byte, from FNV-1a's offset basis with its
prime, as the existing implementation does (it is not FNV-1a, which xors
first). Tau is always over every true hint, hidden ones included.

### S5 — The generator (`seriation/src/generate.rs`, `seriation-gen`)

`seriation-gen [--name=value]… OUT` writes `OUT/manifest.json` (the
command, then each snapshot's identity, build time, order hint and true
revision) and `OUT/snapshots/<identity>/…`, the snapshot's files.

- **A history:** text files evolving over revisions. At each revision a
  file changes with probability 30%: a line appended (40%), a line
  inserted (25%), characters inserted within a line (25%), back to its
  previous version (7%), deleted (3%). A new file appears with probability
  10%.
- **Snapshots:** drawn by build; a build has a revision, a build time
  within `build_noise_seconds` of the revision's (revision 0 at
  1 700 000 000, so that none is negative, which a git date can't be),
  and an order hint: the revision, missing with `hint_missing_percent`,
  the previous revision's with `hint_shared_percent`. Each of a build's `per_build` snapshots holds
  each of the revision's files with probability 70%. Identities are
  `buildNNNN/partK`, valid git branch names.
- **Its own PRNG** (SplitMix64): a dependency update must never change a
  corpus.

**The corpora are not committed**, only their parameters:
`seriation/tests/corpora/NAME.args`, one option per line. The generator is
deterministic and in the crate, so the test regenerates each corpus in
memory; committing its files (about 2 000 small ones for three corpora)
would add bytes and no information.

### S6 — Agreement with the existing implementation (G2)

`NAME.expected.txt`, beside `NAME.args`: the order the existing
implementation gives on the corpus with the default settings, one
identity per line. Where it exists, the test requires the same order. A
difference is either a bug here, or a behavior to decide on: then the
decision is recorded in the documents (G4), and the expected order
replaced, with the reason in the commit.

To import a corpus as its snapshot branches, the existing tool maps
`buildNNNN/partK` to names whose byte order is the identities' (its load
order), the build time to the committer date, and the hint to the author
date. Its 0-for-unknown loses the hint of revision 0, which the default
settings never read: hints only act through `order_hints`.

## Alternatives considered

**Generic version keys** (`K: Ord + Hash`) instead of `u32` indices. The
scores compare keys in their innermost loops, and the existing
implementation already deduplicates to indices for that reason.

**A comparator that reads contents itself,** given paths. The crate would
depend on where contents are stored; `Contents` keeps that the caller's.

**Committing the corpora's files,** as `fdp-scan`'s corpus is. That one is
the output of Python's protobuf, which the test cannot rerun; this one is
the crate's own deterministic output.

**The `rand` crate for the generator.** Its streams have changed between
versions.

## Test plan

`seriation/src/tests.rs`: each score's probe against the change of a full
computation over 200 random moves, with `begin` then `end` leaving score
and ranks unchanged (edge, interleaving); the edge ranks summing to twice
(disagreeing − agreeing); a growing history sorted closer to its hints
than build times sort it; shuffled input giving the same order and
reports; phase 2 putting every hint in order and the barriers keeping it;
hidden hints still counted by tau; skipped phases reported; bad settings,
duplicate identities and files, and the instance limit refused.
`kendall.rs`: inversions against a brute-force count. `order_hints.rs`:
the longest non-decreasing subsequence. `text.rs`: the four outcomes,
characters inserted within a line, no match across a newline, a final
newline, whole characters. `generate.rs`: determinism, arguments round
trip, builds sharing a revision and a time. `tests/corpora.rs`: each
corpus sorted closer to its true revisions than its build times, and S6.

## Measured outcome

Inversions against the true revisions (2026-11-22), the build-time order
against the sort's:

| Corpus | Snapshots | By build time | Sorted |
|---|---|---|---|
| `plain` | 60 | 53 | 12 |
| `noisy` (build times ±10 days, 40% of hints missing) | 80 | 312 | 52 |
| `builds` (10 snapshots per build) | 100 | 100 | 66 |

**G2 holds on all three:** the order is the existing implementation's,
identity for identity (`NAME.expected.txt`, its `merge-snapshots` with the
default settings, the corpora imported as its snapshot branches).
Perturbing one line of an expected order fails the test, so the check is
live.
