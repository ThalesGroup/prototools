<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0466 — git-seriate: snapshots written as an archive, with no repository

Status: implemented
Implemented in: 2026-11-23
App: git-seriate | git-seriate-fdp
Refs: docs/specs/0452-git-seriate-the-store-and-the-history.md (`add`,
        the snapshot branches)

## Background

Snapshots are extracted where the binaries are, and there is often no
repository there: a downstream front end writes them as a tar of a bare
repository, to be fetched from elsewhere. The store writes only into a
repository, so that front end extracts into a temporary one, then reads
every object back and packs it again: 13.3 s for a 4 GB archive, where
its Python tool, building the pack in memory, took 10.3 s. Any front end
extracting with no repository wants the same.

## Goals

- **G1.** `extract` and `add` write their snapshots as an archive, with no
  repository: `--tar-out FILE`.
- **G2.** The same input gives the same archive, byte for byte.
- **G3.** The archive, unpacked, is a bare repository holding exactly what
  the same command would have written into one.

## Non-goals

- **N1.** Packing the objects written into a repository (loose today):
  later, on top of S1.
- **N2.** Deltas, or compression of the tar itself: a caller compresses
  it if it wants.
- **N3.** Streaming the pack to the tar as it is built: objects are
  sorted by id at the end (G2), so the compressed pack is held.

## Specification

- **S1. A pack sink.** The store writes through an object sink: a
  repository's object database, as today, or a pack being built. The pack
  sink compresses each object as it is written and keeps only that, with
  its id, kind and CRC, once per id; no object is kept uncompressed, and
  nothing is read back. The refs `add` would set are collected, not
  written. Peak memory is the compressed pack: about a fifth of the
  rendered text for prototext's descriptors.

- **S2. The archive,** a tar of a bare repository, entries at its root
  (unpacked with `mkdir r.git && tar xf FILE -C r.git`), in this order:
  - `HEAD`: `ref: ` and the first branch by name;
  - `config`: `[core]` with `repositoryformatversion = 0`,
    `filemode = true`, `bare = true`;
  - the directories `refs/`, `refs/heads/`, `objects/`, `objects/info/`,
    `objects/pack/`;
  - `packed-refs`: the header `# pack-refs with: peeled fully-peeled
    sorted`, then each branch, sorted by name (git looks refs up by
    binary search when the header says `sorted`);
  - `objects/pack/pack-<hash>.pack` and `.idx`: one pack, version 2,
    without deltas, its objects sorted by id, zlib at the default level;
    the index version 2, with 64-bit offsets past 2 GiB.

  GNU tar headers; files mode 0644, directories 0755, owner and group 0,
  modification time 0. This is the layout the downstream front end's
  archives have today, so its consumers see no change.

- **S3. `--tar-out FILE`** on `extract` and `add`: the archive written to
  `FILE`, or to standard output for `-` (the command's messages then on
  standard error); nothing written into any repository, and none needed.
  Outside a repository, an empty one stands in for the settings a front
  end reads (all defaults); inside one, only its configuration is read
  (`seriate.fdpSchema` and the like). The branch names and messages are
  those the command would write into a repository.

  Generic: any subcommand, a front end's own too, given an argument whose
  id is `tar_out` runs with no repository.

- **S4. `store::add_to`,** with a `Sink`: a repository or an archive.
  `add` writes its own trees (git's order, checked level by level as
  `git fsck --strict` checks them), the same trees `edit_tree` wrote; so
  every object reaches the sink once, and nothing is read back.

## Alternatives considered

**`git bundle`.** One file, deterministic too, but a bundle is fetched
from, not opened: the front end's consumers open a repository.

**Writing the pack with gix's pack generator.** It is built to count and
delta-compress a repository's objects; one pack of a few thousand objects
without deltas is a header, the objects and a trailer.

**gix's in-memory object database** (`with_object_memory`). It keeps
every object uncompressed until the end: the whole rendered text, five
times the pack, gigabytes for a 4 GB archive.

**Extracting into a temporary repository, then packing it,** as the
downstream front end does today: every object written loose, read back
and compressed again; 13.3 s against 10.3 s for its earlier in-memory
pack.

## Test plan

1. `tar_out_is_deterministic` — the same `extract --tar-out` twice, and
   with the inputs in another order: the same bytes.
2. `tar_out_is_the_repository` — the archive unpacked: `git fsck` clean;
   its refs and objects those of the same `extract` into a repository.
3. `tar_out_needs_no_repository` — run outside any repository.
4. `git seriate settle` over a clone of the unpacked archive: the same
   history as over the repository.
5. Measured: `extract --tar-out` against `extract` into a repository
   followed by packing, on the well-known types and a generated corpus.

## Measured outcome

2026-11-23, same host and method as 0463: the 300 executables of 0464's
measure, `extract --tar-out`, 0.46 s, against 0.36 s into a repository
(loose objects, no archive). The downstream front end's path, a temporary
repository then a pack, is not measured here.

Tests: the archive is the same whatever the order of the files, to a
file or to standard output; `git fsck --strict` is clean; its refs and
objects are an extraction's into a repository, and `settle` over each
gives the same trees. `add` writes its own trees, checked against
`git write-tree` of the same files.
