<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0032 — reproto: eliminate the `Re*Options` class family

**Status:** implemented 2026-05-03
**App:** reproto

---

## Purpose

The seven per-type options-decorator classes
(`ReEnumOptions`, `ReMessageOptions`, `ReServiceOptions`,
`ReMethodOptions`, `ReFieldOptions`, `ReFileOptions` in
`option_renderers.py`, and `ReExtensionRangeOptions` in
`simple_types.py`) were a first-generation rendering approach.
A second-generation approach (`ReOptions` + `ReExtensions` in
`option_renderers.py`, and `render_options` / `render_options_from_message`
on `NodeBase` in `base.py`) has since replaced them at every call site
except one.

This spec removes the dead code, fixes the surviving call site, and
eliminates the mutable-default-argument bug that accompanies every class.

---

## Goals

1. Delete the six fully-dead classes: `ReEnumOptions`, `ReMessageOptions`,
   `ReServiceOptions`, `ReMethodOptions`, `ReFieldOptions`, `ReFileOptions`.
2. Replace the one live call site of `ReExtensionRangeOptions` with
   `render_options_from_message`, then delete `ReExtensionRangeOptions`.
3. Fix the `filter_out: list[str] = []` mutable-default-argument bug that
   appeared in every class (7 occurrences total; only 1 survives to be
   fixed).
4. Remove the re-exports of the deleted classes from `re_simple.py`
   (`__all__` and import list).
5. Remove now-unused imports from `option_renderers.py` (the six
   protobuf `*Options` imports, `Message`, `Callable`, `Sequence`).
6. All 61 existing tests continue to pass without modification.

---

## Non-goals

- Changing `render_options`, `render_options_from_message`, `ReOptions`,
  or `ReExtensions` (they are correct and stay as-is).
- Modifying any test fixtures or test logic.
- Changing the public API (`__all__` retains `ReOptions`, `ReExtensions`,
  `ReFieldDescriptor`, `ReMessage`, `ReReservedRange`, `ReExtensionRange`).
- Touching Round 4–6 items from spec 0031.

---

## Current state audit

### Dead classes (no runtime instantiation)

| Class | File | Instantiation sites |
|---|---|---|
| `ReEnumOptions` | `option_renderers.py:46` | none |
| `ReMessageOptions` | `option_renderers.py:86` | none |
| `ReServiceOptions` | `option_renderers.py:126` | none |
| `ReMethodOptions` | `option_renderers.py:166` | none |
| `ReFieldOptions` | `option_renderers.py:206` | none |
| `ReFileOptions` | `option_renderers.py:248` | none |

All six are re-exported from `re_simple.py` and listed in its `__all__`.
They are used nowhere in the runtime call graph; the callers all went
through `render_options` / `render_options_from_message` as of Round 1.

### Live class (one call site)

`ReExtensionRangeOptions` (`simple_types.py:415`) is instantiated once:

```python
# simple_types.py:372–374
options = ReExtensionRangeOptions(self.options)
texts = options.render(ctx, depth, ['declaration'])
```

Its `render` method:

1. Iterates `self.ListFields()` (built-in options only — no extensions).
2. Calls `option.dump_option(ctx, value, depth+1)`.
3. Marks orphan lines with `line.type = ORPHAN`.
4. Appends commas to all non-last, non-commented texts.
5. Returns `list[Block]` (not `list[Block]` with `option …;` wrapping —
   the caller handles the bracket/semicolon formatting itself).

The `filter_out` parameter receives `['declaration']` to suppress the
`declaration` field, which is a repeated field.

`render_options_from_message` already handles all of steps 1–4 with
`composite=True` and `exclude={"declaration"}`. The caller's existing
bracket/semicolon formatting logic in `ReExtensionRange.render` is
compatible with the `list[Block]` return value of
`render_options_from_message`.

However, `ReExtensionRangeOptions` does not skip extension options,
while `render_options_from_message` does iterate extensions.
`ExtensionRangeOptions` has no extensions defined in the standard
protobuf library, so this difference is inert in practice.

