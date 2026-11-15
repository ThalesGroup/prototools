<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0149 — reproto: path vs FQDN disambiguation for `-s`/`-p`/`-C` filtering

Status: implemented
Implemented in: 2026-07-19
App: reproto

## Background

This spec was originally "0149 — `-C/--filter-config` YAML file for
seed/prune lists" and was implemented on 2026-07-19 (Round 1): a
`-C/--filter-config PATH` option loading `seed:`/`prune:` lists from a
YAML file, unioned with `-s`/`-p` CLI flags, plus a simplification of
the pre-existing bare-value heuristic so that any unprefixed `-s`/`-p`/
`-C` value was assumed to mean `file:<value>`.

That simplification turned out to be wrong. Running `reproto -I
CORPUS . --dry-run` on a large corpus surfaced the real issue: `file:some/path` is
used throughout reproto to name two genuinely different things that
usually — but not always — coincide:

1. a path on the filesystem, typically a serialized `FileDescriptorSet`
   (`.pb`) or a `.proto` source file, found under one of the `-I` roots;
2. the `name` field of a `FileDescriptorProto` — a logical identity
   inside the reconstructed FQDN graph, established once a file is
   parsed, independent of which physical artifact produced it.

A build system can produce more than one
physical `.pb` artifact declaring the same `FileDescriptorProto.name`
(e.g. two independent targets both emit a descriptor for
`google/protobuf/struct.proto`); when both land in the merged output
tree, the build disambiguates the colliding filenames with a `~2`, `~3`, ...
suffix (`struct.pb` / `struct~2.pb`). `reproto`'s loader (spec 0148 G2)
already handles this at the FQDN-identity level — it silently keeps
whichever candidate it encounters first (by `-I` root order, then
filesystem traversal order) and drops the rest with a W7 warning — but
that tie-break is an accident of root/traversal order, not a decision
the user gets to make. Two mitigations were considered and rejected as
not solving the actual problem: sorting candidates deterministically
only makes the arbitrary choice reproducible, not correct; suppressing
the warning when the two files are byte-identical doesn't help when
they legitimately differ (which is exactly the case where the choice
matters).

The correct fix, per discussion, is to stop conflating the two
identities in the CLI/config syntax and let the user express *both*
kinds of intent explicitly: "don't ever load this physical file" and/or
"seed reachability from this physical file's contents", as distinct
from "don't ever load/do seed the FDP with this declared name". This
spec reworks `-s`/`-p`/`-C` accordingly, superseding Round 1's G6/G7.

## Goals

- **G1 (`-C/--filter-config` option — unchanged from Round 1).** The
  option itself, its YAML shape (`seed:`/`prune:` lists of strings, both
  optional), its strict top-level-key validation, and its union-merge
  with `-s`/`-p` CLI flags are all unchanged from the Round 1
  implementation. Only the per-entry bare-value interpretation (G2-G7
  below) changes.

- **G2 (prefixed vs bare disambiguates FQDN vs path).** Every `-s`/`-p`/
  filter-config string is classified by whether it carries a recognized
  prefix:
  - **FQDN prefixes** (`file:`, `desc:`, `enum:`, `serv:`, `meth:`,
    `fdsc:`) → an **FQDN pattern**, semantics fully unchanged from spec
    0074: matched against the post-load FQDN node graph in phases 4/5
    (`_find_matching_nodes`).
  - **`path:` prefix, or no recognized prefix at all** → a
    **filesystem path pattern**, matched against each candidate file's
    `-I`-root-relative path (`QualFile.rel_path`), root-independently
    (G6), *before* that file is loaded. `path:foo/bar.pb` and bare
    `foo/bar.pb` are fully equivalent; `path:` exists purely for
    readability (self-documenting in a shared filter-config file) and
    to let a path that itself contains a literal `:` be written
    unambiguously (see G7).

  This is one uniform rule applied identically to `-s`, `-p`, and
  filter-config `seed:`/`prune:` entries — and it is a full reversal of
  Round 1's G6/G7 ("bare implies `file:`"), which is now understood to
  have erased a real, useful distinction rather than removed an
  accidental one.

