<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0036 — reproto: Round 5 — split long files

**Status:** implemented
**Implemented in:** 2026-05-03
**App:** reproto

---

## Purpose

Round 5 of spec 0031. Four long files are split to produce genuinely
smaller files. Two items (3 and 4) were completed in the first pass;
items 1 and 2 require a second pass to move code into new modules.

| File | Before | After (target) | Action |
|---|---|---|---|
| `reproto.py` | 1163 | ~60 | Phase functions move to `phases.py` |
| `base.py` | 585 | ~300 | Options renderers + protocols move to `option_rendering.py` |
| `simple_types.py` | 425 | 223 ✓ | `ReFieldDescriptor` moved to `field_descriptor.py` |
| `re_descriptor.py` | 567 | 470 ✓ | Source-code-info helpers moved to `source_info.py` |

---

## Goals

1. `reproto.py`: move all phase functions and helpers into a new
   `phases.py`; `reproto.py` becomes a ~60-line public entry point
   containing only `reproto()` and `matches_any_pattern`.
2. `base.py`: move the three options-rendering functions
   (`render_options_from_message`, `format_composite_options`,
   `render_options`) and the two Protocols (`DescriptorMessage`,
   `OptionsMessage`) into a new `option_rendering.py`; `base.py` keeps
   only `NodeBase` and `Node`.
3. `simple_types.py` → `field_descriptor.py`: **done**.
4. `re_descriptor.py` → `source_info.py` mixin: **done**.
5. All 61 existing tests continue to pass without modification.
6. All public names remain importable from their current locations
   (re-export shims where needed).

---

## Non-goals

- Changing any logic or observable output.
- Modifying test fixtures or test logic.
- Addressing Round 6 behavioural items.
- Splitting `re_file.py`, `re_enum.py`, `re_field.py`, `re_service.py`,
  or `re_method.py` (they are already at acceptable sizes).

---

## Specification

### Item 1 — Move phase functions out of `reproto.py` into `phases.py`

#### Rationale

The internal reorganization in the first pass named the seven phases as
private functions and made `reproto()` a readable index, but all code
remained in `reproto.py` (1038 lines). The file must actually shrink.

#### New module: `phases.py`

Move the following from `reproto.py` to a new `phases.py`:

- Exception classes: `DescriptorProtoMissingError`,
  `DescriptorProtoHasTargetsError`, `WellKnownTypeHasTargetsError`
- Helper functions: `import_annotations`, `fqdn_to_path`,
  `patch_go_package`, `_dump_resolved_features_yaml`,
  `_find_matching_nodes`, `_fuzzy_suggest`
- Phase functions: `_make_context`, `_phase1_load_files`,
  `_phase2_build_pool`, `_phase3_build_graph`, `_phase4_pruning`,
  `_phase5_reachability`, `_phase6_summoning`, `_phase7_output`

`phases.py` imports everything it needs directly (no re-export shims
needed for private names).

#### Updated `reproto.py`

After the split, `reproto.py` contains only:

- The module docstring (the long OVERVIEW / ALGORITHM PHASES / KEY
  PRINCIPLES / EXAMPLE FLOW / DEBUGGING FLAGS prose — keep it here as
  it documents the public entry point).
- `from .phases import ...` — import all names used by `reproto()` and
  `matches_any_pattern`.
- `matches_any_pattern` — public utility, stays here.
- `reproto()` — the public entry point, unchanged in logic.

No re-export shims for private names are needed; all callers of the
phase functions are inside `reproto.py` itself.

---

### Item 2 — Move options renderers and protocols out of `base.py` into `option_rendering.py`

#### Rationale

`base.py` contains two structurally distinct things: (a) `NodeBase`,
the registry/graph base class, and (b) three module-level
options-rendering functions plus two Protocols. These have no
mutual dependency within the file — (b) does not reference `NodeBase`,
and `NodeBase` only calls (b) through one-line delegate methods. Splitting
them removes 180 lines from `base.py`.

#### New module: `option_rendering.py`

Move the following from `base.py` to a new `option_rendering.py`:

- `DescriptorMessage` Protocol
- `OptionsMessage` Protocol
- `render_options_from_message` function
- `format_composite_options` function
- `render_options` function

`option_rendering.py` imports everything it needs directly. It does
**not** import `NodeBase` — the `node` parameter of `format_composite_options`
and `render_options` is typed as `'NodeBase[Any]'` using a string
forward reference.

#### Updated `base.py`

After the split, `base.py` contains only `NodeBase` and `Node`.
It re-exports the two Protocols and three functions for backward
compatibility:

```python
from .option_rendering import (
    DescriptorMessage,
    OptionsMessage,
    render_options_from_message,
    format_composite_options,
    render_options,
)
```

This preserves all existing `from .base import ...` call sites.

---

## Files changed

| File | Change |
|---|---|
| `phases.py` | **New** — all phase functions and helpers from `reproto.py` |
| `reproto.py` | Keep only `reproto()`, `matches_any_pattern`, docstring; import from `phases.py` |
| `option_rendering.py` | **New** — Protocols + 3 options-rendering functions from `base.py` |
| `base.py` | Keep only `NodeBase` + `Node`; re-export moved names |
| `field_descriptor.py` | **Done** — `ReFieldDescriptor` moved from `simple_types.py` |
| `simple_types.py` | **Done** — re-exports `ReFieldDescriptor` from `field_descriptor.py` |
| `source_info.py` | **Done** — `SourceCodeInfoMixin` moved from `re_descriptor.py` |
| `re_descriptor.py` | **Done** — inherits from `SourceCodeInfoMixin` |

---

## Implementation order

1. `phases.py` + `reproto.py` — move phase functions; run tests.
2. `option_rendering.py` + `base.py` — move renderers/protocols; run tests.
3. Run `pyright` over all changed files.
