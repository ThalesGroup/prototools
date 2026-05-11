<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0052 — reproto: missing WKT fallbacks and stub-node crash in phase 6

**Status:** implemented
**Implemented in:** 2026-05-11
**App:** reproto

---

## Background

Running `reproto --use-variant all` against a descriptor set that references
`google/protobuf/api.proto`, `google/protobuf/type.proto`,
`google/protobuf/field_mask.proto`, or `google/protobuf/source_context.proto`
(e.g. `googleapis/api-common-protos`) crashes with an `AssertionError` in
`re_descriptor.py:151` and produces no output.

---

## Root causes

### Issue 1 — Four well-known types missing from the embedded fallback set

`--use-variant all` loads embedded `.pb` fallbacks for seven well-known types:
`descriptor`, `any`, `empty`, `timestamp`, `duration`, `struct`, `wrappers`.
Four additional protobuf well-known types are not included:

| Proto file | Depends on |
|---|---|
| `google/protobuf/source_context.proto` | (none) |
| `google/protobuf/field_mask.proto` | (none) |
| `google/protobuf/type.proto` | `any`, `source_context` |
| `google/protobuf/api.proto` | `timestamp`, `source_context` |

Their `.proto` sources already exist in
`reproto/src/resources/google/protobuf/` (seeded from `pkgs.protobuf` by the
Nix derivation) but they are not compiled to `.pb` and not shipped as
fallbacks.

Because `api.proto` etc. are absent, the import-discovery loop in phase 1
cannot find them on disk and they remain as ref-only entries in `topo.files`.
Phase 3 skips ref-only entries, so no `ReFileDescriptorProto` node is created
for them.  Consequently, type stubs for `.google.protobuf.Api` etc. — created
by `from_ref` when a field references them — are never finalized: their
`_this` stays `None` (`is_present()` returns `False`) and their `_parent`
stays `None`.

### Issue 2 — `_all_type_targets` passes non-present (stub) nodes to `_host_file`

`_phase6_summoning` collects type targets of each summoned file via
`_all_type_targets`, then calls `_host_file(target)` on each to find the
hosting `ReFileDescriptorProto`.  `_all_type_targets` has no `is_present()`
guard, so it includes stub nodes (unresolvable type references).
`_host_file` walks `cur.parent` — which on a stub `ReDescriptorProto` fires:

```python
assert isinstance(self._parent, (ReFileDescriptorProto, ReDescriptorProto))
```

because `_parent is None` on a stub.

Issue 2 is triggered by Issue 1 in the specific case of missing WKT fallbacks.
But it is a latent bug independent of Issue 1: any genuinely unresolvable
type reference (from a user-supplied schema with external dependencies not on
the `-I` path) would hit the same crash.  The correct behaviour for such
nodes is to skip them silently — they have no hosting file, so the
import-bridge summoning logic has nothing to do with them.

---

## Goals

1. Add `source_context`, `field_mask`, `type`, and `api` to the set of
   embedded fallbacks compiled by `patch_reproto.sh` and loaded by
   `--use-variant all`.

2. Guard `_all_type_targets` so it only returns present (non-stub) nodes,
   making `_phase6_summoning` robust against any unresolvable type reference,
   not just missing WKTs.

3. `reproto --use-variant all` on the full `googleapis/api-common-protos`
   descriptor set completes without crash and without W5 warnings for any
   google/protobuf well-known type.

---

## Non-goals

- Supporting arbitrary missing dependencies beyond the protobuf well-known
  types (those remain W5/W1 warnings as before).
- Changing the `ReDescriptorProto.parent` assert (it remains strict for
  non-stub, fully-initialized nodes).

---

## Specification

### §1 — Extend `patch_reproto.sh`

Add the four missing files to the `google_proto_files` array, in dependency
order (leaves first so each `.pb` can be compiled without `--include_imports`):

```bash
"google/protobuf/source_context.proto"   # no imports
"google/protobuf/field_mask.proto"       # no imports
"google/protobuf/type.proto"             # imports any, source_context
"google/protobuf/api.proto"              # imports timestamp, source_context
```

Each entry is compiled exactly like the existing ones: a mono-fdp `.pb`
(without `--include_imports`) written to both `resources/` and
`variants/google-protobuf/`.

### §2 — Extend `cli.py`

Add the four new names to the `--use-variant all` expansion set and their
corresponding `fallback_protos` append blocks:

```python
if 'source_context' in use_variant_set:
    fallback_protos.append(_wk('google/protobuf/source_context.proto'))
if 'field_mask' in use_variant_set:
    fallback_protos.append(_wk('google/protobuf/field_mask.proto'))
if 'type' in use_variant_set:
    fallback_protos.append(_wk('google/protobuf/type.proto'))
if 'api' in use_variant_set:
    fallback_protos.append(_wk('google/protobuf/api.proto'))
```

The `all` expansion becomes:

```python
use_variant_set = {
    'any', 'empty', 'timestamp', 'duration',
    'struct', 'wrappers', 'descriptor',
    'source_context', 'field_mask', 'type', 'api',
}
```

The order of appends to `fallback_protos` matters for the topo-sort:
`source_context` and `field_mask` (leaves) must appear before `type` and
`api` (which depend on them).  The existing WKTs (`any`, `timestamp`) that
`type` and `api` also depend on are already appended earlier in the list.

### §3 — Guard `_all_type_targets` against stub nodes

In `_phase6_summoning`, change `_all_type_targets` to skip non-present nodes:

```python
for t in n.targets:
    if not isinstance(t, ReFileDescriptorProto) and t.is_present():
        result.add(t)
```

This is the correct behaviour regardless of why a node is absent: a stub has
no hosting file, so including it in the summoning walk serves no purpose and
risks crashing on any unresolvable type reference the user may supply.

### §4 — Regression test

Add a test to `test_roundtrip.py` (or `test_emit_scoring_graphs.py`) that:

1. Compiles a `.proto` file that references `google.protobuf.Api` (e.g. a
   small hand-crafted fixture or `google/longrunning/operations.proto`) to a
   mono-fdp `.pb`.
2. Runs `reproto --use-variant all --emit-scoring-graphs` on it.
3. Asserts exit code 0 and no `missing dependency file` warnings for any
   `google/protobuf/` well-known type.

---

## Open questions

None.
