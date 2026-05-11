<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0051 — reproto: fix fallback loading breaking topo-sort object identity

**Status:** implemented
**Implemented in:** 2026-05-11
**App:** reproto

---

## Background

When `--use-variant all` is used, reproto loads embedded fallback files for
well-known types (`google/protobuf/any.proto`, `duration.proto`, etc.) in
addition to `descriptor.proto`.  The intent is that these authoritative
embedded copies replace any version found in the input files.

`descriptor.proto` works correctly.  The well-known types (`any.proto` etc.)
triggered a spurious W5 warning and their content was silently dropped from the
descriptor pool, causing render failures for any file that imports them.

---

## Root cause

During phase 1, the import-discovery loop processes each seed file's
`fdp.dependency` list.  For every named import it calls
`ReFile(topo, dep_name)`, which creates a **ref** `ReFile` instance (no
`qfile` yet) and stores it both in `topo.new_files` and in the parent file's
`targets` set.  Call this instance `X`.

Later, the fallback-loading loop (lines 379–391 of `phases.py`) does:

```python
topo.files.pop(fallback_proto, None)   # removes X from topo.files
load_embedded_proto_fallback(...)      # creates a NEW instance Y
```

`topo.files.pop` removes `X`.  `load_embedded_proto_fallback` calls
`ReFile(topo, qual_file)`, which finds no existing entry via
`topo.find_file(name)` and therefore allocates a **new** instance `Y`.

The importing file's `targets` set still holds `X`.  `topo.files` now holds
`Y`.  They are different Python objects.

In phase 2, the topological-sort loop builds:

```python
files = {file for file in topo.files.values()}   # contains Y, not X
```

The leaf check `all(t not in files for t in n.targets)` tests object
identity.  `X not in files` is True (only `Y` is present), so the importing
file (`status.proto`) is classified as a leaf — at the same rank as `any.proto`
itself.  Within a rank, merge order is arbitrary; `status.proto` is merged
into `pool_db` before `any.proto`, which fails with `TypeError` (silently
caught).  `status.proto` is never registered in `pool_db`, and rendering it
later triggers a W5 warning.

`descriptor.proto` is never listed in any `fdp.dependency`, so no ref `X` is
ever created for it.  The pop is a no-op, and `load_embedded_proto_fallback`
correctly creates a fresh instance with no identity conflict.

---

## Goals

1. Fix the fallback-loading loop so that object identity is preserved: the
   `ReFile` instance held in an importing file's `targets` set is the same
   object that ends up in `topo.files` after the fallback is loaded.

2. Ensure the fallback content always wins (replacement semantics), covering
   all three cases:
   - No prior entry (`descriptor.proto` case) — unchanged behaviour.
   - Ref-only prior entry (`any.proto` imported but not found on disk) —
     finalize the existing ref in-place.
   - Full prior `ReFile` (`any.proto` found on disk before the fallback loop
     runs) — overwrite `qfile` on the existing instance in-place so the
     embedded fallback takes precedence.  This case can occur when the user
     has the well-known type on their `-I` path.

3. No spurious W1/W5 warning when `--use-variant all` is used with input files
   that import well-known types not present on the `-I` path.

---

## Non-goals

- Changing the semantics of any other phase.
- Altering behaviour when `--use-variant descriptor` is used (no regression).

---

## Specification

### §1 — Remove the `topo.files.pop` call

Delete `topo.files.pop(fallback_proto, None)` from the fallback-loading loop
in `_phase1_load_files`.

Without the pop, `ReFile.__new__` finds the existing instance (ref or full)
via `topo.find_file` and returns it.  `ReFile.__init__` finalizes it in-place
if it was a ref.  All `targets` references held by importing files continue to
point to the same object, which is now correctly present in `topo.files`.  The
topo-sort leaf check (`all(t not in files for t in n.targets)`) therefore
correctly classifies importing files as non-leaves.

### §2 — Force-overwrite `qfile` after loading

Have `load_embedded_proto_fallback` return the `QualFile` it built (or `None`
on failure).  After `topo.merge_files()`, unconditionally assign that
`QualFile` to the merged `ReFile`:

```python
fallback_qfile = load_embedded_proto_fallback(fallback_proto)
topo.merge_files()
if fallback_qfile is not None:
    topo.files[fallback_proto].qfile = fallback_qfile
```

This handles the case where a full `ReFile` was already loaded from disk
before the fallback loop ran: `ReFile.__init__` would have returned early
without overwriting (`if not self.is_ref(): return`), so the force-assign
is the only way to ensure the embedded fallback always wins.

### §3 — W1 suppression via `register_fallback_file`

Add a `register_fallback_file(name)` method to `WarningCollector` and a
corresponding `_fallback_files: set[str]` field.  `w1()` checks this set
and returns early (no warning, no counter increment) when the missing file
will be provided by an embedded fallback.

At the start of `_phase1_load_files`, before the import-discovery loop,
call `get_collector().register_fallback_file(fp)` for each name in
`ctx.fallback_protos`.

This is intentionally separate from `_pruned_files` (which suppresses W5 at
render time): a fallback file is not pruned — it is actively loaded and
emitted.  Conflating the two concepts would suppress render-time W5 warnings
for fallback files, masking genuine problems.

### §4 — Regression test

Add `test_roundtrip_use_variant_all_wkt` to `test_roundtrip.py`.  The test:
1. Compiles `well_known_types.proto` to a mono-fdp `.pb` (without
   `--include_imports`), so `any.proto`, `empty.proto`, `timestamp.proto`,
   and `duration.proto` are absent from the descriptor set.
2. Runs `reproto --use-variant descriptor --use-variant all` on that `.pb`.
3. Asserts exit code 0 and no `missing dependency file` in stderr.
4. Runs the full roundtrip check (recompile output and compare descriptors).

---

## Open questions

None.
