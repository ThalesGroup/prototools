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

**Add each copy as a snapshot.** Each becomes a commit of its own, kept
at `refs/seriate/snapshots/<identity>` (the directory's name, or `--id`),
apart from your branches; its build time is the newest file's
modification time, or `--build-time`:

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
would settle main/default: kept 0, wrote 4 snapshot(s), replayed 0 commit(s)
$ git seriate settle
HEAD named main, unborn: it names main/default now, a lineage of main
main/default is checked out: run 'git read-tree -m -u main/default' to bring the index and work tree to it, keeping local changes
settled main/default: kept 0, wrote 4 snapshot(s), replayed 0 commit(s)
recorded in refs/seriate/histories/main
$ git read-tree -m -u main/default
```

The history `main` is a branch per **lineage**, `main/<lineage>`: one
line of versions per producer (section 7 says when you need more than
one). Every snapshot nobody says otherwise of is in `default`, so this
one has a single branch, `main/default`; HEAD, which named `main` and
had no commit yet, now names it. `refs/seriate/histories/main` records
the lineages and their tips: one hash for the whole history.

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
rack-b's, and rack-c's from rack-a's. `--stats` shows the sort's work,
phase by phase. Given to that first `settle` (here with `--dry-run`), it
shows one move, rack-a's, past rack-b:

```console
$ git seriate settle --dry-run --stats
sort: 4 snapshot(s)
phase 1 build time: 2 position(s) changed
phase 2 order hints: skipped
phase 3 containment: 1 move(s), 6 comparison(s), 2 ordered; edge score -4; interleaving score 0
  move rack-a: 0 -> 1, +1; now between rack-b and rack-e
phase 4 interleaving: 0 move(s); edge score -4; interleaving score 0
would settle main/default: kept 0, wrote 4 snapshot(s), replayed 0 commit(s)
```

Phase 1 orders the snapshots by build time; phase 3 moves those whose
files say otherwise. [`docs/seriation/`](../seriation/design.md) explains
the phases and their scores. Each commit's diff is what changed
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
main/default is checked out: run 'git read-tree -m -u main/default-1 main/default' to bring the index and work tree to it, keeping local changes
settled main/default: kept 3, wrote 1 snapshot(s), replayed 0 commit(s); previous tip saved as main/default-1
recorded in refs/seriate/histories/main
$ git read-tree -m -u main/default-1 main/default
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
Whenever a lineage's branch moves, its previous tip is kept as
`main/default-N`, the lowest `N` free: nothing is lost, and `git branch
-D main/default-1` removes it when you no longer need it.

## 4. Compacting the old part

`compact` garbage-collects the history before a cutoff, at one of three
levels, each including the ones below:

1. **lossless, the default:** drop the manual commits that change
   nothing, and the second copy of a snapshot stored twice;
2. also drop the snapshot commits that change nothing, and the commits
   whose every change is made again before the cutoff;
3. also remove from snapshot commits the files changed again later.

rack-e is an exact copy of rack-a: its commit changes nothing. But it is
also the only record that rack-e held those files, as of its build time,
and `git seriate-fdp advise` (section 6) dates files by such records. So
level 1 keeps it:

```console
$ git seriate compact --cutoff 2026-04-01 --dry-run
would compact main/default: dropped 0, reduced 0, converted 0
```

Level 2 gives up records for a shorter history; `--dry-run` names each
one it would lose:

```console
$ git seriate compact --cutoff 2026-04-01 --level 2 --dry-run
drop 2b3f522a ('Snapshot rack-b'): every change it makes is made again later
drop ce9a19df ('Snapshot rack-a'): every change it makes is made again later
drop 0912a3bc ('Snapshot rack-e'): changes nothing; the only record of "rack-e" goes with it
delete snapshot rack-a: its last commit is dropped
delete snapshot rack-b: its last commit is dropped
delete snapshot rack-e: its last commit is dropped
would compact main/default: dropped 3, reduced 0, converted 0
$ git seriate compact --cutoff 2026-04-01 --level 2
drop 2b3f522a ('Snapshot rack-b'): every change it makes is made again later
drop ce9a19df ('Snapshot rack-a'): every change it makes is made again later
drop 0912a3bc ('Snapshot rack-e'): changes nothing; the only record of "rack-e" goes with it
delete snapshot rack-a: its last commit is dropped
delete snapshot rack-b: its last commit is dropped
delete snapshot rack-e: its last commit is dropped
compacted main/default: dropped 3, reduced 0, converted 0; previous tip saved as main/default-2
$ git log --format='%cs %s'
2026-04-15 Snapshot rack-d
2026-11-22 Explain where the files come from
2026-03-09 Snapshot rack-c
```

rack-c rewrites both files before the cutoff, so rack-b's and rack-a's
versions never reach it. The tree at the cutoff, and every commit after
it, stay the same: the files you have checked out don't change. The
snapshots of what was dropped go too; left, the next `settle`
would take them for new snapshots and put them back. The previous tip,
`main/default-2`, keeps everything until you delete it.

## 5. Snapshots, and other repositories

Once settled, a snapshot lives in the history: `settle` can rebuild it
from its history commit when it needs it. So the store's copies can go:

```console
$ git seriate prune
deleted rack-c
deleted rack-d
```

`prune --glob 'rack-*'` deletes only those matching. Pruning removes no
snapshot: the history still holds them. (`prune` was once called `drop`,
which still works, and warns.)

To take a snapshot out of the history, say one extracted by a scanner
since fixed, forget it, saying why, then add its replacement the normal
way:

```console
$ git seriate forget --reason "extracted before the scanner fix" rack-c
```

It says what it forgot, which snapshot it deleted, how it rewrote the
history (the previous tip saved, as by `settle`), and the record that
keeps what was forgotten with the reason, `refs/seriate/forgotten/1`;
then, as a reminder, to extract the replacement and settle before sharing
the history. `--dry-run` shows the same, and writes nothing.

`git seriate import
OTHER-REPO` copies another repository's snapshots into this one, to be
settled here: several people can extract on their side and merge the
results in one history. A repository made before snapshots moved out of
the branches (spec 0499) is read too; your own is moved once with `git
seriate migrate`, which every other command asks for: `git seriate
migrate main` also turns the history `main` into `main/default`, the
former branch kept under `refs/seriate/migrated/`, or, with `--into
NEW`, writes `NEW/default` and leaves `main` as it is, for a branch a
server won't let you delete.


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
$ git ls-tree -r --name-only main/default
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
protoback build said of them (`protoback --report-out FILE`), each
finding under the problem it explains, and with `--strict` exits 1 if
the build found something `advise` didn't foresee. A build that applies
`advise`'s `fixups.yaml` checks it with `--strict`, not with protoback's
`--fail-on W24`: a forward move `advise` decided, and reported, costs an
importer types by design.

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

## 7. Producers and lineages

Two producers of one file break the sort's assumptions when one of them
transforms it: say a bundler that embeds copies of your schemas in
`shopd`, their options dropped. `extract` keeps such an embedded set
apart, as a snapshot of its own (`builds/shopd-v3!set@<offset>`), but on
one line its stripped copy always sits inside the span of the full one,
which comes before it in older builds and after it in later ones.

Say which snapshots that producer made, once they are extracted, and the
next `settle` sorts them on a line of their own, `main/bundler`, beside
`main/default`:

```console
$ git seriate-fdp list
$ git seriate-fdp lineage set bundler 'builds/shopd-v3!set@1024'
$ git seriate-fdp settle
```

`lineage set` says what moved (`default -> bundler`), `settle` the same
of the history (`main/default -> main/bundler`), rewriting `main/default`
without the set, its previous tip saved as ever. git-seriate never
guesses a lineage from the files' contents: you know your producers, it
doesn't. `lineage set --from FILE` takes one `<identity> <lineage>` per
line, `lineage unset` puts snapshots back in `default`, and `lineage
list` shows the statements, which `refs/seriate/lineages` keeps, a
commit per change. Combining the lineages' tips into one schema is yours
to do.

## Settings

| Setting | What |
|---|---|
| `seriate.itemPattern` | the paths that are snapshot files (a glob; all files when unset) |
| `seriate.fdpSchema`, `seriate.fdpRoot` | `git seriate-fdp`'s descriptor schema, and its root message |
| `user.name`, `user.email` | the committer of the rewritten commits |
| `commit.gpgSign`, `gpg.format = ssh`, `user.signingKey` | sign them, with an SSH key |

The design is in specs 0450 (the overview), 0451 (the sort), 0452 (the
store and the history), 0453 (`git seriate-fdp`), 0454 (`advise`) and
0499 (lineages), under [`docs/specs/`](../specs/).
