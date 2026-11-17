<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0429 — protoback, rendering

Status: implemented
Implemented in: 2026-11-16
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
  Once files render in parallel (S4), run once with `--jobs 1` passed to
  protoback and once without.
- **S3. The gates.** Almost every file has options, so before stage 3
  almost every file differs from reproto's. The early stages live with
  that: their diffs are read, on a sample of cases, and every hunk must
  be one a later stage brings (an option, a comment, a feature), none in
  what the stage ported. From stage 3, the harness restricted as S2
  says must be clean on every case outside what stages 4 to 6 bring;
  after stage 6, on every case.

### Threads

- **S4. A render context per file** (spec 0424 S17) from the first
  stage: phases 1 to 6's state is read-only during rendering (`Shared`),
  and everything reproto keeps on its `Context` for the file being
  rendered lives in a context of its own, one per file (`Ctx`). The six
  stages render on one thread; once rendering is complete, files render
  on a rayon pool, their outputs written, and their warnings recorded, in
  reproto's file order (spec 0424 S18), with `--jobs`, and the harness
  checks `--jobs 1` against all cores then.
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
    that set. Confirmed 2026-11-15 by the harness's `order/` cases (a
    file setting an extension its import of was dropped, rendered after
    and before the extension's file): the option is written, then
    dropped, by reproto and protoback alike. protoback computes, before
    rendering, each loaded file's rank (the first summoned file whose
    render loaded it); while the k-th renders, an extension counts only
    if its file's rank is at most k (`render/options.rs`, `PoolLoads`,
    `Scope`).

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
  *Fixed in reproto 2026-11-17* (protoback already reported once): a
  container renders its children's text with `ctx.out_desc` unset, so
  each child renders and reports once there; the descriptor's re-renders
  are muted (`descriptor_rerender_muted`, `lib.warnings.muted`), their
  reports given by the text's. reproto still renders a node for the text,
  then once more per enclosing element, the file included (about d + 2
  times, no longer 2^(d+1)); only the time shows it. Measured before: a B1 two messages deep
  printed 8 times under `-b`. Test `test_descriptor_rerender.py`.
- **S12. `..` in a type name.** Under `--force-proto2-output`, a
  DELIMITED field's synthetic group gets `type_name =
  f'.{prefix}'.rstrip('.')` (`re_field.py:518`), and the prefix already
  starts with a dot: `..reproto.test.rendering.AllFeatures.DelimitedField`,
  in the golden `editions_rendering.emit_binary.force_proto2_output.golden.pb`.
  *Fixed in both tools 2026-11-17:* one leading dot; the golden corrected
  is the regression test.
- **S13. `proto3_optional` under `--force-proto2-output`**, confirmed
  2026-11-15: from `syntax = "proto3"; message M { optional int32 x = 1;
  int32 y = 2; }`, `--emit-binary` writes a proto2 descriptor whose `x`
  keeps `proto3_optional`, which protoc refuses (`The
  [proto3_optional=true] option may only be set on proto3fields`); the
  `.proto` renders `x` in an explicit `oneof _x { int32 x = 1; }`, a
  real oneof once compiled, not the optional field it was.
  *Fixed in both tools 2026-11-17:* a oneof is synthetic by protoc's rule
  (`OneofDescriptor::is_synthetic`: one field, with `proto3_optional`)
  whatever the target syntax. Written as anything but proto3, the field is
  rendered `optional` outside any oneof, its descriptor loses
  `proto3_optional` and `oneof_index`, its message the synthetic oneofs,
  the real ones renumbered. Test `test_proto3_optional_as_proto2.py`: the
  emitted message equals the one protoc compiles from the text.
- **S14. The render order** (S4): a custom option whose extension the
  file does not import renders or not depending on the files rendered
  before it.
- **S15. To confirm, each by a test first:** `ctx.syntax` and
  `ctx.out_sci` carried from one file into the next when a render fails;
  a file's, message's, service's or method's options dropped without a
  warning when the pool does not know the element.
- **S15b. `short_ref`'s collision test misses** (read 2026-11-15,
  `utils.py`): it looks a shortened candidate up under the reference's
  own FQDN prefix, so a message never collides with an enum of the same
  name, and an extendee, looked up as `message:…` (a prefix no node has,
  `_render_extend_blocks`), never collides at all. protoback reproduces
  both (`render/names.rs`).

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
  - **C5's cause seen in the wild, not met since.** Spec 0041's corpus
    (not in the repository) met `Couldn't find Extension N` rendering a
    `message_set_extension` field's options, with an older reproto.
    Neither the harness, the tests, nor a MessageSet fixture made for it
    reaches it, and a large proprietary corpus, processed as its own
    build processes it (its variant, load-time plugin and prunes),
    renders with no render failure at all (2026-11-15). It is written
    `internal error` until a reproducer names its cause.

