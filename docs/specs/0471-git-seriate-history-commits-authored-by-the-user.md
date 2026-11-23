<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0471 — git-seriate: history commits authored by the user

Status: implemented
Implemented in: 2026-11-23
App: git-seriate
Refs: docs/specs/0452-git-seriate-the-store-and-the-history.md (S3: a
        snapshot commit's author, `git-seriate <>`, its date the order
        hint; S5: the history commits `settle` writes)

## Background

`store::add` writes a snapshot commit as `git-seriate <>`, an empty email,
its author date the order hint (or 0) and its committer date the build
time (0452 S3): the same snapshot is the same commit anywhere, which is
the point of a snapshot branch.

`settle` keeps that author on the history commit it writes for each
snapshot (`SortInput::author`), and commits and signs as the repository's
user. A downstream history of 1,165 commits has 1,122 authored
`git-seriate <>`, committed by a person. The history branch is what is
pushed and reviewed: a forge's branch protection, or a review policy, may
require every author to be an account with an email, and an empty email
lists oddly in every tool.

The downstream's forge accepts only commits whose author is a real
account, and checks a force push against the commits it replaces as well
as those it adds: every history commit needs a real author, or the push
after a `compact` or a replaying `settle` is refused. Its snapshots come
from people and from an automaton (archives, 0466) that has no author
worth naming, and it relies on the same snapshot being the same commit
(re-extraction writes nothing, archives byte for byte, `import` copying
once). Snapshot branches aren't pushed.

The author's date matters, its name doesn't: `advise` dates a file by the
author date of the history commit that wrote it (`advise/mod.rs`); no code
reads the author's name or email.

## Goals

- **G1.** A contract: a commit whose author email is empty has no author.
  Producers leave the author out that way, keeping snapshots
  deterministic.
- **G2.** Every history commit `settle` or `compact` writes has an author:
  the commit's own when it has one, else the user's who writes it.

## Non-goals

- **N1.** Changing who authors a snapshot commit: `store::add` writes
  `git-seriate <>`, the same for every producer (0452 S3).
- **N2.** A setting for the author, or a re-authoring command: histories
  settled before are re-authored as they are rewritten (S3).

## Specification

- **S1. No author.** A commit whose author email is empty has no author;
  its name is informative only (`git-seriate` from `store::add`). The name
  stays non-empty: `git fsck` flags `author <>`.

- **S2. A new history commit.** `settle` writes each snapshot's history
  commit with the snapshot commit's author if it has one (S1), else the
  identity it commits and signs with (`GIT_AUTHOR_NAME` and
  `GIT_AUTHOR_EMAIL` first, as git reads them, then `user.name`,
  `user.email`, then the committer's identity itself: a settle that could
  commit before 0471 still can). The author date is always the snapshot commit's (the order
  hint, or 0): `advise` dates files by it. A dry run needs no more
  identity than before: the committer's stands in.

- **S3. A rewritten commit.** A history commit `settle` replays or
  re-sorts, or `compact` rewrites, keeps its author, unless it has none
  (S1, as every snapshot's history commit settled before 0471): it then
  gets the writing user's identity, its date kept. Manual commits fall
  under the same rule.

- **S4. Consequences, documented.** A snapshot branch recreated from the
  history (0452 S5) is written by `store::add`, `git-seriate <>`: the
  history's author isn't copied onto it, which keeps it the same commit as
  the original extraction. A history rebuilt from snapshot branches alone
  (after `drop`, or settled in another repository after `import`) is
  authored by its settler: earlier authors were only in the history.

## Alternatives considered

**A "real identity" test** (an email holding an `@`, not the tool's name),
as first drafted. It guesses at bogus authors; an empty email is a
contract producers can keep.

**Always the user who settles.** It discards a real author a snapshot or
a manual commit carries.

**The snapshot's date replaced by the settle's.** `advise` dates files by
it: a re-sort would make every file look written today.

## Test plan

1. `a_tool_authored_snapshot_is_authored_by_the_user` — a history settled
   from `store::add`'s snapshots: each commit authored by the configured
   user, its author date the snapshot's; the snapshot branches still
   `git-seriate <>`.
2. `a_snapshots_author_is_kept` — a snapshot commit authored `Alice
   <alice@example.com>` (written by hand): its history commit authored so.
3. `a_rewrite_re_authors_only_the_authorless` — a history of tool-authored
   commits and a manual commit by Bob: `settle --no-cutoff` replaying it,
   then `compact`: tool-authored ones become the user, Bob kept, dates and
   trees unchanged.
4. `a_recreated_branch_is_the_original_commit` — a snapshot branch dropped,
   recreated by `settle` from the history: the same commit id as before.
5. `advise`'s dates unchanged on the existing tests.

## Measured outcome

2026-11-23. The four tests of `tests/author.rs` pass, and git-seriate's
and git-seriate-fdp's suites (92) under `env -i`, `advise`'s dates
unchanged.

Found implementing it: a history commit re-sorted while its snapshot
branch exists took its author from the branch (`git-seriate <>`), losing
an author the history held; it now takes the history commit's (S3). The
author identity falls back to the committer's, so that a settle that
could commit before (`GIT_COMMITTER_*` only) still can.
