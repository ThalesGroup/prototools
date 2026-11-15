<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0080 — Schema DB WKT completion

**Status:** implemented
**Implemented in:** 2026-05-23
**App:** reproto

---

## Background

`--build-schema-db` produces a `FileDescriptorSet` (`.desc` file) that
prototext uses as its schema database.  For the database to be self-contained,
every `FileDescriptorProto` referenced by an import in the set must itself be
present in the set.

After phase 6 (summoning), `is_summoned` is True only for files that contain
at least one type reachable from the user's input.  Well-known types (WKTs such
as `google/protobuf/timestamp.proto`, `google/protobuf/any.proto`, etc.) and
the variant's `descriptor.proto` are loaded as *fallback* files.  They are
present in `ctx.nodes` with fully-populated child Re* nodes (phase 3 creates
`ReFileDescriptorProto` nodes for every pool entry), but they are not summoned
unless a user explicitly targets them.  As a result:

- Phase 7's render loop skips them (`is_summoned=False`).
- They are absent from `ctx.schema_db_fdps`.
- The schema DB is incomplete: types in summoned files that import WKTs cannot
  be decoded because the decoder cannot find the WKT FDP in the set.

The previous workaround (commits `260f81d` and `4c9f111`) forced
`write_variant_descriptor=True` and `keep_variant_descriptor=True` whenever
`--build-schema-db` is active.  This is incorrect:

- It entangles schema-DB concerns with the text-rendering pipeline.
- It only covers `descriptor.proto`; other WKTs that happen not to be summoned
  remain absent.
- It forces the variant descriptor path (e.g.
  `proto2/descriptor.proto`) to be preserved verbatim, which may be
  the right behaviour for the DB but is reached via a side effect rather than
  an explicit mechanism.

Both commits should be reverted.

---

## Goals

1. Produce a self-contained schema DB: every file whose name appears in any
   `dependency` list of a DB entry must itself appear in the DB.
2. Remove the two workaround lines introduced by commits `260f81d` / `4c9f111`.
3. Keep `--keep-descriptor-path` and `--emit-descriptor` orthogonal to
   `--build-schema-db` (no forced coupling in `cli.py`).

---

## Non-goals

- Changing the summoning logic in phases 5–6 for normal (non-schema-DB) runs.
- Changing phase 7's `.proto`/`.pb` output — WKT files are not written there.
- Renaming `--keep-descriptor-path` or `--emit-descriptor`.

---

## Design

### Overview

The fix has three parts:

1. **Phase 6 sub-pass 3**: identify WKT/fallback nodes that are transitive
   dependencies of summoned files but not themselves summoned.  Collect them
   into a new list `ctx.schema_db_extra_nodes` on `Context`.  Do *not* modify
   `is_summoned` or `is_reachable` — phase 7 runs completely unmodified.

2. **Inside `_phase_build_schema_db`**, after the phase 7 render loop has
   populated `ctx.schema_db_fdps` with user-summoned FDPs: iterate
   `ctx.schema_db_extra_nodes`, promote each node to
   `is_summoned=True` / `is_reachable=True` (and its `contains`-descendants),
   render its binary FDP, and append to `ctx.schema_db_fdps`.

3. The existing Kahn's topological sort then assembles a complete
   `FileDescriptorSet` from the now-full `ctx.schema_db_fdps`.

WKT types are **included** in the scoring graph and index — they are legitimate
types that users may want to score against.  No exclusion is needed.

### Why not a flag on NodeBase

Storing the extra nodes in a list on `Context` rather than a flag on `NodeBase`
keeps the schema-DB concern localised.  `NodeBase` should not carry
infrastructure that only applies to a single advanced CLI flag.  The list is
also more economical: there are typically very few WKT/fallback files.

### Why promotion happens inside `_phase_build_schema_db`

Phase 7 writes `.proto` and `.pb` files to disk for user-summoned files.  We
must not alter the summoned set before phase 7 completes, or WKT files would
be written to the output tree.  `_phase_build_schema_db` runs after phase 7
and has full control over what enters `ctx.schema_db_fdps`; it is the right
place to inject the WKT binary FDPs.

---

## Specification

### Step 1 — Revert workarounds in `cli.py`

In `reproto/src/reproto/cli.py`, inside the `Context(...)` constructor call in
`main()`:

- Restore `keep_variant_descriptor=keep_descriptor_path` (remove the
  `or build_schema_db is not None` clause added by `4c9f111`).
- Restore `write_variant_descriptor=emit_descriptor` (remove the
  `or build_schema_db is not None` clause added by `260f81d`).

### Step 2 — Add `schema_db_extra_nodes` to `Context`

In `reproto/src/reproto/context.py`, add a field:

