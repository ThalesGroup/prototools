<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0454 — advise: what a history of descriptors can't hold, and which sources could fix it

Status: draft (S1–S8 implemented 2026-11-22; S9 awaits the downstream
        front end)
App: git-seriate-fdp | protoback
Refs: docs/specs/0450-git-seriate-a-history-from-snapshots.md (S5 step 4),
      docs/specs/0453-git-seriate-fdp-descriptors-from-executables.md (the
        histories this reads),
      docs/specs/0445-filter-fixups-move.md (`fixups.yaml`: `prune:` and
        `move:`, which this writes),
      docs/specs/0446-build-report.md (protoback's report, which this reads
        back)

## Background

A history of descriptors (0453) is the union of what many executables
embedded, built at different times. Nothing in it is ever removed, so its
tree holds several generations of the same files:

- **copies:** one executable embeds a file twice, once without
  `json_name`; the store keeps both, as `<name>~N.pb`;
- **versions:** two sources hold different versions of one file;
- **moves and splits:** the same symbols under a new path in a newer
  source, or part of a file moved to a new one;
- **reused extension numbers:** one number on one message, given to a new
  extension later;
- **files updated in place:** the tree keeps only the newest version of a
  path, so a file of an older source may refer to symbols that version no
  longer has.

A schema database (protoback `--schema-db-out`) holds one version of each
symbol, so these must be settled before building one, with a
`fixups.yaml` (0445). Written by hand, it goes wrong: on one downstream
repository (7 554 descriptors, 12 executables), a build aborted; then 80
files were refused; three entries pruned files nothing conflicted with
any more; one pruned the newer of two files. A downstream tool's `advise`
settles them from the history itself, and says which sources to extract
again to refresh what can't be settled. This spec is it, generic: it
knows sources, groups and descriptors, nothing of where executables come
from.

## Goals

- **G1.** `advise` analyzes a history of descriptors: its conflicts, each
  file's age, the decisions, and the problems left.
- **G2.** It writes, from the decisions, one `fixups.yaml`, each entry with
  its reason, and a detailed report.
- **G3.** It names, for each problem, the groups whose sources are
  candidates to fix it, and orders them.
- **G4.** It finds version skew itself, by loading the descriptors into a
  descriptor pool, without running protoback.
- **G5.** It reads only what survives `settle` and `compact`: the tree and
  the blocks, never commit hashes.
- **G6.** `advise --check` tells whether the generated files present are
  up to date.
- **G7.** On the downstream repository it was first built on, the same
  `prune:` and `move:` entries and problems as the downstream `advise`.

## Non-goals

- **N1.** Extracting, or fetching anything: `advise` says what to extract.
- **N2.** Building the schema database.
- **N3.** Renaming: output targets (0447) make no collisions, so `advise`
  writes no `variants:` section.
- **N4.** Wording candidates for one kind of source ("package", "build
  target"): they are groups, worded generically; a front end words them
  through S7's `Wording`.

## Specification

### S1 — The command

```
git seriate-fdp advise [--into NAME] [--out DIR] [--json] [--check] [--report FILE]
```

- reads the history `NAME` (the current branch by default), its tip's
  tree and its snapshot commits' blocks (through the legacy reader where
  there is one);
- `--out DIR`: writes `fixups.yaml` and `STATUS.md` there; nothing is
  written without it;
- `--json`: the actions (S7) as JSON instead of text;
- `--check`: only checks the generated files present (S8);
- `--report FILE`: a protoback report (0446) of a build with the current
  `fixups.yaml`, read back (S4);
- exit status 0 with the actions printed; 1 if a decision couldn't be
  made (a tie) or an out-of-order source is found, the files still written
  for everything else; with `--check`, 1 if a file is out of date.

The analysis is a module of `git-seriate-fdp`, `advise`, with no
knowledge of any source format: its inputs are the repository and the
history's blocks.

### S2 — Sources

