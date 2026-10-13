<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0418 — protoscan without prost

Status: implemented
Implemented in: 2026-10-13
App: protoscan
Refs: docs/specs/0415-prototext-on-upb.md (N1b, which this ends; S5,
      the descriptor layer this uses; S11, the comparison method);
      docs/specs/0417-protolens-on-upb.md (after which protoscan is the
      last prost user); docs/specs/0407-protoscan-in-rust.md;
      docs/specs/0239-the-schema-says-where-a-descriptor-ends.md (G4: the
      scanner returns only records it has scored as descriptors)

## Background

After specs 0415 and 0417, protoscan is the workspace's last user of
prost: `prost` and `prost-types`, in two places.

- `src/main.rs`: for each record the scanner returns,
  `prost_types::FileDescriptorProto::decode` reads the file's name, to
  print it and to name the file `--out` writes. The comment above it says
  why decoding cannot fail in practice: the scanner returns only records
  it has already scored as `FileDescriptorProto`s (spec 0239 G4).
- `tests/cli.rs`: a test helper builds a small `FileDescriptorProto` with
  `prost_types` and encodes it with `prost::Message`.

`prototext_upb::descriptor` (spec 0415 S5) has the same structure, with
the same field names, encoding and decoding, in pure Rust.

## Goals

- **G1.** prost and prost-types leave protoscan, and with it the
  workspace: no crate depends on them.
- **G2.** protoscan's output is unchanged: the names it prints and the
  bytes it writes with `--out`, on every input of the gate (S3).

## Non-goals

- **N1.** Changing the scanner (fdp-scan) or its acceptance rule.
- **N2.** Decoding more of a descriptor than its name.

## Specification

- **S1. The name.** `main.rs` reads a record's name with
  `prototext_upb::descriptor::FileDescriptorProto::decode`. The error
  message for a record that does not decode is kept, as protection
  against a scanner change breaking spec 0239 G4.

  The two decoders differ only on records the scanner never returns:
  prost decodes every field of `descriptor.proto` and fails on a
  malformed one; the descriptor layer decodes the fields it declares and
  skips the rest. On a record scored clean, both read the same name.
- **S2. The test helper** builds its `FileDescriptorProto` with the
  descriptor layer and `Encode`, as prototext's tests do since spec 0415.
- **S3. The gate.** Before the change, the current build records, and
  after it the new build must reproduce byte for byte:
  - the names printed, and the files written by `--out`, for
    googleapis.desc (7,771 descriptors), the prototext binary (which
    embeds its well-known types), and the bobapp binary when its store
    path is available;
  - the exit status and stderr of each, and of a file with no
    descriptor.
- **S4. Dependencies.** `protoscan/Cargo.toml` replaces `prost` and
  `prost-types` with `prototext-upb`, already in protoscan's tree through
  fdp-scan. `workspace-hack` is regenerated; `cargo tree -i prost` finds
  nothing, and neither does `Cargo.lock`.
- **S5. Measured** as spec 0415 S12: `protoscan googleapis.desc`, before
  and after (spec 0313 measured 0.561 s), reported for review.

## Alternatives considered

### Read the name with a hand-written field-1 reader in protoscan

Smaller still, but a second wire reader for descriptors beside the
layer that exists for exactly this; the layer's decoder is the one
already tested against malformed input.

## Test plan

1. protoscan's tests pass, the helper on the descriptor layer.
2. The gate (S3): no difference.
3. `cargo tree -i prost` and `cargo tree -i prost-types` report nothing
   for the workspace; `ci` and `cargo package` for protoscan pass.

## Measured outcome

Implemented 2026-10-13, on the `upb` branch, before spec 0417.

- **S1–S2:** `main.rs` and the test helper use
  `prototext_upb::descriptor`; protoscan's 9 tests pass.
- **S3:** 6 of 6 golden cases identical (`scripts/upb-golden-protoscan`)
  between the prost build and this one: googleapis.desc (7,771 names
  printed and 7,771 files written by `--proto-out`, each byte for byte),
  the prototext binary (11), the bobapp binary (33), a text file, the
  anomaly fixture, and a missing file.
- **S4:** with protolens out of the workspace, `Cargo.lock` holds no
  `prost` crate and `cargo tree -i` finds none; `workspace-hack` needed no
  change. Spec 0417 brings protolens back without prost, keeping both
  true. Nine publishable crates package and build from their `.crate`
  files.
- **S5:** `protoscan googleapis.desc`, pinned: 368.8 ms ± 3.3 before,
  366.0 ms ± 11.2 after — no change.
