<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0026 — Editions feature resolution engine

**Status:** implemented
**Implemented in:** 2026-05-02
**App:** reproto

---

## Purpose

Implement the feature resolution engine that is the prerequisite for editions
rendering (phase 1 of the strategy described in
`docs/specs/0025-editions-rendering-strategy.md`).

This spec covers only resolution — deriving the effective `FeatureSet` for any
element in an edition file.  Rendering changes (emitting `edition = "...";`,
`features { }` blocks, updated field labels, etc.) are deferred to a
follow-on spec.

---

## Background

In an edition file, every rendering decision that differs between proto2 and
proto3 — field presence, packed encoding, message encoding, enum closedness —
is governed by a `FeatureSet` that must be **resolved per element**.

A `FeatureSet` is a protobuf message with one optional enum field per feature.
Only non-default values are stored in the descriptor.  The effective value for
a given element is obtained by merging a chain from coarse to fine:

```
edition defaults  →  file overrides  →  message overrides  →  field/enum/oneof overrides
```

Each level is a sparse `FeatureSet`; unset fields are inherited from the level
above.  The edition defaults are the base.

reproto must implement this chain itself because:
1. The protobuf Python runtime's built-in resolution is tightly coupled to
   `_pb2.py` generated code and is not usable on arbitrary `.pb` files.
2. Variants may supply a custom `descriptor.pb` compiled against a
   non-standard `descriptor.proto` (e.g. Google's internal `proto2`
   dialect).  The edition defaults for that variant differ from the standard
   ones and must be read from the variant's own descriptor, not from the
   Python runtime.

---

## Key finding: where edition defaults live

Edition defaults are **not** a separate file.  They are stored inline in the
variant's `descriptor.pb` (or `descriptor.proto`), as `FieldOptions.edition_defaults`
repeated entries on each field of the `FeatureSet` message.

Each `edition_defaults` entry is a `(edition: Edition, value: string)` pair
where `value` is the name of the enum value (e.g. `"EXPLICIT"`, `"PACKED"`).

Example (from `net/proto2/proto/descriptor.proto`):

```
message FeatureSet {
  optional FieldPresence field_presence = 1 [
    edition_defaults = { edition: EDITION_LEGACY  value: "EXPLICIT" },
    edition_defaults = { edition: EDITION_PROTO3  value: "IMPLICIT" },
    edition_defaults = { edition: EDITION_2023    value: "EXPLICIT" }
  ];
  optional RepeatedFieldEncoding repeated_field_encoding = 3 [
    edition_defaults = { edition: EDITION_LEGACY  value: "EXPANDED" },
    edition_defaults = { edition: EDITION_PROTO3  value: "PACKED"   }
  ];
  ...
}
```

The default for a given edition and feature is found by taking the entry with
the **highest `edition` value that is ≤ the file's edition**.  If no entry
exists for a feature, the field is treated as unset (zero/unknown).

This mechanism is entirely self-describing and variant-agnostic: reproto reads
the defaults directly from whichever `descriptor.pb` the variant declares.

---

## Goals

1. At reproto startup, extract the edition default table from the variant's
   `descriptor.pb` and store it in `Context`.
2. Provide a `resolve_features(edition_defaults, edition, *feature_sets) -> ResolvedFeatures`
   function that merges any number of sparse `FeatureSet` messages (coarsest
   first) against the edition default table.
3. `ResolvedFeatures` is a plain dataclass exposing one attribute per
   `FeatureSet` field, with a typed Python value (enum integer).
4. The engine must be correct for any edition present in the variant's
   default table, not just edition 2023.
5. The engine must be tested in isolation, independently of any rendering code.

---

## Non-goals

- Rendering changes: emitting `edition = "...";`, `features { }` blocks,
  updated field labels, etc.  Covered in the follow-on spec.
- Language-specific feature extensions (`pb.cpp`, `pb.java`, `pb.go`, etc.).
  These are custom extensions of `FeatureSet` and are not needed for rendering
  decisions made by reproto.
- `RETENTION_SOURCE` features (`enforce_naming_style`,
  `default_symbol_visibility`): not stored in the runtime descriptor; no
  rendering implication.

---

## Specification

### 1. Extracting the edition default table at startup

The variant's `descriptor.pb` is already loaded into reproto as a
`FileDescriptorProto` (it is the file identified by
`ctx.variant_descriptor_proto`).  That `FileDescriptorProto` contains (among
others) the `FeatureSet` message definition.

