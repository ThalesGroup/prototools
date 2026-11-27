<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0494 — extract records the evidence: origins, types, registrants, provenance

Status: draft
App: git-seriate, git-seriate-fdp
Refs: docs/specs/0452-git-seriate-the-store-and-the-history.md
        (the snapshot, its block S4, and adding a snapshot S3, which this
        changes),
      docs/specs/0480-fdp-scan-inflates-zip-and-gzip-members.md
        (origins: S6's text),
      docs/specs/0492-extraction-hints-what-an-executable-tells-beyond-its-descriptors.md
        (the registrants recorded),
      docs/specs/0495-units-derived-from-evidence-and-directives.md
        (what reads this evidence)

## Background

A snapshot is to be atomic relative to its source repository: one source,
one revision (spec 0495). Deciding that is not `extract`'s: the decision
can need the caller's knowledge, and must be possible again later, as
evidence and knowledge improve, without the original inputs, which a
downstream may no longer hold. So `extract` observes and records; units
are derived afterwards (0495). What isn't recorded can't be used later.

Today a snapshot records an origin only for a descriptor found inside a
container member, as one `record` entry per file (spec 0480 S8): nothing
for the input's own descriptors, no buffer's type, no registrant, and
nothing of where the input itself came from. A downstream's largest
snapshot has 6,777 descriptors: one entry per file repeats 6,777 paths
the block already lists in `contents`.

## Goals

- **G1.** Every copy's evidence in its snapshot: where it was found, in
  what kind of buffer, and which code registers it, where the bytes say.
- **G2.** Where the input came from, as the caller knows it, outermost
  first.
- **G3.** Compact: each copy's evidence where its path already is.
- **G4.** Histories that mix formats: blocks of format 1 readable forever,
  new ones written in format 2, one history holding both.
- **G5.** An input extracted again, by a newer extractor, replaces its
  earlier observation, recorded as such.

## Non-goals

- **N1.** Splitting or grouping: spec 0495, from this evidence.
- **N2.** A migration of format-1 blocks: they stay as written; an
  in-place migration, if ever, as an option of its own.

## Specification

- **S1. Each copy's evidence in `contents`:** the leaf of each path, `null`
  in format 1, holds its evidence:
  - a string, its origin, when that is all: `START..END` for the input's
    own descriptors, the chain then the range inside a container
    (`zip@7340032!assets/api.binpb!88..4210`);
  - a flow map when there is more: `{at: <origin>, by: <registrant>}`,
    the registrant from spec 0492's hints: `go:<package>` (its init
    function's package), `cpp:<library>` (a descriptor table's holder),
    and `module: <module>@<version>` where the build info or a `.pb.go`
    path resolves it.
- **S2. `buffers` in the block:** once per buffer holding descriptors, by
  the chain its copies' origins begin with (`""` for the input), its type
  by magic bytes (`elf`, `mach-o`, `pe`, `wasm`, `zip`, `gzip`, `class`,
  `pyc`, `text` for no control byte, else `other`) and, for a zip member,
  its entry time.
- **S3. `source.provenance`:** the layers above the input, as the caller
  gives them, outermost first, each a flow map of the caller's keys
  (`{archive: …, version: …}`, `{member: …, storage: …}`), shown, never
  interpreted. `extract --layer KEY=VALUE[,KEY=VALUE…]`, repeatable, one
  per layer; the extractor's interface takes the same. One home per fact:
  what is above the input (the archive, its version and part, the member
  and how the archive stores it) goes in `provenance`; the input's own
  facts (its file name, its size) stay in `source.metadata`; none in
  both.
- **S4. `extractor` in the block:** `{name, version, options}` of what
  observed, `options` those that change what is seen, as the extractor
  declares them (for `git seriate-fdp extract`: `--inflate` and its
  limits, `--increasing-field-numbers`).
- **S5. Formats per block:** these keys make a block of format 2. A
  reader takes format 1 and 2 blocks alike, a history may hold both, and
  a writing command refuses only a block of a format above the ones it
  knows (amending spec 0452 S4, which refused the repository).
- **S6. Extracting again:** adding a snapshot whose identity exists with
  another commit (refused by spec 0452 S3) replaces it when the new
  observation's extractor is one the history's record names, and is a
  newer version of the old one's, or the same with other options, or
  another named extractor: the snapshot's ref moves, the new block
  records `replaces: <commit>`, and `settle` takes the new one at its next
  run. The replaced commit stays reachable, at
  `refs/seriate/replaced/<identity>/<n>`, `n` counting the replacements,
  so that `replaces` can be checked. An extractor the record doesn't
  name, an older version, or the same name, version and options with
  another result, is refused as before.

## Test plan

1. An input with descriptors of its own and in a gzip member: every leaf
   of `contents` an origin; `buffers` with `""` and `gzip@N`, each typed.
2. Each type from a minimal buffer of its magic; a zip member's entry
   time.
3. `--layer` given twice: `source.provenance` in that order.
4. A Go executable fixture with an init function: its copy's leaf a flow
   map with `by: go:<package>`.
5. A history of a format-1 snapshot and a format-2 one: read, settled;
   a build knowing format 1 only refusing to write the second.
6. The same input extracted again by a newer version of a named
   extractor: the ref moved, `replaces` recorded, the old commit at
   `refs/seriate/replaced/<identity>/1`; with other options: replaced too;
   by an extractor the record doesn't name, or by the same version and
   options: refused.
7. `--layer` facts absent from `source.metadata`.

## Measured outcome

Filled in at implementation.
