<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0158 — reproto: loud error on schema-db canonical-name collision after import rewrite

Status: implemented
App: reproto
Implemented in: 2026-07-22

## Background

`reproto --schema-db-out=...` can silently produce an invalid
`FileDescriptorSet`: one whose `.desc` file contains two entries with
the same `FileDescriptorProto.name` but different content. Loading
such a set later (e.g. `prost_reflect::DescriptorPool::decode` in
`protolens`/`prototext`) fails with:

```
error: descriptor '.../db.desc': invalid descriptor: a different file
named 'google/protobuf/descriptor.proto' has already been added
```

Root cause, confirmed with a real corpus
(a private 466-file proto tree, in the sibling `prototools` repo,
containing both a variant's `descriptor.proto` under another package,
imported by 22 other files, and a
standalone `google/protobuf/descriptor.proto`, imported by nothing
else):

- The active `$REPROTO_VARIANT` (`proto2.yaml`) defines an
  `import_rewrites` rule `proto2/` → `google/protobuf/`,
  applied by `canonize_dependency()` (`mappings.py`) at *render* time
  (`re_file.py`, `ReFileDescriptorProto.render()`,
  `fdp_out.name = canonize_dependency(ctx, fdp_out.name)`).
- Both `proto2/descriptor.proto` and the standalone
  `google/protobuf/descriptor.proto` are independently `is_summoned`
  (spec 0148's load-time dedup does not merge them — they have
  distinct declared `.name`s at load time, so it correctly treats
  them as two different files). Both are rendered by
  `_phase7_output`'s `summoned` loop.
- After rendering, `proto2/descriptor.proto`'s
  `fdp_out.name` is rewritten to `google/protobuf/descriptor.proto`
  by `canonize_dependency` — now colliding, byte-for-byte differently,
  with the standalone file's own (already-canonical)
  `google/protobuf/descriptor.proto` entry.
