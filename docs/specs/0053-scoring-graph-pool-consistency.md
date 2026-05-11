<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0053 — reproto: scoring-graph crash when a dependency was pruned as duplicate

**Status:** implemented
**Implemented in:** 2026-05-11
**App:** reproto

---

## Background

Running `reproto --use-variant all --emit-scoring-graphs` on a large corpus
(e.g. the full `googleapis/googleapis` repo) crashes with:

```
TypeError: Couldn't build proto file into descriptor pool:
  Depends on file 'google/api/resource.proto', but it has not been loaded
```

in `_phase_emit_scoring_graphs`.

---

## Root cause

### Two pools, two semantics

`Context` holds two related objects:

```python
self.pool_db: DescriptorDatabase = DescriptorDatabase()
self.pool:    DescriptorPool     = DescriptorPool(self.pool_db)
```

`pool_db` is a raw key→serialized-FDP store.  `pool` is a lazy-resolving
descriptor pool backed by `pool_db`: when `pool.FindFileByName(name)` is
called it reads the FDP from `pool_db` and **recursively resolves every
declared `dependency`** — each must be present in `pool_db` under the exact
path name stated in the FDP.  There is no lazy loading or tolerance for
unresolved references.

### Duplicate-symbol pruning can leave a path name unregistered

When two files declare identical symbols (e.g. `google/api/resource.proto`
and `preview/google/api/resource.proto`), phase 2 adds whichever is processed
first to `pool_db` and marks the other `is_pruned`.  The pruned file is never
added to `pool_db`.  Which copy wins is non-deterministic (it depends on
topo-sort rank ordering within a rank).

If the `preview/` copy wins, `pool_db` contains the symbols under
`"preview/google/api/resource.proto"`.  The canonical path
`"google/api/resource.proto"` is never registered in `pool_db`.

### The render path never calls `pool.FindFileByName` on importers

Phase 3 builds `ReFileDescriptorProto` nodes by calling
`ctx.pool_db.FindFileByName(desc.name)` directly — this returns the raw
`FileDescriptorProto` stored in `pool_db` with no recursive dependency
resolution.  The proto render path (phase 7) works entirely from these
pre-built `Re*` nodes and never calls `ctx.pool.FindFileByName` on any
importer file.

### The scoring-graph path calls `pool.FindFileByName` on importers

`_phase_emit_scoring_graphs` needs the richer `FileDescriptor` API
(`message_types_by_name`, `fields_by_number`, `field.is_packed`,
`field.message_type.full_name`, `field.enum_type.values_by_number`) that is
only available on `google.protobuf.descriptor.FileDescriptor` objects, not on
raw `FileDescriptorProto` messages.  It obtains these by calling
`ctx.pool.FindFileByName(proto_name)` for each summoned file.

When summoned file `A` declares `dependency: "google/api/resource.proto"` and
that path was never registered in `pool_db` (the `preview/` copy won),
`pool.FindFileByName(A)` raises:

```
TypeError: Couldn't build proto file into descriptor pool:
  Depends on file 'google/api/resource.proto', but it has not been loaded
```

Note: calling `pool.FindFileByName` eagerly at phase 2 time (right after
`pool_db.Add`) would **not** help — the same `TypeError` would fire there for
the same reason.  The pool's strict dependency resolution is the fundamental
constraint.

### Consequence: scoring-graph output is inconsistent with proto rendering

If a file was rendered successfully (because the render path bypasses
`pool.FindFileByName`), its scoring graph must also succeed.  Currently it
does not.

---

## Goals

1. The scoring-graph emitter must not crash when a summoned file has a
   dependency that was pruned.

2. Scoring-graph output must be **consistent** with proto rendering: every
   file that was rendered gets a scoring graph.

3. The pruned import must still appear in the reconstructed `.proto` as an
   orphan line so that information is not silently lost.

4. Files for which a scoring graph still cannot be built (defensive catch)
   emit a W6 warning rather than crashing silently.

