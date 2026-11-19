<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0447 — output targets and profiles, instead of variants

Status: draft
App: protoback and reproto (reproto a subset: S9)
Refs: docs/specs/0440-one-renaming-table.md (the renaming this replaces);
        docs/specs/0437-rewrite-collisions.md (the collisions it removes);
        docs/specs/0445-filter-fixups-move.md (moves, and its `variants:`
        sections); docs/specs/0438-warning-codes-and-fail-on.md (W22,
        orphans)

## Background

**Who the output is for.**

1. **Stock inputs:** regular descriptor sets, compiled against protoc's own
   `google/protobuf/descriptor.proto`, from wherever they come (protoc's
   `--descriptor_set_out`, executables implementing gRPC reflection, which
   often leave out the descriptor and the well-known files). These users
   know nothing of vendor descriptors, and need no option: every setting of
   this spec is a no-op for them.
2. **Vendor inputs:** descriptors compiled against a vendor's own
   descriptor, at another path and in another package (here
   `vendor/proto/descriptor.proto`, package `vendor`), with option fields
   stock protoc's lacks, and custom options declared on `vendor.*Options`.
   Their output is read by:
   1. a vendor protoc, with the vendor descriptor built in, as the
      vendor's own compiler has it;
   2. a protoc-like compiler, which reads `descriptor.proto` from disk when
      a file imports it (stock protoc does);
   3. a less capable compiler, which only knows its own built-in
      descriptor.

**Today.** One concept, the variant (a YAML file and a folder beside it),
and options around it (`--proto-variant`, `--use-variant`,
`--keep-descriptor-path`, `--[no-]emit-descriptor`, `$REPROTO_VARIANT`)
answer unrelated questions at once: which descriptor the input was compiled
against, which names to rewrite, which files to replace. Category 1 still
needs `--use-variant`: without it, a dump lacking `descriptor.proto` stops the
run, and a missing well-known file is W5. Category 2 renames every name of
the vendor's namespace (spec 0440), which makes collisions (spec 0437) and
needs `variants:` sections in fixups (spec 0445), for no gain (below).

**What was measured.** A vendor corpus of 7,468 files, rendered by protoback
and compiled **each file on its own** (one protoc run per file): a protoc run
builds one pool of definitions, so files compiled together see one another's
imports, and a batch hides failures.

- **How protoc reads a built-in option** (`option x = …`, `[x = …]`): against
  the `descriptor.proto` in its pool. That is the one on disk if the file
  imports it, even indirectly, or another file of the same run did;
  otherwise protoc's compiled-in copy. A protoc patched to redirect to
  `vendor.*Options` does so only when the vendor descriptor is in the pool,
  just the same, unless the vendor descriptor is its built-in one.
- **Built-in option fields cannot become extensions:** the vendor's are
  numbered below 1000, and an options message only accepts extensions from
  1000 up (`does not declare 28 as an extension number`).
- **Results**, failing files, each compiled on its own:

  | Reader | Output | Failing |
  |---|---|---|
  | protoc patched to redirect, descriptor not built in | names kept (today) | 2,991 |
  | the same | names kept + import rule (S4) | 409 |
  | vendor protoc, descriptor built in (measured downstream, naming-style check off) | names kept, no import rule | 38 |
  | stock protoc | full renaming (today) | 2,960 |
  | stock protoc | full renaming + import rule | 369 |
  | stock protoc | descriptor only (S3 `protoc`) + import rule | 368 |
  | stock protoc, own descriptor only | `portable` (S3) | 369 |

  Every row's remainder also fails with no mapping and the vendor protoc:
  causes shared by all outputs, the corpus's own inconsistencies (mostly
  types no file defines), and protoback's own rendering gaps (edition 2024
  visibility, `import option`, an empty `features`, found downstream since
  and fixed apart). Renaming the vendor namespace beyond the descriptor
  buys nothing: the vendor's other files compile as ordinary files under
  their own paths and packages.

## Goals

- **G1.** Category 1: no option needed. Built-in copies fill in what the
  input lacks.
- **G2.** Category 2: one setting, the target (S3), for each kind of reader.
- **G3.** Every output file compiles on its own, not only within a batch,
  as far as the input allows.
