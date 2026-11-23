<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0460 — one protobuf release for every copy of descriptor.proto

Status: implemented
Implemented in: 2026-11-23
App: prototext-upb | reproto | protoback | prototext
Refs: docs/specs/0409-python-extensions-on-maturin-reproto-from-source.md
        (S4: reproto's well-known types, committed and checked),
      docs/specs/0405-prototools-in-nixpkgs-one-small-pr-at-a-time.md
        (prototext's committed fixtures and WKT graph, checked),
      docs/specs/0415-prototext-on-upb.md (S1: upb vendored by
        `scripts/update-upb`),
      docs/specs/0457-protoback-edition-names-from-the-input.md (what an
        older built-in `descriptor.proto` costs)

## Background

The repository carries `descriptor.proto`, and the other well-known
types, in several forms:

| Copy | Release | Kept by |
|---|---|---|
| `prototext-upb/vendor/descriptor.{proto,pb}` | v36.2 | `scripts/update-upb` (a protobuf tag) |
| `reproto/src/reproto/variants/google-protobuf/google/protobuf/*.pb` | 34.1 | `reproto-wkt-check`, against the pin's protoc |
| `prototext/fixtures/prebuilt/*.pb` | 34.1 | the prebuilt fixtures' check, likewise |
| `prototext/wkt/prebuilt/*.rkyv` | 34.1 | `wkt-prebuilt-check`, likewise |
| Python protobuf (`descriptor_pb2`) | 7.34.1 | the pin |

Two sources of truth: the main nixpkgs pin's protobuf (nixos-26.05: protoc
34.1, Python protobuf 7.34.1), which three checks compare against, and the
tag `update-upb` was last run with. Nothing compares them, and they have
drifted: upb (the descriptor pool, the text parser) knows edition 2026,
the built-in variant both tools fill in doesn't. So a text input can't
hold `edition: 2026` (W15), an input without its own `descriptor.proto`
resolves edition 2026's defaults as 2024's (0458), and reproto, whose
Python protobuf is 7.34.1, can't name edition 2026 at all.

reproto and protoback themselves can't drift: protoback embeds reproto's
variant files (`variant.rs`, `include_bytes!`).

The main pin has no protobuf 36 (`protobuf_21` to `protobuf_34`). The
nixos-unstable pin (`nix/unstable.json`), which the staged nixpkgs
recipes are already checked against, has protoc 36.2 and Python protobuf
7.36.2.

## Goals

- **G1.** One protobuf release behind every copy: the vendored upb and
  its `descriptor.proto`, the variant's well-known types, prototext's
  prebuilt fixtures and WKT graph, the protoc the builds and tests run,
  and Python protobuf.
- **G2.** A copy from another release fails `ci`, saying how to refresh.
- **G3.** That release is v36.2.

## Non-goals

- **N1.** One committed file for all copies. Their formats differ (a
  `FileDescriptorSet` with source info; one `FileDescriptorProto` per
  file without), and reproto reads its variant as package data
  (`importlib.resources`), which a link out of the package wouldn't
  survive in a wheel. Derived and checked instead.
- **N2.** Moving the main nixpkgs pin. Only protobuf is taken elsewhere
  (S1), until the pin has it.
- **N3.** The test fixtures written once and read as data
  (`prototext-core/fixtures/descriptor.pb` and the like): they are inputs,
  not copies.

## Specification

- **S1. The release.** `nix/protobuf.nix` defines it once: `protobuf`
  (protoc, its include directory) and Python `protobuf`, of the same
  release. Until the main pin has that release, protoc is the
  nixos-unstable pin's `protobuf_36` (a tool: nothing links against it),
  and Python protobuf is the main pin's, its version and source
  overridden to the matching 7.36.2. Every derivation that used
  `pkgs.protobuf` or `pythonPkgs.protobuf` takes them from there, the
  dev-shell included.

- **S2. The vendored upb follows it.** A check, `upb-release-check`:
  `vendor/VERSION`'s protobuf tag is S1's release, and
  `vendor/descriptor.proto` is that release's
  `include/google/protobuf/descriptor.proto`, byte for byte. On failure it
  prints the `scripts/update-upb` command for S1's release. `update-upb`
  compiles with S1's protoc by default (`PROTOC` stays an override).

- **S3. The other copies follow it,** by their existing checks, now
  against S1's protoc: `reproto-wkt-check`, the prebuilt fixtures' check,
  `wkt-prebuilt-check`. Unchanged otherwise.

- **S4. The copies agree with each other.** A check, `descriptor-copies-check`:
  the variant's `descriptor.pb` holds the same `FileDescriptorProto` as
  `vendor/descriptor.pb` with `source_code_info` cleared. It catches a
  copy refreshed by hand, whichever.

- **S5. In `ci`,** with the other copy checks; not in `ci-no-clippy`,
  which leaves them out: what they compare doesn't depend on the
  platform.

- **S6. The move to v36.2:** `update-upb` run for v36.2 (a no-op: it is
  vendored), the variant, prebuilt fixtures and WKT graph refreshed with
  S1's protoc, Python protobuf 7.36.2, and fdp-scan's corpus (0448 S8),
  which is the well-known types serialized by Python protobuf. The
  nixpkgs staging (0423) keeps nixpkgs' own Python protobuf: it builds the
  recipes as nixpkgs will.

- **S7. What changes in the output,** identically in both tools (they
  read the same variant): every rendered `descriptor.proto` (fields,
  values and options of 35.x and 36.x, `EDITION_2026` among them); edition
  2026's defaults for inputs without their own `descriptor.proto`; text
  inputs may hold `edition: EDITION_2026`; reproto names edition 2026 with
  `Edition.Name`, as protoback does since 0457. The harness, both tools
  being moved together, still compares them byte for byte; its committed
  expectations, if any are tied to the old descriptor, are refreshed.

## Alternatives considered

**reproto left on Python protobuf 7.34.1 until the main pin moves.** Its
descriptor classes would then lag the variant it renders, the one place
where the tools would differ for edition 2026 inputs.

**The whole main pin moved to nixos-unstable.** Every package moves for
the sake of one; the staged recipes' checks already run against unstable
separately.

**Python protobuf from the unstable pin.** A package of another nixpkgs
in one Python environment brings its own interpreter closure; overriding
the version in the main pin's set keeps one.

**The variant generated from `vendor/descriptor.pb` at build time,** not
committed. reproto builds from its source as committed (0409); the
dev-shell runs it from the tree.

## Test plan

1. `upb-release-check`: passes; fails on a `vendor/VERSION` naming
   another tag, and on one byte changed in `vendor/descriptor.proto`.
2. `descriptor-copies-check`: passes; fails on a variant `descriptor.pb`
   compiled by another protoc.
3. The existing checks pass on the refreshed copies.
4. protoback: a text input with `edition: EDITION_2026` is read; an input
   without its own `descriptor.proto` gets edition 2026's `STRICT`
   default (0458's test, without its vendored copy).
5. reproto and protoback render an edition 2026 file identically.
6. `ci`: the harness, both modes, against reproto; the self-check's
   differences listed in the Measured outcome.

## Measured outcome

2026-11-23. `ci` passes: the six copy checks, reproto's suite under
Python protobuf 7.36.2 on both tools, the harness in both modes against
reproto.

- `upb-release-check` passed unchanged: the vendored upb was v36.2,
  its `descriptor.proto` byte for byte protoc 36.2's. Of the variant's
  11 well-known types only `descriptor.pb` changed; prototext's prebuilt
  `descriptor.pb` and WKT graph, and fdp-scan's whole corpus, were
  refreshed. Each check fails, with its refresh command, on a copy
  tampered with (test plan 1, 2).
- The self-check against the previous build: 129 of 1 669 cases change,
  all in a rendered `google/protobuf/descriptor.proto` (`.proto`, `.pb`,
  scoring YAML) or a schema database holding it; 1 503 of 1 655 graph
  dumps, by `descriptor.proto`'s own nodes (`ProtoLimitsFeature`,
  `enforce_proto_limits`, and the like).
- Two `--target portable` tests took protoc's descriptor to lack edition
  1002 and the `FileOptions` numbers 990 to 998; protobuf 36 has both
  (`EDITION_2026`; `extensions 990 to 998`, for first-class custom
  options). Their vendor fixtures now use 1003 and 980 to 989.
- Python protobuf 7.34.1 read edition 1002 as unset: reproto wrote
  `edition = "UNKNOWN"` for an edition 2026 file. With 7.36.2 it writes
  `edition = "2026";`, as protoback does (`test_edition_2026.py`).
