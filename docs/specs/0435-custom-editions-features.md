<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0435 — custom editions features

Status: implemented
Implemented in: 2026-11-16
App: reproto, protoback
Refs: docs/specs/0424-protoback-reproto-in-rust-on-upb.md (protoback
        reproduces reproto's output; both tools change together),
      docs/specs/0434-type-names-as-protoc-resolves-them.md (N3: this
        defect, left to this spec)

## Background

An editions file can set *custom* features: extensions of `FeatureSet`,
such as `features.(pb.cpp).string_type = VIEW` or
`features.(pb.java).nest_in_file_class = YES`. Both tools dropped them,
at every level (file, message, field, enum), while writing the standard
ones (`features.field_presence`) and keeping the import that declares
the extension, which protoc then reports as unused.

Two gaps added up. `render_features_block` (reproto `syntax.py`,
protoback `render/file.rs`'s `features_block`) wrote the six standard
features only; and the generic option renderer skips `features`, a
built-in orphan of every options message (`variant.py`), so it never
reached the feature set's extensions either. The feature set itself is
read from the FDP with the stock `descriptor_pb2`, where those
extensions are unknown fields.

## Goals

- **G1.** Every custom feature is written, at every level that can carry
  one; compiling the output gives every element's options back, byte for
  byte.
- **G2.** Written by the renderer that writes custom options
  (`ReExtensions`; protoback `options.rs`), not a second one.

## Non-goals

- **N1.** The spelling of a message-valued custom option. A custom feature
  is written as any message-valued custom option is, an aggregate:
  `features.(.pb.cpp) = { string_type: VIEW }`, not the dotted
  `features.(pb.cpp).string_type = VIEW` hand-written files use. Both
  compile to the same bytes; changing the spelling is a change for every
  custom option, not for features alone.
- **N2.** Features under `--force-proto2-output`: a proto2 file has none,
  standard or custom, as before.

## Specification

- **S1. `FeatureSet` as an options message.** Its class is built from the
  options pool beside the `*Options` classes (`ctx.fts_desc`,
  `ctx.fts_cls`; protoback `Options::FeatureSet`), absent when the
  variant's descriptor.proto predates editions.
- **S2. The custom features.** After the standard ones, an element's
  feature set is parsed again with that class, where its extensions are
  known (`parse_options`), and its extensions rendered by `ReExtensions`
  with the prefix `features.`: `option features.(.x.y) = …;` standalone,
  `features.(.x.y) = …,` among a field's options. protoback splits its
  options renderer the same way: `extensions()` beside `options()`, one
  rendering loop for both.
- **S3. A value the pool cannot name** (an enum number without a value)
  fails reproto's render where it is raised; protoback drops that
  feature set's custom features instead. protoc rejects such a value, so
  no compiled input carries one.

## Alternatives considered

- **A features renderer of its own for extensions**, writing the dotted
  form. A second renderer of custom option values, with its own handling
  of repeated, nested and enum values, for a spelling N1 leaves to a
  change of all custom options.
- **Dropping `features` from the built-in orphans**, so the generic
  renderer writes the whole feature set: the standard features would then
  be written twice, and as an aggregate.

## Test plan

1. `tests/test_custom_features.py`, against both tools: an edition 2023
   file (file and field features, one beside a standard feature) and an
   edition 2024 file (message and enum features); recompiled, every
   element's options equal the original's byte for byte.
2. The same file, a variant rooted on a descriptor.proto of another
   package with its names kept (package `proto2`, as for a protoc
   patched to use such a root): custom features extend that package's
   `FeatureSet` and are written. Built from protoc's own descriptor.proto
   and cpp_features.proto, moved; stock protoc cannot compile the output,
   so its text is checked.
3. The harness: both tools identical.

## Measured outcome

- Test plan 1 and 2: 3 tests, failed before the change (no custom
  feature was written), pass on both tools. reproto's suite: 335 passed.
- Test plan 3: 1,668 cases, identical.
