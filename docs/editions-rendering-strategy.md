<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# Editions rendering — why it is harder than proto3, and how to approach it

## Background

reproto reconstructs `.proto` source files from binary `FileDescriptorSet`
(`.pb`) descriptors.  Its core invariant is **roundtrip fidelity**: given a
`.proto` file compiled to `pb1` by `protoc`, reproto's output recompiled to
`pb2` must satisfy `pb1 == pb2`.

reproto currently supports proto2 and proto3.  Editions (introduced in
protobuf 27 / 2023) remain a stub — edition files are rendered as proto2 with
a warning.  This document explains why lifting that stub is harder than adding
proto3 support was, and outlines a strategy for doing it correctly.


---

## Why proto3 support was straightforward

The differences between proto2 and proto3 are **per-file and enumerable**.
Once you know a file is proto3, a small fixed set of rules applies uniformly
to every element in that file:

- Singular field labels are implicit (no keyword) unless `proto3_optional` is
  set.
- `required` fields, `[default = ...]`, extension ranges, `extend` blocks on
  non-option messages, groups, and `import weak` are all illegal — warn and
  degrade.
- `packed` defaults to true for numeric types; mirror the explicit bit from
  the descriptor.
- Suppress synthetic oneofs (names starting with `_`, single
  `proto3_optional` member).

Every decision is binary and scoped to the file.  The full catalogue fits in
a single spec (0015) that took one pass to write.  The implementation was a
handful of guards in three files.


---

## Why editions is fundamentally different

### 1. The unit of resolution is the element, not the file

In proto2 and proto3, the syntax is declared once at the top of the file and
applies uniformly to everything in it.  In editions, each element — file,
message, field, enum, enum value — carries its own **`FeatureSet`** that can
override the edition defaults.

This means two fields in the same message can legitimately require different
rendering rules.  For example, one field may have `field_presence = IMPLICIT`
(proto3-like, no default value, no has-bit) while its neighbour has
`field_presence = EXPLICIT` (proto2-like, has-bit, can have a default value).
A single file-level "this is proto3" flag is simply the wrong abstraction.

### 2. Only explicit overrides are stored in the descriptor

protoc does not propagate edition defaults into the `.pb`.  If a field's
`field_presence` equals the edition default, nothing is written.  reproto
must implement the resolution algorithm itself:

```
edition defaults → file-level overrides → message-level overrides → field-level overrides
```

Each level in this chain is a sparse `FeatureSet` message that partially
overrides the level above it.  The edition default table is a
`FeatureSetDefaults` structure embedded in `descriptor.proto`; it must be
extracted once and hardcoded as a constant (or loaded from the descriptor at
startup).

This resolution engine does not exist yet and is the single most concentrated
new piece of work.

### 3. Features replace constructs, not just rules

In proto2/proto3, the descriptor structure is the same — only the rendering
conventions differ.  In editions, some constructs are structurally replaced:

| Proto2/proto3 construct | Edition replacement |
|-------------------------|---------------------|
| `TYPE_GROUP` field | `type = TYPE_MESSAGE` + `message_encoding = DELIMITED` |
| `label = LABEL_REQUIRED` | `label = LABEL_OPTIONAL` + `field_presence = LEGACY_REQUIRED` |
| `[packed = true/false]` option | `repeated_field_encoding = PACKED/EXPANDED` in features |

This means the existing detection paths in reproto break silently for edition
files: a delimited-encoded field looks like a plain message field to the
current group detector; a legacy-required field looks optional; a packed
repeated field appears to have no packed option at all.

### 4. The output format changes

Edition files do not use `syntax = "editions";` — they use
`edition = "2023";` (or another edition identifier).  They also emit
`features { ... }` blocks at the file, message, field, and enum level to
record the non-default overrides.  These are new top-level constructs that
require new rendering paths.

### 5. `features` is itself an extension point

`FeatureSet` is defined in `descriptor.proto` and can be extended by
language runtimes and third-party tools (e.g. `pb.cpp`, `pb.java`,
`pb.go`).  The edition descriptor may contain feature entries that reproto
does not know about and must render generically — the same challenge as
unknown custom options, but woven into the core feature inheritance chain.


---

## What is easy and what is hard

### Easy (already solved or trivially adaptable)

- **Most rendering constructs are unchanged.**  Services, RPCs, enums,
  `import`, `package`, file options, `reserved`, `oneof` bodies — none of
  these change in editions.  The existing rendering code handles them without
  modification.

- **`import weak` and `message_set_wire_format` are accepted in editions.**
  Empirically confirmed (findings Parts XVI–XVII).  The existing guards that
  allow them in proto2 should also allow them in editions.

- **`json_name`, `ctype`, `jstype`, `deprecated`, `weak`, and most
  `FieldOptions`/`MessageOptions` fields** are syntax-neutral and already
  handled via the generic options path.

