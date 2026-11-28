<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0502 — extract on bytes: the front end's split, for a caller's inputs

Status: implemented
Implemented in: 2026-11-28
App: git-seriate-fdp
Refs: docs/specs/0495-extract-makes-atomic-snapshots.md (the parts of
        an input, S2; grouping, S3; their identities and record entries,
        S4, S4a),
      docs/specs/0499-lineages-a-producer-s-snapshots-sorted-apart.md
        (Q1: lineages stated after extraction, by identity),
      docs/specs/0466-git-seriate-archives-without-a-repository.md (the
        sink: a repository, or an archive)

## Background

`extract_files_into` does two things: it reads its inputs from disk
(their bytes, modification times, labels and identities, and the
warnings about times), then splits each into parts and writes them
(0495: the rest and each framed set a snapshot, record entries with
their part, `--as-one` gathering several inputs into one). The second
half, `Parts`, is private.

A downstream front end reads an archive's members in place, each
scanned as one extent, without writing them to disk. It calls
`FdpExtractor` itself, and so makes one snapshot per member: no set is
a snapshot of its own, no group is one snapshot, and record entries all
go with the rest. To get 0495's parts it had to dump its members to disk
and run `git seriate-fdp extract` on them. It asks for the second half
on bytes, so that it can call it rather than copy it.

## Goals

- **G1.** 0495's split and grouping, and their writing, callable on
  inputs held in memory.
- **G2.** `extract_files_into` built on it: the files' snapshots exactly
  as today.
- **G3.** Each outcome names the identity it wrote, or would have, so
  that a caller can state lineages afterwards (0499 Q1).

## Non-goals

- **N1. No reading of archives here:** what a member is, and where its
  bytes are, stay the caller's (0495 S1: these inputs are one build is
  the caller's statement).
- **N2. No new rule:** the parts, identities, groups and record entries
  are 0495's, unchanged.
- **N3. No change to the command line.**

## Specification

- **S1. `Input`:** `label` (its name in records and, in a group, the
  prefix of its origins, escaped as `input_label` escapes a path),
  `identity` (its snapshot's, unless grouped), `bytes`, a `Cow<'a, [u8]>`
  (borrowed from a caller's map, or owned when a caller copied a member
  stored in several extents), `build_time`, `metadata` (its snapshot's
  labels, unless grouped).
- **S2. `extract_inputs_into(sink, meeter, kit, extractor, inputs,
  options) -> Result<Vec<Outcome>, Error>`:** for each input, 0495's
  parts (`Parts::of`, the split unless `no_split`), written under its
  identity and build time; with `as_one`, every input's parts gathered
  (`within` its label) and written once under that identity, its build
  time `options.build_time`, else the latest of the inputs', its labels
  `options.metadata` (an input's are about that input, and unused in a
  group). `id`, meaningful only for one file, is the caller's to put in
  `Input`.
- **S2a. An input holding nothing:** no item, no record entry, no
  refusal: nothing written (`Added::Nothing`), its outcome still naming
  the identity it would have had. An input whose every descriptor was
  refused is a record-only snapshot, keeping the refusals (0495, as
  today).
- **S3. `Outcome`:** `input` (the label, a file's path for
  `extract_files_into`), `identity` (written, or that would have been:
  a rest's or a set's `<identity>!<label>`), `set` (`None` for a rest, a
  set's label otherwise, `set@<start>` after the chain of the member it
  lies in, and after its input's label in a group: what the identity
  ends with, as a field, so that nothing is parsed out of an identity),
  `added` (as today). `file: PathBuf` becomes `input: String`.
- **S4. `extract_files_into`:** reads each file, makes its `Input`
  (label `input_label`, identity `--id` or `identity_of`, build time
  `--build-time` or its modification time), warns about times as today,
  and calls `extract_inputs_into`.

## Alternatives considered

- **`Parts` made public:** the downstream's other offer. It exposes the
  split's internals (`within`, `absorb`, the order of record entries),
  which callers would then combine themselves, and each in its own way.
  One entry point keeps 0495's rules in one place.
- **A temporary file per member:** what the downstream does today; the
  bytes written to disk only to be read back.

## Test plan

1. `inputs_on_bytes_equal_files`: inputs given as bytes and the same
   bytes as files give the same snapshots, commit for commit: an input
   with framed sets, one without, and `as_one` over two.
2. `an_outcome_names_its_identity`: each set's outcome names
   `<identity>!set@<start>` and its `set`, in a group after the input's
   label; an input holding nothing writes nothing and names the identity
   it would have had.
3. `bytes_borrowed_or_owned`: the same input as a borrowed slice and as
   an owned `Vec`: the same snapshot.
4. `a_groups_labels`: with `as_one`, the snapshot's labels
   `options.metadata`, the inputs' unused.
5. The existing `extract` tests unchanged, through `extract_files_into`.

## Measured outcome

2026-11-28. `git-seriate-fdp/tests/fdp.rs`: test plan 1 to 4 pass
(`inputs_on_bytes_equal_files`, `an_outcome_names_its_identity`,
`bytes_borrowed_or_owned`, `a_groups_labels`), the snapshots on bytes
and on files equal in commit tree and message, alone (a rest and two
sets) and grouped; the existing `extract` tests unchanged through
`extract_files_into`, which now reads each file when its turn comes and
hands it on as an `Input`. The command line's messages name an input by
`Outcome.input`, a file's path as before.
