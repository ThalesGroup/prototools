<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0045 — reproto: --emit-scoring-graph option

**Status:** implemented
**Implemented in:** 2026-05-10 (updated 2026-05-10, label field added 2026-05-10)
**App:** reproto

---

## Background

Spec 0043 defines a pipeline for building a Hopcroft-deduplicated schema
database.  Stage 1 of that pipeline runs reproto over a corpus and produces
canonical `.proto` files; Stage 2 runs protoc to produce `.pb` files; only
then can the graph builder (spec 0043 Sub-step A) begin.

This spec short-circuits Stages 1 and 2 by having reproto emit the scoring
graph directly in YAML format, as a side effect of its existing walk.  The
graph builder (schema-db) then reads these YAML files instead of `.pb` files.

reproto already:
- Loads all FDPs into a `DescriptorPool` (phase 2).
- Builds a rich FQDN graph over all message types (phase 3).
- Has `ctx.pool` giving resolved `FileDescriptor` / `Descriptor` objects
  for every loaded file and message type.
- Has the `show_graph` infrastructure for HTML visualisation.

Adding `--emit-scoring-graph` is therefore mostly wiring: a new phase that walks
`ctx.pool`, extracts the scoring-relevant subset of each field, and writes
YAML.  No new schema loading or field-resolution logic is needed.

### Lifecycle

`--emit-scoring-graph` is a **build-time** operation.  It runs once (or when the
corpus changes) and produces a set of YAML files that are fed into the
schema-db compilation pipeline.  It is not on the hot path.

---

## Goals

1. Add `--emit-scoring-graph` CLI flag to reproto.
2. For each **summoned** `FileDescriptorProto` (i.e. each file that is written
   as a `.proto` output), write one YAML file alongside it under
   `--output-root`, e.g. `OUT/google/rpc/status.yaml` next to
   `OUT/google/rpc/status.proto`.  Files that are loaded but not summoned
   (pruned, unreachable, or outside the seed set) produce no YAML.
3. The YAML contains only the information required for scoring (spec 0042
   §Schema information required for scoring): for each message type, a list
   of `(field_number, scoring_kind[, child_fqdn])` triples.
4. All node identifiers in the YAML use **fully-qualified proto names
   (FQDNs)** so that multiple YAML files can be combined unambiguously by the
   schema-db pipeline.
5. `--emit-scoring-graph` is **independent** of `--emit-binary`, `--dry-run`:
   it can be combined with any of them.  `--output-root` remains required
   (as usual) since the YAML files are written next to the `.proto` output.

## Non-goals