### The variant's renaming, after the port

- **S17. One renaming table, after the port.** reproto applies a
  variant's `namespace_rewrites` and `import_rewrites` (`mappings.py`) as
  it writes each name, at a dozen call sites (`re_file.py`,
  `re_field.py`, `re_method.py`, `utils.py`'s `short_ref` and
  `shorten_type_name`, `phases.py`, `syntax.py`); the input is never
  rewritten. Recorded, like S11–S15, to change once protoback is
  complete, in protoback first; until then protoback writes reproto's
  names, byte for byte:
  - each call site repeats the `keep_variant_descriptor` guard (one
    leaves it to its callers), so the text and the descriptor can drift;
  - a rewritten type is never shortened: `short_ref` returns the full
    rewritten name before scope shortening, its existence checks being
    in the input's names;
  - two input types rewritten to one name (a private package's `Empty`
    and `google.protobuf.Empty`, under a rule from that package to
    `google.protobuf`) pass without a word in the `.proto` output;
  - every reference is rewritten again, at each of S11's repeated
    renders (not profiled).

  The change: one table from each input name (type, package, file) to
  its output name, built once after phase 3 with the rules and the
  `keep_variant_descriptor` exception; collisions reported when it is
  built; rendering looks names up; shortening done in the output's
  names. protoback may build the table during the port if its output
  stays reproto's (to be confirmed by the harness).

### The options types, from the variant's descriptor.proto

- **S18. No options package assumed** (done in reproto, 2026-11-15,
  before stage 1, as S16). The package of the options types, and of the
  nine `*Options` a proto3 file may extend, was guessed by reproto in
  three places: `option_messages.py` (`google.protobuf` for a
  `descriptor_proto` path under `google/`, a fixed second package
  otherwise), `mappings.py` (two fixed patterns recognizing built-in
  option names), `syntax.py` (A5's list, the same two packages written
  out). A variant whose `descriptor.proto` declared any other package
  stopped reproto at startup. Now:
  - the options types are those of the variant's `descriptor.proto`, in
    the package that file declares (`ctx.options_package`), and built-in
    option names are recognized in that package;
  - A5 accepts the nine `*Options` of `google.protobuf` and of every
    `descriptor.proto` in the input (a file declaring
    `FileDescriptorProto`), each also under the variant's rewritten
    name; and an extendee the input does not define when its name ends
    in one of the nine, whatever its package (input compiled against a
    `descriptor.proto` the input lacks). A message the input does define
    and is not one of them is refused, as before.
  - Tests: `test_variant_descriptor_package.py` (a variant's
    `descriptor.proto` in a package of its own), failing before;
    `test_A5_proto2_options_extendee_not_omitted` still passing. Every
    harness case's output unchanged, reproto before against after.

## Alternatives considered

### Porting rendering in one go

A difference in googleapis' output, found at the end, would have its
cause anywhere in ten modules; a stage's gate finds it in the stage
that made it.

### Threads from the first stage

Chosen first, then dropped (2026-11-15): each gate would have checked
that no state is shared by mistake. But every gate's criterion is the
output's bytes, which threads cannot improve, and their own check means
more on a complete output; the cost of a mistake is kept low instead by
the split S4 makes from the start (`Shared` never written while
rendering, `Ctx` per file), which turns the file loop parallel in one
place once rendering is complete. What a file sees of the pool (S4) is
not deferred with it: stage 3 needs it on one thread already.

## Test plan

1. Each stage: S3's gate; once files render in parallel, `--jobs 1`
   and all cores.
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

