<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0035 — reproto: Round 4 cleanup — oneof and extend deduplication

**Status:** implemented 2026-05-03
**App:** reproto

---

## Purpose

Round 4 of spec 0031. Two items:

1. Delete the dead `render_oneofs` method from `re_descriptor.py`.
2. Deduplicate the extend-block grouping loop that exists in both
   `re_file.py:render` and `re_descriptor.py:render_extensions`.

Both items are listed in spec 0031 §Round 4.

---

## Goals

1. Delete `ReDescriptorProto.render_oneofs` (lines 135–185 of
   `re_descriptor.py`). It is dead code: `render` never calls it; it
   contains its own inline oneof loop.
2. Extract the extend-block grouping logic into a shared helper
   `_render_extend_blocks` and call it from both sites.
3. All 61 existing tests continue to pass without modification.

---

## Non-goals

- Changing observable output in any way.
- Altering orphan-tracking semantics.
- Touching Round 5 or 6 items.
- Changing `re_file.py:render_file_options` (it uses `ReOptions` +
  `ReExtensions` directly; that is a Round 5/6 concern).

---

## Current state audit

### Item 1 — Dead `render_oneofs`

`ReDescriptorProto.render_oneofs` (`re_descriptor.py:135–185`) contains
a full oneof rendering loop. It is **never called** anywhere:

```
$ grep -r render_oneofs reproto/src/
# no output
```

`ReDescriptorProto.render` (lines 498–548) contains an identical but
slightly extended inline loop that also handles:
- Synthetic oneof filtering (spec 0019) — absent from `render_oneofs`.
- Oneof features blocks (editions) — absent from `render_oneofs`.
- Correct `depth+2` for field rendering inside the oneof block —
  `render_oneofs` uses `depth+1`.

`render_oneofs` is therefore strictly dead code. Deleting it is safe.

### Item 2 — Duplicated extend-block grouping

The extend-block grouping pattern appears twice:

**Site A — `re_file.py:render`, lines 388–426**

```python
extendee_short_names: list[str] = []
for e in self.extension:
    extension_proto = cast(FieldDescriptorProto, e)
    if not allow_extend_block(ctx, extension_proto.extendee):
        out.append(report("A5", depth, ...))
        continue
    fd = ReFieldDescriptorProto(ctx, extension_proto, parent=self)
    ref = short_ref(ctx, Fqdn(f'message:{fd.extendee}'), self)
    if ref not in extendee_short_names:
        extendee_short_names.append(ref)

for ref in extendee_short_names:
    block = Block()
    is_orphan = True
    for e in self.extension:
        extension_proto = cast(FieldDescriptorProto, e)
        if not allow_extend_block(ctx, extension_proto.extendee):
            continue
        fd = ReFieldDescriptorProto(ctx, extension_proto, parent=self)
        ref2 = short_ref(ctx, Fqdn(f'message:{fd.extendee}'), self)
        if ref2 != ref:
            continue
        blk = fd.render(ctx, depth+1)
        if fd.is_summoned:
            is_orphan = False
        else:
            blk.abandon()
        block.extend(blk)
    block.insert(0, BlockLine(f'extend {ref} {{', depth,
                              type=ORPHAN if is_orphan else CODE))
    block.append(BlockLine('}', level=depth,
                           type=ORPHAN if is_orphan else CODE))
    out.extend(block)
```

**Site B — `re_descriptor.py:render_extensions`, lines 86–133**

```python
extendee_short_names: list[str] = []
for e in self.extension:
    extension_proto = cast(FieldDescriptorProto, e)
    if not allow_extend_block(ctx, extension_proto.extendee):
        out.append(report("B1", depth, ...))
        continue
    fd = ReFieldDescriptorProto(ctx, extension_proto, parent=self)
    short_name = fd.short_type_name(ctx, fd.extendee)
    if short_name not in extendee_short_names:
        extendee_short_names.append(short_name)

for short_name in extendee_short_names:
    out.append(BlockLine(f'extend {short_name} {{', depth))
    for e in self.extension:
        extension_proto = cast(FieldDescriptorProto, e)
        if not allow_extend_block(ctx, extension_proto.extendee):
            continue
        fd = ReFieldDescriptorProto(ctx, extension_proto, parent=self)
        name = fd.short_type_name(ctx, fd.extendee)
        if name != short_name:
            continue
        from .syntax import field_label
        ext_features = _resolve_field_features(...)
        lbl = field_label(...)
        out.append(BlockLine(f'{lbl}{fd.short_type_name(ctx)} '
                        f'{fd.name} = {fd.number};', depth+1))
    out.append(BlockLine('}', level=depth))
```

#### Key differences between the two sites