One additional difference: `ReExtensionRangeOptions.render` does not
prepend `option ` or append `;` — it returns raw `name = value` blocks
for the caller to assemble into brackets. `render_options_from_message`
with `composite=True` also does not prepend `option ` or append `;`
(it appends `,` instead). The caller already strips trailing commas
from the last entry via its loop at `simple_types.py:446–452`.

**Conclusion:** the replacement is safe.

### Context descriptors for ExtensionRangeOptions

`Context` does not expose `exo_desc` / `exo_cls` for
`ExtensionRangeOptions`, and **none need to be added**.

Unlike the other descriptor types, `ReExtensionRange` is a plain helper
class (not a `NodeBase`) that already holds a direct reference to the
live `ExtensionRangeOptions` message in `self.options`. There is no
pool lookup or re-parsing step needed.

`render_options_from_message` needs an `options_descriptor` only to
call `ctx.pool.FindAllExtensions(options_descriptor)`. That descriptor
is always available directly from the live message: `self.options.DESCRIPTOR`.
Passing it inline costs nothing and requires no new context attribute.

### Mutable default argument bug

`filter_out: list[str] = []` appears in 6 classes in `option_renderers.py`
and once in `simple_types.py:430`. After deletion, zero occurrences
remain (the surviving call uses `exclude: set[str] | None = None` which
is already correct in `render_options_from_message`).

---

## Specification

### Step 1 — Replace the `ReExtensionRangeOptions` call site

In `simple_types.py`, replace:

```python
options = ReExtensionRangeOptions(self.options)
texts = options.render(ctx, depth, ['declaration'])
```

with:

```python
from .re_simple import ReExtensionRange as _sentinel  # noqa: F401
```

No — more precisely, replace it with a call to
`render_options_from_message` on any available `NodeBase` instance.
`ReExtensionRange` is not a `NodeBase`; it is a plain helper class.

The cleanest approach is to promote the rendering to a module-level
function in `simple_types.py`:

```python
def _render_extension_range_options(
    ctx: Context,
    options_msg: Message,
    depth: int,
    exclude: set[str],
) -> list[Block]:
    """Render ExtensionRangeOptions fields for bracket assembly."""
    from .simple_types_helpers import _do_render  # NOT this approach
```

Actually the simplest correct approach: import
`render_options_from_message` from `base.py` as a module-level
function. But it is currently an instance method on `NodeBase`.

**Resolution:** extract `render_options_from_message` logic into a
standalone module-level function `_render_options_from_message` in
`base.py` and have the `NodeBase.render_options_from_message` method
delegate to it. Then call it directly from `simple_types.py`.

Alternatively — and more simply — factor the call site inline:

```python
# simple_types.py  (inside ReExtensionRange.render)
from .base import NodeBase
from .simple_types import ReFieldDescriptor  # already imported

texts: list[Block] = []
for fd_desc, val in self.options.ListFields():
    if fd_desc.is_extension:
        continue
    if fd_desc.name in ('declaration',):
        continue
    opt = ReFieldDescriptor(fd_desc)
    block, is_orp = opt.dump_option(ctx, val, depth + 1)
    if not block:
        continue
    block.postpend(',')
    block.set_type(ORPHAN if is_orp else CODE)
    texts.append(block)
```

This is 12 lines and avoids a dependency on `NodeBase` from
`simple_types.py`. However it duplicates rendering logic.

**Chosen approach:** promote `render_options_from_message` to a
standalone function `render_options_from_message` at module level in
`base.py` (keeping the `NodeBase` method as a one-line delegate).
Then import and call it from `simple_types.py`.

The standalone function signature:

```python
def render_options_from_message(
    ctx: Context,
    opts_msg: Message,
    options_descriptor: Descriptor,
    composite: bool = False,
    depth: int = 0,
    exclude: set[str] | None = None,
) -> list[Block]:
```

The `NodeBase` method becomes:

```python
def render_options_from_message(self, ctx, opts_msg, options_descriptor,
                                composite=False, depth=0, exclude=None):
    return render_options_from_message(
        ctx, opts_msg, options_descriptor, composite, depth, exclude)
```

In `simple_types.py`, the replacement is:

```python
from .base import render_options_from_message as _rom
texts = _rom(
    ctx=ctx,
    opts_msg=self.options,
    options_descriptor=self.options.DESCRIPTOR,
    composite=True,
    depth=depth,
    exclude={'declaration'},
)
```

No new context attribute is needed: `self.options.DESCRIPTOR` is the
`ExtensionRangeOptions` descriptor, obtained directly from the live
message object.

The existing bracket-assembly code in `ReExtensionRange.render`
(lines 375–408) uses `texts` in a pattern that expects
`list[Block]` where each block is a list of `BlockLine`.
`render_options_from_message` returns `list[Block]`.

There is one semantic difference: `render_options_from_message` with
`composite=True` appends `,` to each block, while the old code did
not append commas (the caller's loop added them to all-but-last).
The existing caller loop at lines 375–408 does:

```python
for text in reversed(texts):
    if is_last:
        if text and not text[0].text.startswith('//'):
            is_last = False
    elif text and not text[0].text.startswith('//'):
        text[-1].text += ','
```

With `composite=True`, every block already ends in `,`, so the caller
loop would add a second comma to all-but-last. Therefore either:

a. Call `render_options_from_message` with `composite=False` and adapt
   the bracket-assembly code to add commas itself, or
b. Call with `composite=True` and remove the caller's comma-adding loop,
   and strip the trailing `,` from the last non-orphan block.

**Chosen approach (b):** call with `composite=True`.
The caller's comma-adding loop (lines 446–452) is replaced by stripping
the trailing `,` from the last non-commented, non-orphan block (matching
what `format_composite_options` does). The bracket-assembly logic for
the 0/1/many cases is retained unchanged.

### Step 3 — Delete `ReExtensionRangeOptions`

Delete the class `ReExtensionRangeOptions` from `simple_types.py`
(lines 415–453).

Remove the import of `ReExtensionRangeOptions` from the call site
(now replaced in Step 2).

### Step 4 — Delete the six dead `Re*Options` classes

Delete from `option_renderers.py`:
- `ReEnumOptions` (lines 46–81)
- `ReMessageOptions` (lines 86–121)
- `ReServiceOptions` (lines 126–161)
- `ReMethodOptions` (lines 166–201)
- `ReFieldOptions` (lines 206–243)
- `ReFileOptions` (lines 248–275)

Remove the now-unused imports at the top of `option_renderers.py`:

```python
from google.protobuf.descriptor_pb2 import (
    EnumOptions,
    FieldOptions,
    FileOptions,
    MessageOptions,
    MethodOptions,
    ServiceOptions,
)
from google.protobuf.message import Message
from collections.abc import Callable, Sequence
```

All three import groups become unused. `logging` and
`FieldDescriptor` remain (used by `ReOptions` and `ReExtensions`).
`Any` remains (used by `ReOptions` and `ReExtensions`).

### Step 5 — Update `re_simple.py`

Remove from the import list and `__all__`:

```
ReEnumOptions, ReFieldOptions, ReFileOptions,
ReMessageOptions, ReMethodOptions, ReServiceOptions
```

`ReOptions`, `ReExtensions`, and `ReExtensionRange` remain in both.
`ReExtensionRangeOptions` was never exported from `re_simple.py`
(it was in `simple_types.py` directly) — confirm and leave as-is.

### Step 6 — Verify and run tests

Run `pytest tests/reproto/` and confirm all 61 tests pass.
Run `pyright` over the changed files and resolve any type errors.

---

## Files changed

| File | Change |
|---|---|
| `base.py` | Promote `render_options_from_message` body to module-level function; `NodeBase` method delegates to it |
| `option_renderers.py` | Delete 6 classes, remove unused imports |
| `simple_types.py` | Replace `ReExtensionRangeOptions` call site; delete class; adapt bracket-assembly loop |
| `re_simple.py` | Remove 6 names from import and `__all__` |

---

## Implementation order

1. `base.py` (promotes function — needed by `simple_types.py`)
2. `simple_types.py` (replace call site; delete class)
3. `option_renderers.py` (delete classes; remove imports)
4. `re_simple.py` (update re-exports)
5. Run tests + pyright