- Hopcroft minimisation (schema-db's job).
- Scoring algorithm (prototext's job).
- HTML visualisation of the scoring graph (a separate tool can render the
  YAML; not part of this spec).
- Services, enums, extensions: not included in the scoring graph.
  Only message types and their fields are emitted.

---

## Specification

### §1 — CLI

New flag added to `reproto/src/reproto/cli.py` and `Options` in
`context.py`:

```
--emit-scoring-graph
    Write per-file scoring-graph YAML files alongside the reconstructed
    .proto files under --output-root.  One YAML file per FileDescriptorProto
    in the loaded corpus.  Requires --output-root.
```

In `Options`:
```python
emit_scoring_graph: bool = False
```

The `--emit-scoring-graph` option belongs to the **Advanced** section in
`cli.py`'s `_SECTIONS` dict:
```python
'--emit-scoring-graph': 'Advanced',
```

`--output-root` remains required as before; no change to the existing
guard in `cli.py`.

### §2 — YAML format

One YAML file per `FileDescriptorProto`, written next to the corresponding
`.proto` output file under `--output-root`: the proto file name with
`.proto` replaced by `.yaml`.  Example: `OUT/google/rpc/status.proto` →
`OUT/google/rpc/status.yaml`.

```yaml
# One entry per top-level or nested message type in this FileDescriptorProto.
# Keys are fully-qualified proto names (no leading dot).
messages:
  google.rpc.Status:
    kind: LENDEL
    fields:
      - number: 1
        kind: VARINT
        label: required
      - number: 2
        kind: LEN_STRING
      - number: 3
        kind: MESSAGE
        child: google.protobuf.Any
        label: repeated
      - number: 4
        kind: ENUM
        enum_min: 0
        enum_max: 5
```

Notes:
- Each message entry has a top-level `kind` field: `LENDEL` (entered via a
  length-delimited wire frame, `WT_LEN=2`) or `GROUP` (entered via a
  start-group tag, `WT_START_GROUP=3`).  When absent, `LENDEL` is assumed
  (backward compatibility).  This attribute is a property of the node itself
  and is used by the Hopcroft minimization to keep LENDEL and GROUP nodes in
  separate equivalence classes (spec 0058).
- Field `kind: MESSAGE` means the destination is a non-leaf node (either a
  length-delimited message or a group).  The framing of the destination is
  encoded in the destination node's own `kind`, not on the edge.
- `child` key is present when field `kind` is `MESSAGE`.  Absent for all
  other field kinds.
- `enum_min` and `enum_max` are present when `kind` is `ENUM`; they give
  the minimum and maximum integer values defined for the enum type of this
  field.  An observed varint outside `[enum_min, enum_max]` is a **veto**
  — a strong negative score impact equivalent to a wire-type mismatch.
- `label` is one of `optional`, `required`, or `repeated`.  `optional` is
  the default and is **omitted** when it applies, to keep the YAML compact.
  `required` and `repeated` are always written explicitly.
- Fields are listed in ascending `number` order.
- Nested message types (defined inside another message) are included as
  top-level entries in `messages`, using their full FQDN.  For example,
  a message `Outer` containing `message Inner` produces two top-level
  entries: `pkg.Outer` and `pkg.Outer.Inner`.
- Synthetic map-entry types generated by `map<K, V>` fields are included
  as regular nested message entries (they appear as nested types in the
  pool and are scored like any other message type).
- Message types from *imported* files that happen to appear in the pool
  are **not** emitted in this file — only types whose
  `FileDescriptorProto.name` matches the current file.  Cross-file
  references appear only as `child` FQDNs; their definitions live in their
  own YAML file.
- If a `FileDescriptorProto` defines no message types (e.g. a file
  containing only enums or extensions), the YAML file is still written
  with `messages: {}`.  Enum content is not included in this spec (deferred
  to a future spec once the scoring model is extended to use enum value
  ranges).

### §3 — ScoringKind and label mapping

The `kind` field is one of eight string values derived from each field's
proto type and packing state.  The mapping uses `FieldDescriptor` objects
from `ctx.pool` rather than raw `FieldDescriptorProto` bytes: this is
important because `FieldDescriptor.is_packed` already accounts for proto3
default packing rules, so no manual syntax-version check is needed.

| Proto type(s) | `.is_packed`? | Field `kind` | Message entry `kind` |
|---|---|---|---|
| TYPE_INT32, TYPE_INT64, TYPE_UINT32, TYPE_UINT64, TYPE_SINT32, TYPE_SINT64, TYPE_BOOL | false | `VARINT` | — |
| same | true | `LEN_PACKED` | — |
| TYPE_ENUM | false | `ENUM` (with `enum_min`, `enum_max`) | — |
| TYPE_ENUM | true | `LEN_PACKED` | — |
| TYPE_FIXED64, TYPE_SFIXED64, TYPE_DOUBLE | false | `I64` | — |
| TYPE_FIXED64, TYPE_SFIXED64, TYPE_DOUBLE | true | `LEN_PACKED` | — |
| TYPE_FIXED32, TYPE_SFIXED32, TYPE_FLOAT | false | `I32` | — |
| TYPE_FIXED32, TYPE_SFIXED32, TYPE_FLOAT | true | `LEN_PACKED` | — |
| TYPE_STRING | n/a | `LEN_STRING` | — |
| TYPE_BYTES | n/a | `LEN_BYTES` | — |
| TYPE_MESSAGE | n/a | `MESSAGE` | `LENDEL` |
| TYPE_GROUP | n/a | `MESSAGE` | `GROUP` |

The message entry `kind` (`LENDEL` or `GROUP`) is a property of the message
node itself, not of the field referencing it.  A message is tagged `GROUP` if
and only if it appears as the `message_type` of at least one `TYPE_GROUP` field
anywhere in the loaded pool.  All other message entries are tagged `LENDEL`.

`enum_min` and `enum_max` are derived from `field.enum_type.values_by_number`:
```python
values = list(field.enum_type.values_by_number.keys())
enum_min, enum_max = min(values), max(values)
```

**Label mapping**: each field also carries a `label` key derived from
`FieldDescriptor.label`:

| `FieldDescriptor.label` | YAML value | Emitted? |
|---|---|---|
| `LABEL_OPTIONAL` | `optional` | No — default, omitted |
| `LABEL_REQUIRED` | `required` | Yes |
| `LABEL_REPEATED` | `repeated` | Yes |

**Proto3 implicit packing** is handled automatically: `FieldDescriptor.is_packed`
returns `True` for a `LABEL_REPEATED` field of a packable type in a proto3
file even without an explicit `[packed=true]` option, and `False` for a
field with explicit `[packed=false]`.  No manual syntax-version check is
needed.

Implementation note: use `ctx.pool.FindFileByName(proto_name)` (returns a
`FileDescriptor`) — not `ctx.pool_db.FindFileByName()` (which returns a
raw `FileDescriptorProto` and lacks `.message_types_by_name`,
`.fields_by_number`, `.nested_types`, `.full_name`, and `.is_packed`).

### §4 — Implementation: new phase

A new function `_phase_emit_graph` is added to `phases.py` and called from
`reproto()` in `reproto.py` after `_phase7_output`:

```python
# reproto.py — after _phase7_output(ctx, out_repo):
if ctx.emit_scoring_graph:
    _phase_emit_graph(ctx, out_repo)
if ctx.graph is not None:
    show_graph(ctx, output_path=ctx.graph)
```

`_phase_emit_graph` signature:

```python
def _phase_emit_graph(ctx: Context, out_dir: Path) -> None:
    """Emit one scoring-graph YAML file per FileDescriptorProto (spec 0045)."""
```

Implementation sketch:

```python
def _phase_emit_graph(ctx: Context, out_dir: Path) -> None:
    import yaml

    for re_file in ctx.nodes.values():
        if not isinstance(re_file, ReFileDescriptorProto):
            continue
        if not re_file.is_present():
            continue
        proto_name = re_file.name          # e.g. "google/rpc/status.proto"

        try:
            fd = ctx.pool.FindFileByName(proto_name)
        except KeyError:
            continue

        messages: dict = {}

        def _collect(desc, messages):
            """Collect desc and all nested message types into messages."""
            fields_out = []
            for f in sorted(desc.fields_by_number.values(),
                            key=lambda f: f.number):
                kind, child = _scoring_kind(f)
                entry = {"number": f.number, "kind": kind}
                if child is not None:
                    entry["child"] = child
                fields_out.append(entry)
            messages[desc.full_name] = {"fields": fields_out}
            for nested in desc.nested_types:
                _collect(nested, messages)

        for msg_desc in fd.message_types_by_name.values():
            _collect(msg_desc, messages)

        yaml_path = out_dir / Path(proto_name).with_suffix(".yaml")
        yaml_path.parent.mkdir(parents=True, exist_ok=True)
        with open(yaml_path, "w", encoding="utf-8") as fh:
            yaml.dump({"messages": messages}, fh,
                      sort_keys=False, allow_unicode=True)
```

`_scoring_kind(field: FieldDescriptor) -> tuple[str, str | None]`:

```python
def _scoring_kind(field) -> tuple[str, str | None]:
    from google.protobuf.descriptor import FieldDescriptor as FD
    TYPE = field.type
    if TYPE == FD.TYPE_MESSAGE:
        return "MESSAGE", field.message_type.full_name
    if TYPE == FD.TYPE_GROUP:
        return "MESSAGE", field.message_type.full_name
    if TYPE == FD.TYPE_STRING:
        return "LEN_STRING", None
    if TYPE == FD.TYPE_BYTES:
        return "LEN_BYTES", None
    if TYPE in (FD.TYPE_DOUBLE, FD.TYPE_FIXED64, FD.TYPE_SFIXED64):
        if field.is_packed:
            return "LEN_PACKED", None
        return "I64", None
    if TYPE in (FD.TYPE_FLOAT, FD.TYPE_FIXED32, FD.TYPE_SFIXED32):
        if field.is_packed:
            return "LEN_PACKED", None
        return "I32", None
    varint_types = {
        FD.TYPE_INT32, FD.TYPE_INT64, FD.TYPE_UINT32, FD.TYPE_UINT64,
        FD.TYPE_SINT32, FD.TYPE_SINT64, FD.TYPE_BOOL, FD.TYPE_ENUM,
    }
    if TYPE in varint_types:
        if field.is_packed:   # True for proto3 implicit packing too
            return "LEN_PACKED", None
        return "VARINT", None
    raise ValueError(f"Unknown field type: {TYPE}")
```

### §5 — Changes to existing files

| File | Change |
|---|---|
| `cli.py` | Add `--emit-scoring-graph` flag (is_flag); add `'--emit-scoring-graph': 'Advanced'` to `_SECTIONS` |
| `context.py` | Add `emit_scoring_graph: bool = False` to `Options` |
| `reproto.py` | Call `_phase_emit_graph(ctx, out_repo)` after `_phase7_output`; export it |
| `phases.py` | Add `_phase_emit_graph`, `_scoring_kind`, and `_field_label` functions |

No changes to `show.py`, `re_descriptor.py`, `re_field.py`, or any test
infrastructure.

### §6 — Tests

Unit tests in `reproto/src/reproto/tests/`, added to a new file
`test_emit_graph.py`.  Tests use existing fixtures from `tests/fixtures/`
compiled on demand via `compile_proto()`.

**TC-1 — basic emission**: compile `field_comprehensive.proto` (covers all
proto2 scalar types, enums, message fields, groups) and run reproto with
`--emit-scoring-graph`.  Assert:
- A YAML file is written for the root FDP alongside its `.proto` output.
- Every message type in the FDP appears as a key in `messages`.
- Field numbers are correct.
- `kind` values match the ScoringKind table in §3.
- `child` is present and correct for `TYPE_MESSAGE` and `TYPE_GROUP` fields.
- `child` is absent for all other kinds.

**TC-2 — nested message types**: `field_comprehensive.proto` contains
`Outer` with nested `Outer.Middle` and `Outer.Middle.Inner`.  Assert all
three appear as separate top-level keys in `messages` with their full
FQDNs (`test.field.Outer`, `test.field.Outer.Middle`,
`test.field.Outer.Middle.Inner`).

**TC-3 — cross-file reference**: compile `address_book.proto` (which
imports `phone_number.proto`).  Assert:
- The YAML for `address_book.yaml` contains `child: <phone_number FQDN>`
  for the relevant message field.
- The YAML for `address_book.yaml` does NOT contain the phone-number
  message type as a key in `messages` (it belongs to `phone_number.yaml`).

**TC-4 — proto3 implicit packing**: compile `packed_proto3.proto`
(`default_int = 1` is a `repeated int32` with no explicit `[packed]`
option).  Assert `kind: LEN_PACKED` is emitted for that field.
Also assert that `explicit_false = 3` (explicit `[packed=false]`) emits
`kind: VARINT`.  Also assert that `doubles_def = 9` (`repeated double`,
packed in proto3) emits `kind: LEN_PACKED`, and `floats_true = 10`
(`repeated float [packed=true]`) emits `kind: LEN_PACKED`.

**TC-5 — group child FQDN**: `field_comprehensive.proto` contains
`GroupTest` with `RepeatedGroup` and `OptionalGroup` (proto2 group fields).
Assert that the group fields appear with field `kind: MESSAGE` and a `child`
FQDN pointing to the synthetic group message type.  Assert that the synthetic
group message entries themselves carry `kind: GROUP` (not `LENDEL`).

### §7 — Validation on protodb corpus (companion document)

See companion document `0045-reproto-emit-graph-protodb.md` (gitignored).

Expected outputs for the protodb corpus:
- One YAML file per `.proto` file produced by reproto.
- `google/rpc/status.yaml` matches Example B from spec 0044.
- `google/longrunning/operations.yaml` matches Example A from spec 0044.
- Total combined `messages` count to be measured and recorded in the
  companion document.