**Algorithm — `build_edition_defaults(descriptor_fdp) -> EditionDefaultTable`:**

1. Locate the `FeatureSet` message in `descriptor_fdp.message_type` by name.
   If not found, return an empty table (variant does not support editions).
2. For each field `f` in `FeatureSet`:
   a. Collect all `f.options.edition_defaults` entries as a list of
      `(edition_number: int, value_name: str)` pairs.
   b. Sort by `edition_number` ascending.
   c. Store as `table[f.name] = sorted_list`.
3. Return the table.

The result is stored on `Context` as `ctx.edition_defaults`.  It is computed
once per reproto invocation (the variant's descriptor does not change mid-run).

`descriptor_fdp` is the `FileDescriptorProto` for the file named by
`ctx.variant_descriptor_proto` — already available in the topology at phase 2.

**Type:**

```python
# Maps feature field name → sorted list of (edition_number, value_name)
EditionDefaultTable = dict[str, list[tuple[int, str]]]
```

### 2. Resolving a single feature for a given edition

**`_resolve_one(table, feature_name, file_edition, *feature_sets) -> int`**

- `table`: the `EditionDefaultTable` from step 1.
- `feature_name`: e.g. `"field_presence"`.
- `file_edition`: the integer edition of the file being rendered
  (e.g. `1000` for `EDITION_2023`).
- `*feature_sets`: the same override chain passed to `resolve_features`
  (raw `FeatureSet` messages or `None`).

Algorithm:
1. Determine the edition default: walk the sorted list from the end and take
   the entry with the highest `edition_number ≤ file_edition`.  If no entry
   matches, default is 0 (unknown).
2. Walk the override chain left to right.  For each non-None `FeatureSet`, if
   `fs.HasField(feature_name)` is true, update the result with
   `getattr(fs, feature_name)`.  The last set value wins.
3. Return the final integer.

The `HasField` check and enum-number lookup happen inside the function;
callers pass raw `FeatureSet` messages without pre-extracting values.

### 3. Resolving a full FeatureSet chain

**`resolve_features(edition_defaults, file_edition, *feature_sets) -> ResolvedFeatures`**

- `file_edition`: integer, from `fdp.edition`.
- `*feature_sets`: zero or more `FeatureSet` proto messages, ordered from
  coarsest to finest (file → message → field/enum/oneof).  Any may be absent
  (pass `None`; it is skipped).

Algorithm:
1. For each `FeatureSet` field name `f` in the default table:
   a. Start with the edition default for `f` at `file_edition`.
   b. For each non-None `FeatureSet` in order: if `fs.HasField(f)`, override
      with `getattr(fs, f)`.
2. Return a `ResolvedFeatures` instance populated with the resolved values.

**`ResolvedFeatures` dataclass:**

```python
@dataclass
class ResolvedFeatures:
    field_presence:           int  # FieldPresence enum value
    enum_type:                int  # EnumType enum value
    repeated_field_encoding:  int  # RepeatedFieldEncoding enum value
    utf8_validation:          int  # Utf8Validation enum value
    message_encoding:         int  # MessageEncoding enum value
    json_format:              int  # JsonFormat enum value
    # enforce_naming_style and default_symbol_visibility are RETENTION_SOURCE;
    # they are not stored in runtime descriptors and are excluded.
```

Named integer constants for the enum values are defined alongside
`ResolvedFeatures` as module-level literals (e.g. `FIELD_PRESENCE_EXPLICIT = 1`,
`MESSAGE_ENCODING_DELIMITED = 2`, etc.).  They are hardcoded rather than
derived from the pool at startup; this is simpler and sufficient for the
standard `google.protobuf` variant whose enum numbering is stable.

### 4. Handling unknown / missing FeatureSet fields

The `FeatureSet` message may gain new fields in future editions.  Fields not
present in reproto's `ResolvedFeatures` dataclass are silently ignored.  If a
field is present in the dataclass but absent from the variant's `FeatureSet`
message (older variant), its resolved value is 0.

### 5. Context and CLI changes

Add to `Options` / `Context`:

```python
edition_defaults: dict[str, list[tuple[int, str]]] = {}   # populated at phase 2
dump_resolved_features: str = ""                           # set by --dump-resolved-features
```

`edition_defaults` is populated at the end of phase 2, after the variant's
`descriptor.pb` has been parsed into the pool, by calling
`build_edition_defaults(ctx.pool_db.FindFileByName(ctx.variant_descriptor_proto))`.

`dump_resolved_features` is set from the hidden CLI flag described in the
Testing section.  When non-empty, the rendering pipeline is short-circuited
after phase 3 (see §6 below).

### 6. Module placement and modified files

The engine lives in a new module `reproto/feature_resolution.py` containing:

- `EditionDefaultTable` type alias
- `ResolvedFeatures` dataclass and enum constants
- `build_edition_defaults(descriptor_fdp) -> EditionDefaultTable`
- `resolve_features(edition_defaults, file_edition, *feature_sets) -> ResolvedFeatures`
- `feature_value_name(table, feature_name, value) -> str`  (YAML helper)

Additional files modified in this phase:

- `context.py` — `edition_defaults` and `dump_resolved_features` fields added to `Options`.
- `reproto.py` — `build_edition_defaults` call after phase 2; `_dump_resolved_features_yaml`
  function; early-return hook after phase 3 when `dump_resolved_features` is set.
- `cli.py` — hidden `--dump-resolved-features` option; `--output-root` made optional
  when that flag is present.

---

## Call site (future, for reference)

Once phase 2 (rendering integration) is implemented, a field render will call:

```python
resolved = resolve_features(
    ctx.edition_defaults,
    fdp.edition,
    fdp.options.features,          # file-level
    msg.options.features,          # message-level (if rendering a field)
    field.options.features,        # field-level
)
if resolved.field_presence == FIELD_PRESENCE_IMPLICIT:
    ...  # proto3-like: no label, no default, no has-bit
elif resolved.field_presence == FIELD_PRESENCE_LEGACY_REQUIRED:
    ...  # required field
```

This is out of scope for the current spec.

---

## Testing

### Strategy

Testing has two layers:

1. **Unit tests** — pure Python, no CLI, no filesystem.  They test
   `build_edition_defaults` and `resolve_features` directly using synthetic or
   real-descriptor inputs.  Fast and isolated.

2. **Golden regression tests** — run reproto end-to-end with a handcrafted
   `.proto` fixture and compare the output against a checked-in golden YAML
   file.  These catch regressions in the full pipeline (descriptor loading,
   engine invocation, output formatting).

Only the `.proto` fixture source is committed to Git.  The `.pb` is compiled
at test time by `protoc` (same pattern as the existing roundtrip tests).

### Hidden CLI flag: `--dump-resolved-features <proto-file-name>`

A hidden flag (not shown in `--help`, not listed in the CLI group table) that
makes reproto, instead of rendering `.proto` output, emit a YAML document
describing the resolved `ResolvedFeatures` for every element in the named
file.

The flag is "hidden" in the same spirit as `--debug` and `--debug-fqdn`: it
is a diagnostic / testing aid, not a user-facing feature.  It can be removed
once phases 2–4 produce enough rendered output to make golden roundtrip tests
sufficient on their own.

**Output format** — one YAML document per run, written to stdout:

```yaml
file: editions_resolution.proto
edition: 1000                      # raw Edition enum integer (1000 = EDITION_2023)
edition_defaults:
  field_presence: EXPLICIT
  enum_type: OPEN
  repeated_field_encoding: PACKED
  utf8_validation: VERIFY
  message_encoding: LENGTH_PREFIXED
  json_format: ALLOW
file_features:
  utf8_validation: NONE            # only features explicitly set at file level
file_resolved:
  field_presence: EXPLICIT
  ...
messages:
  Request:
    resolved:
      field_presence: EXPLICIT
      ...
    overrides: {}                  # no message-level features set
    fields:
      optional_name:
        resolved:
          field_presence: IMPLICIT
          ...
        overrides:
          field_presence: IMPLICIT  # explicitly set at field level
      name:
        resolved:
          field_presence: EXPLICIT
          ...
        overrides: {}
enums:
  Status:
    resolved:
      enum_type: CLOSED
      ...
    overrides:
      enum_type: CLOSED
```

Rules:
- `edition_defaults` shows the raw defaults for the file's edition (before
  any overrides), resolved from the variant's `descriptor.pb`.
