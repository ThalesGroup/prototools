<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0500 — a snapshot met again: the original preferred, a conflict said

Status: draft
App: git-seriate, git-seriate-fdp
Refs: docs/specs/0452-git-seriate-the-store-and-the-history.md
        (the store, `import`, a snapshot rebuilt from the history),
      docs/specs/0464-git-seriate-a-canonical-history-taken-as-it-is.md
        (the stamp, `canon_rules`, and which commits are trusted),
      docs/specs/0497-forget-a-snapshot-and-prune-instead-of-drop.md
        (S5, `prune`, whose promise this replaces),
      docs/specs/0499-lineages-a-producer-s-snapshots-sorted-apart.md
        (snapshots under `refs/seriate/snapshots/`, histories of lineages)

## Background

A history stores each snapshot's items in canonical form, `C(raw)`, the
rule sets recorded on the commit (its stamp, `canon_rules`); the store
keeps the raw form. Raw forms are lost, by design and unavoidably:
`prune` deletes them, and a history pushed and cloned arrives without
its snapshots. When `settle` needs a snapshot the store lacks, it
rebuilds it from the history's items: the canonical form, a different
commit from the original whenever the rule sets changed anything.

The original can come back later: `import` from someone who kept it,
`extract` of the same input again. Today each meeting goes wrong:

- **Beside a rebuilt copy,** `add` and `import` refuse the original
  ("exists with another commit"): the copy with more information is
  turned away for its own canonical form.
- **Beside a history commit,** `settle` takes the history's items when
  the commit is trusted (0464) and never looks at the raw: an identity
  reused for other bytes goes unnoticed.

Rule sets don't commute in general, nor does applying C1 then C2 equal
applying C2. So a raw snapshot can be compared with a history commit only
under the rule sets that wrote it, which today's code may no longer
have.

## Goals

- **G1.** Losing raw forms stays allowed: the history alone is enough.
- **G2.** An original that comes back is recognized as such, and
  preferred to a copy rebuilt from the history.
- **G3.** A snapshot that is not the one the history holds, under the
  same identity, is refused, saying why, never ignored.
- **G4.** What cannot be decided is said so, and decided by the user,
  never guessed.

## Non-goals

- **N1.** Keeping raw forms against `prune` or a clone: they are lost by
  design (S5).
- **N2.** Comparing under the rule sets of another version: a front end
  has its own code only.

## Specification

- **S1. Meeting a history commit.** A raw snapshot R meets the history
  commit H of its identity (in any lineage) when `add`, `import` or
  `settle` finds both. Their sources must be equal (identity, build time,
  order hint, group); then, with C today's canonicalization:
  - **H's stamp is today's rule sets:** R *is* H's snapshot when C(R)'s
    items are H's items; *is not* otherwise.
  - **H's stamp differs:** R *is* H's snapshot when C(R) equals C(H's
    items); *undecided* otherwise: C(C_old(raw)) need not equal C(raw),
    and a difference cannot be told from different bytes.

  Sources differing: *is not*. Metadata, its labels, doesn't count: it
  describes how a tool read the input, not what the input is, and it
  changes with the tool (S1a).
- **S1a. Labels differing are reported, in bulk:** one line per key,
  `labels differ on N snapshot(s): + KEY, - KEY, ~ KEY` (added, removed,
  changed), each snapshot listed with `--verbose`. R, accepted, carries
  its own labels into the history only where a commit is rewritten
  anyway: a commit the plan would keep is never rewritten for its labels
  alone, since each rewrite of a shared history is a force-push. Old
  commits may keep their old labels for good; nothing reads them as
  anything but information. A label whose change means a different
  snapshot belongs in the identity.
- **S2. What follows:**
  - **is:** R is accepted, and preferred. It replaces in the store a copy
    rebuilt from the history, and later re-canonicalizations start from
    it. Nothing else changes: the history already holds it.
  - **is not:** refused, naming the identity, H, and the way out
    (`forget`, then `extract` again).
  - **undecided:** refused, naming the identity, the two stamps, and the
    paths that differ, and how to decide (S3).

  A command meeting several snapshots decides them all before refusing,
  and its refusal lists every undecided one, `<identity> <R's commit>`
  per line, the input `accept --from` takes (S3): an undecided meeting
  comes in bulk (a rule set dropping a rule makes undecided every
  original that still holds what the rule removed).
