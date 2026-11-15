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
  merged only when its gate (S3) passes:
  1. structure: files, messages, fields, enums, services, methods,
     declaration order, layout, imports, package;
  2. built-in options;
  3. custom options and message-valued options;
  4. editions, `--force-proto2-output`, `--force-proto2-for-editions`;
  5. comments and SourceCodeInfo, `--redact-comments`,
     `--redact-orphans`;
  6. `--emit-binary`, `--no-emit-descriptor`, `--no-source-info`.

  Until a stage lands, protoback refuses the options it brings (spec
  0428 S9).
- **S2. The harness, output trees.** `scripts/protoback-harness run
  --compare PATH…` compares, besides the exit status, only the files
  written under the given paths of the run directory (`out/proto`,
  then `out/proto` and the `.pb` files); stdout and stderr are not
  compared until step 9 (N1), as under `--dump-graph` (spec 0428 S10).
  Run once with `--jobs 1` passed to protoback and once without.
- **S3. The gates.** Almost every file has options, so before stage 3
  almost every file differs from reproto's. The early stages live with
  that: their diffs are read, on a sample of cases, and every hunk must
  be one a later stage brings (an option, a comment, a feature), none in
  what the stage ported. From stage 3, the harness restricted as S2
  says must be clean on every case outside what stages 4 to 6 bring;
  after stage 6, on every case.

### Threads

- **S4. A render context per file** (spec 0424 S17) from the first
  stage: phases 1 to 6's state is read-only during rendering, and
  everything reproto keeps on its `Context` for the file being rendered
  lives in a context of its own, one per file. Files render on a rayon
  pool; their outputs are written, and their warnings recorded, in
  reproto's file order (spec 0424 S18).
  - **What a file sees of the pool.** reproto's `ctx.pool` builds a file
    when first asked for it (`DescriptorPool(pool_db)`), and rendering
    asks for each file in turn with its imports (`re_file.py:141`);
    `option_extensions` (`option_messages.py:80`) lists the extensions
    `ctx.pool` holds so far. A file's custom option declared in a file
    it does not import, directly or not, so renders as the extension
    if an earlier file's render loaded that file, and is dropped as an
    unknown field otherwise: the output depends on the render order.
    protoback reproduces it without shared state: before rendering,
    the files loaded before the k-th file renders are computed (the
    import closures of files 1 to k), and the k-th file's render reads
    that set. To confirm first, by a fixture that reaches the case, and
    by a check that no other lookup during rendering loads files.

### Stages

Paths are reproto's (`reproto/src/reproto/`); sizes, the Python ported.