- **G3 (prune-by-path).** `-p some/path/pattern*` (equivalently, a bare
  `prune:` entry in a filter-config file) excludes every physical
  candidate file whose root-relative path matches the pattern from ever
  being loaded. This is enforced at file-discovery time (inside
  `_load_files()`, `load.py`), *before* parsing and *before* spec
  0148's G2 duplicate-FDP-name dedup pass — necessary because once G2
  discards a duplicate candidate its physical identity is gone; there's
  no way to recover it later at the FQDN-graph level. Consequences:
  - Pruning the "losing" side of a shadow-duplicate pair by its exact
    path is a no-op (it was already going to be dropped) — but pruning
    the "winning" side makes its sibling the sole remaining candidate,
    so it is kept deterministically, with **no W7 warning**, regardless
    of `-I` root order or filesystem traversal order.
  - If a prune-by-path pattern eliminates *every* physical candidate
    for some import name, `_load_files()` naturally returns an empty
    list for that lookup and the pre-existing W1 "missing dependency"
    warning machinery reports it — no new warning path is needed.

- **G4 (seed-by-path).** `-s some/path/pattern*` (equivalently, a bare
  `seed:` entry): for every physical candidate file whose
  root-relative path matches the pattern, *every* `FileDescriptorProto`
  produced by parsing that one physical file becomes an ordinary
  `file:<name>` FQDN seed, fed unmodified into Phase 5's existing
  reachability machinery. A single physical file can legitimately
  contain more than one FDP (`.pbset`/`.protoset`/a multi-`file{}`-entry
  `.textpb` — see `split_fdps()`), so "seed this path" means "seed
  everything this artifact declares". This is pure convenience/
  expansion: it does not by itself decide which physical duplicate
  "wins" a shared FDP name (G3 does that), and it does not change
  Phase 4/5's FQDN-matching logic at all.

- **G5 (`file:` FQDN identity unchanged).** `file:path/to/foo.proto`
  continues to mean exactly what it means today: "the FDP whose
  declared `name` is `path/to/foo.proto`", independent of which
  physical artifact produced it. Worked example: `-s
  file:path/to/foo.proto` seeds the single FDP named
  `path/to/foo.proto` (presumably found inside FDS
  `path/to/foo.pb`), whereas the bare form `-s path/to/foo.pb` instead
  seeds *every* FDP contained in that physical FDS.

- **G6 (root-independent path matching).** A bare path pattern is
  matched against each candidate's root-relative path directly, never
  against `root / rel_path` or an absolute filesystem path. The same
  pattern therefore matches a same-shaped file found under *any* `-I`
  root, not just the first/a specific one.

- **G7 (clear error on prefix-typo-shaped bare values).** If a bare
  value contains a `:` whose left-hand side is *not* one of the seven
  recognized prefixes (the six FQDN prefixes plus `path:`) — e.g. a
  typo like `fille:foo.proto` — reject with a clear `UsageError` naming
  the unrecognized prefix, rather than silently treating the whole
  string — colon included — as a path pattern that will almost
  certainly never match anything on disk. A path that itself
  legitimately contains a `:` character is written with the explicit
  `path:` prefix (`path:some:odd:name.pb`), which is always available
  as an escape hatch — this is what makes G7's strictness safe rather
  than a loss of expressiveness.

- **G8 (literal/glob pattern partitioning — performance baseline).**
  Path patterns are partitioned once, at parse time, into **literal**
  patterns (no `*`/`?`/`[` metacharacters) and **glob** patterns.
  Literal patterns are matched via `O(1)` set membership; only glob
  patterns fall back to a linear `fqdn_match()` scan. Since the common case
  — pruning or seeding one specific, exactly-named duplicate — is a
  literal path, this keeps per-candidate matching cost close to
  `O(files)` rather than `O(files × patterns)` even when a
  filter-config file lists many entries. Built in from the start, not
  deferred as a later optimization.

## Non-goals

- N1: No absolute-path pattern support — path patterns are always
  matched against root-relative paths.
- N2: No precedence between path-form and FQDN-form entries within the
  same `-s`/`-p`/filter-config list; both kinds freely coexist in one
  list and are simply routed to different matching engines internally.
- N3: No new glob syntax — path patterns reuse the exact same
  `PurePosixPath.full_match()`-based glob engine already used for FQDN
  patterns (`fqdn_match`, spec 0074, Python 3.13+), by treating `path:`
  as an ordinary FQDN prefix. `*` matches exactly one path segment,
  `**` matches any number of segments including zero — identical
  semantics to today's `file:`/`desc:`/etc. patterns, via the literal
  same function, not a re-implementation. (`matches_any_pattern`, the
  older `fnmatch`-based matcher in `load.py`/`reproto.py`, is legacy/
  dead code — its only caller is the `is_pruned()` function this spec
  deletes — and is *not* what path patterns are built on.)