- **S3. The user decides an undecided meeting:** `git seriate accept
  --reason TEXT IDENTITY...`, identities or globs of them as `forget`
  takes, or `--from FILE` (`-` for standard input) in the form a refusal
  lists them; `--dry-run` says what it would accept. It takes each R as
  H's snapshot, R stored; or `forget`, as for *is not*. The next `settle`
  reaching H re-canonicalizes from R, under today's rule sets.
- **S3a. The decisions' record, `refs/seriate/accepted`:** a commit per
  call, its parents the previous record and each R accepted (keeping
  them, as `refs/seriate/forgotten/` keeps the forgotten), its tree a
  file `accepted` listing each identity, R's commit, H's stamp and
  today's, the differing paths, and its message the reason. Not in the
  store, which `prune` deletes: after the next `settle`, H's items are
  C(R), and the record is the only trace of why the history changed under
  that identity. **Local,** as `refs/seriate/forgotten/` is: it reaches
  raw forms, which invariant 0002 keeps unpushed; snapshot commits being
  deterministic, a colleague meeting the same original computes the same
  R.
- **S4. A rebuilt copy, told apart without a marker:** of two copies of
  one source in the store, a rebuilt one is its own canonical form under
  H's stamp. When H's stamp is today's: C(copy) = copy. A copy rebuilt
  under another stamp is told from the original only through H (S1).
- **S5. `prune` loses raw forms, by design.** 0497 S5's "listing first
  those whose raw form only the branch keeps" was never implemented and
  is dropped: `prune` says how many of the snapshots it deletes have a
  raw form other than their history's, and deletes them.
- **S6. Who:** git-seriate, with the front end's canonicalizer, at each
  meeting: `add` and `import` against the store and the history, `settle`
  against the history. A downstream's rule sets take part as they do in
  `settle`: they decide what canonical means, so what "the same
  snapshot" means. `import` and `add` take the kit's canonicalizer, as
  `settle` does.

## Alternatives considered

- **Refusing `prune` when raw forms differ:** the raw forms are lost
  anyway, by a clone; refusing one way of losing them protects nothing.
- **A marker on rebuilt copies:** S1 and S4 decide from the contents and
  the stamp; a marker adds state that a clone, again, does not carry.
- **Labels in S1's equality:** labels change with the tool, not with the
  input. Measured downstream: three labels added to an extractor on one
  date, and two front ends writing different label sets for one input;
  of 47 snapshots of one lineage, 34 with 3 labels, 12 with 4, 1 with 6.
  Each re-extracted by today's tool would have met as *is not*.
- **Recording an `accept` in R's place in the store:** `prune` deletes
  it, and with it the only trace of why the history changed (S3a).
- **Deciding an undecided meeting in favor of R:** it would replace a
  history's content by other bytes under the same identity whenever a
  rule set changed. That is G3's failure, silently.

## Test plan

1. A snapshot settled with a rule set that changes it, pruned, rebuilt by
   a re-sort; the original imported: accepted, replacing the rebuilt copy
   (S1 *is*, S2).
2. A different snapshot under the same identity, imported or extracted:
   refused, naming `forget` (*is not*).
3. The history written under rule set A, today's B, not commuting with
   A on the snapshot: `import` refused as *undecided*, every undecided
   identity listed, naming both stamps; `accept --from` the list then
   takes them, `refs/seriate/accepted` recording each, and the next
   `settle` re-canonicalizes from them (S3, S3a). `accept --dry-run`
   writes nothing.
3a. A downstream's bump of its own rule set dropping a rule, on its copy
   of a history: the meetings undecided where the rule removed something.
4. `prune` counts the raw forms it loses (S5).
5. S1a: the same snapshot re-extracted with one label more: *is*, the
   difference reported in one line per key; a commit the plan keeps is
   not rewritten for it; one the plan rewrites takes R's labels.

## Measured outcome

Filled in at implementation.
