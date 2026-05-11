<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0055 — Protobuf instance generator

**Status:** implemented
**Implemented in:** 2026-05-11
**App:** prototools (new tool: `proto-gen`)

---

## Background

The scorer test harness (spec 0054) requires a curated set of protobuf
message instances stored as `#@` prototext files.  Hand-crafting these for
a large and evolving corpus is impractical.  This spec defines an automated
generator that, given a root message type and a random seed, produces a
realistic binary protobuf instance and converts it to the `#@` prototext
format with ground-truth hint comments.

The generator is also used to produce illustrative instances for the scoring
tutorial (spec 0050), where the output may be post-edited by hand for
pedagogical clarity.

---

## Goals

1. A Python CLI tool (`proto-gen`) that accepts a descriptor directory,
   a fully-qualified message type name, and a random seed, and writes a
   `#@` prototext `.pb` file.
2. Pseudo-random but reproducible: identical inputs always produce
   identical output.
3. Produces valid, schema-faithful protobuf binary (all field numbers,
   wire types, and nested types correct).
4. Output format is `#@` prototext (via `prototext -d`) with ground-truth
   hint comments inserted immediately after the magic line.
5. Does not implement its own text serialization — delegates entirely to
   `prototext -d` for the textual output.

---

## Non-goals

- Covering every possible protobuf feature exhaustively (extensions, maps,
  oneof exhaustive coverage, unknown fields, etc. are out of scope for v1).
- Producing maximally adversarial or edge-case instances (that is a
  separate hardening task).
- Post-editing for pedagogical clarity (done manually by the spec author).

---

## Design

### Language and dependencies

Python 3.12.  Dependencies:

- `google.protobuf` — descriptor pool, reflection API, binary serialization.
- `subprocess` — to invoke `prototext -d` for text conversion.
- Standard library only otherwise (`random`, `pathlib`, `argparse`).

### Descriptor pool loading

The generator loads all `.pb` files from the input directory into a
`google.protobuf.descriptor_pool.DescriptorPool`, using
`google.protobuf.descriptor_pb2.FileDescriptorSet` / `pool.Add`.  This
mirrors the approach used by reproto's phase 2, and handles cross-file
dependencies naturally as long as all transitive imports are present in
the directory.

Files are loaded in topological order (dependency-leaves first) using the
same topo-sort logic already present in reproto.  If a dependency is
missing, the file is skipped with a warning (not a fatal error).

### Random walk over the FDP

The generator uses `pool.FindMessageTypeByName(fqdn)` to obtain the root
`Descriptor`, then recursively populates a `message.Message` instance by
walking the descriptor's fields.

For each field the generator makes pseudo-random choices:

| Field kind | Choice |
|---|---|
| `LABEL_OPTIONAL` (proto3 implicit) | include with probability `p_optional` (default 0.7) |
| `LABEL_OPTIONAL` (proto2 explicit / oneof member) | include with probability `p_optional` |
| `LABEL_REQUIRED` | always included |
| `LABEL_REPEATED` | count drawn from `randint(0, max_repeated)` (default `max_repeated=3`) |
| oneof | choose one member uniformly at random (or none with probability `1 - p_optional`) |

Leaf field values:

| Type | Generation |
|---|---|
| `TYPE_STRING` | `f"s{rng.randint(0, 9999)}"` |
| `TYPE_BYTES` | `rng.randbytes(rng.randint(0, 8))` |
| `TYPE_BOOL` | `rng.choice([True, False])` |
| `TYPE_ENUM` | uniform choice from `enum_descriptor.values_by_number` |
| Integer types (`INT32`, `INT64`, `UINT32`, `UINT64`, `SINT32`, `SINT64`) | `rng.randint(0, 1000)` |
| Fixed types (`FIXED32`, `FIXED64`, `SFIXED32`, `SFIXED64`) | `rng.randint(0, 1000)` |
| `TYPE_FLOAT` / `TYPE_DOUBLE` | `rng.uniform(0.0, 1000.0)` |
| `TYPE_MESSAGE` | recurse (depth-limited) |
| `TYPE_GROUP` | recurse (depth-limited) |

#### Depth limit

To prevent unbounded recursion on self-referential schemas, the generator
tracks a recursion depth and stops populating `TYPE_MESSAGE` / `TYPE_GROUP`
fields beyond `max_depth` (default 4), leaving them unset.

#### Well-known types

`google.protobuf.Any`, `Struct`, `Value`, and `ListValue` have dynamic
schemas that the standard reflection API cannot populate generically.  For
v1, the generator leaves these fields unset when encountered as nested
message types (emits a warning).

---

## Output format

### Step 1 — Serialize to binary

```python
binary = message.SerializeToString()
```

### Step 2 — Convert to `#@` prototext

```bash
prototext -d \
    --descriptor <flat_pb_for_root_type> \
    --type <fqdn_without_leading_dot> \
    <binary_input>
```

This produces a file whose first line is `#@ prototext: protoc` followed
by schema-annotated field lines.

### Step 3 — Insert hint comments

The generator inserts the following `#` comment lines immediately after
the magic line, before any field content:

```
#@ prototext: protoc
# ground_truth: .my.pkg.MyMessage
# seed: 42
```

`ground_truth` is the fully-qualified type name with a leading dot (the
canonical FQDN used by the scorer).  `seed` is the integer seed passed on
the CLI.

### Output file

Written to `<output_dir>/<sanitized_type_name>_<seed>.pb` where
`<sanitized_type_name>` replaces `.` with `_` and strips the leading `_`.

---

## CLI

```
proto-gen [OPTIONS] --type FQDN

Options:
  -I, --descriptor-dir PATH   Directory containing .pb descriptor files
                              (required; may be repeated)
  -t, --type FQDN             Fully-qualified message type (e.g.
                              .google.api.HttpRule)
  -s, --seed INT              Random seed (default: 0)
  -O, --output-dir PATH       Output directory (default: .)
  --max-depth INT             Maximum recursion depth (default: 4)
  --max-repeated INT          Maximum repeated field count (default: 3)
  --p-optional FLOAT          Probability of populating an optional field
                              (default: 0.7)
  -q, --quiet                 Suppress warnings
  -h, --help                  Show this message and exit
```

---

## Implementation location

`tools/proto-gen/proto_gen.py` — a standalone Python script, no package
structure required for v1.  Invoked directly or via a thin shell wrapper
committed to `bin/proto-gen`.

---

## Regression test

Add `tests/harness/test_proto_gen.py` that:

1. Invokes `proto-gen -I <wkt_pb_dir> -t .google.protobuf.FileOptions -s 0`
2. Asserts exit code 0.
3. Asserts the output file exists and its first line is `#@ prototext: protoc`.
4. Asserts the second line is `# ground_truth: .google.protobuf.FileOptions`.
5. Asserts `prototext -e` on the output file exits 0 (round-trip encode
   succeeds, confirming the binary is valid).

---

## Open questions

- Whether `proto-gen` should also be usable as a library (importable
  Python module) for use by the harness test runner directly, rather than
  as a subprocess.  Likely yes for v2.
- Handling of `map<K,V>` fields (represented as a repeated synthetic
  message in the FDP): treat as repeated with count 0–`max_repeated` for
  v1.