- N4: Round 1's "bare implies `file:`" behavior is fully reverted, not
  deprecated or shimmed behind a flag. Round 1 was never tagged/
  released as a standalone behavior, so there is no external
  compatibility surface to preserve.
- N5: `-C/--filter-config` itself — the option, the YAML shape, strict
  top-level-key validation, union-merge with CLI flags — is unchanged
  (see G1); this spec only changes how individual `seed:`/`prune:`/
  `-s`/`-p` *entries* are classified.
- N6: No automatic/heuristic resolution of the
  shadow-duplicate case. The user must explicitly write a `prune:` (or
  `-p`) path pattern naming the unwanted physical duplicate; no
  "pick the better candidate" logic is added (this is the earlier,
  explicitly rejected "cheap fix" direction).

## Specification

### `reproto/src/reproto/phases.py`

`_normalise_fqdn_name()` gains `path` alongside `file` in its
"leave-as-is" branch (paths, like `file:` names, already use `/` and
must never have their `.` characters rewritten to `/`):

```python
def _normalise_fqdn_name(prefix: str, name: str) -> str:
    if prefix in ('file', 'path'):
        return name
    return name.replace('.', '/').lstrip('/')
```

This is the only change needed to `fqdn_match()` itself — with `path:`
recognized, `fqdn_match(Fqdn('path:<pattern>'), Fqdn('path:<subject>'))`
already does exactly the right thing, via the exact same
`PurePosixPath.full_match()` call used for every other prefix (N3).

### `reproto/src/reproto/load.py`

A small helper implements the literal/glob partitioning from G8 on top
of `fqdn_match()`, and replaces the existing, dead `is_pruned()` (never
called anywhere; it re-wraps a `rel_path` as a `file:` FQDN and checks
it against `ctx.pruned_fqdns` via the legacy `fnmatch`-based
`matches_any_pattern` — exactly the conflation, and the wrong matching
engine, this spec removes):

```python
_GLOB_CHARS = set('*?[')

class PathPatterns:
    """A set of root-relative path patterns, partitioned for fast lookup.

    Literal patterns (no glob metacharacters) are matched via O(1) set
    membership (G8). Glob patterns are matched via fqdn_match() using a
    synthesized 'path:' pseudo-FQDN — the exact same
    PurePosixPath.full_match()-based engine already used for -s/-p FQDN
    patterns (spec 0074): '*' matches one path segment, '**' matches
    any number of segments including zero.
    """
    def __init__(self, patterns: set[str]) -> None:
        self.literals = {p for p in patterns if not (_GLOB_CHARS & set(p))}
        self.globs = [p for p in patterns if _GLOB_CHARS & set(p)]

    def matches(self, rel_path: Path) -> bool:
        s = rel_path.as_posix()
        if s in self.literals:
            return True
        return any(
            fqdn_match(Fqdn(f'path:{p}'), Fqdn(f'path:{s}'))
            for p in self.globs
        )
```

### `reproto/src/reproto/context.py`

`Context` gains two new `PathPatterns` fields, threaded the same way
`pruned_fqdns` already is, plus one accumulator populated during
loading:

```python
class Context(Options):
    def __init__(
        self,
        pruned_fqdns: set[Fqdn],
        pruned_paths: PathPatterns,
        path_seeds: PathPatterns,
        **opts_kwargs: Any,
    ):
        ...
        self.pruned_paths = pruned_paths
        self.path_seeds = path_seeds
        # Populated by load.py as matching candidate files are discovered
        # (G4); consumed by reproto() after phase 1 to extend the FQDN
        # seed list passed to _phase5_reachability.
        self.path_seed_fqdns: set[Fqdn] = set()
```

`Context.from_options()` and `_make_context()` (`phases.py`) gain the
same two extra parameters, threaded from `reproto()`'s new
`path_seeds`/`path_prunings` parameters (see below); `_make_context`
wraps the raw `list[str]` pattern lists it receives in `PathPatterns(...)`
before constructing `Context`.

`_load_files()` (`load.py`) calls `ctx.pruned_paths.matches(rel_path)`
for every physical candidate it discovers, in both its branches,
skipping (not appending to `loaded_files`) any match (G3):

- directory-scan branch: inside the `for f in res_path.rglob('*')`
  loop, before constructing the `QualFile`.
- single-file branch: before each of the three `loaded_files.append(...)`
  call sites (text extension, binary extension, and the
  try-all-extensions `.proto` fallback loop).

