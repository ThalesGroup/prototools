<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0427 — text input parsed with the input's own schema

Status: implemented
Implemented in: 2026-11-14
App: prototext-upb, prototext-pyo3, reproto, protoback
Refs: docs/specs/0424-protoback-reproto-in-rust-on-upb.md (S5: the
      text-format parser this gives a schema to);
      docs/specs/0426-protocs-text-of-unknown-fields-read-back.md (its
      rules for fields by number, applied here while parsing instead of
      after it); docs/specs/0018-variant-annotation-modules.md (the
      variant key this replaces)

## Background

A custom option in a descriptor set's text appears in one of two forms,
depending on what protoc was given to print it:

- **by number**, `50002 { 1: "me" }`, when protoc had `descriptor.proto`
  alone;
- **by name**, `[txt.info] { owner: "me" }`, when protoc also had the file
  declaring the option.

A name can be parsed only with the declaration in hand: it gives the
extension's number, and the field names inside its value. Python's
`text_format` looked names up in Python's default pool, which knew only
what the variant's `annotation_modules` imported (spec 0018). Step 3 of
spec 0424 removed those modules and parses with `descriptor.proto` alone,
so names are skipped, silently: a regression.

Measured on a synthetic variant, with old reproto (spec 0018's modules)
and step 3's:

| input | old, with the module | old, without | step 3 |
|---|---|---|---|
| binary, declaring file in the input | 8 options | 8 | 8 |
| text by number | 0 | 0 | 8 |
| text by name, declaring file in the input | 8 | 0 | **0** |
| text by name, declaring file missing | 0 | 0 | 0 |

The module helped only where the declaring file was in the input. The
last row is the same everywhere: reproto strips an import it cannot find,
and the options that import declared with it.

## Goals

- **G1.** An option by name, declared in a file of the input, is read
  as the binary set has it.
- **G2.** One parse decides every value's framing, by name or by number,
  from the declarations, instead of a parse without them followed by a
  correction of its bytes.
- **G3.** No variant key, Python module or other side channel: the
  declarations come from the input.

## Non-goals

- **N1.** A declaring file outside the input is not looked for. It is
  provided as any dependency is: on the command line or under `-I`. Old
  reproto did no better (Background, last row).
- **N2.** No `extension_files` key in the variant, listing declaring
  files to load: N1's command line does the same, and the file then also
  goes to the output, where the output's `import` needs it.

## Specification

- **S1. Pass 1**, as each file is loaded: a text file is parsed with the
  vendored `descriptor.proto` alone (spec 0424 S5). Its bytes serve only
  to learn the file's imports and declarations, which are known fields
  of `descriptor.proto`; its options are discarded with them. Its text is
  kept for S3.
- **S2. The input's pool.** Once every file of the input is loaded, one
  upb pool is built from all of them, imports first: binary files as
  they are, text files from their pass-1 bytes. A file upb refuses (an
  import the input lacks, a type nobody declares) is retried mended,
  imports first: the records that do not resolve removed (imports not in
  the pool, fields, extensions and methods whose types are not), the
  rest, options and features included, copied byte for byte. Without
  this, the declarations of its own options are lost, and its strings
  and groups mis-framed. What still fails is reported, and its
  declarations are not in the pool. The mended file serves the pool
  only: reproto mends, and reports, for itself (phase 2).
- **S3. Pass 2:** each text file is parsed again, as the
  `FileDescriptorProto` of the input's own `descriptor.proto`, the file
  the variant names (`descriptor_proto`), so that the options' types are
  those its extensions extend (`proto2.MessageOptions` in a variant built
  on `net/proto2/proto/descriptor.proto`). An input without that file
  gets the vendored one. The pass-2 bytes are the file's contents from
  then on.
- **S4. Names**, `[pkg.ext]`, are resolved in the input's pool. A name
  the pool does not declare is skipped, with warning W3: "extension
  `pkg.ext` is declared nowhere in the input: skipped".
- **S5. Fields by number** are framed while parsing, by the declaration
  of the field or extension of that number in the input's pool, with
  spec 0426's table:

  | declared as | framed |
  |---|---|
  | a message, length-delimited | length-delimited, its content parsed as that message |
  | a message, delimited | a group, its content parsed as that message |
  | a string or bytes | length-delimited, its content as protoc's text gives it, groups kept (0426 S3, W2 when not empty) |
  | nowhere | length-delimited, its content treated alike; W1 |

  A scalar printed as `N: value` keeps its wire type (0426 S1).
- **S6. Warnings** W1–W3 name the file, and are printed once per value.
  Pass 1 prints none.
- **S7.** The correction after the fact (0426 S2, `delimited::correct`)
  and the declaration table it reads (`Declarations`) are removed: S5
  does their work with the pool's declarations.

## Alternatives considered

### Keep `annotation_modules`

Python modules: protoback cannot import them. They also duplicate a
declaration the input holds already, wherever they helped (Background).

### Resolve names only, and keep the correction for numbers

Two mechanisms deciding the same question, a value's framing, from the
same declarations, one of them by rewriting bytes. One parse with the
schema is shorter and has a single rule.

### Pass 2 only for files that skipped a name

Saves the second parse of a text file that has no name, but keeps the
correction for those files' numbers: the alternative above.

## Test plan

1. reproto `test_text_input_options.py`: options by number, declared in
   an import (exists); a group option declared in its own file, with
   one W2 (exists); an option by number declared nowhere, W1 (exists);
   options by name, declared in the input
   (`test_options_printed_by_name_survive_the_text_form`, failing today).
2. reproto: an option by name whose declaring file is missing: W3, and
   the file rendered; a file whose import is missing, with options it
   declares itself, as from the binary set; text that does not parse,
   reported, the other files rendered.
3. reproto's whole suite, every binary input given as protoc's text form
   (`reproto.tests.via_text`, `reproto-tests-via-text` in CI): every test
   passes as on the binary sets.
4. `prototext-upb`: the fixtures' texts by number and by name parsed in
   two passes give back their sets; a `descriptor.proto` under another
   package (`proto2`) with an option extending its `MessageOptions`; one
   without the descriptor types, which falls back to the vendored ones;
   a file whose import is missing, mended.
5. googleapis printed by `protoc --decode` with `descriptor.proto`
   alone, read in two passes: every file byte for byte, 19 W2 as with
   spec 0426; the time of the two passes.

## Measured outcome

2026-11-14, `prototext-upb` (`text_format::load_input`,
`descriptor_type`, `parse_reporting`), `prototext_codec_lib`
(`DescriptorPool`), reproto (`phases._reparse_text_files`).

- The test plan's cases pass: reproto renders from the text, by number or
  by name, what it renders from the binary set; a `descriptor.proto`
  under `package proto2` gets its options as `proto2.MessageOptions`
  extensions.
- Test 3 found two bugs, fixed: a variant `descriptor.proto` declaring
  the options messages alone left no type to parse with (a panic in
  `prototext_codec_lib`); a file whose import is missing lost its own
  options' declarations (the mending of S2).
- googleapis (7,771 files, 56 MB of text by number), read in two passes:
  every file byte for byte, 19 W2, no W1, as with spec 0426's correction.
- Time, googleapis, one E-core, `release` profile (the test's own
  timers, not the bench process): first pass 1.45 s, pool 0.57 s, second
  pass 1.45 s; 3.4 s against 1.45 s for one pass, x2.4.
