<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0452 — git-seriate: the snapshot store and the history

Status: implemented
Implemented in: 2026-11-22
App: git-seriate
Refs: docs/specs/0450-git-seriate-a-history-from-snapshots.md (the
        engine: S2 its plugin points, S3 the commit format, S4 the
        binary and `compact`, S5 this crate's place in the order of work),
      docs/specs/0451-seriation-the-sort-as-a-library.md (the sort this
        crate drives)

## Background

Spec 0450 makes `git-seriate` the crate that stores snapshots in git and
rebuilds one linear history from them, with `seriation` (0451) deciding
the order. An existing implementation does all of this inside one tool,
for one kind of snapshot. This spec states the store, the format, the
history's rebuild and the binary in generic terms, keeps that
implementation's behavior where it is right, and fixes it where it is
known to be wrong.

## Goals

- **G1.** A snapshot is added once, as a branch whose single commit holds
  its items and a self-describing block; adding it again is a no-op.
- **G2.** `settle` rebuilds the recent history in sorted order, and nothing
  older; manual commits on the history survive it, after the snapshot
  they were made after.
- **G3.** Every rewrite is recoverable: the previous tip is kept as a
  backup branch before the history moves.
- **G4.** Deterministic: the same repository and settings give the same
  trees and messages, whatever the order snapshots were added in; and the
  same commits for the same committer and signing key (S5 step 6 makes the
  user running `settle` the committer).
- **G5.** Every tree passes `git fsck --strict`; no item is lost silently.
- **G6.** On the same snapshots, the same sequence of snapshots and trees
  as the existing implementation's history (0450 G6), canonicalization
  off.

## Non-goals

- **N1.** Network operations: fetching, pushing, cloning.
- **N2.** Merges. The history is linear; two lines of development are the
  comparator's concern, not the store's.
- **N3.** Removing a file from the history when a later snapshot lacks it.
  A snapshot is a partial view: a source that doesn't hold a file says
  nothing about it.
- **N4.** Signing formats other than SSH.
- **N5.** The protobuf plugins (`git-seriate-fdp`, 0450 S5 step 3).

## Specification

### S1 — Modules

| Module | Holds |
|---|---|
| `format` | the commit block: writing `format: 1`, reading it, handing older blocks to the legacy reader |
| `store` | snapshot branches: add, list, drop, import |
| `history` | `settle`: the walk, the plan, the rewrite |
| `compact` | garbage collection before a cutoff (0450 S4) |
| `plugins` | the traits of S2 and their generic defaults |
| `bin/git-seriate` | the command line (S8) |

Every operation takes one `Config` (S7). Nothing reads globals or the
environment, except git's own configuration, read through `gix` as git
reads it.

### S2 — Plugin points

```rust
pub trait Extractor {
    /// The items in a source's bytes, and the candidates refused.
    fn extract(&self, bytes: &[u8]) -> Extracted;
}
pub struct Extracted { pub items: Vec<Item>, pub rejected: Vec<Rejection> }
pub struct Item { pub path: String, pub bytes: Vec<u8> }
pub struct Rejection { pub name: String, pub reason: String }

pub trait Renderer {
    /// How an item is stored in the tree, and back.
    fn render(&self, path: &str, item: &[u8]) -> Result<Vec<u8>, String>;
    fn parse(&self, path: &str, stored: &[u8]) -> Result<Vec<u8>, String>;
}

pub trait Canonicalizer {
    /// The rule sets composed, each with its version (0450 S2).
    fn rule_sets(&self) -> Vec<(String, u32)>;
    fn canon(&self, path: &str, item: &[u8]) -> Vec<u8>;
}

pub trait ItemPaths {
    /// Whether `path` holds an item. A manual commit that changes none is
    /// set aside by `settle` (S5); one that changes any stops it.
    fn is_item(&self, path: &str) -> bool;
}

pub trait LegacyReader {
    /// A commit block of an older format, as a source and its contents.
    fn read(&self, message: &str) -> Option<Snapshot>;
}
```

The comparator is `seriation::Comparator`. Generic defaults: no extractor
(`add` takes a directory, S8); the identity renderer and canonicalizer
(no rule set); `ItemPaths` from the repository's git configuration,
`seriate.itemPattern` (a glob), and true for every path when none is set;
no legacy reader; `seriation::LinesChars`.

With no pattern, nothing is ever wrong, only cautious: every manual commit
stops `settle`'s walk. So when one does, and no pattern is set, `settle`
says why and how to change it (S5 step 3). Refusing to settle without a
pattern was considered: it costs every repository a setting before its
first `settle`, to prevent a stop that the warning already explains.

A **render failure** refuses the item, reported in the block's `rejected`
with the renderer's message: one rule for every writer, never raw bytes
stored in its place.

### S3 — The store

**A snapshot branch is named its source's identity,** which the store
checks as a git reference name and refuses otherwise. The source plugin
chooses identities (0450 S3).

**Adding** a snapshot `(source, items, rejected, record)`:

1. Each item's path is checked component by component, as git's
   `core.protectHFS` and `core.protectNTFS` would (no empty, `.` or `..`
   component, no `.git` in any of its forms), and refused otherwise, in
   `rejected`.
2. Two items with one path and the same bytes are one; with different
   bytes, the second becomes `<stem>~N<ext>` for the smallest free `N ≥ 2`,
   checked against every path written, files and directories, in the
   items' order. A name that is genuinely of that form takes part like any
   other: `bar~2.txt` after a generated `bar~2.txt` becomes `bar~2~2.txt`;
   before it, the generated one becomes `bar~3.txt`. An item under a path
   that is already a file is refused.
3. The tree is written with git's own entry order, and checked before it
   is: sorted, no duplicate, no invalid name. A failed check is an error,
   never a written tree.
4. The commit: parentless, the block of S4, committer date the build
   time, author date the order hint or 0.
5. If the branch exists: the same commit is a no-op; a different one is
   refused, naming the branch.

A source with no item but a `record` (an extractor's statistics, say)
gets a **record-only** branch: an empty tree, `kind: record-only` in its
block. `settle` never reads one, `drop` keeps them unless asked, `import`
copies them.

**Listing:** a branch is a snapshot branch when its tip is parentless,
carries a block, and is named as its block's identity says. A history
branch, even of one commit, fails the last test.

**Dropping** deletes snapshot branches (matching a glob, if given). Their
commits stay reachable from the history. Where canonicalization removed
something, the raw item exists only on the snapshot branch: `drop` lists
those snapshots before deleting them.

**Importing** copies snapshot and record-only branches from other
repositories; a branch already present with a different commit is
refused.

### S4 — The block

A snapshot commit's message is a subject, a line `---`, and:

```yaml
format: 1
source:
  identity: …
  build_time: 1735689600
  order_hint: 123456789     # absent when unknown
  group: …                  # absent when none
  metadata:                 # shown, never interpreted
    key: value
items: 412
contents:                   # the tree's paths, as nested maps, leaves null
  dir:
    file.ext: null
rejected:                   # absent when empty
  - {name: …, reason: …}
record: {…}                 # the extractor's, shown, never interpreted
```

`kind: record-only` replaces `items` and `contents` on a record-only
commit. A history commit adds `canon_rules: {<rule set>: <version>, …}`
(absent with no rule set) and, once `compact` has removed items from it,
`reduced: true`.

Keys appear in this order, strings quoted when YAML would read them as
something else, so that one snapshot always gives one message. A block
whose `format` is above 1 makes every writing command refuse the
repository. A block without `format` is the legacy reader's; with no
legacy reader, the commit is a manual commit.

### S5 — `settle`

`settle [--into NAME] [--cutoff DATE | --no-cutoff] [--settling-days N] [--dry-run]`:

1. **The candidates.** Snapshot branches not yet in `NAME`, or the ones
   given.
2. **The cutoff.** `--cutoff`, or the oldest candidate's build time less
   the settling window (`--settling-days`, 60). `--no-cutoff`: none, for a
   full re-sort.
3. **The walk.** From `NAME`'s tip along first parents, down to the first
   snapshot commit built more than the settling window before the cutoff,
   the root, or a manual commit that stops it (below). **The tail** is
   every commit above the deepest snapshot commit of the walk built at or
   after the cutoff, that one included: build times are not monotonic
   along a history, so commits built before the cutoff can be in it. With
   `--no-cutoff`, the whole walk is the tail.
   - a snapshot commit is read for its identity and build time;
   - a manual commit that changes no item path is set aside, with its
     **anchor**: the identity in its `Snapshot-Anchor:` trailer, or, if it
     has none, the nearest snapshot commit below it in the whole history
     (past the cutoff and any stop), or `none`;
   - a manual commit that changes an item path stops the walk, with a
     warning naming the commit and the path. When `seriate.itemPattern`
     is unset, the warning adds that every path counts as an item path
     without it, and how to set one:

     ```
     warning: settle stopped at 1a2b3c4d ('docs: release notes'), which changes README.md:
     no seriate.itemPattern is set, so every file counts as a snapshot file.
     Set one (git config seriate.itemPattern '*.txt') to let settle pass over hand-made commits.
     ```
4. **The sort.** The tail's snapshots and the candidates, sorted by
   `seriation` with the comparator, versions read as canonical stored
   forms: each item's bytes are parsed, canonicalized and rendered, and
   two instances are one version when the results are equal. A tail
   snapshot whose branch is gone is rebuilt from its history commit: its
   `contents` paths, read from that commit's tree. The sort places each
   snapshot once, so a tail holding one identity twice is refused,
   naming both commits; when the later copy changes nothing, the refusal
   says so and names `compact --level 1`, which drops it.
5. **The plan.** Below the first position where the sorted order differs
   from the tail's, nothing changes: those commits are kept, as they are,
   with the manual commits anchored to them. The prefix also ends at the
   first commit whose `canon_rules` differs from the current composition:
   its items would canonicalize differently. From there on:
   - each snapshot becomes a history commit whose tree is the previous
     tree with the snapshot's items, canonicalized and rendered, written
     over it; the block gains `canon_rules`;
   - each set-aside manual commit is replayed right after its anchor
     (after the last commit with that identity), in its original order
     among those of one anchor; `none`, or an anchor below the rewrite,
     at the bottom; an anchor no longer on the branch, after the snapshot
     that preceded it, with a message. An inferred anchor is recorded as
     a trailer on replay;
   - two set-aside commits replayed in the opposite order to the original
     and changing a common path: a warning, naming the path.
6. **The commits.** Author kept; committer the user, from
   `GIT_COMMITTER_NAME`/`EMAIL` or `user.name`/`email` (missing: an
   error), with the source's build time, or the manual commit's own
   committer time; signed with `ssh-keygen -Y sign` when `commit.gpgSign`
   is true and `gpg.format` is `ssh`, refused when the key fails.
7. **The ref.** If the new tip differs from the old: the old saved as
   `NAME-N`, the lowest `N` free, then `NAME` moved. If `NAME` is checked
   out, the index and the work tree are left alone, with a note to run
   `git reset --keep`: rewriting an index behind its user's back can lose
   staged work, which `git reset --keep` refuses to. `--dry-run` writes nothing and prints the plan, warnings
   included.

Empty snapshots (whose items change nothing) are written like any other:
they date their files (0450 G2's history), and `compact` is the way to
drop them.

### S6 — `compact`

`compact --cutoff DATE [--level N] [--dry-run]`: the history before
`DATE`, which is explicit, garbage-collected. "Before" is below the
deepest commit built at or after `DATE`, as for `settle`'s tail. Three
levels, each including those below it, the first by default:

1. commits that change nothing in their own tree are dropped: lossless;
2. commits whose every change a later commit before the cutoff makes
   again are dropped: what is lost is when a version appeared and where
   it was ordered, never the tree;
3. from the snapshot commits kept, the items a later commit changes again
   are removed, and the block says so: `reduced: true`, `contents` what
   is left. Manual commits are never reduced: they have no block to say
   so.

Every commit dropped or reduced is listed, with why. The rebuilt tree at
the cutoff must equal the old one, or nothing is written. Commits at and
after the cutoff keep their trees, on the new parents. The previous tip is
saved as `NAME-N`, as by `settle`; commits are written with the committer
and signing of S5 step 6. A manual commit whose trailer anchors it to a
snapshot no kept commit holds is warned about: `settle` will look for
that anchor. A dropped copy of a snapshot another kept commit holds
leaves its anchors resolving, and is not warned about.

### S7 — Settings

One `Config`, from the command line over git configuration
(`seriate.*` keys) over defaults: the sort's `seriation::Settings`, the
settling window, the item pattern, the cutoff. Recorded nowhere in the
history: the history depends on the snapshots and on these, and
`settle --dry-run` shows what a change would do.

### S8 — The binary

```
git seriate add [--id ID] [--build-time T] [--order-hint N] [--group G] DIR
git seriate settle [--into NAME] [--cutoff DATE | --no-cutoff] [--settling-days N] [--dry-run] [BRANCH…]
git seriate list [--record-only]
git seriate drop [--glob GLOB] [--record-only]
git seriate import REPO…
git seriate compact --cutoff DATE [--into NAME] [--level 1|2|3] [--dry-run]
```

`add DIR` takes every file under `DIR` as an item, path relative to `DIR`,
bytes as they are; `--id` defaults to `DIR`'s name, `--build-time` to the
newest file's modification time. `settle` without `--into` settles the
current branch. Errors go to stderr, one line each; the exit status is 0,
1 for a refusal, 2 for a usage error.

### S9 — Agreement with the existing implementation (G6)

The `seriation` corpora (0451 S5), imported with `add`, settled in one go,
then again incrementally (newest half first): the sequence of
`(identity, tree)` of the history equals the existing implementation's on
the same snapshots, and both of ours are equal. Messages differ, by
format.

## Alternatives considered

**Rewriting the whole settling window on every `settle`,** as the
existing implementation does when a manual commit is set aside. Its tip
tree is the same either way, but every commit above the cutoff gets a new
hash, every time: a history that churns with nothing new in it. S5 step 5
keeps the unchanged prefix.

**Storing a render failure's raw bytes,** as two of the existing
implementation's three writers do. The reader of the history then gets a
blob in no known form, and no record of why.

**Removing files absent from a later snapshot** (N3). A source holds a
subset of the files; absence is not deletion.

**History branches that merge their snapshot branches.** Linear history
is what people review and what `advise` reads; merges would make the
snapshot branches unprunable, and add nothing the block doesn't record.

**One block format per source kind.** The block is generic, and what is
specific to a source goes into `metadata` and `record`, shown and never
interpreted; a second format would be a second reader to maintain.

## Test plan

Rust, unit tests beside the code and end-to-end tests running the binary
on repositories in temporary directories:

1. `add`: a directory as a snapshot; again, a no-op; changed, refused;
   paths with `.`, `..`, `.git` in its forms refused; a duplicate path with
   different bytes as `~2`; a genuine `bar~2.txt` after and before a
   generated one; an item under a file refused; `git fsck
   --strict` clean on every case.
2. The block: written in key order; read back; a higher `format` refused;
   an older block handed to a test legacy reader.
3. `settle`: the corpora in one go and incrementally give one history
   (G4); a manual commit changing no item path replayed after its anchor,
   its trailer recorded, kept in place by a second `settle`; one changing
   an item path stops the walk, and without a pattern says how to set
   one; two swapped commits on a common path
   warned about; a missing anchor placed after its predecessor.
4. The unchanged prefix: a `settle` with nothing new writes nothing and
   moves nothing; one new snapshot at the top rewrites one commit; a
   changed rule set ends the prefix at the first commit it affects. The
   tail: a commit built before the cutoff above a deeper one built after
   it is re-sorted; `--no-cutoff` re-sorts the whole walk.
5. Backups, committer identity, SSH signing (with a test key, checked by
   `git verify-commit`), `--dry-run` writing nothing.
6. `compact`: each level, the listing, `reduced: true`, the tree at the
   cutoff unchanged; an anchor on a snapshot no longer held warned about,
   one on a snapshot whose other copy stays not; a history holding one
   snapshot twice refused by `settle`, naming `compact --level 1`, and
   settled after it.
7. S9's agreement, against the existing implementation's histories on the
   `seriation` corpora.

## Measured outcome

**S9 holds:** on the three `seriation` corpora, settled in one go and in
two halves (the newer half by build time first), the history is the
existing implementation's, `(identity, tree)` for `(identity, tree)`
(`git-seriate/tests/histories/`, its `merge-snapshots --into`, defaults).

**Signing** runs `ssh-keygen` once per commit written, as git does:
about 55 ms each, so a settle of 100 snapshots with signing on takes
about 5 s, nearly all of it there; without signing, well under a second.
Signing in-process, through the SSH agent's socket, is the remedy if that
matters; not done.

The legacy reader is a plugin point the generic binary leaves empty; its
test reads a toy older format, settles new snapshots onto it, and has
`compact` convert what is left.
