<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0064 — Multi-FDP input support for reproto

**Status:** implemented
**Implemented in:** 2026-05-14
**App:** reproto

---

## Background

reproto currently requires that every input `.pb` / `.textpb` file contain
exactly one `FileDescriptorProto` (FDP) — either as a bare FDP or as a
single-entry `FileDescriptorSet` (FDS).  Multi-FDP descriptor sets (e.g.
those produced by `protoc --descriptor_set_out --include_imports`, or by
`buf build`) are silently rejected: `parse_qfile` in `load.py` checks
`len(fds.file) == 1` and falls through to failure if the count is higher.

This is a significant usability limitation: many real-world descriptor corpora
are distributed as multi-FDP FDS files, and users currently have to split them
manually before feeding them to reproto.

---

## Goals

1. reproto's disk loader (`load.py`) accepts multi-FDP FDS files in both
   binary (`.pb`, `.binpb`, `.pbset`, `.protoset`, `.desc`) and text
   (`.textpb`, `.pbtxt`, `.prototxt`, `.ascii_proto`) formats.
2. Each FDP extracted from a multi-FDP FDS is ingested individually — the rest
   of the pipeline (Phase 1 onward) continues to work with single-FDP units
   and requires no changes.
3. Name collisions across files (two FDPs with the same `fdp.name`) are handled
   by the existing first-wins deduplication already present in `ReFile.__new__`
   (no new collision logic required).
4. After `parse_qfile`, every `QualFile` carries a `FileDescriptorProto` in
   `qf.desc` (never a `FileDescriptorSet`) and a clean FDP fragment in
   `qf.contents` (binary bytes or bare text, already decapsulated).  Phase 2
   is updated to reflect these tighter invariants: `FileDescriptorSet`
   dispatch branches and `decapsulate()` calls are removed.

---

## Non-goals

- Changes to Phase 3 or any later phase.
- A new CLI flag or user-visible option.
- Detection or warning when two FDPs with the same name but different content
  arrive from separate files (first-wins is sufficient; a future spec may add
  a warning).
- Support for FDS files that contain FDPs listed out of topological order
  (dependency before dependant is not guaranteed by all producers; the existing
  topo-sort in Phase 2 already handles this).

---

## Design

### Principle: split at load time, FDP-only thereafter

The cleanest architectural fix is to confine multi-FDP awareness entirely to
the disk-loading layer.  `parse_qfile` splits a multi-FDP FDS into N
individual `QualFile` objects — one per FDP — and returns them all.
`load_from_path` accumulates the results.  Everything from `ReFile` onward
sees only single-FDP `QualFile` objects, preserving the existing invariant
that one `QualFile` = one proto file.

### Unified input pipeline: `reproto.split_fdps`

All format detection, proto type detection, `entry { }` decapsulation, and
FDP splitting is delegated to `split_fdps()` (spec 0065).  `parse_qfile` is
reduced to calling `split_fdps` and assembling `QualFile` objects from the
returned `(name, fragment)` pairs.

The existing `decapsulate()` function in `load.py` is removed — its
responsibility is now handled structurally by the tree-sitter scanner inside
`split_fdps`.  The `decapsulate()` import and call sites in `phases.py` are
likewise removed.

### `parse_qfile` changes

`parse_qfile` is simplified to:

```python
def parse_qfile(ctx: Context, file: QualFile) -> list[QualFile]:
    try:
        fragments = split_fdps(file.contents, file.rel_path.suffix)
    except ValueError:
        logger.warning("Cannot parse '%s'", file.rel_path, ...)
        return []
    result = []
    for name, fragment in fragments:
        qf = QualFile(file.root, file.rel_path, fragment)
        qf.name = name
        if isinstance(fragment, bytes):
            fdp = FileDescriptorProto()
            fdp.ParseFromString(fragment)
            qf.desc = fdp
        else:
            qf.desc = text_format.Parse(
                fragment, FileDescriptorProto(),
                allow_unknown_field=True, allow_unknown_extension=True,
            )
        result.append(qf)
    return result
```

`load_from_path` is updated to handle the list return (iterating and
accumulating all `QualFile` objects).

### `QualFile.desc` invariant after splitting

After splitting, each `QualFile` produced from a multi-FDP source has:

- `contents`: serialised bytes of the single FDP (binary case), or verbatim
  text of the `file { ... }` block interior (text case).
- `desc`: a `FileDescriptorProto` instance (never `FileDescriptorSet`).
- `name`: `fdp.name`.

Phase 2's `match qf.desc` dispatches to the `FileDescriptorProto` branch in
both cases.

### Name collision behaviour

When two FDPs from different source files share the same `fdp.name`, the
existing `ReFile.__new__` first-wins deduplication applies unchanged:
the second `QualFile` is silently discarded.  No new collision handling is
introduced.

---

## Files changed

| File | Change |
|---|---|
| `reproto/src/reproto/load.py` | Remove `decapsulate()`; simplify `parse_qfile` to delegate to `split_fdps`; extend return type to `list[QualFile]`; update `load_from_path` |
| `reproto/src/reproto/phases.py` | Remove `decapsulate` import and all call sites; remove `FileDescriptorSet` dispatch branches from Phase 2 and Phase 3 |
| `reproto/src/reproto/split_fdps.py` | New module (see spec 0065) |

---

## Test plan

Unit tests for `split_fdps` are covered by spec 0065.  The following
integration test cases live in `reproto/src/reproto/tests/test_phases.py`.

A `compile_proto_multi` helper is added to `conftest.py`:

```python
def compile_proto_multi(out_path: Path, *proto_names: str) -> Path:
    """Compile multiple .proto files into a single multi-FDP FDS .pb
    (protoc --include_imports).  Returns the .pb path."""
```

### TC-M1 — multi-FDP binary FDS round-trips correctly

Build a two-file FDS with `protoc --include_imports` from two protos that
have a dependency between them (e.g. `child.proto` imports `parent.proto`).
Run reproto on the single multi-FDP `.pb`; run it again on the two
individual single-FDP `.pb` files.  Assert the output directories are
identical.

### TC-M2 — multi-FDP text FDS round-trips correctly

Same as TC-M1 but with a `.textpb` multi-FDP FDS serialised via
`text_format.MessageToString(fds)`.  Verify reproto produces the same
output as the two individual `.pb` files.

### TC-M3 — single-FDP binary input still works (regression)

Feed one single-FDP `.pb` to reproto and verify it processes without error.
Guards against regressions introduced by the `parse_qfile` refactor.

### TC-M4 — FDP name collision across two FDS files (first-wins)

Build two multi-FDP FDS files that share one FDP name (same `fdp.name`,
different content).  Feed both to reproto; verify it exits cleanly and
processes exactly one copy of the overlapping FDP.