- **S5. Structure** (about 1,300 lines). `_phase7_output`
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
- **S6. Built-in options** (about 650). `ReOptions`, the built-in loop of
  `render_options_from_message` (`option_rendering.py`),
  `format_composite_options`, `Scalar`, `canonize_opt_name`; a field's
  `default`, `json_name`, `packed`. Options come in field number order
  (upb's `ListFields`). Python's formatting (spec 0424 S8): `str` of a
  float (an option's float32 widened to double first; a default parsed
  by `float()`), `json.dumps` with `ensure_ascii`, `render_bytes`, a
  default's bytes through `unicode-escape` then latin-1. Fixtures:
  `default_values_proto2`, `packed_proto2`, `packed_proto3`,
  `json_name`, `message_set_proto2`, `file_comprehensive`.
- **S7. Custom and message-valued options** (about 350). The pool's
  extensions by number, `(.full.name)`, orphaned unless their extension
  field is summoned; aggregates (`ReMessage`, `simple_types.py`), whose
  scalars are Python's `repr` (single quotes, `True`), maps as repeated
  entries in wire order; oneof and extension range options. Fixtures:
  `custom_options_proto3`, `ext_with_options_extender`,
  `ext_no_options_*`, `editions_custom_option_dep`; tests
  `test_aggregate_options`, `test_extension_field_options`,
  `test_oneof_and_range_options`.
- **S8. Editions and `--force-proto2-*`** (about 800). `syntax.py`,
  `feature_resolution.py`, the target syntax per file, the A1 and A2
  header comments, features blocks, synthetic groups for DELIMITED
  fields under `--force-proto2-output` (spec 0428 builds their nodes),
  the proto3 to proto2 anomalies. Fixtures: `editions_rendering` and its
  two `.golden.proto`, `editions_roundtrip`, `editions_resolution`; the
  proto3 fixtures under `--force-proto2-output`.
- **S9. Comments and SourceCodeInfo** (about 300). `source_info.py`: the
  file's and messages' comments only (fields', enums' and services' are
  not rendered); the output SourceCodeInfo, spans counted in code
  points, in lines that survive redaction; `--redact-comments`,
  `--redact-orphans`, applied in `flush`. Fixtures: `comments`,
  `address_book`; tests `test_comments`, `test_source_info`.
- **S10. `--emit-binary`** (about 350). Each node's output descriptor,
  built beside its text: names canonized, package rewritten, syntax and
  edition and features per the target, the children filtered as the
  text is at file level and not within a message. Serialized as upb
  serializes, fields in number order, unknown fields kept (where custom
  options live). Fixtures: the two `editions_rendering.emit_binary.*`
  goldens; test `test_emit_binary`.

### reproto's defects found on the way

reproto stays the reference until protoback is complete: these are not
fixed in step 5. protoback reproduces each, and each is recorded here,
with its evidence, to be fixed in both tools once the port is done.
(S16's fixed messages are the exception, decided apart: they change
reproto first.)

- **S11. Renders repeated.** Under `--emit-binary` and
  `--schema-db-out`, a node's descriptor side-channel renders each
  child again, text included, after the text pass rendered it
  (`re_descriptor.py`, `re_file.py`): a node at depth d renders about
  2^(d+1) times. Only the time and the W4 to W6 counts and immediate
  warnings change with it: the re-renders pass no SourceCodeInfo path,
  so they record no span, and the file's spans are resolved before
  they run. protoback renders each node once, and counts warnings as
  reproto does (step 9).
- **S12. `..` in a type name.** Under `--force-proto2-output`, a
  DELIMITED field's synthetic group gets `type_name =
  f'.{prefix}'.rstrip('.')` (`re_field.py:518`), and the prefix already
  starts with a dot: `..reproto.test.rendering.AllFeatures.DelimitedField`,
  in the golden `editions_rendering.emit_binary.force_proto2_output.golden.pb`.
- **S13. `proto3_optional` under `--force-proto2-output`**, confirmed
  2026-11-15: from `syntax = "proto3"; message M { optional int32 x = 1;
  int32 y = 2; }`, `--emit-binary` writes a proto2 descriptor whose `x`
  keeps `proto3_optional`, which protoc refuses (`The
  [proto3_optional=true] option may only be set on proto3fields`); the
  `.proto` renders `x` in an explicit `oneof _x { int32 x = 1; }`, a
  real oneof once compiled, not the optional field it was.
- **S14. The render order** (S4): a custom option whose extension the
  file does not import renders or not depending on the files rendered
  before it.
- **S15. To confirm, each by a test first:** `ctx.syntax` and
  `ctx.out_sci` carried from one file into the next when a render fails;
  a file's, message's, service's or method's options dropped without a
  warning when the pool does not know the element.

### Text that is Python's

- **S16. Fixed messages for render failures, in both tools.** Today a
  failed render writes a caught exception's text: in the `.proto`
  comment of A3 (file options) and C5 (field options), and in the W4 to
  W6 warning of a file skipped (`_phase7_output`) or an option dropped.
  The text is Python's (`str(KeyError(...))`, quoted) or Python
  protobuf's, and reproto even parses it to classify the failure
  (`_classify_exc`: `couldn't resolve name '…'` is W4, `Depends on file
  '…'` is W5, anything else W6). Reproducing that text in Rust would tie
  protoback to Python's and to Python protobuf's wording, which can
  change in any release.
  - reproto changes first, with a failing test per cause: each failure
    it can name raises its own error, carrying its cause and subject
    (an unresolved type, a missing dependency, an unknown extension
    number, an enum value with no name, …), and the message, the
    comment and the W4 to W6 classification come from that cause, not
    from the text. `_classify_exc` goes.
  - What no named cause covers is a bug: it is written as one fixed
    message naming the element (file, and field or option), the same in
    both tools, so that the harness compares it; the exception is shown
    in full under `--debug` only.
  - **What they catch in practice** (2026-11-15): every exception an
    `except` clause in reproto caught, recorded with `sys.monitoring`
    (`EXCEPTION_HANDLED`), over the 542 harness cases and reproto's
    312 tests. In rendering, two causes only:
    - **The pool refuses the file.** `ctx.pool` cannot build it, and
      A3 (file options) and `render_options` (message, enum, service
      and method options) fail on it, the file rendered without them.
      Reasons met: a name that does not resolve
      (`ext_no_options_extender`, `test_A5_…`), a duplicate symbol
      under `--keep-duplicates` (`duplicates/keep`, `test_P3_…`), a
      proto3 required field (`test_C3_…`), an implicit-presence field
      with a default (`test_C4_…`), a non-canonical map entry, which
      upb reports as `out of memory` (`test_C1_…`).
    - **A synthetic group has no descriptor in the pool**: under
      `--force-proto2-output`, `render_options` looks up the group
      made for a DELIMITED field (spec 0428), which exists only in the
      graph (`KeyError`, the four editions fixtures and three tests).
      Logged, and there is nothing to render: not a failure.
    Never reached: C5 (a field's options), `_phase7_output`'s skipped
    file, `Couldn't find Extension N`, an enum value with no name.
  - The pool's reasons are upb's C text, prefixed by Python protobuf
    (`Couldn't build proto file into descriptor pool: `); upb's text
    changes with its version, and the two tools' upb differ (Python
    protobuf 7.34.1, upb of protobuf 34.1; protoback's, 36.2). The fixed
    message therefore names the cause and the file, not upb's reason,
    which `--debug` shows.
  - **Done in reproto** (2026-11-15, `test_render_failures.py`, each
    test failing before): A3 and C5 take a `reason`, `POOL_REFUSED` or
    `INTERNAL_ERROR` (`anomalies.py`), filed as W6 under it;
    `_classify_exc` and its tests are gone. The file's own pool lookup
    has its own `try` in `render_file_options`, so that a refusal is
    told from a failure in what follows it. A file `_phase7_output`
    skips is W6 `internal error`. `render_options`' failed lookups are
    unchanged: they go to a logger nothing shows, not even `--debug`,
    and reach no output.
  - **Open: C5's cause seen in the wild.** Spec 0041's corpus (not in
    the repository) met `Couldn't find Extension N` rendering a
    `message_set_extension` field's options; neither the harness, the
    tests, nor a MessageSet fixture made for it reaches it. It is
    written `internal error` until a reproducer names its cause.

## Alternatives considered

### Porting rendering in one go

A difference in googleapis' output, found at the end, would have its
cause anywhere in ten modules; a stage's gate finds it in the stage
that made it.

### Single-threaded first, threads later

Spec 0424 S18's check would then come after all six stages, and a context shared
by mistake would show only then, across every stage's code. With a
context per file from the start, each gate checks it.

## Test plan

1. Each stage: S3's gate, `--jobs 1` and all cores.
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