- `file_features`, `overrides` at each level show only features that are
  **explicitly set** (i.e. `HasField` is true) at that element — the sparse
  delta, not the merged result.
- `resolved` at each level shows the fully merged value.
- All enum values are rendered as their string name (e.g. `EXPLICIT`, not `1`).
- Messages, enums, and fields are listed in descriptor order.
- Nested messages are rendered recursively.

### Fixture: `editions_resolution.proto`

A single handcrafted fixture that exercises the full resolution chain.
Location: `reproto/src/reproto/tests/fixtures/editions_resolution.proto`.

It must cover:
- A file-level feature override (`utf8_validation = NONE`).
- A message with no overrides (inherits from file).
- A field with a field-level override (`field_presence = IMPLICIT`).
- A field with no override (inherits from file).
- A repeated field with `repeated_field_encoding = EXPANDED`.
- A field with `message_encoding = DELIMITED`.
- A top-level enum with an enum-level override (`enum_type = CLOSED`).

Note: `features.enum_type` cannot be set on a `MessageOptions` — protoc rejects
it.  The original spec example (`message with enum_type = CLOSED`) was
incorrect.  The inheritance chain is fully exercised via the file → field path.

The fixture uses `edition = "2023"` and the standard `descriptor.pb`
(no variant).

### Golden file: `editions_resolution.yaml`

