<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0450 — git-seriate: a history rebuilt from undated snapshots

Status: draft
App: seriation | git-seriate | git-seriate-fdp
Refs: docs/specs/0448-fdp-scan-undeclared-fields-and-rejections.md (the
        scanner the protobuf extractor wraps, and its refusal reasons),
      docs/specs/0449-fdp-scan-dependency-indices-and-field-order.md (one
        more refusal, for inputs the extractor meets),
      docs/specs/0445-filter-fixups-move.md (`fixups.yaml`, which
        `advise` writes)

## Background

A downstream tool extracts the descriptors embedded in executables and
keeps them in a git repository, one snapshot per executable. The
snapshots arrive undated and out of order: an executable has a build
time, sometimes a revision number, and neither is fully reliable. The
tool orders the snapshots by how their files evolve, rebuilds one linear
history from them, and advises on conflicts before a schema database is
built (protoback `--schema-db-out`).

Only the last part is about protobuf. Ordering finds by their
similarity is what archaeologists call *seriation*. Rebuilding the
history of a set of files from snapshots of it is a general technique,
and the downstream implementation already separates it from its sources
in places (a comparator with a clean four-way result, a source model in
`advise`).

It doesn't separate it everywhere: its sort loads snapshots and fetches
file contents through git itself, calls one text comparator directly,
and reads its settings from thread-local globals. Its command line is a
Python layer over Rust bindings. This spec defines the engine as it
should be in oss-prototools; the downstream tool is then rebuilt on it.

## Goals

- **G1.** A generic engine, with no protobuf in it: snapshots of any
  files, ordered and turned into one history.
- **G2.** The sort is a library of its own, independent of git and of
  any file format.
- **G3.** One plain Rust binary, `git-seriate`, run as `git seriate`. No
  Python layer.
- **G4.** One commit format, owned by the engine, versioned, and the
  contract between every tool that writes or reads a snapshot
  repository.
- **G5.** What is specific to a kind of file comes in through compiled-in
  plugin points: the protobuf layer is one set of plugins.
- **G6.** On the same snapshots, the engine with no canonicalization gives
  the same order and the same history as the downstream implementation.

## Non-goals

- **N1.** Dynamic plugins (shared libraries, WASM, subprocesses). The
  comparator runs on every pair of versions of a file within a window, so
  it is in-process. Later, if outside users want plugins without
  rebuilding.
- **N2.** A probing window for the greedy phases (positions limited by
  time and by edge partners). It changes which moves are found, so it
  would break G6. An option, once G6 holds.
- **N3.** Writing any repository format but the engine's own. A tool with
  repositories in an older format reads them through the legacy reader
  (S2) until `compact` has converted them (S4).
- **N4.** Scanning a file stored in several pieces without joining them
  (a seam-aware scan). The front ends hand the extractor one slice.
- **N5.** Binary storage of items. Forges show binary diffs as "binary
  files differ", and the readable history is what people review.
- **N6.** A repository of its own for the engine. The crate boundaries
  keep it possible; nothing asks for it yet.

## Specification

### Terms

- **Snapshot:** the set of files one source held, at one unknown point
  of the history.
- **Source:** what a snapshot comes from (an executable, an archive, a
  directory). It has an **identity**, unique and stable across rewrites,
  a **build time**, an optional **order hint** (a monotonic revision
  number, when the source has one), a **group** (what to extract again
  to refresh it) and **metadata** (shown, never interpreted).
- **Item:** one file of a snapshot, a path and its bytes.
- **Version:** an item's content. Two items are one version when their
  canonical forms are equal (S2).
- **History:** the linear branch the engine rebuilds; its recent part,
  the **settling window**, may still be reordered.

### S1 — Three crates

| Crate | Holds | Depends on |
|---|---|---|
| `seriation` | the sort: its phases, scores, greedy search and self-test | nothing of git or protobuf |
| `git-seriate` | the snapshot store, the history (`settle`), the plugin points, the binary | `seriation`, `gix` |
| `git-seriate-fdp` | the protobuf plugins: extractor (`fdp-scan`), renderer (prototext), canonicalizer; `advise`; its binary | `git-seriate`, `fdp-scan`, `prototext-core`, `prototext-upb` |

- `seriation` takes snapshots (build time, order hint), the version key
  of each of their items, and a comparator. It never reads a repository
  or a byte of content itself: the comparator gets contents through the
  caller.
- Settings are passed in one struct per run, never through globals.
- Only `git-seriate-fdp` pulls in upb.
- Names checked on crates.io (2026-11-22): `git-seriate`, `seriation`
  and `git-seriate-fdp` are free; `seriate` is taken by an unrelated
  project. *Amended 2026-11-22: the third crate was `protosets`, the name
  of the downstream tool that will build on it; renamed so that the two
  never collide on a path, in a dependency, or in a spec.*

### S2 — Plugin points

Rust traits, composed in each binary.

| Plugin | Role | Generic default |
|---|---|---|
| **Extractor** | finds the items in a source's bytes, with their paths; reports refused candidates with reasons | none (`git seriate add` takes a directory, S4) |
| **Renderer** | how an item is stored in the tree (bytes to text, and back) | identity |
| **Canonicalizer** | `canon(item) -> item`, the form versions are identified and compared by | identity |
| **Comparator** | given two versions of a path: equal, the first contained in the second (older), the second in the first, or incomparable | line subsequence, then character subsequence |
| **Legacy reader** | reads every commit of an older format: snapshot and history commits as sources, manual commits with their anchors (N3) | none |

**The canonicalizer** is the only normalization: the comparator is
handed canonical forms, so it does not normalize again. Its rules are:

