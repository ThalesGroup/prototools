<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0074 — Glob pattern support for `--seed` and `--prune`

**Status:** implemented
**App:** reproto

---

## Background

`--seed` and `--prune` accept FQDNs of the form `file:foo.proto` or
`desc:my.pkg.MyMsg`.  The matching in phase 4 (pruning) and phase 5 (seeds)
already uses `fnmatch` — so glob patterns like `file:vendor/common/util_*.proto`
silently work at node-selection time.  However:

1. This is undocumented and untested.
2. `fnmatch` treats `*` as matching any sequence including `/` and `.` — it
   behaves like `**` everywhere, which is unintuitive.
3. The pattern matching does **not** apply during the pool-loading phase
   (phase 2).  A user-pruned file is loaded into the pool first, wins any
   symbol conflict, and only gets pruned later.  This causes a visible bug: if
   file A and file B share a symbol, and the user passes `--prune file:A`,
   reproto auto-prunes B (because A loaded first and "won"), then user-prunes
   A — leaving neither file in the pool but emitting a confusing warning about
   B being pruned.

---

## Goals

1. Define a proper `fqdn_match` helper using `PurePosixPath.match()` so that
   `*` does not cross `/` or `.` separators and `**` is needed for recursive
   matching — consistent with user expectations.
2. Apply `--prune` patterns at pool-load time: if a file's FQDN matches any
   user pruning pattern, skip loading it into the pool entirely.
3. Document glob pattern support for `--seed` and `--prune` in `--help` and
   the manpage.
4. Add regression tests covering:
   - Plain FQDN `--prune` at load time (the bug above).
   - Glob pattern `--prune` at load time.
   - Plain FQDN `--seed` with glob pattern `--prune`.
   - Glob pattern `--seed`.

## Non-goals

- Changing the semantics of `--seed` or `--prune` beyond what is described
  here.
- Supporting regex patterns (only `PurePosixPath`-style globs: `*`, `**`,
  `?`, `[seq]`).

---

## Specification

### FQDN pattern normalisation

All FQDN patterns are normalised at ingestion in `_normalise_fqdn` (in
`cli.py`), unconditionally:

- For `file:` patterns: the name part is left as-is (already uses `/`).
- For all other prefixes (`desc:`, `enum:`, `serv:`, `meth:`, `fdsc:`, etc.):
  replace every `.` in the name part with `/`, and strip the leading `/` if
  present.

This means `desc:my.pkg.MyMsg` is stored internally as `desc:my/pkg/MyMsg`,
and `desc:my.pkg.*` becomes `desc:my/pkg/*`.  All FQDNs in `ctx.nodes` are
similarly normalised at node-creation time so that matching is consistent.

### `fqdn_match` helper

Add a module-level helper (e.g. in `fqdn.py` or `phases.py`):

```python
from pathlib import PurePosixPath

def fqdn_match(pattern: Fqdn, subject: Fqdn) -> bool:
    """Return True iff subject matches pattern.

    Both pattern and subject must already be normalised (. replaced with /
    in the name part for non-file FQDNs).  Matching is anchored (full path);
    * matches one segment, ** matches any number of segments.
    """
    if ':' not in pattern or ':' not in subject:
        return pattern == subject
    p_prefix, p_name = pattern.split(':', 1)
    s_prefix, s_name = subject.split(':', 1)
    if p_prefix != s_prefix:
        return False
    return PurePosixPath(f'/{s_name}').match(f'/{p_name}')

def fqdn_matches_any(subject: Fqdn, patterns: list[Fqdn]) -> bool:
    return any(fqdn_match(p, subject) for p in patterns)
```

Replace all existing `fnmatch` call sites (currently only in
`_find_matching_nodes`) with `fqdn_match`.

### Load-time pruning (fix)

In `_phase2_build_pool`, immediately before `ctx.pool_db.Add(fdp)`, check
whether `file:<fdp.name>` matches any pattern in the user pruning list.  If
it does, mark the node as pruned and skip loading:

```python
if fqdn_matches_any(Fqdn(f'file:{fdp.name}'), prunings):
    n.is_pruned = True
    ctx.pruned_file_names.add(fdp.name)
    continue
```

Both `n.is_pruned = True` and `ctx.pruned_file_names.add(fdp.name)` are
required: without them, phases 3/4 would still treat the node as present, and
importers of the skipped file would not have their dependency stripped —
causing W5 warnings or crashes.  This is the same bookkeeping that
`_prune_if_duplicate` performs.

This check must run **before** `_prune_if_duplicate` so that a user-pruned
file never enters the pool and never wins a symbol conflict.

