<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0037 — reproto: Round 6 — behavioural correctness items

**Status:** implemented
**Implemented in:** 2026-05-03
**App:** reproto

---

## Purpose

Round 6 of spec 0031.  Address the four behavioural correctness items that
were deferred because they may change observable output or require new tests.

---

## Pre-implementation audit

Before writing code, each item from 0031 §Round 6 was investigated against
the current codebase to establish its actual status.

### Item 1 — `filter_out: list[str] = []` mutable default

0031 points to six occurrences in `option_renderers.py`.  Audit shows that
`filter_out` no longer exists anywhere in the codebase — it was eliminated
during the Round 3 options-rendering refactor (spec 0032).  **Already done;
no action required.**

### Item 2 — `HasField` property on `NodeBase`

`base.py` exposes:

```python
@property
def HasField(self) -> Callable[..., bool]:
    return self.this.HasField
```

Audit of all call sites shows that **no caller uses `node.HasField(...)`
through a `NodeBase` instance**.  Every call site uses `self.this.HasField(...)`
directly on the underlying protobuf message, or uses `HasField` on a raw
`FieldDescriptorProto` / options message.  The property is dead code.

Action: remove the property.

### Item 3 — `is_ref()` implemented via `hasattr`

`topology.py:File.is_ref()` uses `not hasattr(self, '_qfile')` as a sentinel.
`_qfile` is declared as a class-level annotation (`self._qfile: QualFile`) but
is only assigned in `ReFile.__init__` when a `QualFile` is provided.  Until
then the attribute is absent, so `hasattr` returns `False`.

Action: initialise `_qfile` to `None` in `File.__init__`; change the type to
`QualFile | None`; test with `self._qfile is None`.  Update all code paths
that assign or read `_qfile` accordingly.

Note: `phases.py` also contains `hasattr(file, 'qfile')` (line 702) — this
must be updated to `file._qfile is not None` at the same time.

### Item 4 — `shorten_type_name` `do_more=False` edge case (REVIEW §6.6)

The REVIEW describes a potential bug: when `do_more=False` is returned by a
recursive `shorten()` call, an outer `ReDescriptorProto` branch might still
strip its prefix.

Current code:

```python
case ReDescriptorProto() | ReServiceDescriptorProto():
    do_more, name = shorten(name, proto.parent)
    container_name = proto.name
    if do_more and name.startswith(f'.{container_name}.'):
        return True, name[len(container_name) + 1:]
    else:
        return False, name
```

Careful analysis: if `do_more=False` from the recursive call, the condition
`if do_more and ...` is `False` and the prefix is **not** stripped.  The
propagation is therefore correct for the `do_more=False` case.

However, the `else` branch also returns `(False, name)` when `do_more=True`
but the container prefix doesn't match.  This is intentional: if the container
name doesn't appear in the remaining type path, no further outer container can
safely strip its own prefix either (the shortening must be contiguous from the
root).

Conclusion: **no bug exists**.  The edge case documented in REVIEW §6.6 is
handled correctly by the current code.  No code change is required, but the
inline documentation should be improved so future readers do not re-investigate.

Action: add a clarifying comment to the `else` branch of `shorten`.

---

## Goals

1. Remove the dead `NodeBase.HasField` property (`base.py`).
2. Fix `is_ref()` in `topology.py`: initialise `_qfile: QualFile | None = None`;
   use `self._qfile is None` instead of `hasattr`.
3. Fix the `hasattr(file, 'qfile')` guard in `phases.py:702` to match.
4. Add a clarifying comment to `shorten_type_name` explaining why the `else`
   branch returns `(False, name)` and not `(do_more, name)`.
5. All 61 existing tests continue to pass.

---

## Non-goals

- Changing any logic or observable output (items 1–3 are refactors only).
- Adding new tests (the existing roundtrip suite already covers name resolution).
- Addressing any Round 3, 4, or 5 items.

---

## Specification

### Item A — Remove `NodeBase.HasField` property

Delete the property from `base.py`:

```python
@property
def HasField(self) -> Callable[..., bool]:
    return self.this.HasField
```

One call site — `re_descriptor.py` line 393 — used `fd.HasField("oneof_index")`
where `fd` is a `ReFieldDescriptorProto`.  The raw `FieldDescriptorProto`
(`field_proto`) is already in scope at that point, so change the call to
`field_proto.HasField("oneof_index")`.  After this fix, no caller reaches
`NodeBase.HasField` through a `NodeBase` instance, and the property is dead.

Remove `Callable` from the `collections.abc` import in `base.py` as it becomes
unused.

### Item B — Fix `is_ref()` sentinel in `topology.py`

In `File.__init__`:
- Change the annotation `self._qfile: QualFile` to an assignment
  `self._qfile: QualFile | None = None`.

In `File.is_ref()`:
- Change `return not hasattr(self, '_qfile')` to `return self._qfile is None`.

In `File.qfile` setter:
- No change needed — it still assigns a `QualFile` value.

In `File.qfile` getter:
- The return type is `QualFile`; callers only call it when `is_ref()` is
  `False`, so `self._qfile` is never `None` at that point.  Add an assertion
  to make this invariant explicit:
  ```python
  assert self._qfile is not None
  return self._qfile
  ```

### Item C — Fix `hasattr(file, 'qfile')` in `phases.py`

Line 702 of `phases.py`:

```python
if hasattr(file, 'qfile') and file.is_seed:
```

Change to:

```python
if not file.is_ref() and file.is_seed:
```

This is equivalent but uses the proper API.

### Item D — Clarify `shorten_type_name` comment

In `utils.py`, inside the `else` branch of `shorten()`:

```python
else:
    return False, name
```

Add a comment:

```python
else:
    # Return do_more=False: shortening must be contiguous from the package
    # root, so a non-matching level stops all further stripping by callers.
    return False, name
```

---

## Files changed

| File | Change |
|---|---|
| `base.py` | Remove `HasField` property; remove `Callable` import |
| `re_descriptor.py` | `fd.HasField(...)` → `field_proto.HasField(...)` at line 393 |
| `topology.py` | `_qfile: QualFile | None = None`; `is_ref()` uses `is None` |
| `phases.py` | Replace `hasattr(file, 'qfile')` with `not file.is_ref()` |
| `utils.py` | Add clarifying comment to `shorten()` else branch |

---

## Implementation order

1. `base.py` — remove dead property.
2. `topology.py` — fix `_qfile` initialisation and `is_ref()`.
3. `phases.py` — fix `hasattr` guard.
4. `utils.py` — add comment.
5. Run `pytest reproto/src/reproto/tests/ -x -q`.
6. Run `pyright` over all changed files.