- **Stage 1, first cut** (2026-11-15): `protoback/src/render/` (the line
  model, `short_ref`, files, messages, fields, oneofs, maps, groups,
  enums, services, methods, ranges, extend blocks, the structure
  anomalies A2, A4, A5, B1–B3, C1–C4; phase 7's paths, `buf.yaml`,
  `--dry-run`, `--no-emit-descriptor`, the redactions); options, source
  comments, features blocks and binary output not yet. The harness with
  `--compare out/proto`, every corpus: 542 cases; 223 refused by
  protoback, their flag sets needing later stages (`--emit-binary`,
  `--schema-db-out`, the `--force-proto2-*`); of the 319 it renders, 232
  identical byte for byte, and the other 87 identical once what later
  stages bring is taken out of reproto's files (option and features
  lines, inline and multi-line `[…]` options, A3), but for one message
  of `name_resolution` (in its three cases) whose only body is an option,
  which reproto therefore does not collapse to `{}` (S3's review: every
  remaining hunk is a later stage's).
  - Read in reproto while porting, where the map S5 is built from was
    wrong: a blank orphan line two levels deep keeps trailing spaces
    (`/// `); `json.dumps` escapes DEL (`\u007f`); a message's
    trailing-blank trim keeps one line; whether a oneof is synthetic, and
    whether a field is a group, read resolved features in an editions
    file, so `field_presence` and `message_encoding` resolve in stage 1
    (`features.rs`, shared with phase 3).
  - Not yet: files rendered in parallel (after stage 6, S4).
- **Stage 2, built-in options** (2026-11-15): `render/options.rs`. Each
  element's options parsed with the variant's descriptor.proto, as
  reproto's options pool parses them, and written as its `dump_option`
  writes them: standalone or composite, orphans from the variant, a
  oneof's under their full name (`canonize_opt_name`'s pattern lacks
  `OneofOptions`), message values as aggregates (`edition_defaults`);
  a field's `default`, `json_name`, `packed`; methods' brace form;
  extension ranges' three forms. Python's formatting from tables Python
  generated (`render/pyfmt/`): `repr` of floats (337 values, float32
  ones widened) and of strings (`str.isprintable`'s 712 ranges, from
  Unicode 15.1), the bytes default's `unicode-escape` and its failures.
  reproto's `ctx.pool`, which refuses some files (S16), computed once in
  render order before rendering (`refused_files`, a upb pool loading
  each file after its imports): a refused file's options are A3, its
  elements' are dropped, and an enum value in it fails the file, as in
  reproto.
  - The harness, `--compare out/proto`, every corpus: of the 319 cases
    protoback renders, 290 identical; the 29 others differ only in files
    holding custom options or features (stages 3, 4), every one of the 78
    differing hunks touching one. Every rendered descriptor.proto
    identical.
- **Stage 3, custom and message-valued options** (2026-11-16): after the
  built-in ones, an element's custom options by number, `(.full.name)`,
  orphans unless their extension is summoned; known only when S4's
  ranks say reproto's options pool has their file, at the top level and
  in aggregates alike; the options pool holding each loaded file with
  `map_entry` cleared (`_MapsAsRepeatedDatabase`), so a map in an option
  is its entries in wire order. The harness's `order/` cases added (S4).
  - `--compare out/proto`, every corpus: 544 cases, 223 refused; of the
    321 rendered, 305 identical; the 16 others differ only in files with
    features (stage 4), every differing hunk touching one.
- **Stage 4, editions and `--force-proto2-*`** (2026-11-16): the target
  syntax per file from the two flags (`_set_target_syntax`), A1; the
  features blocks, an editions target's only, standalone or inline, the
  six runtime features in reproto's order, named by the edition
  defaults' tables; an editions file written as proto2: labels from
  resolved presence, `packed` from the resolved encoding, and, under
  `--force-proto2-output`, a DELIMITED field as `group Name { … }` from
  its synthetic group, which reproto's pool does not have: no options
  for it or what it encloses, and an enum value under it fails the file
  (phase 3's graph records the group, `synthetic_group`,
  `editions_group`).
  - `--compare out/proto`, every corpus: 544 cases, 467 identical; the
    77 others refused, their flag sets needing `--emit-binary` or
    `--schema-db-out` (stage 6, step 6 of spec 0424). Every
    `--force-proto2-output` and `--force-proto2-for-editions` case
    identical.