- Both entries are appended to `ctx.schema_db_fdps` with no collision
  check (`phases.py`, `_phase7_output` line ~1452 and
  `_phase_build_schema_db`'s WKT-promotion loop line ~1636).
- `_phase_build_schema_db` then builds
  `fdp_by_name = {f.name: f for f in ctx.schema_db_fdps}`
  (`phases.py` line ~1638) — a plain dict comprehension that silently
  keeps whichever entry appears *last* in iteration order and drops
  the other. The resulting `.desc` file (built from
  `ctx.schema_db_fdps` directly, not from the deduped `fdp_by_name`)
  still contains **both** raw entries under the same name — an
  invalid `FileDescriptorSet` that fails downstream at consumption
  time, with no diagnostic anywhere in `reproto` itself pointing at
  the actual cause.

This is a distinct timing from spec 0148's load-time dedup: spec 0148
catches two *identically-named* files loaded from different `-I`
roots, before any rewrite. This bug is two *differently-named* files
that only become identical **after** `canonize_dependency`'s
output-time rewrite — a case spec 0148 cannot see.

## Goals

- **G1.** After `_phase_build_schema_db` has accumulated all entries
  into `ctx.schema_db_fdps` (i.e. immediately before the existing
  `fdp_by_name` dict comprehension, `phases.py` line ~1638), detect
  any two entries that share a canonicalized `.name` but have
  different serialized content, and abort immediately with a clear,
  actionable error naming both original (pre-canonicalization) source
  files and the shared canonical name — instead of silently keeping
  one and discarding the other.
- **G2.** Entries that canonicalize to the same name **and** have
  byte-identical serialized content are not an error — silently keep
  one, exactly as today's dict-comprehension behavior already does
  for that (harmless) case. This mirrors `prost_reflect`'s/protobuf's
  own `DescriptorPool::decode` semantics, which only reject a
  duplicate name when content actually differs.
- **G3.** The error is fatal (`sys.exit(1)`, matching the existing
  I/O-error precedent a few lines above in the same loop) — building
  a `.desc` that is silently invalid is worse than failing the build.

## Non-goals

- N1: No change to `canonize_dependency`'s rewrite rules or logic —
  this spec only adds detection of the resulting collision, not any
  change to which rewrites apply or how.
- N2: No automatic resolution of a detected collision (e.g. "prefer
  the file more other files depend on," "keep first," "keep last").
  The fix makes the failure loud and immediate; resolving it (e.g. via
  `-p`/`--prune` to remove one of the colliding sources from the
  corpus) is left to the user.
- N3: No change to `--proto-out`'s (non-schema-db) plain-file output
  path — two files canonicalizing to the same disk path there just
  overwrite each other on disk, a different, pre-existing, and
  out-of-scope concern.
- N4: No change to spec 0148's load-time dedup mechanism — it remains
  the correct, separate fix for same-named collisions *before* any
  rewrite; this spec's collision check is for collisions that only
  appear *after* the rewrite.

## Specification

### `reproto/src/reproto/context.py`

New field parallel to `schema_db_fdps`, tracking each entry's
original (pre-canonicalization) name for diagnostics:

```python
# Original (pre-canonicalization) .name of each entry in
# schema_db_fdps, same index correspondence — spec 0158, used only to
# name both sides of a canonical-name collision in the error message.
self.schema_db_fdp_origins: list[str] = []
```

### `reproto/src/reproto/phases.py`

Both existing `ctx.schema_db_fdps.append(slot.out)` call sites gain a
matching append to the new parallel list:

- `_phase7_output`'s render loop (~line 1452):
  ```python
  ctx.schema_db_fdps.append(slot.out)
  ctx.schema_db_fdp_origins.append(re_fdp.name)
  ```
- `_phase_build_schema_db`'s WKT-promotion loop (~line 1636):
  ```python
  ctx.schema_db_fdps.append(slot.out)
  ctx.schema_db_fdp_origins.append(extra_node.name)
  ```

The `fdp_by_name` dict comprehension (~line 1638) is replaced with a
collision-checking loop:

```python
seen: dict[str, tuple[str, FileDescriptorProto]] = {}
for origin, fdp in zip(ctx.schema_db_fdp_origins, ctx.schema_db_fdps):
    prior = seen.get(fdp.name)
    if prior is not None:
        prior_origin, prior_fdp = prior
        if prior_fdp.SerializeToString() != fdp.SerializeToString():
            cli_error(
                f"error: schema-db name collision: '{prior_origin}' and "
                f"'{origin}' both canonize to '{fdp.name}' with "
                "different content — cannot build a valid "
                "FileDescriptorSet. Use -p/--prune to remove one of "
                "the colliding sources."
            )
            sys.exit(1)
        continue
    seen[fdp.name] = (origin, fdp)
fdp_by_name = {name: fdp for name, (_origin, fdp) in seen.items()}
```

`sys` and `cli_error` are already imported/used in this function (see
the existing I/O-error handling a few lines above).

## Test plan

- New regression test: two seed FDPs with distinct `.name`s that
  canonicalize to the same name under a variant's `import_rewrites`
  (e.g. a minimal variant YAML with a `rewrite` rule) and *different*
  content — assert the run exits non-zero and the error message names
  both original files and the shared canonical name.
- New test: same setup but with *identical* content on both sides —
  assert the run succeeds and the resulting `.desc` contains exactly
  one entry under the canonical name (G2).
- Regression: rebuild that private tree (sibling `prototools` repo,
  with its variant) without
  `-p`/`--prune` — assert the new collision error now fires (this was
  the reported bug) instead of silently producing an invalid `.desc`.
- Regression: existing `--schema-db-out` tests with no colliding names
  continue to pass unchanged.
- Regression: spec 0148's shadow-warning (W7) tests continue to pass
  unchanged — that mechanism is untouched (N4).