`load_from_path()` additionally performs the seed-by-path expansion
(G4): for each raw candidate (pre-dedup) whose root-relative path
matches `ctx.path_seeds.matches(...)`, after `parse_qfile()` splits it
into `(name, fragment)` pairs, every resulting `name` is added to
`ctx.path_seed_fqdns` as `Fqdn('file:' + name)` — regardless of whether
that candidate goes on to survive the pre-existing G2 (spec 0148)
duplicate-name dedup pass. (A seeded-but-shadowed name still produces a
W7 warning exactly as today; G3/prune-by-path — not G4 — is the tool
for resolving that ambiguity.)

### `reproto/src/reproto/reproto.py`

`reproto()` gains two new parameters, `path_seeds: list[str]` and
`path_prunings: list[str]` (root-relative glob patterns, no FQDN
prefix), passed through `_make_context()` into the new `Context`
fields. After `_phase1_load_files()` returns (all files, both from the
initial positional-argument scan and from transitive import discovery,
have gone through `load_from_path()` and therefore through the G4
seed-by-path expansion above), the accumulated
`ctx.path_seed_fqdns` are unioned into the seed list passed to Phase 5:

```python
_phase5_reachability(ctx, seeds + list(ctx.path_seed_fqdns), topo)
```

No change to `_phase4_pruning()`, `_phase5_reachability()`,
`_find_matching_nodes()`, or `matches_any_pattern()` — all remain
purely FQDN-based, exactly as today. Prune-by-path only ever removes
candidates *before* they reach the FQDN graph; seed-by-path only ever
*adds* ordinary FQDN seeds before Phase 5 runs.

### `reproto/src/reproto/cli.py`

`_resolve_seed_or_prune()` (Round 1's "bare implies `file:`"
simplification) is removed. In its place, a classifier routes each raw
`-s`/`-p`/filter-config string to either the FQDN or the path bucket:

```python
_FQDN_PREFIXES = ('file', 'desc', 'enum', 'serv', 'meth', 'fdsc')
_VALID_PREFIXES = _FQDN_PREFIXES + ('path',)

def _split_seed_or_prune(value: str) -> tuple[str, str]:
    """Classify a raw -s/-p/filter-config value as ('fqdn', v) or ('path', v).

    A value with a recognized FQDN prefix (file:, desc:, enum:, serv:,
    meth:, fdsc:) is an FQDN pattern, unchanged from spec 0074. A value
    with the path: prefix, or a bare value with no recognized prefix at
    all, is a filesystem path pattern, matched root-independently
    against each candidate file's -I-root-relative path before it is
    loaded (spec 0149). path: and bare are fully equivalent; path:
    exists for readability and to allow a literal ':' inside a path
    (G7).
    """
    if ':' in value:
        prefix, _, rest = value.partition(':')
        if prefix == 'path':
            return ('path', rest)
        if prefix in _FQDN_PREFIXES:
            return ('fqdn', value)
        raise click.UsageError(
            f"'{value}': '{prefix}:' is not a recognized prefix "
            f"({', '.join(_VALID_PREFIXES)}); if this is meant to be a "
            f"filesystem path, use the 'path:' prefix"
        )
    return ('path', value)
```

