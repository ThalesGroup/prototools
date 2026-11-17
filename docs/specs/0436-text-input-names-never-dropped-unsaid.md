<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0436 — text input: no name dropped unsaid, no schema mended

Status: implemented
Implemented in: 2026-11-17
App: prototext-upb, reproto, protoback
Refs: docs/specs/0427-text-input-parsed-with-the-inputs-schema.md (the
        reader this fixes: texts parsed with the input's own schema, a
        second parse where the first skipped a name),
      docs/specs/0426-protocs-text-of-unknown-fields-read-back.md (its warnings W1–W3)

## Background

The text format names a field (`internal_flag: 5`), where the binary
format numbers it. A name can be encoded only by a schema that declares
it. Spec 0427 finds each declaration in the input itself:

- **Extensions** (`[pkg.ext]`): every input file goes into one pool,
  imports first (0427 S2), and protoc requires a file using an option to
  import the file declaring it; so when a text is parsed, its declaring
  files are in the pool, where its names are looked up (S6).
- **The schema's own fields** (`internal_flag`, a field of the schema's
  `FileOptions`): the text is parsed as the `FileDescriptorProto` of the
  file the variant names, `descriptor_proto` (S3). But no text imports
  the schema, so import order guarantees nothing for it: a text that
  imports nothing may come first.

Two defects remained, the first in that gap.

- **A skipped name was not said.** The lenient parser skipped a field name
  its message's type does not declare without a warning (W1 to W3 cover
  numbers and extensions). The second parse, which runs only where the
  first warned, never ran for it. Texts are read in import order, and a
  file that imports nothing can be read before the schema is in the
  pool, as the vendored descriptor.proto's: its fields beyond the
  vendored ones were dropped, silently. Measured on a downstream corpus
  of 467 texts read with a variant whose descriptor.proto adds options
  fields: 5 of 220 such option values lost, all in files without
  imports; released reproto 0.3.6, which parsed with Python's stock
  `descriptor_pb2`, lost all of them.
- **A refused schema was mended.** A file upb refuses is mended (what it
  cannot resolve dropped) and loaded again. Mending the schema itself
  gives one without those fields, and every text read as it loses them:
  a descriptor.proto with its `FileDescriptorProto.name` broken made
  every file nameless, and reproto then failed an assertion in phase 5.

## Goals

- **G1.** A field name skipped in a text is warned about, and settled by
  the second parse when the input declares it.
- **G2.** The schema is used as the input has it, or not at all.

## Non-goals

- **N1.** Reading the schema before the other files. G1's second parse
  already settles what an early read skipped; a second mechanism for the
  same decision is what spec 0427 avoided.

## Specification

- **S1. W4.** `Warning::UnknownField { message_type, name }`: "field NAME
  of TYPE is declared nowhere in the input: skipped". Pushed where the
  lenient parser skips an unknown name; one of the warnings that call
  for the second parse (`settles_more`), so that only a name declared
  nowhere is reported.
- **S2. The schema is not mended.** `read_input` adds the variant's
  descriptor.proto as it is: refused, it is reported in `skipped` and the
  texts are read with the vendored schema (`descriptor_type`'s
  fallback). The run then stops where the binary path does, the options
  pool refusing the same file (`DescriptorProtoUnbuildableError`).

## Test plan

1. `prototext-upb/tests/text_format.rs`: a file without imports keeps a
   field of the schema's `FileOptions`, read before or after the schema;
   a name no schema declares gives W4; a refused schema is reported and
   not mended, the text read with the vendored one.
2. reproto's suite, on binary and on text inputs (`via_text`), and
   against protoback: `test_descriptor_proto_refused.py` refuses on all
   three.
3. The harness: both tools identical.

## Measured outcome

- Test plan 1 and 2: pass. On the downstream corpus above, every option
  value of the schema's added fields is kept (220 of 220), with no
  warning.