`prunings` (the list of normalised user-specified patterns) must be threaded
into `_phase2_build_pool`.  It is already available as a parameter to
`reproto()` and passed to `_make_context`; it should also be passed directly
to `_phase2_build_pool`.

Note: only `file:` patterns can be applied at load time (messages are not
individually loaded); `desc:` and other patterns will simply never match a
`file:` FQDN and are naturally skipped.

### Documentation updates

**`cli.py` help text** — update both `--seed` and `--prune`:

```
-s, --seed TEXT   FQDN or glob pattern to treat as an output root.
                  Plain FQDN: desc:my.pkg.MyMsg or file:foo.proto.
                  Glob: file:vendor/common/*.proto (one level),
                  file:vendor/** (recursive), desc:my.pkg.* (one level).

-p, --prune TEXT  FQDN or glob pattern to exclude from output.
                  Plain FQDN: desc:my.pkg.MyMsg or file:foo.proto.
                  Glob: file:vendor/common/*.proto (one level),
                  file:vendor/** (recursive), desc:my.pkg.* (one level).
```

All patterns must start with a valid prefix (`file:`, `desc:`, `enum:`,
`serv:`, `meth:`, `fdsc:`).  Bare patterns without a prefix are rejected
with a clear error.

**`man/man1/reproto.1`** — update the `.TP` entries for `-s`/`--seed` and
`-p`/`--prune` to mention glob patterns and the `*`/`**` distinction.

---

## Regression tests

Add a new test file `test_prune_seed_patterns.py`.  All tests use small
in-memory proto fixtures (compiled via `compile_proto`) rather than subprocess
calls where possible, falling back to subprocess for CLI flag tests.

### Fixtures needed

```
prune_glob_a.proto   — package prune_glob; message MsgA {}
prune_glob_b.proto   — package prune_glob; message MsgB {}  (imports a)
prune_glob_c.proto   — package prune_glob; message MsgC {}  (imports a)
```

And a duplicate-conflict pair:

```
prune_conflict_winner.proto  — package prune_conflict; message Shared {}
prune_conflict_loser.proto   — package prune_conflict; message Shared {}
```

### Test cases

**T1 — load-time prune: user-pruned file does not win symbol conflict**

Setup: `prune_conflict_winner.proto` and `prune_conflict_loser.proto` define
the same symbol.  Pass `--prune file:prune_conflict_winner.proto`.

Expected:
- `prune_conflict_winner.proto` is not loaded into the pool.
- `prune_conflict_loser.proto` is loaded and rendered.
- No warning about `prune_conflict_loser.proto` being auto-pruned.

**T2 — glob pattern `--prune` at load time**

Setup: all three `prune_glob_*.proto` files present.
Pass `--prune 'file:prune_glob_b*'` (single-segment glob).

Expected:
- `prune_glob_b.proto` is not rendered.
- `prune_glob_a.proto` and `prune_glob_c.proto` are rendered.

Also verify that `*` does not cross `/`: a pattern `file:subdir/*` does not
match `file:subdir/nested/foo.proto`, while `file:subdir/**` does.

**T3 — glob pattern `--seed`**

Setup: all three `prune_glob_*.proto` files present.
Pass `--seed 'file:prune_glob_b*'`.

Expected:
- Only `prune_glob_b.proto` (and its transitive imports: `prune_glob_a.proto`)
  are rendered.
- `prune_glob_c.proto` is not rendered.

**T4 — glob `--prune` combined with plain `--seed`**

Setup: all three `prune_glob_*.proto` files present.
Pass `--seed file:prune_glob_c.proto --prune 'file:prune_glob_a*'`.

Expected:
- `prune_glob_a.proto` is excluded (pruned at load time).
- `prune_glob_c.proto` is rendered (its import of `a` is stripped with a W5
  warning or silently, since `a` is intentionally pruned).

---

## Implementation notes

- `_normalise_fqdn` in `cli.py` gains the `.`→`/` substitution for non-`file:`
  name parts, applied unconditionally.  Node creation code must apply the same
  normalisation so that `ctx.nodes` keys are consistent with normalised patterns.
- `fqdn_match` and `fqdn_matches_any` are new helpers; the `fnmatch` import in
  `phases.py` can be removed once all call sites are migrated.
- `_phase2_build_pool` currently receives `ctx` and `topo`.  Add `prunings:
  list[Fqdn]` as a third parameter and pass it from `reproto()`.
- Existing behaviour for auto-duplicate pruning (`_prune_if_duplicate`) is
  unchanged.
- Prefix validation (rejecting bare patterns without a prefix) should be added
  to `_normalise_fqdn` and emit a clear `UsageError`.

---

## Implemented in

2026-05-20