| Aspect | Site A (file) | Site B (message) |
|---|---|---|
| Anomaly code | `A5` | `B1` |
| Type name resolution | `short_ref(ctx, Fqdn(...), self)` | `fd.short_type_name(ctx, fd.extendee)` |
| Field rendering | `fd.render(ctx, depth+1)` | Inline label+type+name+number string |
| Orphan tracking | Yes — `is_orphan`, `blk.abandon()` | No — no orphan tracking |
| `extend {` line type | `ORPHAN if is_orphan else CODE` | Always `CODE` (no type kwarg) |
| `}` line type | `ORPHAN if is_orphan else CODE` | Always `CODE` |
| Features handling | Via `fd.render()` | Inline via `_resolve_field_features` + `field_label` |

The two sites differ in three semantically meaningful ways:

1. **Orphan tracking**: Site A checks `fd.is_summoned` and calls
   `blk.abandon()` on unsummoned fields. Site B never tracks orphans —
   the `extend {` and `}` lines have no `type=` argument and default to
   `CODE`. This is a pre-existing discrepancy, but it is correct: in the
   message case, `render_extensions` is only called when the message
   itself is reachable, so all its extension fields are always visible.
   In the file case, a file-level extension can be orphaned even when the
   file is rendered.

2. **Type name shortening**: Site A uses `short_ref(ctx, Fqdn(...),
   self)` (the full scope-aware algorithm from `utils.py`). Site B uses
   `fd.short_type_name(ctx, fd.extendee)` (the older
   `shorten_type_name`-based path). Both produce correct results in their
   respective scopes; the implementations differ only in how they
   determine the shortest safe name.

3. **Field rendering**: Site A calls `fd.render(ctx, depth+1)` which
   produces a full field line including label, type, name, number, and
   options. Site B manually builds `f'{lbl}{type} {name} = {number};'`
   with no options — a regression risk if extension fields ever carry
   options, but correct for the current test suite.

#### Deduplication strategy

The two sites are close enough to share a helper, but the differences
above mean the helper needs parameters to control orphan tracking,
anomaly code, and type-name resolution. The cleanest factoring is a
module-level function in `re_descriptor.py` (where Site B already lives)
that accepts enough parameters to cover both call sites.

**Proposed signature:**

```python
def _render_extend_blocks(
    ctx: Context,
    owner: 'ReFileDescriptorProto | ReDescriptorProto',
    extensions: RepeatedCompositeFieldContainer,
    depth: int,
    anomaly_code: str,
    track_orphans: bool,
) -> Block:
```

`owner` is the message or file node that owns the extensions. It is
passed to `short_ref` (for file scope) or used as the `parent=` for
`ReFieldDescriptorProto`.

**Unified field rendering**: Use `fd.render(ctx, depth+1)` at both
sites — this replaces Site B's manual string build. This is a pure
improvement: it brings extension-field options rendering to the message
extend block (previously absent), and the output is identical for the
current test suite where no extension fields carry options.

**Unified type-name resolution**: Use `short_ref(ctx,
Fqdn(f'message:{fd.extendee}'), owner)` at both sites, replacing Site
B's `fd.short_type_name(ctx, fd.extendee)`. `short_ref` is already
imported in `re_file.py` and can be imported from `utils.py` in
`re_descriptor.py`.

**Orphan tracking**: Controlled by the `track_orphans` parameter.
When `True` (file site): track `is_orphan` and call `blk.abandon()`,
set `type=ORPHAN if is_orphan else CODE` on bracket lines.
When `False` (message site): emit bracket lines without `type=` kwarg
(i.e. default `CODE`) and never call `blk.abandon()`.

---

## Specification

### Step 1 — Delete `render_oneofs`

In `re_descriptor.py`, delete the method `render_oneofs` (lines
135–185). It is never called and is superseded by the inline oneof loop
in `render`.

No other file changes are needed for this step.

### Step 2 — Extract `_render_extend_blocks`

Add the following module-level function to `re_descriptor.py`, above
`render_extensions`:

```python
def _render_extend_blocks(
    ctx: Context,
    owner: 'ReFileDescriptorProto | ReDescriptorProto',
    extensions: RepeatedCompositeFieldContainer,
    depth: int,
    anomaly_code: str,
    track_orphans: bool,
) -> Block:
    """Render grouped extend blocks for a file or message owner.

    Iterates extensions twice: first to collect distinct extendee short
    names (preserving encounter order), then to emit one extend { }
    block per extendee.

    Args:
        ctx: Rendering context.
        owner: The file or message node that owns the extension fields.
               Used as scope for short_ref and as parent= for field nodes.
        extensions: The repeated extension field descriptors to render.
        depth: Indentation depth for the extend { } brackets.
        anomaly_code: Anomaly code to report for illegal extend blocks
                      (e.g. "A5" for files, "B1" for messages).
        track_orphans: When True, check fd.is_summoned and mark
                       unsummoned fields and brackets as ORPHAN.
                       When False, emit all lines as CODE.

    Returns:
        Block containing the rendered extend statements.
    """
    from .re_field import ReFieldDescriptorProto
    from .syntax import allow_extend_block
    from .utils import short_ref

    out = Block()

    # Pass 1: collect distinct extendee short names in encounter order,
    # reporting anomalies for illegal extendees.
    extendee_refs: list[str] = []
    for e in extensions:
        extension_proto = cast(FieldDescriptorProto, e)
        if not allow_extend_block(ctx, extension_proto.extendee):
            from .anomalies import report
            out.append(report(anomaly_code, depth,
                              msg=getattr(owner, 'name', ''),
                              file=getattr(owner, 'name', ''),
                              extendee=extension_proto.extendee))
            continue
        fd = ReFieldDescriptorProto(ctx, extension_proto, parent=owner)
        ref = str(short_ref(ctx, Fqdn(f'message:{fd.extendee}'), owner))
        if ref not in extendee_refs:
            extendee_refs.append(ref)

    # Pass 2: emit one extend { } block per extendee.
    for ref in extendee_refs:
        block = Block()
        is_orphan = True
        for e in extensions:
            extension_proto = cast(FieldDescriptorProto, e)
            if not allow_extend_block(ctx, extension_proto.extendee):
                continue
            fd = ReFieldDescriptorProto(ctx, extension_proto, parent=owner)
            if str(short_ref(ctx, Fqdn(f'message:{fd.extendee}'), owner)) != ref:
                continue
            blk = fd.render(ctx, depth + 1)
            if track_orphans:
                if fd.is_summoned:
                    is_orphan = False
                else:
                    blk.abandon()
            else:
                is_orphan = False
            block.extend(blk)
        bracket_type = ORPHAN if (track_orphans and is_orphan) else CODE
        block.insert(0, BlockLine(f'extend {ref} {{', depth,
                                  type=bracket_type))
        block.append(BlockLine('}', level=depth, type=bracket_type))
        out.extend(block)

    return out
```

### Step 3 — Replace `render_extensions` in `re_descriptor.py`

Replace the body of `render_extensions` with a call to the helper:

```python
def render_extensions(self, ctx: Context, depth: int = 0) -> Block:
    """Render message extensions grouped by extendee."""
    return _render_extend_blocks(
        ctx=ctx,
        owner=self,
        extensions=self.extension,
        depth=depth,
        anomaly_code='B1',
        track_orphans=False,
    )
```

Remove the now-unused imports inside the old body:
`ReFieldDescriptorProto`, `allow_extend_block`, `get_file_node`,
`_resolve_field_features`, `field_label` (if solely used there).
Keep any imports still needed elsewhere in the file.

### Step 4 — Replace the file extend loop in `re_file.py`

In `ReFileDescriptorProto.render`, replace the extend-block section
(lines 388–426) with:

```python
# --- File extensions --------------------------------------------------
from .re_descriptor import _render_extend_blocks
extend_block = _render_extend_blocks(
    ctx=ctx,
    owner=self,
    extensions=self.extension,
    depth=depth,
    anomaly_code='A5',
    track_orphans=True,
)
if extend_block:
    out.extend(extend_block)
out.append_div_maybe(depth)
```

### Step 5 — Verify and run tests

Run `pytest reproto/src/reproto/tests/ -x -q` and confirm all 61 tests
pass.

Run `pyright` over the changed files and resolve any type errors.

---

## Anomaly report argument note

The anomaly helper `report(code, depth, **kwargs)` accepts named
keyword arguments that vary per code:

- `A5`: `file=`, `extendee=`
- `B1`: `msg=`, `extendee=`

In the helper, `owner` is either a `ReFileDescriptorProto` (has `.name`
as the file name) or a `ReDescriptorProto` (has `.name` as the message
name). Passing both `file=owner.name` and `msg=owner.name` is safe —
`report` only uses the kwargs it needs for each code. Alternatively,
pass the correct kwarg per `anomaly_code`. The spec leaves this
implementation detail to the implementer, as long as the rendered anomaly
text is identical to what the current code produces.

---

## Files changed

| File | Change |
|---|---|
| `re_descriptor.py` | Delete `render_oneofs`; add `_render_extend_blocks`; replace `render_extensions` body |
| `re_file.py` | Replace file extend-block loop with call to `_render_extend_blocks` |

---

## Implementation order

1. Delete `render_oneofs` from `re_descriptor.py` (zero risk, no call sites).
2. Add `_render_extend_blocks` to `re_descriptor.py`.
3. Replace `render_extensions` body to delegate to the helper.
4. Replace the file extend loop in `re_file.py`.
5. Run tests + pyright.