Location: `reproto/src/reproto/tests/fixtures/editions_resolution.yaml`.

Generated once by running (from the repo root, inside the nix-shell):

```
protoc --descriptor_set_out=/tmp/editions_resolution.pb \
       --include_imports \
       -I reproto/src/reproto/tests/fixtures \
       reproto/src/reproto/tests/fixtures/editions_resolution.proto

python -m reproto.cli \
       --use-variant descriptor \
       -I /tmp \
       --dump-resolved-features editions_resolution.proto \
       /tmp/editions_resolution.pb \
       > reproto/src/reproto/tests/fixtures/editions_resolution.yaml
```

Committed to Git and thereafter treated as the reference.  Any change to the
engine that alters the output must be accompanied by a deliberate golden
update.

### Unit tests (`test_feature_resolution.py`)

Separate from the golden test; lives in
`reproto/src/reproto/tests/test_feature_resolution.py`.  Tests the engine
functions directly, using either synthetic `FileDescriptorProto` objects built
in Python or the built-in `resources/google/protobuf/descriptor.pb`.

| Test | What it covers |
|------|----------------|
| T1 — real descriptor | `build_edition_defaults` on the built-in `descriptor.pb`; assert known entries are present for `field_presence` and `repeated_field_encoding` |
| T2 — default lookup | `resolve_features` with no override chain at `EDITION_LEGACY`, `EDITION_PROTO3`, `EDITION_2023`, and edition 0 (below all defaults) |
| T3 — explicit override wins | A `FeatureSet` with `field_presence = IMPLICIT` overrides the edition default |
| T4 — finer wins over coarser | Field-level override supersedes file-level override |
| T5 — None levels skipped | `resolve_features` with all-None levels equals plain edition defaults |
| T6 — empty variant | `build_edition_defaults` on a descriptor with no `FeatureSet` message returns `{}` |
| T7 — synthetic descriptor | Round-trip with a hand-built `FileDescriptorProto`; verifies table structure and enum mapping |

### Golden regression test (`test_feature_resolution.py`)

`test_editions_resolution_golden` in `test_feature_resolution.py`:

1. Writes `editions_resolution.proto` to a `tmp_path` directory.
2. Compiles it with `protoc --include_imports` to produce a `.pb`.
3. Runs `reproto.cli --use-variant descriptor --dump-resolved-features
   editions_resolution.proto` via subprocess.
4. Parses both the actual stdout and the checked-in golden YAML via
   `yaml.safe_load` and asserts structural equality.

The test fails if either the engine logic or the YAML serialisation changes
unexpectedly.  Both `.proto` fixture and `.yaml` golden are committed to Git;
the `.pb` is ephemeral (compiled at test time).

---

## Resolved design decisions

1. **`FeatureSet` field enumeration**: `ResolvedFeatures` hardcodes the six
   RETENTION_RUNTIME field names.  Extra fields in a variant's `FeatureSet`
   are silently ignored.  Accepted for phase 1; dynamic construction can be
   revisited if needed in later phases.

2. **Enum constants**: hardcoded as module-level integer literals in
   `feature_resolution.py`.  Simpler than pool-derived extraction and
   sufficient given that the standard `google.protobuf` enum numbering is
   stable.
