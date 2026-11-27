<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0495 — extract makes atomic snapshots

Status: draft
App: git-seriate, git-seriate-fdp
Refs: docs/specs/0494-extract-records-the-evidence.md
        (the origins and provenance the parts are cut by and recorded in),
      docs/specs/0496-fdp-scan-a-framed-descriptor-ends-at-its-frame.md
        (the frames a set is a chain of),
      docs/specs/0452-git-seriate-the-store-and-the-history.md
        (the snapshot, one commit, and its identity as its ref's name),
      docs/specs/0497-forget-a-snapshot-and-prune-instead-of-drop.md
        (`forget`, by which a split is redone)

## Background

The sort assumes that a snapshot is the state of one source repository at
one revision. An executable often isn't one, both ways:

- **too large:** one input holds sets built elsewhere or apart: a schema
  bundle embedded in a database client, an archive packed into a binary.
  Measured downstream: a database client embedding eleven bundles, in raw
  bytes, each a chain of framed records; about 150 of a history's 160
  straddles traced to one executable holding such bundles beside its own
  compiled copies;
- **too small:** one build spread over many inputs, such as the 1,039
  single-module snapshots of one Python program's runfiles tree.

So `extract` makes the atomic entities themselves snapshots, each one
commit, as an executable is one today: finer than an executable where its
structure shows parts, coarser where the caller knows several inputs are
one build. Structure is what can be read at extraction, cheaply and the
same each time; a split, once made, is redone only by extracting again
(spec 0497), never by `settle` or `advise`.

## Goals

- **G1.** One snapshot, one commit, per atomic entity: a source at one
  revision, as far as the input's structure and the caller tell.
- **G2.** Made at extraction, from the input's structure, and from the
  caller's grouping of inputs.
- **G3.** An input with no structure found is one snapshot, as today.
- **G4.** Redone only by `forget` and `extract` again: nothing splits,
  groups or replaces a stored snapshot afterwards.

## Non-goals

- **N1.** Splitting or grouping after extraction, by `settle`, `advise` or
  anything else: a decision derived from the history would change with
  it, and make the history intractable. `advise` reports (S8).
- **N2.** Lineages: a later spec, built on atomic snapshots. It will want
  a set's stable label, since `set@<start>` moves between versions of its
  host: generically, the set's root, its last record's `name` in
  topological order, recorded in the block, not the identity.
- **N3.** A C++ build's scattered descriptors, one per translation unit,
  unframed: they are the executable's own build, and stay in its rest.
- **N4.** Splitting by registrant (a Go module other than the build's
  own): after the hints are recorded (0494), and measured.

## Specification

- **S1. Who decides what:**

  | decision | by | from |
  |---|---|---|
  | this part of an input is a set of its own | `extract` | the input's structure (S2) |
  | these inputs are one build | the caller | what it knows of the archive (a runfiles tree, a wheel): it hands them as one (S3) |
  | this input is not to be split | the caller | `extract --split none` |
  | where the input comes from | the caller | `--layer` (0494 S3) |
  | something looks wrong | `advise`, reported | the sort's report and the snapshots' contents (S8) |

- **S2. The parts of an input,** each a snapshot:
  1. **each container member holding descriptors,** a zip or gzip member
     found inside the input (its origin chain, 0494 S1), its own parts in
     turn;
  2. **each framed set** in a buffer: a chain of framed records, each
     frame starting where the previous record ends (spec 0496); a record
     whose decoded `name` repeats within a chain starts another set (a
     file's `name` appears once in a set);
  3. **the rest:** the copies of the buffer in no set, one part.

  `--split none` keeps the input whole, as today. A part with nothing in
  it is no snapshot.
- **S3. Grouping:** `extract --as-one IDENTITY FILE…` makes one snapshot of
  several inputs (or `--files-from FILE`, `-` for standard input, for more
  than a command line holds: a runfiles tree of thousands of modules), the caller's statement that they are one build; each
  copy's origin starts with its input's path in the group, and S2 applies
  to the group as to one input.
- **S4. A part's identity:** its input's (or group's) identity, then
  `!` and the part: the member's chain (`zip@7340032!assets/api.binpb`),
  `set@<start>` for a framed set, nothing more for the rest. As a ref
  name: every byte git refuses in one (`~ ^ : ? * [ \`, a space, a control
  byte, `..`, `@{`, a component starting with `.` or ending with `.lock`),
  and `%`, percent-encoded; a name past 200 bytes, its last component
  replaced by a 12-digit hash of it, the whole part in the block.
- **S5. A part's block:** its input's group, so that what to extract again
  stays known; `source.provenance` (0494 S3), then the part,
  `{part: container|set|rest, at: <chain or range>}`. Its build time: a
  zip member's entry time (0494 S2) where plausible (later than the
  sentinels: the DOS epoch 1980-01-01, the Unix epoch, 2010-01-01; not
  later than its input's), else its input's; which, recorded.
- **S6. Re-splitting:** a better rule, or a wrong split, is redone by
  forgetting the input's snapshots and extracting it again, with the new
  rule or `--split none` (spec 0497). The stored copies' bytes and origins
  (0494) keep what a later tool would need to re-split without the
  original input.
- **S7. The sort** orders snapshots as before: a part is a snapshot like
  any other, compared with every copy of its files.
- **S8. `advise` reports, never splits:** where the sort leaves something
  it can't explain (a straddle without a candidate, a snapshot's copies
  differing from another snapshot's of the same input by options only, a
  part that looks like two builds), it may read the snapshots' contents,
  and reports what it suspects, naming the snapshots and the evidence. It
  doesn't split, group or forget: the user decides, by `forget` and
  `extract`.

## Open questions

- **Q1.** Container members that are one build with their host (a jar
  packed into a binary built at the same revision): split anyway, as S2
  does; they sort next to their host. To confirm on a measured case that
  this costs nothing.
- **Q2.** A part's upper-bound time in phase 1: deferred. In the measured
  case the bundles agreed copy for copy with their host's compiled
  copies, options set aside: built at the host's revision, by a bundler
  that strips options, so the host's time was right.

## Test plan

1. An input with descriptors of its own and in a zip member: two
   snapshots, `<input>` and `<input>!zip@N!<member>`, each with its block.
2. Two framed sets back to back, then copies in no set: three snapshots,
   two `set@<start>` and the rest; a repeated `name` in one chain: two
   sets.
3. `--split none`: one snapshot, as today.
4. `--as-one` over three single-file inputs: one snapshot, each copy's
   origin starting with its input's path.
5. A member name with `~`, a space and `..`: its identity escaped, a valid
   ref name; a long one hashed, the whole part in the block.
6. S5: a sentinel entry time ignored, a plausible one taken, the choice
   recorded.

## Measured outcome

Filled in at implementation.