Each snapshot commit's block is a **source**: its identity (a source added
twice counts once), its build time (the only thing that dates files), its
`contents` (the paths it holds), its group (what to extract again to
refresh it; its identity when it has none), and its metadata (shown, never
interpreted). A path no source lists is **manual**: dated by the author
date of the commit that last wrote it, an upper bound of its build time.
The author date, not the committer date: it says when the change was
written, and survives any rewrite, `settle`'s and `compact`'s included
(G5).

### S3 — Analysis and decisions

**A file's age:** the build time of the newest source listing it.

**What is compared:** the files of the history's tree, which `settle`
stores canonical (0453 S4). "Identical" below means identical canonical
bytes.

**Copies after canonicalization.** A source that embeds one file twice,
with and without `json_name`, has both in its snapshot, as `x.pb` and
`x~2.pb`; the history keeps both paths, now with one canonical content.
The `copies` row still finds them, identical, and prunes the `~N` one.

**Symbols:** what a descriptor pool registers for a file, by full name:
messages, nested ones included; enums, and their values, in the enum's
enclosing scope; services; extensions, also as (extendee, number). Each
`.pb` is read as prototext text back to binary (0453 S3), then as a
`FileDescriptorProto` through `prototext-upb`.

| Conflict | Detection | Decision |
|---|---|---|
| copies | one descriptor name, several files, identical apart from `json_name` | keep the newest (then the plain name); prune the others by path |
| versions | one descriptor name, several files, otherwise different | keep the newest; prune the others by path |
| moves | files of different names sharing symbols (a rename, or a split) | keep one, in the direction below; move the other onto it (0445: pruned by name, its importers re-pointed) |
| reused numbers | one (extendee, number), different extensions | keep the newest; prune the older by name |
| ties | two conflicting files equally new | no decision |

**The direction of a move,** from the history alone:

- **backward,** keeping the older file, when it defines every symbol the
  newer one does and each shared definition is identical (in their
  canonical bytes, everything nested included): the move loses
  nothing, and the older file's other definitions keep their importers;
- **forward,** keeping the newer, in every other case: importers that
  used a definition only the older file has are then refused (S4), and S7
  names the sources to extract again.

### S4 — Problems