- **Stage 5, comments** (2026-11-16): `render/comments.rs`, the file's
  comments (the locations of path `[]` and `[12]`, each comment followed
  by a blank line) and each message's (its own path, `[4, i]` then
  `[3, j]` steps found by name, its definition's: a synthetic group gets
  its type's), as reproto renders them; no other element's. The output
  SourceCodeInfo moves to stage 6: it exists only in the `.pb`, which
  only `--emit-binary` writes, so only that stage can check it.
  - The harness's `sourceinfo/` corpus: every fixture with its imports
    and their source info, each also under `--redact-comments
    --redact-orphans`. `--compare out/proto`, every corpus: 690 cases,
    613 identical; the 77 others refused (stage 6, step 6).
- **Stage 6, `--emit-binary`** (2026-11-16): `render/binary.rs`, each
  file's descriptor as reproto's second pass builds it, built once (S11:
  the output is reproto's, the repeated renders are not): the definition
  copied, names canonized and renamed, syntax, edition and features made
  the target's, the children the second pass keeps rebuilt in (at file
  level the visible, reachable, summoned ones; within a message all, but
  a malformed map), a field's label as the second pass computes it
  (`is_oneof` false), its packing, its default, a synthetic group's
  type name (`..` until S12 was fixed). The decisions it shares with the text, map
  detection and packing, made once for both (`render/message.rs`). The
  SourceCodeInfo synthesized from the text (`render/sci.rs`): each
  element's lines marked with its path, by reproto's counters, then
  resolved against the lines `flush` writes; reproto matches markers to
  lines by identity, protoback's lines carry ids. `--no-source-info`.
  `prototext-upb` gains `MessageMut::merge_from`, protobuf's
  `MergeFromString`, which appends a child's descriptor as reproto does.
  - The harness's `emit-binary` cases: every fixture with its imports,
    also under `--force-proto2-output`; every `sourceinfo/` fixture,
    also redacted and without source info. `--compare out/proto`, every
    corpus: 1,055 cases, 978 identical, `.pb` files included; the 77
    others are `--schema-db-out`'s (step 6 of spec 0424), refused.
  - Rendering complete.
- **Files in parallel, S4 and G3** (2026-11-16): `render_all`, a rayon
  pool of `--jobs` threads (protoback's own option; all cores by
  default), each file rendered with its own context over `Shared`, which
  needed no change to be shared: `par_iter().map().collect()` keeps the
  plan's order, and the files are written from it, in reproto's order,
  directories first. googleapis under `--emit-binary`: the 15,543 files
  identical, byte for byte, on one thread and on 12.
  - The first parallel run was slower than one thread: `Shared::def`
    found a node's definition by building every sibling's reader at each
    step of its path (`message_type()` and the like), quadratic, and on
    12 threads mostly allocation (45.8 s of CPU for 12 s of wall). Each
    step now reads the one child it names: 19.9 s of CPU, 8.6 s of wall
    (unpinned, a quick build; G4 measures).
- **G4, the timings** (2026-11-16, at 233b27c): `BENCH_RENDER=1
  scripts/bench-protoback`, phases 1 to 7, each case with the flags of
  its database but the schema database's (`--schema-db-out`,
  `--emit-scoring-yaml`, `--emit-extension-ranges`: step 6), googleapis
  with `--emit-binary`; vCPUs 0–7, host idle; reproto `python3 -m
  reproto.cli`, protoback a release build, on the 8 vCPUs and on one
  thread (`--jobs 1`):

  | Case | reproto | protoback, 8 threads | protoback, `--jobs 1` | Peak memory, reproto / 8 threads / 1 |
  |---|---|---|---|---|
  | WKT | 351 ms ± 8, 8 runs | 7.9 ms ± 0.7, 185 runs | 7.8 ms ± 0.8, 169 runs | 45 / 13 / 13 MB |
  | custom | 393 ms ± 11, 7 runs | 13.1 ms ± 0.9, 133 runs | 13.2 ms ± 0.6, 126 runs | 46 / 13 / 13 MB |
  | googleapis | 122 s ± 1.4 (123.8, 122.4, 120.9) | 5.54 s ± 0.07 (5.62, 5.49, 5.51) | 6.11 s ± 0.13 (5.97, 6.13, 6.23) | 811 / 639 / 610 MB |

  googleapis 22 times faster (20 on one thread); rendering is most of
  reproto's run (phases 1 to 6, 12.6 s, spec 0428). The threads gain
  0.6 s for 14 s more CPU (19.4 s user, against 5.4 s): unexplained, not
  pursued in step 5. Leads: what of the 3.9 s past phases 1 to 6 is
  sequential (the pool loads, the options pool, writing the files), and
  what in rendering contends (the allocator).
