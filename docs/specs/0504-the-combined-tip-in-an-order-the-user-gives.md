<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0504 — the combined tip, in an order the user gives

Status: draft (reviewed by the downstream, 2026-11-28)
App: git-seriate, git-seriate-fdp
Refs: docs/specs/0499-lineages-a-producer-s-snapshots-sorted-apart.md
        (G5: no merged tree; N3: the combined tip, the consumer's; S5:
        what reads a history),
      docs/specs/0503-advise-over-a-history.md (advise over the combined
        tip, in this order),
      protoback's `-I` roots (`protoback/src/load.rs`: a name across
        roots, the first wins, W7)

## Background

A history of several lineages (0499) has to be combined before a schema
is built from it: one include root per lineage's tip, in an order, as
`protoc -I a -I b` takes a path from the first root holding it. 0499
leaves that to the consumer (N3), and the order is the consumer's: which
producer's copy of a file it trusts most is no tool's to decide.

A downstream builds this way, and asks for the order as a generic
feature, given per run, on the commands that combine a history's
lineages: `advise` (0503), and a command laying out the roots for a
builder. Without it, every consumer writes its own layout script, and
`advise` reads one lineage while the build reads them all.

## Goals

- **G1.** An order of lineages, given per run, on every command that
  combines a history's lineages: the first named lineage holding a path
  gives its file, whole.
- **G2.** No order given: `default` alone, as reading `H/default` does
  today; a lineage comes in only when named.
- **G3.** A lineage the order leaves out is said, never silently
  forgotten.
- **G4.** `git seriate roots`: the named lineages' tips laid out, one
  root each, and the `-I` options printed in order, for protoback or
  protoc.
- **G5.** 0499 G5 kept: nothing merged in the history, the order
  recorded nowhere in it.

## Non-goals

- **N1. No order in the history:** a downstream gives it in its own
  commands (its README); one hash pins the lineages, not their
  combination. To revisit if a consumer needs it pinned.
- **N2. Nothing in `settle` or `compact`:** they sort and compact each
  lineage alone; the order matters only to what combines.
- **N3. No per-path override:** a path several lineages hold is reported
  (0503 S4), never fixed up.
- **N4. No order by name or by default beyond `default` alone:** an
  order nobody chose is no order.

## Specification

- **S1. `--lineage NAME`,** repeatable, in order, on the commands that
  combine (S4, and `advise --into H`, 0503). Each a lineage of the
  history, else refused, naming the history's lineages; one named twice,
  refused. None given: `default` alone (G2).
- **S2. The combined tip:** each item path from the first lineage named
  whose tip holds it, the whole file. A command reads it; none writes it.
- **S3. The lineages left out:** one line each, on standard error, for a
  lineage of the history the order doesn't name, with the number of
  items its tip holds: `lineage bundler left out: 41 files (name it
  with --lineage)`. Always printed, never an error.
- **S4. `git seriate roots --into H [--lineage NAME]… --out DIR`:**
  `DIR/<lineage>/`, for each lineage named, holding its tip's items (the
  item pattern's paths, as in the tree), and on standard output the
  roots in order, one `-I DIR/<lineage>` per line. `DIR` new or empty,
  else refused: roots of two runs never mix. A command of
  `git_seriate::cli::run`, so of every front end built on it, as
  `lineage` is.
- **S5. The build and `advise` agree:** protoback takes a path several
  `-I` roots hold from the first, and warns of each shadowed file (W7),
  as protoc does; so a build from S4's roots, in S4's order, reads S2's
  combined tip, and `advise` over it (0503) speaks of the files the
  build used, with one caveat: protoback shadows by descriptor name,
  S2 combines by path, and a `~N` copy (`a/b~2.pb`, named `a/b.proto`)
  has another path than its name's. One lineage's `~2` and another's
  `a/b.pb` both reach the build, which keeps the first root's and warns
  W7; `advise` counts them as an overlap of their name (0503 S4).

## Alternatives considered

- **The order recorded in the history's record:** one hash would pin the
  combined schema too. Declined by the downstream for now (N1); the
  order is a consumer's choice of trust, which may differ per build.
- **`default`, then the others by name, when none is given:** an order
  nobody chose (N4).
- **A merged branch:** 0499 G5.

## Test plan

1. `the_order_is_checked`: an unknown lineage, and one named twice,
   refused, naming the history's lineages.
2. `default_alone_without_an_order`: no `--lineage`, `default`'s tip
   only, each other lineage said left out with its count.
3. `roots_in_order`: two lineages holding one path with different
   contents; `roots` lays out both, prints `-I` in the order given; the
   first's file is the combined tip's.
4. `roots_refuses_a_used_directory`: `--out` naming a directory holding
   files, refused.
5. `protoback_reads_the_first_root`: protoback over `roots`' output, a
   path in both roots: the first's descriptor built, W7 for the other.

## Measured outcome

Filled in at implementation.