| Kind | Found by | Detail |
|---|---|---|
| `stale-importer` | a kept file importing a moved one | the import, and what it now points to |
| `refused` | loading every kept file into a descriptor pool, the prunes and moves applied in memory: each file the pool refuses | the pool's reason |
| `missing-import` | a kept file importing a name no file has | the import |
| `manual` | a file no source lists | the subject of the commit that last wrote it |
| `tie` | S3 | both files, their age |
| `out-of-order` | a source written after a newer one sharing files with it, its older versions replacing newer ones (below `settle`'s reach: past a cutoff, or a manual stop) | both sources, the files shared |

A file refused because an import was refused is reported once, under the
import. With `--report FILE`, protoback's findings (refused files, types
it couldn't resolve, idle directives) are attached to the problem found
for the same file, or reported as they are; a report written against
another `fixups.yaml` (S8's stamp) is refused.

### S5 — `fixups.yaml`

0445's format: `prune:` (copies, versions, reused numbers) and `move:`
(S3's direction). A header (how to use it, why, the rule), entries grouped
by conflict kind with a comment each, every entry with its reason and both
files' ages; the stamp of S8; no `variants:`.

### S6 — `STATUS.md`

The sources (build time, order hint, group, identity, metadata, the
number of files), the conflicts by kind
with counts, every decision with its reason, the problems by kind, the
order check; the stamp of S8.

### S7 — Actions, and their wording

For each problem, the **candidate groups**: every group with a source that
holds the file to refresh (for `refused` and `stale-importer`, the
importing file), each with its sources and its newest one. Problems no
group can address (`missing-import`, `manual`, `tie`, `out-of-order`) are
listed under **no known source**. The summary orders the groups by the
number of problems they are candidates for, then the one whose newest
source is oldest first. Text by default, the same as JSON with `--json`.

**The wording** is a parameter of `advise`'s entry point,
`advise::run(repo, &kit, &dyn Wording, &options)`, with generic defaults:
the command's name, as `fixups.yaml` and `STATUS.md` say what generated
them (`git seriate-fdp advise`), the noun for a group (`group`) and for a
source (`source`), the heading for the problems no group can address
(`No known source`), and a
candidate's line from its newest source (by default its identity and
build date; a front end shows the `metadata` keys it knows, such as a
version, and the source's `order_hint`). A `manual` problem carries the
commit's subject as a field of its own, `subject`, which the actions
show whole. Not a field of `git-seriate`'s `Kit`, which
knows nothing of `advise`: a front end declares its own `advise` command
and calls the entry point with its wording, one printer for both.

### S8 — Up to date: the stamp and `--check`

`fixups.yaml` and `STATUS.md` record the sources they were computed from
(a `# computed from:` comment block, one identity per line; a section).
`--check` compares each generated file present with the sources of the
history, exits 1 naming what differs, 0 otherwise; an absent file is not an
error. A `settle` that sorts a source below them, or a new extraction,
makes them out of date.

### S9 — Agreement with the downstream implementation (G7)

On the downstream repository, with its history read through its legacy
reader: the same `prune:` and `move:` entries, the same problems by kind,
and the same first candidate group as its own `advise`, both run on one
state of the repository. It needs the downstream front end (its source,
rule set and legacy reader on `cli::run`) first, so it follows this
spec's implementation; the downstream maintainers run it and report the
result, as for 0452 S9.

## Alternatives considered

**An adapter turning blocks into sources,** as the downstream `advise`
has. The block is the source here (0450 S3): identity, build time, group
and metadata are its fields, and a downstream format reaches `advise`
through the legacy reader.

**Dating files by commit time.** A commit's time is when it was written,
not when its source was built; `settle` rewrites commits.

## Test plan

1. Toy histories, one per conflict kind (copies, versions, a reused
   extension number, a tie) and per move (a rename, forward; a split with
   identical definitions, backward; one whose definitions differ, forward,
   the older file's other importers `refused`), and per problem kind
   (version skew `refused` with the pool's reason, `missing-import`,
   `manual`, `out-of-order`): decisions, entries and comments, problems,
   candidates and their order.
2. Each toy's `fixups.yaml` applied by protoback: exit 0 with
   `--fail-on W24 --fail-on W26`, and no W3. *Moved to S9: protoback is a
   binary, which this crate's tests can't call; the downstream acceptance
   applies the real `fixups.yaml`.*
3. `--report`: a toy build's report read back; an idle directive reported
   as stale; a report from another `fixups.yaml` refused.
4. `--check`: passes on a fresh output; fails after a new extraction,
   naming the source.
5. The same snapshots settled in one go and incrementally: the same output.
6. A manual file's age the same before and after a `compact` that
   rewrites its commit. (Not `settle`: a manual commit adding a `.pb` stops
   its walk, so `settle` never moves one.)
7. Copies differing only by `json_name`, settled with `fdp` v1: two paths,
   one canonical content, one `copies` conflict pruning the `~N` file.
8. A test `Wording`: its nouns, heading and candidate line in the text
   output; the JSON output unchanged by it.

## Measured outcome

The analysis is the downstream implementation's, ported: its modules
(decisions, problems, `fixups.yaml`, `STATUS.md`, actions, `--check`,
`--report`) and their unit tests pass unchanged against this workspace's
`prototext-upb`. New: the input, read from the history's blocks; manual
files dated by author date; the text of the actions, in Rust, through
`Wording`.

On toys (test plan 1, 4–8): a rename moved forward, with its stamp and
`--check`; copies after canonicalization, two paths of one content, the
`~N` one pruned; the same advice settled in one go or incrementally; a
manual file's age through a `compact`; a front end's wording, the JSON
unchanged by it.

S9 and test 2 wait for the downstream front end.
