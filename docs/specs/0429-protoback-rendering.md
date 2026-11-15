<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0429 — protoback, rendering

Status: draft
App: protoback, reproto
Refs: docs/specs/0424-protoback-reproto-in-rust-on-upb.md (step 5 of its
      work plan; S8, Python's formatting; S11, the harness; S17, shared
      state then per-file work; S18, byte-identical whatever the
      threads; S21, timings); docs/specs/0428-protoback-phases-1-to-6-and-the-graph-dump.md
      (the state rendering starts from, identical to reproto's)

## Background

After step 4, protoback's phases 1 to 6 leave the state reproto's leave,
node for node and flag for flag, on every corpus of the harness (spec
0428, measured outcome). Step 5 renders it: the `.proto` files, and with
`--emit-binary` their descriptors, as reproto's `_phase7_output` writes
them. A difference in a rendered file now has its cause in rendering
alone.

reproto's rendering spans ten modules (the `re_*` node classes, three
`option_*` modules, `source_info`, `text`, `scalar`, `syntax`,
`feature_resolution`). Ported in one go, a difference in googleapis'
output would be found among all of them; spec 0424 splits the step into
stages, each with its own gate.

## Goals

- **G1.** protoback writes the `.proto` files reproto writes, byte for
  byte, on every corpus and flag set of the harness.
- **G2.** With `--emit-binary`, the `.pb` files too.
- **G3.** The same bytes with `--jobs 1` and with all cores.
- **G4.** Rendering timed, against reproto's `_phase7_output`.

## Non-goals

- **N1.** Warnings and progress as reproto prints them: step 9. A stage
  compares the files written, not stderr.