- **Cross-file feature isolation.** Edition features do not propagate across
  file boundaries.  The hierarchy is strictly: edition defaults → file →
  message → field/enum.  An imported file's features have no effect on the
  importer.  This keeps the resolution scope bounded.

- **All data is present in the `.pb`.**  The descriptor contains every
  explicit override needed for resolution.  reproto's `.pb`-first approach is
  not a handicap here.

### Hard (requires new work)

- **Feature resolution engine.**  The 3-level merge (defaults + inherited
  overrides) must be implemented.  The edition default table must be
  extracted from `descriptor.proto`'s `FeatureSetDefaults` field and embedded.

- **New detection paths.**  `DELIMITED` encoding, `LEGACY_REQUIRED`
  presence, `EXPANDED` vs `PACKED` encoding — all require inspecting
  `features` rather than the legacy `type`, `label`, and `packed` fields.

- **`features { ... }` output.**  reproto must emit per-element `features`
  blocks containing only the non-default overrides (mirroring what protoc
  stores).  This is a new rendering path with no proto2/proto3 analogue.

- **`edition = "...";` header.**  Trivial to emit, but the edition identifier
  must come from `fdp.edition` (an enum) rather than from `fdp.syntax`.

- **Synthetic oneof suppression in editions.**  In proto3, `proto3_optional`
  is the signal.  In editions, `field_presence = IMPLICIT` is the signal —
  different field, same conceptual problem, requires a new detection branch.


---

## Recommended strategy

### Phase 0 — keep the current stub, but make it explicit

The current behavior (editions → warning + proto2 rendering) is already
correct interim behavior.  It should stay in place until editions support is
complete.  The warning (anomaly A1) already appears in the output.

### Phase 1 — implement the feature resolution engine in isolation

Write a pure function `resolve_features(ctx, element) -> FeatureSet` that:

1. Starts from the edition defaults for `fdp.edition`.
2. Merges file-level `options.features`.
3. Merges message-level `options.features` (for fields and enums).
4. Merges element-level `options.features`.

This function has no rendering side effects and can be unit-tested against
known edition descriptor fixtures independently of the rest of reproto.  It
is the prerequisite for everything else.

The edition default table should be extracted once from the `descriptor.proto`
that ships with the protobuf distribution and stored as a Python constant.

### Phase 2 — thread `FeatureSet` into the rendering helpers

The existing `syntax.py` helpers (`field_label`, `allow_groups`,
`packed_option`, etc.) already take `ctx` as their first argument and read
`ctx.target_syntax`.  For editions, each helper that needs to consult
features gains an optional `features: FeatureSet | None = None` parameter.
When `features` is not None, the helper reads from it instead of from syntax
string comparisons.

The callers (`re_field.py`, `re_descriptor.py`) resolve the per-element
`FeatureSet` once at the top of each `render()` call and pass it down.

This design means proto2 and proto3 rendering paths are **untouched** — they
continue to pass `features=None` and nothing changes for them.

### Phase 3 — add `features { ... }` rendering

Add a `render_features` helper that emits a `features { ... }` block from a
`FeatureSet` message, omitting fields that equal the edition default (since
protoc itself omits them).  Hook it into `re_file.py`, `re_descriptor.py`,
`re_field.py`, and `re_enum.py` at the appropriate depth.

### Phase 4 — replace the stub with real edition rendering

Once phases 1–3 are complete:
- Detect `ctx.syntax == "editions"` in `re_file.py` and emit `edition = "...";`
  instead of `syntax = "...";`.
- Remove the A1 warning for files that have been fully rendered.
- Add edition fixtures and roundtrip tests.

### Recommended sequencing rationale

Phases 1 and 2 are largely independent of each other and of the existing
rendering code.  Phase 1 (the resolution engine) is the highest-risk piece —
it involves understanding `FeatureSetDefaults` and getting the merge semantics
right — and should be tackled first in isolation so that bugs in it are easy
to isolate.  Phases 3 and 4 are straightforward once 1 and 2 are solid.


---

## Summary

| Dimension | Proto3 | Editions |
|-----------|--------|----------|
| Resolution granularity | Per file | Per element |
| Default propagation | Implicit in syntax string | Explicit merge algorithm needed |
| Structural descriptor changes | None | `DELIMITED`, `LEGACY_REQUIRED`, `features` replace legacy fields |
| New rendering constructs | None | `edition = "...";`, `features { }` blocks |
| Risk concentration | Low — enumerable fixed rules | Medium — resolution engine + new detection paths |
| Existing rendering code affected | Minimal guards | New parameter thread through helpers |
| Cross-file complexity | None | None (file boundaries are hard) |
| Data available in `.pb` | Complete | Complete |
