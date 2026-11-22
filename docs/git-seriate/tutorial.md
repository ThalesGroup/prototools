<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# git-seriate: one history from undated copies

You have several copies of the same files, taken at different times, and
no record of the order: configuration saved from several machines, build
outputs collected from old releases, or schemas embedded in executables.
`git seriate` stores each copy as a **snapshot** and rebuilds a single
linear git history from them, ordered by how the files evolve from one
snapshot to the next. A build time is a hint, not the answer.

Two programs:

- `git seriate`, for files of any kind;
- `git seriate-fdp`, the same, plus `extract`, which finds the protobuf
  descriptors embedded in executables, and `advise`, which says what the
  history can't hold in one schema database.

git runs either as a subcommand: `git seriate settle` runs
`git-seriate settle`. `git help seriate` opens its manual.

## Installing

```shell
cargo install git-seriate        # git seriate
cargo install git-seriate-fdp    # git seriate-fdp
```

Or, from a clone of this repository, `nix-shell` puts both on `PATH`, with
their man pages and shell completions.

## 1. A history from undated copies

Four copies of a small configuration tree were recovered from four racks.
Each is a directory; the files' modification times are all we know of
their dates, and one of them, rack-a's, was restored from a backup with
an older date than it should have:

```
found/rack-a/conf/{server.conf,log.conf}    modified 2026-02-03
found/rack-b/conf/{server.conf,log.conf}    modified 2026-02-10
found/rack-c/conf/{server.conf,log.conf}    modified 2026-03-09
found/rack-e/conf/{server.conf,log.conf}    modified 2026-03-01, a copy of rack-a's
```

Make a repository for the history, and tell it which paths are the
snapshots' files (section 2 says why):

```console
$ git init -b main history && cd history
$ git config seriate.itemPattern 'conf/**'
```

**Add each copy as a snapshot.** Each becomes a branch of its own, named
by the snapshot's identity (the directory's name, or `--id`); its build
time is the newest file's modification time, or `--build-time`:

```console
$ git seriate add ../found/rack-c
added rack-c
$ git seriate add ../found/rack-a
added rack-a
$ git seriate add ../found/rack-e
added rack-e
$ git seriate add ../found/rack-b
added rack-b
$ git seriate list
rack-a
rack-b
rack-c
rack-e
```

The order you add them in doesn't matter. `add` also takes `--group`
(what to extract again to refresh the snapshot), `--order-hint` (a
revision number, when the source has one) and `--meta KEY=VALUE`, all
recorded and shown, never used to sort.

**Settle them into the history.** `--dry-run` shows what would be done:

```console
$ git seriate settle --dry-run
would settle main: kept 0, wrote 4 snapshot(s), replayed 0 commit(s)
$ git seriate settle
main is checked out: run 'git read-tree -m -u main' to bring the index and work tree to it, keeping local changes
settled main: kept 0, wrote 4 snapshot(s), replayed 0 commit(s)
$ git read-tree -m -u main
```

`settle` never touches your index or work tree; when the branch it
rewrites is checked out, it prints the command that brings them over,
keeping your local changes.

```console
$ git log --format='%cs %s'
2026-03-09 Snapshot rack-c
2026-03-01 Snapshot rack-e
2026-02-03 Snapshot rack-a
2026-02-10 Snapshot rack-b
```

rack-a comes after rack-b, though its date is older: its files grew from
rack-b's, and rack-c's from rack-a's. Each commit's diff is what changed
from one copy to the next:

```console
$ git show --format=%s HEAD~2 -- conf/server.conf
Snapshot rack-a

diff --git a/conf/server.conf b/conf/server.conf
index fda3f21..7163ecc 100644
--- a/conf/server.conf
+++ b/conf/server.conf
@@ -1 +1,2 @@
 port = 80
+tls = on
```

A commit's date is its snapshot's build time, so dates in the log need
not increase. Its message records the snapshot, which is how later
commands recognize it:

```console
$ git log -1 --format=%B rack-c
Snapshot rack-c

---
format: 1
source:
  identity: "rack-c"
  build_time: 1773014400
items: 2
contents:
  "conf":
    "log.conf": null
    "server.conf": null
```

## 2. Your own commits

The history is an ordinary branch: you can commit to it.

```console
$ git add README
$ git commit -m "Explain where the files come from"
```

When `settle` rebuilds the history, it replays such a commit after the
snapshot it followed (recorded in a `Snapshot-Anchor:` trailer). It can
only do so if the commit changes no snapshot file: that is what
`seriate.itemPattern` tells it. Without the setting, every file counts as
a snapshot file, and `settle` stops at your commit with a warning that
says how to set one.

## 3. A newer copy

A fifth copy turns up, from rack-d, dated 2026-04-15. Add it and settle
again:

```console
$ git seriate add ../found/rack-d
added rack-d
$ git seriate settle
main is checked out: run 'git read-tree -m -u main-1 main' to bring the index and work tree to it, keeping local changes
settled main: kept 3, wrote 1 snapshot(s), replayed 0 commit(s); previous tip saved as main-1
$ git read-tree -m -u main-1 main
$ git log --format='%cs %s'
2026-04-15 Snapshot rack-d
2026-11-22 Explain where the files come from
2026-03-09 Snapshot rack-c
2026-03-01 Snapshot rack-e
2026-02-03 Snapshot rack-a
2026-02-10 Snapshot rack-b
```

`settle` re-sorts only the recent part of the history: from the oldest
new snapshot's build time, less a **settling window** of 60 days
(`--settling-days`), or from `--cutoff DATE`; `--no-cutoff` re-sorts it
all. Below the first change, commits are kept as they are ("kept 3").
Whenever the branch moves, its previous tip is kept as `main-N`, the
lowest `N` free: nothing is lost, and `git branch -D main-1` removes it
when you no longer need it.

## 4. Compacting the old part

rack-e is an exact copy of rack-a: its commit changes nothing. `compact`
garbage-collects the history before a cutoff:

```console
$ git seriate compact --cutoff 2026-04-01 --dry-run
drop 0912a3bc ('Snapshot rack-e'): changes nothing
would compact main: dropped 1, reduced 0, converted 0
$ git seriate compact --cutoff 2026-04-01
drop 0912a3bc ('Snapshot rack-e'): changes nothing
compacted main: dropped 1, reduced 0, converted 0; previous tip saved as main-2
$ git log --format='%cs %s'
2026-04-15 Snapshot rack-d
2026-11-22 Explain where the files come from
2026-03-09 Snapshot rack-c
2026-02-03 Snapshot rack-a
2026-02-10 Snapshot rack-b
```

Three levels, each including the ones below:

1. drop the commits that change nothing (lossless; the default);
2. also drop those whose every change is made again later;
3. also remove from snapshot commits the files changed again later.

The tree at the cutoff, and every commit after it, stay the same: the
files you have checked out don't change.

## 5. Snapshot branches, and other repositories

Once settled, a snapshot lives in the history: `settle` can rebuild its
branch from its commit when it needs it. So the branches can go:

```console
$ git seriate drop
deleted rack-a
deleted rack-b
deleted rack-c
deleted rack-d
deleted rack-e
```

`drop --glob 'rack-*'` deletes only those matching. `git seriate import
OTHER-REPO` copies another repository's snapshot branches into this one,
to be settled here: several people can extract on their side and merge
the results in one history.

## 6. Descriptors embedded in executables

Programs that use protobuf often embed their schemas, as
`FileDescriptorProto`s. `git seriate-fdp` makes a history of those: each
executable is a snapshot, each descriptor a file, stored as annotated
text, so that diffs read as schema changes.

Three builds of a service, `shopd`, were kept, from three versions of its
sources. Between v2 and v3, the `Customer` message moved from
`shop/customer.proto` to a new `shop/people.proto`:

```console
$ git init -b main history && cd history
$ git seriate-fdp extract ../builds/shopd-v1 ../builds/shopd-v2
added builds/shopd-v1
added builds/shopd-v2
$ git seriate-fdp settle
$ git seriate-fdp extract ../builds/shopd-v3
added builds/shopd-v3
$ git seriate-fdp settle
$ git ls-tree -r --name-only main
shop/customer.pb
shop/order.pb
shop/people.pb
```

(`settle`'s output is as in section 1.) Each snapshot is named by the
file's path, or `--id` for a single file; its build time is the file's
modification time, or `--build-time`. A file reads like this:

```
#@ prototext: protoc
name: "shop/order.proto"  #@ string = 1
package: "shop"  #@ string = 2
dependency: "shop/people.proto"  #@ repeated string = 3
message_type {  #@ repeated DescriptorProto = 4
 name: "Order"  #@ string = 1
 ...
```

The history keeps every file it has ever seen: `shop/customer.pb` from
v2 is still there, beside `shop/people.pb` from v3, and both define
`shop.Customer`. A schema database holds one version of each symbol, so
something must go. Build one from this tree with protoback, and it keeps
the older file:

```console
$ protoback -I . --schema-db-out=../shop.desc .
Warning [W3]: file:shop/people.proto pruned — duplicate symbols with:
    file:shop/customer.proto (1 symbol)
Warning [W23]: stripped pruned dependency "shop/people.proto" from shop/order.proto
```

**`advise` says which files to set aside, and why.** It writes
`fixups.yaml`, a filter file for protoback, and `STATUS.md`, a summary:

```console
$ git seriate-fdp advise --out ../advice
$ sed -n '/^move:/,$p' ../advice/fixups.yaml
move:

  # --- Moved or renamed files (forward) (1) ---
  # A newer source has the same symbols under another path. The older file goes,
  # and its importers, if any are still in the snapshot, import the newer one
  # instead. Symbols only the older file had are lost to them: protoback says
  # which (W24).

  # moved or renamed to shop/people.proto (built 2026-08-11): 1 of its 1 symbols are there; this one is built 2026-06-20
  - shop/customer.proto: shop/people.proto
$ protoback -I . --schema-db-out=../shop.desc -C ../advice/fixups.yaml .
```

With the filter, protoback keeps `shop/people.proto`, and every import of
`shop/customer.proto` becomes an import of it: the build prints only its
progress, with no warning. The rule
`advise` follows: of two conflicting files, the newer stays, unless the
older holds everything the newer does, identically, and more.

What it can't settle on its own, it lists, with the sources that could:
a newer build to extract, or files no source lists. `--json` gives the
same as JSON; `--check` exits 1 when the files in `--out` are no longer
current, after a new extraction; `--report FILE` takes in what a
protoback build said of them.

**To extract with another schema** than the stock `descriptor.proto`, one
with custom options for instance, point the repository at a schema
database written by `protoback --schema-db-out DB.desc
--emit-extension-ranges`:

```console
$ git config seriate.fdpSchema path/to/DB.desc
$ git config seriate.fdpRoot google.protobuf.FileDescriptorProto
```

It is a repository setting, not a flag: every command must read and write
the files alike.

## Settings

| Setting | What |
|---|---|
| `seriate.itemPattern` | the paths that are snapshot files (a glob; all files when unset) |
| `seriate.fdpSchema`, `seriate.fdpRoot` | `git seriate-fdp`'s descriptor schema, and its root message |
| `user.name`, `user.email` | the committer of the rewritten commits |
| `commit.gpgSign`, `gpg.format = ssh`, `user.signingKey` | sign them, with an SSH key |

The design is in specs 0450 (the overview), 0451 (the sort), 0452 (the
store and the history), 0453 (`git seriate-fdp`) and 0454 (`advise`),
under [`docs/specs/`](../specs/).