- **N2.** The scoring graphs, `--emit-scoring-yaml`, the schema database
  (`--schema-db-out`'s `.desc`), the HTML: step 6. Their outputs are
  left out of the comparison (S2).
- **N3.** Plugins (step 7), `instantiate` (step 8).

## Specification

### The gates

- **S1. Stages.** Rendering is ported in six stages, in this order, each
  merged only when its gate (S2) passes:
  1. structure: files, messages, fields, enums, services, methods,
     declaration order, layout, imports, package;
  2. built-in options;
  3. custom options and message-valued options;
  4. editions, `--force-proto2-output`, `--force-proto2-for-editions`;
  5. comments and SourceCodeInfo, `--redact-comments`,
     `--redact-orphans`;
  6. `--emit-binary`, `--no-emit-descriptor`, `--no-source-info`.

  Until a stage lands, protoback refuses the options it brings (spec
  0428 S9), and the harness skips the cases that need them.
- **S2. The harness, output trees.** `scripts/protoback-harness run
  --compare PATH…` compares, besides the exit status, only the files
  written under the given paths of the run directory (`out/proto`,
  then `out/proto` and the `.pb` files); stdout and stderr are not
  compared until step 9 (N1), as under `--dump-graph` (spec 0428 S10).
  Each stage's gate: the harness, so restricted, clean on every corpus,
  run once with `--jobs 1` passed to protoback and once without.

### Threads

- **S3. A render context per file** (spec 0424 S17) from the first
  stage: phases 1 to 6's state is read-only during rendering, and
  everything reproto keeps on its `Context` for the file being rendered
  lives in a context of its own, one per file. Files render on a rayon
  pool; their outputs are written, and their warnings recorded, in
  reproto's file order (spec 0424 S18).

### Stages

Paths are reproto's (`reproto/src/reproto/`); sizes, the Python ported.

- **S4. Structure** (about 1,300 lines). `_phase7_output`
  (`phases.py`): the summoned present files, in node order; the output
  path (`canonize_dependency`, `--keep-descriptor-path`), `--dry-run`,
  `--debug`'s `Writing:` lines, `buf.yaml`; a file whose render raises a
  caught error (`KeyError`, `ValueError`, `TypeError`,
  `AttributeError`) skipped and filed as W4, W5 or W6. The line model
  (`text.py`: code, comment and orphan lines, `flush`). Each node's text:
  file (header, syntax line, package, imports by `dependency` order,
  orphaned or not, then the stripped ones; services, enums, messages,
  extensions), message (extend blocks, fields in declaration order, a
  oneof at its first member, nested messages and enums, ranges,
  reserved), field, enum, enum value, service, method (wrapped past
  column 79). Names: `short_ref`, `shorten_type_name` (`utils.py`).
  Fixtures: `message_comprehensive`, `field_comprehensive`, `enum`,
  `service`, `name_resolution`, `group_proto2`, `extensions_proto2`,
  `extension_ranges`, `synthetic_oneof`, `weak_import_proto2`, the
  `bridge_*` and `tc*` sets, `address_book`.
- **S5. Built-in options** (about 650). `ReOptions`, the built-in loop of
  `render_options_from_message` (`option_rendering.py`),
  `format_composite_options`, `Scalar`, `canonize_opt_name`; a field's
  `default`, `json_name`, `packed`. Options come in field number order
  (upb's `ListFields`). Python's formatting (spec 0424 S8): `str` of a
  float (an option's float32 widened to double first; a default parsed
  by `float()`), `json.dumps` with `ensure_ascii`, `render_bytes`, a
  default's bytes through `unicode-escape` then latin-1. Fixtures:
  `default_values_proto2`, `packed_proto2`, `packed_proto3`,
  `json_name`, `message_set_proto2`, `file_comprehensive`.
- **S6. Custom and message-valued options** (about 350). The pool's
  extensions by number, `(.full.name)`, orphaned unless their extension
  field is summoned; aggregates (`ReMessage`, `simple_types.py`), whose
  scalars are Python's `repr` (single quotes, `True`), maps as repeated
  entries in wire order; oneof and extension range options. Fixtures:
  `custom_options_proto3`, `ext_with_options_extender`,
  `ext_no_options_*`, `editions_custom_option_dep`; tests
  `test_aggregate_options`, `test_extension_field_options`,
  `test_oneof_and_range_options`.
- **S7. Editions and `--force-proto2-*`** (about 800). `syntax.py`,
  `feature_resolution.py`, the target syntax per file, the A1 and A2
  header comments, features blocks, synthetic groups for DELIMITED
  fields under `--force-proto2-output` (spec 0428 builds their nodes),
  the proto3 to proto2 anomalies. Fixtures: `editions_rendering` and its
  two `.golden.proto`, `editions_roundtrip`, `editions_resolution`; the
  proto3 fixtures under `--force-proto2-output`.
- **S8. Comments and SourceCodeInfo** (about 300). `source_info.py`: the
  file's and messages' comments only (fields', enums' and services' are
  not rendered); the output SourceCodeInfo, spans counted in code
  points, in lines that survive redaction; `--redact-comments`,
  `--redact-orphans`, applied in `flush`. Fixtures: `comments`,
  `address_book`; tests `test_comments`, `test_source_info`.
- **S9. `--emit-binary`** (about 350). Each node's output descriptor,
  built beside its text: names canonized, package rewritten, syntax and
  edition and features per the target, the children filtered as the
  text is at file level and not within a message. Serialized as upb
  serializes, fields in number order, unknown fields kept (where custom
  options live). Fixtures: the two `editions_rendering.emit_binary.*`
  goldens; test `test_emit_binary`.

### reproto's defects found on the way

Each is reproduced by a failing test and fixed in reproto before the
stage that ports it, as in steps 1 and 4; until then the port copies
reproto's behavior.

- **S10. Renders repeated** (before S9). Under `--emit-binary` and
  `--schema-db-out`, a node's descriptor side-channel renders each
  child again, text included, after the text pass rendered it
  (`re_descriptor.py`, `re_file.py`): a node at depth d renders about
  2^(d+1) times, and the W4 to W6 counts and immediate warnings of a
  render multiply with it.
- **S11. `..` in a type name** (before S9). Under
  `--force-proto2-output`, a DELIMITED field's synthetic group gets
  `type_name = f'.{prefix}'.rstrip('.')` (`re_field.py:518`), and the
  prefix already starts with a dot:
  `..reproto.test.rendering.AllFeatures.DelimitedField`, in the golden
  `editions_rendering.emit_binary.force_proto2_output.golden.pb`.
- **S12. To confirm, each by a test first:** `proto3_optional` kept in a
  descriptor down-converted to proto2; `ctx.syntax` and `ctx.out_sci`
  carried from one file into the next when a render fails; a file's,
  message's, service's or method's options dropped without a warning
  when the pool does not know the element.

### Text that is Python's

- **S13. Exception text in the output.** A3 and C5 put a caught
  exception's text in a `.proto` comment and in a W4 to W6 key
  (`anomalies.py`): Python's `str(KeyError(...))`, quoted, and Python
  protobuf's messages. To decide before S4: reproduce those texts, as
  S8 of spec 0424 reproduces formatting, or replace them in reproto with
  texts of its own, which both tools then write (as spec 0428 S6 does
  for the pool's refusals).

## Alternatives considered

### Porting rendering in one go

A difference in googleapis' output, found at the end, would have its
cause anywhere in ten modules; a stage's gate finds it in the stage
that made it.

### Single-threaded first, threads later

S18's check would then come after all six stages, and a context shared
by mistake would show only then, across every stage's code. With a
context per file from the start, each gate checks it.

## Test plan

1. Each stage: S2's gate, `--jobs 1` and all cores.
2. reproto's CLI tests that check rendered output, run against
   protoback through `REPROTO_CMD` (spec 0424 S2), each from the stage
   it needs: `test_roundtrip`, `test_aggregate_options`,
   `test_extension_field_options`, `test_oneof_and_range_options`,
   `test_editions_rendering`, `test_comments`, `test_source_info`,
   `test_emit_binary`. Of the 27 test files (of 38) that call the CLI,
   those of scoring, the schema database and plugins wait for steps 6
   and 7.
3. Rendering timed on googleapis, protoback against reproto
   (`scripts/bench-protoback`, `scripts/reproto-phase-times`).

## Measured outcome

Filled in stage by stage.
