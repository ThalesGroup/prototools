<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0063 — reproto-instantiate-schema CLI

**Status:** implemented
**Implemented in:** 2026-05-13
**App:** reproto

---

## Background

`prototext instantiate-schema` generates pseudo-random protobuf instances from a
schema descriptor and is used by the stress-test harness to produce `.pb` files
for `prototext list-schemas` to score.

The current implementation uses `prost_reflect::DynamicMessage::encode_to_vec()`
to serialize generated instances.  Due to a prost-reflect bug (see
`docs/PROST-ISSUES.md §1`), repeated scalar fields in proto3 messages whose
`FieldDescriptorProto.options` is present-but-empty (e.g. because an unrelated
custom option such as `google.api.field_behavior` appears on another field in
the same message) are encoded as non-packed individual VARINTs rather than as a
packed LEN record.

The real corpus `.pb` files — compiled with `protoc`, which correctly applies
the proto3 packed default — use packed encoding for such fields.  The scoring
graph therefore records them as `LEN_PACKED`.  When the scorer sees wire_type=0
it vetoes the correct FQDN, causing false negatives in `test_auto_infer`.

The correct fix is to move instance generation to reproto, which uses Python's
`google.protobuf` library.  `google.protobuf` correctly applies proto3 packing
defaults regardless of whether `options {}` is empty or absent, producing
wire-compatible instances that match the corpus encoding.

---

## Goals

1. Add `reproto-instantiate-schema` as a first-class CLI entry point, separate
   from the `reproto` reconstruction command.
2. The command reads a `.desc` file (FileDescriptorSet) and generates one
   binary `.pb` instance per requested FQDN, writing them under an output
   directory using the same path convention as `prototext instantiate-schema`
   (`<fqdn-as-path>.pb`, dots replaced by `/`).
3. Update the stress-test harness (`tests/stress/test_stress.py`) to call
   `reproto-instantiate-schema` instead of `prototext instantiate-schema`.
4. Remove `prototext instantiate-schema` from the prototext CLI (or retain it
   as a deprecated alias — see Non-goals).

---

## Non-goals

- Removing `prototext instantiate-schema` immediately; it may remain as a
  deprecated command for backward compatibility until the stress tests are
  confirmed green.
- Changing the scoring, listing, or decoding commands.
- Changing the `.desc` / `hopcroft.rkyv` DB format.

---

## Specification

### §1 — CLI shape

`reproto-instantiate-schema` is a standalone Click command, implemented in
`reproto/src/reproto/instantiate_cli.py` and registered as a separate entry
point in `pyproject.toml`.  It shares `reproto/src/reproto/instantiate.py`
with any future callers.

```
reproto-instantiate-schema --descriptor DESC [OPTIONS] FQDN [FQDN ...]
```

#### Required option

| Option | Type | Description |
|---|---|---|
| `--descriptor FILE` | file path | Path to the `.desc` FileDescriptorSet to load. |

#### Output option

| Option | Type | Default | Description |
|---|---|---|---|
| `-O, --output-root DIR` | directory | `.` (cwd) | Root directory under which output files are written. Created if absent. |

#### Generation options

| Option | Type | Default | Description |
|---|---|---|---|
| `--seed INT` | integer | `0` | Integer seed passed to the PRNG. Combined with the FQDN to derive a per-type seed via `SHA-256("<seed>:<.fqdn>")`. |
| `--max-depth INT` | integer | `4` | Maximum recursion depth for nested messages. |
| `--max-repeated INT` | integer | `3` | Maximum number of elements for repeated fields. |
| `--p-optional FLOAT` | float 0–1 | `0.7` | Probability of populating an optional field. |

#### Other options

| Option | Description |
|---|---|
| `-q, --quiet` | Suppress per-file progress messages. |

### §2 — Output file naming

For each requested FQDN the output file is:

```
<output-root>/<fqdn-with-dots-as-slashes>.pb
```

Example: FQDN `google.analytics.data.v1alpha.CreateReportTaskRequest` →
`<output-root>/google/analytics/data/v1alpha/CreateReportTaskRequest.pb`.

Parent directories are created automatically.

### §3 — Instance generation

The generator uses `google.protobuf.descriptor_pool.DescriptorPool` loaded from
the `.desc` bytes and `google.protobuf.message_factory` (or
`google.protobuf.reflection`) to obtain a message class for each FQDN.

Pseudo-random generation follows the same algorithm as the existing Rust
implementation:

1. Derive per-type seed: `SHA-256("<seed>:<.fqdn>")` → bytes → seed a PRNG
   (Python `random.Random` seeded from the hash bytes, or equivalent).
2. For each field in the message descriptor:
   - **Required / singular**: populate with a random value of the appropriate
     type (respecting `max_depth` for nested messages).
   - **Repeated**: generate `randint(0, max_repeated)` values.
   - **Optional** (proto3 `optional` or proto2 `optional`): populate with
     probability `p_optional`.
   - **Oneof**: pick one field uniformly at random with probability `p_optional`
     for the whole oneof.
3. Serialize using `message.SerializeToString()`, which correctly applies proto3
   packing defaults.

The output file contains the raw binary wire bytes (not prototext text).

### §4 — Seed derivation compatibility

The seed derivation must match the existing Rust implementation exactly so that
the same `--seed` value produces the same structural choices for a given FQDN:

```
seed_input = f"{seed}:{fqdn_with_leading_dot}"
hash_bytes = SHA256(seed_input.encode())
```

The PRNG is seeded from `hash_bytes`.  Rust uses `rand::rngs::StdRng` (ChaCha12
via `from_seed`); Python must use an equivalent deterministic PRNG seeded from
the same bytes.  Since exact byte-for-byte reproducibility across languages is
not required (the tests only check that the correct FQDN ranks at the top, not
the exact content), using Python's `random.Random(int.from_bytes(hash_bytes))` is
acceptable.

### §5 — Stress-test harness update

In `tests/stress/test_stress.py`, replace the `all_instances` fixture call from:

```python
subprocess.run([
    "prototext", "--descriptor", str(schema_db),
    "-O", str(inst_dir),
    "instantiate-schema", *fqdns,
], ...)
```

to:

```python
subprocess.run([
    "reproto-instantiate-schema",
    "--descriptor", str(schema_db),
    "-O", str(inst_dir),
    *fqdns,
], ...)
```

### §6 — reproto CLI: no changes

The existing `reproto` reconstruction command is untouched.

---

## Files changed

| File | Change |
|---|---|
| `reproto/src/reproto/instantiate_cli.py` | New: Click entry point for `reproto-instantiate-schema` |
| `reproto/src/reproto/instantiate.py` | New: instance generator using `google.protobuf` |
| `reproto/pyproject.toml` | Add `reproto-instantiate-schema` entry point |
| `bin/reproto-instantiate-schema` | New: dev-shell wrapper |
| `tests/stress/test_stress.py` | Call `reproto-instantiate-schema` instead of `prototext instantiate-schema` |
