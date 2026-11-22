<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0455 — advise --report: what the build said, and what advise didn't foresee

Status: implemented
Implemented in: 2026-11-22
App: git-seriate-fdp
Refs: docs/specs/0454-advise-what-a-history-of-descriptors-cannot-hold.md
        (S4's problems and `--report`, which this extends),
      docs/specs/0446-build-report.md (the report's warnings and their
        fields),
      docs/specs/0445-filter-fixups-move.md (S6: W24, what a move costs an
        importer)

## Background

`advise --report FILE` (0454 S4) reads a protoback report back and
attaches its refused files (W19, W6), unresolved types (W4) and missing
imports (W5) to the problems `advise` found for the same file. What
attaches to none is reported as a problem of kind `build`.

A downstream repository with real version skew (1,118 snapshots, 16,164
files written) shows three gaps:

- **W24 isn't read.** `advise` moves a file forward when the older one
  defines symbols the newer lacks (0454 S3); an importer using them is
  then a `stale-importer`. protoback says what the move cost it: W24,
  with `importer`, `definition` and `move` (the old files). The report's
  two W24s attached nowhere and weren't shown.
- **Knock-on refusals stand alone.** 26 W19s were files refused because
  an import of theirs was (`the descriptor pool refused its import
  'c/summary.proto'`), and one W4 was in such a file. Each came out as a
  `build` problem. 0454 S4 reports a file refused for its import once,
  under the import; `advise`'s own pool does, the report's reading
  didn't.
- **No check of the prediction.** The downstream build failed on W24
  (`--fail-on W24`), which every forward move `advise` decided, and
  reported, now produces. What it wants to fail on is a finding of the
  build that `advise` didn't predict.

## Goals

- **G1.** Each W24 attached to the `stale-importer` problem of its
  importer and move.
- **G2.** A knock-on refusal, and what is said of a file refused that
  way, attached where the root refusal is.
- **G3.** `advise --report FILE --strict`: exit 1 when the build found
  something `advise` didn't foresee, and say what.

## Non-goals

- **N1.** Changing a decision. A forward move that costs an importer a
  type stays forward (0454 S3); no source holds an importer consistent
  with the newer file, and S7's candidates say which to extract again.
- **N2.** Reading codes beyond those listed in S1. W3, W23 and the
  others are what `fixups.yaml` asks for, or say nothing about a file's
  consistency.

## Specification

- **S1.** The codes read back: W19, W6, W4, W5 (0454 S4), and W24.

- **S2.** A W24 attaches to the `stale-importer` problem of the file its
  `importer` names whose import is one of its `move`'s old files, moved.
  If none matches, it is a problem of kind `build` of its own: a loss the
  analysis didn't see.

- **S3.** A W19, W20 or W6 carrying `import` (0446 S3, the knock-on form)
  attaches where its import's refusal attached, followed through imports
  refused the same way until a refusal without `import`: its root. The
  root's warnings are placed first, then the knock-on ones, so the order
  of the report doesn't matter. A root that attached nowhere is a `build`
  problem, and the knock-on refusals attach to it. An import chain that
  loops, or ends at a file the report doesn't refuse, attaches to the
  first file of the chain, as before.

- **S4.** A W4 on a file refused for its import attaches where that
  file's refusal attached (S3). Other W4s are attached as before.

- **S5.** A problem's findings from the build are listed under it, as
  now (`build:` lines, the JSON `build` list, `STATUS.md`).

- **S6.** `--strict`, with `--report` only (refused without it): exit
  status 1 if the report holds a finding `advise` didn't foresee, listed
  after the actions under the heading `Not foreseen:`. Not foreseen:

  - each `build` problem (S2, S3, and a finding on a file `advise` has
    no problem with);
  - each idle directive (a `stale-entry`): `advise` wrote that
    `fixups.yaml` from these sources, so an entry protoback found nothing
    to do with is a prediction that failed.

  Exit 1 also covers 0454 S1's cases (a tie, an out-of-order source); the
  printed output says which. Without `--strict`, the exit status is 0454
  S1's.

- **S7.** 0454 S1 and S4 are amended to point here.

## Alternatives considered

**Matching W24 to its importer's first problem**, as W4 is. An importer
may import several moved files; the problem of the move W24 names is the
one it explains.

**Folding knock-on refusals into a count** ("and 26 files refused for its
import"). They are the files lost to the root's refusal, which the reader
needs by name.

**`--strict` without the idle directives.** An idle entry costs the build
nothing, but it means the analysis and the build disagree about what the
input holds, which is what `--strict` exists to catch.

**A separate exit code for `--strict`.** Scripts that read `advise`'s
status read 1 as "look at the output" today; a second non-zero code would
be one more value to handle, for a distinction the output makes.

## Test plan

1. `w24_attaches_to_its_move` — a forward move, one importer using a
   definition only the older file has: the report's W24 is a `build:`
   line of that importer's `stale-importer`; a W24 naming another move
   is a `build` problem.
2. `knock_on_refusals_fold_under_the_root` — a root W19 on a file
   `advise` refused, two files refused through it (a chain), a W4 in the
   last: all attached to the root's `refused` problem, in any report
   order; the same with a root `advise` didn't refuse: one `build`
   problem holding them all.
3. `strict_names_what_was_not_foreseen` — `--strict` exits 0 on a report
   whose findings all attach, 1 with a `build` problem or an idle
   directive, listed under `Not foreseen:`; `--strict` without
   `--report` refused.

## Measured outcome

The tests of the test plan, on toy histories: `w24_attaches_to_its_move`
and `knock_on_refusals_fold_under_the_root` (`advise/report.rs`),
`strict_names_what_was_not_foreseen` (`tests/advise.rs`). A root refusal
attaches to its file's `refused` problem where there is one, rather than
to the file's first problem. Two W24s on one definition through two
moves are two findings.

Not measured here: the downstream repository whose report motivated this
(27 findings standing alone, two W24s unread). Its maintainers run
`advise --report … --strict` on it.