At the `reproto(...)` call site, the merged CLI + filter-config lists
(`all_seeds`/`all_stumps`, unchanged from Round 1's G5 union-merge) are
partitioned into FQDN and path buckets, with `_normalise_fqdn()`
applied only to the FQDN bucket:

```python
def _partition(values: list[str]) -> tuple[list[Fqdn], list[str]]:
    fqdns: list[Fqdn] = []
    paths: list[str] = []
    for v in values:
        kind, resolved = _split_seed_or_prune(v)
        if kind == 'fqdn':
            fqdns.append(_normalise_fqdn(resolved))
        else:
            paths.append(resolved)
    return fqdns, paths

seed_fqdns, seed_paths = _partition(all_seeds)
prune_fqdns, prune_paths = _partition(all_stumps)

reproto(
    list(pb_path) if pb_path else [Path('.')],
    pb_files,
    seed_fqdns,
    prune_fqdns,
    proto_out,
    options,
    path_seeds=seed_paths,
    path_prunings=prune_paths,
)
```

(The exact parameter wiring of `reproto()`'s call — positional vs.
keyword — is illustrative; final shape to be settled at implementation
time to match the existing call convention.)

### `reproto/src/reproto/filter_config.py`

No change: `load()` continues to return raw, unresolved `(seed, prune)`
string lists. `cli.py`'s `_split_seed_or_prune()`/`_partition()` apply
uniformly to CLI and filter-config strings alike (G1/G2), continuing
Round 1's principle that filter-config entries and CLI flags share one
resolution pipeline — only the classification rule itself has changed.

No man-page edit needed (unchanged from Round 1: `gen_man.py` derives
the man page from click metadata).

### Glob semantics note: `*` is one segment, `**` is any number (incl. zero)

Path patterns are matched via `fqdn_match()`, the exact same
`PurePosixPath.full_match()`-based engine already used for `-s`/`-p`
FQDN patterns (N3) — genuine hierarchical glob, not `fnmatch`. `*`
matches exactly one path segment and never crosses a `/`; `**` matches
any number of segments, *including zero*. This is already covered by
existing tests for the FQDN case
(`test_T2_star_does_not_cross_slash`: `subdir/*` does not match
`subdir/nested/foo.proto`, but `subdir/**` does) and applies identically
to path patterns.

Consequently a single `**/*~2.pb` matches **both** a top-level
`foo~2.pb` (`**` matching zero segments) **and** a nested
`bar/foo~2.pb` (`**` matching one segment) — one pattern, any depth.
A bare `*~2.pb` (no `**`), by contrast, matches only a top-level
`foo~2.pb`, since a single `*` never crosses `/`.

### Worked example: pruning all shadow-duplicates in one pattern

```yaml
prune:
  - "**/*~2.pb"   # drop every `~2`-uniquified duplicate, any depth
  - "**/*~3.pb"
```

This is a deliberate, explicit policy chosen by the user — "prefer the
un-suffixed artifact whenever both exist" — not a guess the tool makes
on their behalf; nothing about the `~N` suffix itself implies which
physical target is "more correct" (see Background). If a specific pair
needs the opposite treatment, that exception is expressed by pruning
the *other* (un-suffixed) member of that one pair instead — G3
patterns are a pure union of exclusions with no negation/override
mechanism, so a blanket rule and a per-pair exception cannot both be
listed for the same pair; the more targeted pattern must be used in
place of the blanket one for that pair.

## Test plan

- Unit: `_split_seed_or_prune()` — each of the 6 FQDN prefixes →
  `('fqdn', unchanged)`; `path:foo/bar.pb` → `('path', 'foo/bar.pb')`;
  a bare non-wildcard value → `('path', unchanged)`; a bare wildcard
  value → `('path', unchanged)`; a bare value shaped like an
  unrecognized prefix (e.g. `fille:foo.proto`) → `UsageError` naming
  the bad prefix; `path:some:odd:name.pb` → `('path',
  'some:odd:name.pb')` (only the first `:` is the prefix separator).
- Unit: `PathPatterns` (G8) — a literal pattern set matches via exact
  string equality only (assert a glob-shaped string that happens to
  equal a literal pattern character-for-character still matches, and
  that a non-matching glob-shaped string does not spuriously match a
  literal); a glob pattern set matches via `fqdn_match()`.
- Glob semantics (documentation-pinning test, mirrors
  `test_T2_star_does_not_cross_slash` for the path case): `**/*~2.pb`
  matches both a top-level `foo~2.pb` and a nested `bar/foo~2.pb`;
  a bare `*~2.pb` matches only the top-level one, never the nested one.
- Prune-by-path (G3): fixture recreating the `~2` scenario —
  two physically distinct files under two different `-I` roots that
  parse to the same `FileDescriptorProto.name`. `-p` (or filter-config
  `prune:`) naming one path exactly → the other survives
  deterministically, with no W7 warning, independent of `-I` root
  order.
- Prune-by-path eliminating every physical candidate for an import →
  existing W1 "missing dependency" warning fires; no crash, no new
  warning path.
- Seed-by-path (G4): a `.protoset`/`.pbset` fixture bundling 2+ FDPs;
  `-s <physical-path>` → all bundled FDPs (and their reachable graph)
  appear in the output.
- G5 regression: `-s file:foo.proto` continues to mean "the FDP named
  foo.proto", unchanged from pre-Round-1 behavior.
- G6: a bare path pattern matches identically-shaped files found under
  two different `-I` roots (root-independence).
- Regression: every existing `-s`/`-p`/`-C` test that already spells an
  explicit prefix continues to pass unchanged. The Round-1
  `test_filter_config.py` tests named `G6`/`G7` (which asserted "bare
  → `file:`") are rewritten to assert the new default ("bare → path")
  instead.