- **G4.** Lossless where the reader allows it; where it cannot, the loss
  explicit (orphans, W22's family) and reported.
- **G5.** No renaming beyond what the target needs: no collisions, no
  `variants:` sections.
- **G6.** Saved settings (profiles) for the common cases, and no environment
  variable changing what a command means.

## Non-goals

- **N1.** Renaming a vendor namespace. Only the vendor descriptor's
  definitions are mapped, and only for the `protoc` and `portable` targets.
- **N2.** Built-in option fields as extensions (impossible: Background).
- **N3.** Repairing the input's own inconsistencies (spec 0445's domain, and
  the user's).

## Specification

### The input descriptor

- **S1.** The descriptor the input was compiled against is inferred: the
  file defining the messages the input's custom options and features extend
  (`extend vendor.FieldOptions`, `extend vendor.FeatureSet`). None: protoc's
  `google/protobuf/descriptor.proto`. Extendees in two packages: an error
  naming a file of each, asking for `--descriptor NAME`, which also overrides
  the inference. The descriptor used is stated in the report (spec 0446).

### Filling in

- **S2.** protoback's built-in copies of `google/protobuf/descriptor.proto`
  and the well-known files fill in what the input lacks, silently: no W1 or
  W5 for them. The report lists each file filled in. `--builtin NAME` uses the
  built-in copy even when the input has one; `--no-builtin` turns filling in
  off.

### The target

- **S3.** `--target keep|protoc|portable`, default `keep`. Nothing to do for
  a stock input whatever the target.
  - **`keep`** (category 2.1): names and paths as the input has them.
    Nothing added: a vendor protoc has the vendor descriptor built in, and
    the output's compiled descriptors match the input's, dependency for
    dependency. The import rule (S4) only on request, for a protoc that
    knows the vendor descriptor only once loaded.
  - **`protoc`** (category 2.2), lossless:
    - the vendor descriptor written as `google/protobuf/descriptor.proto`,
      its package the stock one; its definitions renamed (`vendor.X` becomes
      `google.protobuf.X`), and every reference to them, wherever written
      (qualified, or relative from the vendor package);
    - the input's own `google/protobuf/descriptor.proto`, if any, dropped
      with W27: the descriptor in use wins;
    - the import rule (S4), with `google/protobuf/descriptor.proto`.
  - **`portable`** (category 2.3), lossy but explicit:
    - the vendor descriptor kept as an ordinary file, its path and package
      unchanged, so that its types stay usable as field types;
    - extendees mapped: `extend vendor.XOptions` and `extend vendor.FeatureSet`
      become `extend google.protobuf.…`, with `import
      "google/protobuf/descriptor.proto"` in those files (needed to extend,
      not to set an option);
    - redacted (S5): what protoc's built-in descriptor cannot represent.
- **S4. The import rule.** A file setting a built-in option field its reader
  only knows from the target's descriptor, and not importing that
  descriptor, gets the import. Its cost: one more `dependency` than the
  input has, in the descriptors compiled from the output (and protoc's
  warning of an unused import). So: on under `protoc`, which needs it;
  off under `keep`, where `--import-descriptor` (a profile's
  `import_descriptor: true`) turns it on; not applicable to `portable`,
  which redacts what it would import.
- **S5. Redaction** (`portable`): an option set on a built-in field the
  stock descriptor lacks, or to an enum value it lacks (`EDITION_2026` in a
  descriptor newer than the reader), written as an orphan comment with its
  number and value, warned (W28), and listed in the report. In the
  binary descriptors the field stays as the input has it, raw, under its
  number: the binary copies the input's options. Checked: Python's protobuf
  (upb) loads such a descriptor, keeping the field as an unknown field, and
  protoc reads it (`--descriptor_set_in`) and writes it back unchanged. A
  reader that drops unknown fields (prost, by its design) loses the value,
  not the descriptor. The one risk: stock protobuf later giving that number
  another meaning, then misread.

### Profiles

- **S6.** `--profile NAME`, repeatable: a saved set of command-line options,
  applied in order, a later scalar replacing an earlier one, lists appended,
  the command line overriding them all. Looked up in the current directory,
  then in `$PROTOBACK_PROFILE_PATH`'s folders, then in the tool's own;
  `.yaml` optional. A key is an option's long name: `descriptor`, `target`
  (`target: keep` is `--target keep`; the default, so written only to
  override an earlier profile), `import_descriptor`, `editions`, `builtin`,
  `emit_descriptor`, `fail_on`. A profile `X.yaml` may have a folder `X/`
  beside it, laid out as a descriptor root: every file in it replaces the
  input's copy, W29 when the input's copy defines something the
  replacement lacks. The tool's own folder ships `prost.yaml`
  (`editions: proto2`, for readers that do not read editions).
- **S7.** `--editions proto2` (today's `--force-proto2-for-editions`, kept
  as an alias).

### What goes

- **S8.** Variants end, in both tools at once, with no translation layer:
  their one user moves to profiles.

  | Today | Becomes |
  |---|---|
  | `--proto-variant` | a usage error pointing to `--profile` |
  | `$REPROTO_VARIANT` | ignored, with a notice suggesting `--profile` (an inherited variable cannot be an error) |
  | `import_rewrites`, `namespace_rewrites` | `--target` |
  | `--keep-descriptor-path` | `--target keep` |
  | `--use-variant` | `--builtin`, or a profile's folder |
  | `well_known` | nothing (always the identity) |
  | `orphans` | the tools' code |
  | `variants:` in `-C` files | not needed: no renaming collisions remain; refused by the strict loader as an unknown key |
- **S9. reproto, a subset.** Parity holds for the features reproto
  implements (the harness compares them). reproto reads the same profiles
  and options, and implements `descriptor`, `import_descriptor`,
  `editions`, `builtin`, `emit_descriptor`, `fail_on`, and the targets
  `keep` and `protoc` (its rule engine expresses `protoc`, with rules
  generated from the descriptor's definitions). A key or a target it does
  not implement (`portable`) is a usage error naming it, never ignored.
  The harness compares every case whose settings reproto implements; the
  rest are tested on protoback alone.

### Codes

- **S10.** Spec 0438's table gains, in both tools:
  | Code | Warning |
  |---|---|
  | W27 | the input's own `google/protobuf/descriptor.proto` dropped for the vendor descriptor (target `protoc`) |
  | W28 | an option the target's reader cannot represent, redacted (target `portable`) |
  | W29 | a profile folder's file replacing an input's copy that defines something it lacks |

## Alternatives considered

- **Full renaming of the vendor namespace** (today). Measured: no better
  than the descriptor alone once the import rule applies, at the price of
  collisions and `variants:` sections.
- **Moves mapping definitions across packages** (finding of spec 0445's
  first downstream run). The only cross-package case was a renaming
  collision; the target removes it.

## Test plan

1. `stock_dump_needs_no_option` — a dump without `descriptor.proto` and a
   well-known file it imports: runs, no W1 or W5, both listed as filled in.
2. `descriptor_inferred`, `descriptor_ambiguous_is_an_error`.
3. Per target, on a small vendor fixture (a vendor descriptor with an
   internal option field, a custom option, a feature extension), each output
   file compiled on its own:
   - `keep`: the output's compiled descriptors carry the input's names and
     dependencies, unchanged; with `--import-descriptor`, one import more,
     where S4 says (a vendor protoc is out of this repository's reach);
   - `protoc`: compiled with stock protoc, the descriptor written;
   - `portable`: compiled with stock protoc, no descriptor on disk.
4. `import_rule_only_where_needed`, `redaction_of_fields_and_enum_values`.
5. `profiles_compose`, `profile_folder_replaces`, `target_key_in_a_profile`.
6. `proto_variant_is_a_usage_error`,
   `reproto_variant_env_is_ignored_with_a_notice`.
7. `inference_on_public_corpora`: on googleapis and the harness's corpus,
   S1 finds protoc's descriptor (category 1 unaffected).
8. reproto: the same tests for its subset;
   `portable_is_a_usage_error_in_reproto`; the harness comparing profile
   runs.

## Measured outcome

(At implementation.)
