<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0495 — extract makes atomic snapshots

Status: draft (S2, S3 and S4 implemented on 2026-11-27; S5's provenance
        with 0494 S3, S8 to come)
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
- **N5.** Splitting container members: the right grain depends on the
  kind of container, a packaging convention (a jar is one build, its
  classes its members; a wheel is one; a zip of loose descriptor files is
  one bundle; a release tarball's executables are each one), and
  recognizing the kind needs the archive's bounds and member list, which
  today's origins don't give (fdp-scan finds zip members by their local
  headers, so an origin names a member, not its archive). Until fdp-scan
  reads containers as such (a spec of its own: the central directory, the
  members, the kinds they reveal), container members stay in the rest.

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
  1. **each framed set** in a buffer (the input, or a container member
     inside it): a chain of two framed records or more, each frame starting
     where the previous record ends and declaring that record's length
     (spec 0496); a record whose decoded `name` repeats within a chain
     starts another set (a file's `name` appears once in a set);
  2. **the rest:** every copy in no set, container members' included
     (N5), one part.

  `--split none` keeps the input whole, as today. A part with nothing in
  it is no snapshot. An input with no set gives one snapshot, as today,
  byte for byte.
- **S3. Grouping:** `extract --as-one IDENTITY FILE…` makes one snapshot of
  several inputs (or `--files-from FILE`, `-` for standard input, for more
  than a command line holds: a runfiles tree of thousands of modules), the
  caller's statement that they are one build; each copy's origin starts
  with its input's path in the group, and S2 applies to each input of the
  group.
- **S4. A part's identity:** its input's (or group's) identity, then `!`
  and the set's label: `set@<start>`, the offset of its first record,
  after the chain of the member it lies in (`gzip@271!set@14811251`),
  and after the input's path in a group. The rest keeps the input's
  identity. A label is made a valid ref name as an input's path is (spec
  0453 S5: what git refuses replaced by `_`). An identity that is a
  directory of an existing branch's name, or under one (`t.runfiles` and
  `t.runfiles/bin/x`), can't be a branch beside it: the store refuses it
  before writing, naming both.
- **S4a. Record entries:** an entry about one copy goes with that copy's
  part, told by the copy's origin, which the entry's value starts with
  (`frame-cut`, `order-cut`): copies of one path, the same bytes in
  several sets, each keep their own entry. Failing an origin, by the path
  the entry names; else with the rest.
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

- **Q1.** The grain of each kind of container (N5): to measure on real
  cases once fdp-scan reads containers as such.
- **Q2.** A part's upper-bound time in phase 1: deferred. In the measured
  case the bundles agreed copy for copy with their host's compiled
  copies, options and `json_name`s set aside (no message, field, enum or
  dependency differing, measured on whole descriptors): built at the
  host's revision, by a bundler
  that strips options, so the host's time was right.
- **Q3.** A repeated name is a late bound, not a boundary. Two sets glued
  back to back, A = {a, b, c} and B = {d, a, e}, are cut at B's `a`, and
  `d` goes with A: the bytes only say the boundary lies after A's `a` and
  no later than B's. When A and B are different builds, `d` is misplaced,
  possibly a straddle. Proposed: a cut is certain only when the interval
  is one position (B starting with a name A holds); otherwise the
  interval is recorded as evidence and `advise` reports it, the caller
  stating the boundary if it knows better. First, to measure on both
  corpora: how many chains a repetition cuts, and how many of those cuts
  are ambiguous.

## Test plan

1. Two framed sets back to back, then copies in no set: three snapshots,
   two `<input>!set@<start>` and the rest as `<input>`; a repeated `name`
   in one chain: two sets.
2. An input with no set: one snapshot, byte for byte as before; a single
   framed record: no set.
3. `--split none`: one snapshot, as today.
4. `--as-one` over three single-file inputs: one snapshot, each copy's
   origin starting with its input's path; `--files-from` the same.
5. A set inside a gzip member: its identity after the member's chain.
5a. Two sets closing with the same file, each read on past it: each set
   its own `frame-cut` entry (`each_set_keeps_its_own_copys_record_entries`).
5b. An identity that is a directory of another's, either first, in a
   repository and in an archive: refused, naming both
   (`an_identity_clashing_with_another_as_a_ref_is_refused_naming_both`).
6. S5's member times: with fdp-scan's containers (N5).

## Measured outcome

S2 to S4, implemented. The extractor marks each item's set
(`Extracted::sets`): a chain of two framed records or more, read with
fdp-scan's `frame_at`, split again at a repeated path. `extract` writes
each set as `<identity>!set@<start>`, after the member's chain inside one,
and the rest under the input's identity, every part then taking the input
as its group; a record entry about one item goes with that item's part.
An input with no set is written as before. `--split none` keeps an input
whole; `--as-one ID` and `--files-from FILE` (`-` for standard input) make
one snapshot of several inputs, each origin, set label and record key
after its input's path. The tests of the test plan pass, items 1 to 5.

An input embedding a `FileDescriptorSet` is split too: prototext's binary,
which embeds the well-known types' set, gives the set as a snapshot of
its own, beside its rest.

Still to come: S5's provenance and the part in the block (with 0494 S3);
a member's own build time (with fdp-scan's containers, N5); S8's reports
in `advise`.