---

## Non-goals

- Changing the duplicate-symbol pruning logic or making the winner
  deterministic (separate concern).
- Making the render path use `ctx.pool` (it works correctly as-is).

---

## Specification

### §1 — Strip pruned dependencies from FDPs before `pool_db.Add`

Add a helper `_strip_pruned_dependencies(ctx, fdp)` (alongside the existing
`_strip_self_dependency`) that removes from `fdp.dependency` any entry whose
path is in the set of pruned file names.  Call it for every FDP just before
`pool_db.Add` at all three binary/text add-sites in `_phase2_build_pool`.

`ctx` already accumulates pruned names via `n.is_pruned = True` in
`_prune_if_duplicate`; a matching `ctx.pruned_files: set[str]` (or reuse the
existing `ctx.pruned_fqdns` set, stripping the `file:` prefix) provides the
lookup.

After stripping, `pool.FindFileByName` on any importer will succeed because
the pruned path name no longer appears in the FDP's `dependency` list.

Emit an immediate warning (W3-style, not squashed) for each stripped
dependency so the user knows an import edge was silently dropped:

```
Warning: stripped pruned dependency "google/api/resource.proto"
    from google/api/client.proto (pruned in favour of
    preview/google/api/resource.proto)
```

### §2 — Record stripped dependencies on `ReFileDescriptorProto`

Add two new list attributes to `ReFileDescriptorProto`:

```python
stripped_dependencies:        list[str]   # normal imports stripped
stripped_public_dependencies: list[str]   # public imports stripped
```

Initialise them to `[]` in `NodeBase.__new__` guarded by
`if isinstance(instance, ReFileDescriptorProto)` (or unconditionally, as
empty lists are harmless on non-file nodes — implementer's choice).

Populate them in `_strip_pruned_dependencies`: for each removed entry record
its path in the appropriate list on the corresponding `ReFileDescriptorProto`
node (looked up via `ctx.nodes`).

### §3 — Render stripped dependencies as orphan import lines

In `ReFileDescriptorProto.render` (in `re_file.py`), immediately after the
existing dependency-rendering loop, emit one orphan line per stripped
dependency:

```python
for dep in self.stripped_dependencies:
    out.append(BlockLine(f'import "{dep}";', depth, ORPHAN))
for dep in self.stripped_public_dependencies:
    out.append(BlockLine(f'import public "{dep}";', depth, ORPHAN))
```

This renders as `/// import "google/api/resource.proto";` in the output
`.proto` file, consistent with the existing orphan convention.

### §4 — Defensive W6 catch in `_phase_emit_scoring_graphs`

Even after §1, retain `KeyError` and `TypeError` handlers around
`pool.FindFileByName` as a defensive measure, emitting a W6 warning through
the squashed-log engine:

```python
try:
    fd = ctx.pool.FindFileByName(proto_name)
except KeyError:
    get_collector().w6(proto_name, "scoring graph", "not in descriptor pool")
    continue
except TypeError as e:
    get_collector().w6(proto_name, "scoring graph", str(e))
    continue
```

After §1 these handlers should never fire for files affected by duplicate
pruning, but they protect against any future scenario where a pool descriptor
cannot be built.

### §5 — Regression test

Add a test that:

1. Compiles two `.proto` fixture files with identical symbols (`dup_a.proto`,
   `dup_b.proto`) and a third `dup_importer.proto` that imports `dup_a.proto`,
   all to mono-fdp `.pb` files.
2. Runs `reproto --use-variant descriptor --emit-scoring-graphs` on all three.
3. Asserts exit code 0.
4. Asserts the scoring-graph YAML for `dup_importer.proto` exists and is
   non-empty.
5. Asserts the reconstructed `dup_importer.proto` contains an orphan
   `/// import "dup_a.proto";` line (when `dup_a` was the one pruned).

---

## Open questions

None.
