<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# docs/mockup — Proto2 / Proto3 / Editions Empirical Test Fixtures

This directory contains `.proto` source files and their compiled `.pb`
descriptor-set files used to empirically verify the claims in
`docs/proto2-proto3-findings.md`.  The compiled descriptors are also intended
as regression test fixtures for the reproto proto3-rendering implementation
(spec `0015-proto3-rendering.md`).

## Compiling

All `.pb` files were produced with:

```
protoc --descriptor_set_out=fNN.pb [--include_imports] -I. <source files>
```

protoc version: **libprotoc 32.1**

To recompile everything from scratch:

```bash
cd docs/mockup
./compile_all.sh
```

(see `compile_all.sh` in this directory)

## Inspection scripts

| Script | Purpose |
|--------|---------|
| `inspect_all.py` | Master script: inspects all `.pb` files and prints every descriptor field relevant to the rendering spec. Produces the raw data behind `docs/proto2-proto3-findings.md` Parts I–XIII. |
| `inspect_openitems.py` | Targeted script for the five originally-open items (Parts XVI–XX): `import weak` in editions, `message_set_wire_format` in editions, `deprecated_legacy_json_field_conflicts`, `visibility` field, extension range end semantics. |
| `test_packed_wire.py` | Wire-level test: encodes a proto3 message with repeated int32 fields and inspects the raw bytes to confirm that `HasField("packed")==False` in proto3 still produces packed wire encoding at runtime. |

Run from inside this directory:

```bash
cd docs/mockup
python3 inspect_all.py 2>&1 | tee inspect_all.log
python3 inspect_openitems.py 2>&1 | tee inspect_openitems.log
```

## Source files

| File(s) | Finds / Part |
|---------|-------------|
| `f01_syntax_proto2_explicit.proto` | Part I — explicit `syntax = "proto2";` |
| `f02_syntax_proto2_implicit.proto` | Part I — no syntax statement (legacy proto2) |
| `f03_syntax_proto3.proto` | Part I — `syntax = "proto3";` |
| `f04_syntax_edition2023.proto` | Part I — `edition = "2023";` |
| `f05_field_labels_proto2.proto` | Part II — optional/required/repeated/oneof in proto2 |
| `f06_field_labels_proto3.proto` | Part II — implicit/optional/repeated in proto3 |
| `f07_packed_proto2.proto` | Part IV — packed combinations in proto2 |
| `f08_packed_proto3.proto` | Part IV — packed combinations in proto3 |
| `f09_json_name.proto` | Part V — auto vs custom json_name |
| `f10_synthetic_oneof.proto` | Part III — proto3 optional / synthetic oneof |
| `f11_default_values_proto2.proto` | Part VI — all default value types |
| `f12_extensions_proto2.proto` | Part VII — file-level and message-level extensions |
| `f13_groups_proto2.proto` | Part VIII — TYPE_GROUP fields |
| `f14_weak_import_proto2.proto` + `_dep.proto` | Part IX — import weak |
| `f15_enums_closed_open.proto` | Part X — proto2 enum (closed) |
| `f15b_enums_proto3.proto` | Part X — proto3 enum (open) |
| `f16_field_options_proto2.proto` | Part XI — ctype/jstype/deprecated/weak in proto2 |
| `f16b_field_options_proto3.proto` | Part XI — same options in proto3 |
| `f17_message_options.proto` | Part XII — MessageOptions in proto2 |
| `f17b_message_options_proto3.proto` | Part XII — MessageOptions in proto3 |
| `f18_extension_range_options.proto` | Part VII — extension range options (RETENTION_SOURCE) |
| `f19_edition2023_features.proto` | Part XIII — edition 2023 FeatureSet |
| `f20_weak_import_editions.proto` + `_dep.proto` | Part XVI — import weak in editions |
| `f21_message_set_editions.proto` | Part XVII — message_set_wire_format in editions |
| `f22_deprecated_legacy_json.proto` | Part XVIII — deprecated_legacy_json_field_conflicts (proto2) |
| `f22b_deprecated_legacy_json_proto3.proto` | Part XVIII — same option in proto3 |
| `f23_extension_range_end.proto` | Part XIX — extension range end value semantics |
| `f24_packed_wire_test.proto` | Part IV (wire) — proto3 packed wire encoding verification |
| `f24_packed_wire_test_pb2.py` | Generated Python bindings for f24 (used by `test_packed_wire.py`) |

## Compiled descriptors

Each `.proto` file (or group) has a matching `.pb` file.  `f14_full.pb` is the
`f14` descriptor compiled with `--include_imports` so that `weak_dependency`
indices are resolvable.  `f20.pb` likewise uses `--include_imports`.