- deterministic and idempotent;
- versioned: a binary composes several rule sets (the protobuf layer's,
  a tool's own), and each history commit records the version of each,
  by name;
- monotone between versions: for an older rule set `canon_old` and a
  newer `canon_new`, `canon_new(canon_old(x)) == canon_new(x)` for every
  `x`. This is what lets the engine re-canonicalize a history item
  without its raw form.

The history stores canonical items; snapshot branches keep the raw ones.
Where a snapshot branch exists, its raw item is canonicalized and wins;
where it was dropped, the history's item is re-canonicalized. A history
written by a rule set the tool doesn't have, or by a newer version of one,
is refused, with the files listed. **Dropping a snapshot branch drops the only raw copy** of what
canonicalization removed: `drop` says so before it deletes one.

### S3 — The commit format

**Snapshot commit,** the single, parentless commit of a snapshot branch:

- tree: the snapshot's items, as rendered;
- committer date: the source's build time; author date: its order hint,
  or 0 (read in the sort's inner loop without parsing messages). Where the
  two disagree, `source.order_hint` is what is read: 0 in the author date
  can't tell "unknown" from a hint of 0;
- message: a subject, a line `---`, and a YAML block:

```yaml
format: 1
source:
  identity: …
  build_time: 1735689600
  order_hint: 123456789      # absent when unknown
  group: …
  metadata: {…}              # shown, never interpreted
item_count: 412
contents: {…}                # the item paths, as a tree
rejected:                    # absent when empty
  - {name: …, reason: …}
```

- A **snapshot branch** is a branch whose tip is a parentless commit
  with such a block, and whose name is the one the block derives from
  `source.identity`. A history branch never meets the third condition,
  even with one commit. The source plugin chooses identities, and so the
  names their branches get.

**History commit:** a snapshot commit replayed in sorted order. Same
message, plus `canon_rules: {<rule set>: <version>, …}` in the block, and
`reduced: true` once `compact` has removed items from it (S4); author copied; committer the
user running `settle`, with the source's build time; signed when git is
configured for SSH signing.

**Manual commit:** any commit on the history without a block. One that
changes no item path is set aside during `settle` and replayed right
after its **anchor**, the snapshot it was made after, recorded in a
`Snapshot-Anchor:` trailer by identity (never by hash). One that changes
an item path stops `settle`'s walk.

The block's keys and their meaning are fixed by `format`. A tool meeting
a higher `format` refuses to write to the repository.

### S4 — The binary

```
git seriate add [--id ID] [--build-time T] [--order-hint N] [--group G] DIR
git seriate settle [--into BRANCH] [--settling-days N] [--dry-run]
git seriate drop [--filter GLOB]
git seriate import REPO…
git seriate compact --cutoff DATE
```

`add` takes a directory as a snapshot of text files: the generic source,
for users with no extractor and for the tests.

`compact --cutoff DATE` garbage-collects the history before `DATE`, which
is explicit, and lists everything it drops before writing. Three levels,
each lossy by design except the first:

1. commits that change nothing in their own tree are dropped;
2. commits whose removal leaves the tree at the cutoff unchanged are
   dropped: what is lost is when a version appeared and where it was
   ordered, never the tree;
3. items are removed from old commits, as long as the tree at the cutoff
   is unchanged; such a commit no longer records all its source held, and
   says so with `reduced: true`.

`compact` writes its commits in the current format, and may canonicalize
the tree at the cutoff (S2). Run once with the cutoff at the newest commit
of an older format, it converts a repository, after which the legacy
reader is no longer needed. What canonicalization cannot fix in an old
item stays until a new snapshot brings that path again; a path no new
source carries keeps its old bytes. `git-seriate-fdp` is the same
binary with the protobuf plugins composed in, plus `extract` (sources
are executables, items their descriptors) and `advise`.

### S5 — Order of work, each its own spec

1. **`seriation`:** the sort, ported from the downstream implementation's
   specs, with a synthetic generator of snapshots of text files for its
   self-test (hidden order hints, Kendall tau). Acceptance: G6's order on
   the generator's corpora, the downstream tool run on the same inputs.
2. **`git-seriate`:** the store, S3's format, the legacy reader point,
   `settle` with the setting aside of manual commits, `compact` (S4), the
   binary. Fixed in the port rather than
   carried over: a render failure is one rule for every writer (the
   item is refused and reported); `settle` keeps in place the set-aside
   commits whose anchor is in the unchanged part of the history, instead
   of rewriting the whole window. Acceptance: G6's history.
3. **`git-seriate-fdp`:** the extractor, the renderer, the canonicalizer's
   generic descriptor rules (`json_name` equal to its default dropped;
   fields re-encoded in field-number order, recursively), the descriptor
   comparator later.
4. **`advise`** in `git-seriate-fdp`.

## Alternatives considered

**A Python command line over Rust bindings,** as the downstream tool has.
It adds a layer, a second test suite and an interpreter, for option
parsing and output formatting; `clap` does both. git runs any
`git-<name>` program on the path as `git <name>`.

**One crate.** The sort would pull in git, and `advise` would pull upb
into every user of the store. Three crates follow the three
dependencies.

**Normalization inside the comparator.** Phase 3 would see flavors of
one version as one, but phase 4 counts patterns of version keys, which
the comparator never sees: flavors interleaved across snapshots of one
build would still read as history. The canonicalizer fixes the key.

**The name `seriate` for the sort crate.** Taken on crates.io.

## Test plan

Each spec of S5 brings its own, in Rust: unit tests beside the code, and
end-to-end tests that run the binary on repositories built in temporary
directories. Common to all: the synthetic generator, whose corpora are
committed with their generator, as `fdp-scan`'s are.

## Measured outcome

Filled in at implementation.