```python
schema_db_extra_nodes: list['ReFileDescriptorProto'] = field(default_factory=list)
```

alongside the existing `schema_db_fdps` field.

### Step 3 — Sub-pass 3 in `_phase6_summoning`

At the end of `_phase6_summoning` in `reproto/src/reproto/phases.py`, add:

```python
# --- Sub-pass 3: DB dependency closure (only when --build-schema-db) ------
# Identify fallback/WKT files that are transitive dependencies of summoned
# files but not themselves summoned.  Collect them into
# ctx.schema_db_extra_nodes for later binary rendering inside
# _phase_build_schema_db.  Do NOT modify is_summoned or is_reachable here —
# phase 7 must run unmodified.
if ctx.build_schema_db:
    from .re_file import ReFileDescriptorProto as _ReFileFDP
    from .fake_types import Ref as _Ref

    seen: set[str] = {
        node.name for node in ctx.nodes.values()
        if isinstance(node, _ReFileFDP) and node.is_summoned
    }
    work: list[_ReFileFDP] = [
        node for node in ctx.nodes.values()
        if isinstance(node, _ReFileFDP) and node.is_summoned
    ]
    while work:
        file_node = work.pop()
        for dep_name in file_node.dependency:
            if dep_name in seen:
                continue
            seen.add(dep_name)
            fqdn = _ReFileFDP.fqdn_from_ref(_Ref(dep_name))
            dep_node = ctx.find_node(fqdn)
            if dep_node is None or not isinstance(dep_node, _ReFileFDP):
                continue
            if not dep_node.is_present() or dep_node.is_pruned:
                continue
            ctx.schema_db_extra_nodes.append(dep_node)
            work.append(dep_node)
```

### Step 4 — Promote and render extra nodes in `_phase_build_schema_db`

In `reproto/src/reproto/phases.py`, in `_phase_build_schema_db`, after the
phase 7 render loop has run (i.e. after `ctx.schema_db_fdps` is populated),
add a new section before the topological sort:

```python
# ── 3b. Render binary FDPs for WKT/fallback dependencies (spec 0080) ──────
#
# ctx.schema_db_extra_nodes was populated by phase 6 sub-pass 3.  These
# nodes are transitive dependencies of summoned files but were not rendered
# by phase 7 (they were not summoned).  Promote them now, render their
# binary FDPs, and append to ctx.schema_db_fdps so the DB is self-contained.
from .context import DescOut

def _summon_subtree(node: Node) -> None:
    """Promote node and all contains-descendants to summoned+reachable."""
    stack: list[Node] = [node]
    while stack:
        n = stack.pop()
        n.is_summoned = True
        n.is_reachable = True
        for child in n.contains:
            stack.append(child)

for extra_node in ctx.schema_db_extra_nodes:
    _summon_subtree(extra_node)
    from .syntax import fdp_syntax
    saved_force = ctx.force_proto2_for_editions
    if EDITIONS_COMPAT_REQUIRED and fdp_syntax(extra_node.this) == "editions":
        ctx.force_proto2_for_editions = True
    slot = DescOut()
    ctx.out_desc = slot
    try:
        extra_node.render(ctx)
    except (KeyError, ValueError, TypeError, AttributeError):
        pass
    finally:
        ctx.out_desc = None
        ctx.force_proto2_for_editions = saved_force
    if slot.out is not None:
        assert isinstance(slot.out, FileDescriptorProto)
        ctx.schema_db_fdps.append(slot.out)
```

### Step 5 — Update spec 0076

In `docs/specs/0076-render-pb-for-descriptor-desc.md`, add a note referencing
this spec and the revert of the workarounds from commits `260f81d` / `4c9f111`.

---

## Topological sort note

The existing Kahn's topological sort in `_phase_build_schema_db` [1] already
handles files in non-topological order.  After step 4, the WKT/fallback FDPs
are appended to `ctx.schema_db_fdps` after the user-summoned FDPs; the sort
processes them correctly without any changes.

[1] Kahn, A. B. (1962). "Topological sorting of large networks."
    *Communications of the ACM*, 5(11), 558–560.
    https://doi.org/10.1145/368996.369025

---

## Test considerations

- The googleapis schema DB build (`nix-build -A googleapis-db`) should pass
  without `--keep-descriptor-path` or `--emit-descriptor` being forced.
- `prototext --descriptor-set googleapis.desc decode ...` for types whose
  containing file imports `google/protobuf/timestamp.proto` (or any other WKT)
  should succeed after the change — this was the original failure mode.
- WKT types (e.g. `google.protobuf.Timestamp`) should appear as scored types
  in `list-schemas` output when the DB is queried.
- The existing test suite for reproto should continue to pass.
