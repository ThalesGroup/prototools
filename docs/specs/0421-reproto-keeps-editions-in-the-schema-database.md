<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0421 — reproto keeps editions in the schema database

Status: implemented
Implemented in: 2026-10-24
App: reproto
Refs: docs/specs/0057-prost-workaround.md (the conversion this retires,
      and why it existed); docs/specs/0415-prototext-on-upb.md and
      docs/specs/0417-protolens-on-upb.md (the readers that no longer need
      it); docs/specs/0371-packing-mismatch-is-neutral-evidence.md (the
      packing declaration the scorer and the renderer must agree on)

## Background

A schema database (`reproto --schema-db-out`) holds a descriptor set,
read by prototext and protolens, and a scoring graph, `hopcroft.rkyv`,
read for type inference. Since spec 0057, building one converts every
editions file to proto2 (`phases.py`: `EDITIONS_COMPAT_REQUIRED = True`
forces `--force-proto2-for-editions`, for the corpus's files and for the
well-known types), because prost-reflect panicked on editions
descriptors.

That reason is gone: prototext and protolens read schemas through upb
(specs 0415, 0417), which resolves editions as every Google runtime does,
and protoscan reads names only. What is left is the cost:

- the conversion is lossy: implicit presence, open enums, a field's
  `repeated_field_encoding` and `message_encoding = DELIMITED` have no
  exact proto2 spelling, so an editions message renders as an
  approximation of itself;
- the scoring graph never went through it. It is built from reproto's
  input pool (Python protobuf, on upb), whose `is_packed` and
  `TYPE_GROUP` are feature-resolved, so for an editions file the graph
  and the descriptors it ships with can disagree, and a record the graph
  calls packed is a packing mismatch to the renderer.

## Goals

- **G1.** A schema database keeps editions files as editions: its
  descriptors are protoc's, and they agree with its scoring graph.
- **G2.** Nothing changes for proto2 and proto3 files, nor for a corpus
  without editions.
- **G3.** Anyone who must feed an older reader keeps a way to get the
  proto2 form.

## Non-goals

- **N1.** Changing the scoring graph: it is already feature-resolved.
- **N2.** Changing what `--force-proto2-output` or
  `--force-proto2-for-editions` do when given.
- **N3.** Supporting readers older than 0.3.5 by default: they are on
  prost-reflect, and panic on an editions descriptor (S4).

## Specification

- **S1.** `--schema-db-out` no longer forces `--force-proto2-for-editions`:
  `EDITIONS_COMPAT_REQUIRED` and the two places that read it go, as does
  the comment above the option that says the database forces it. The
  forcing applied to the one render that writes both the descriptors and
  the `.proto` text, so the `.proto` files written beside a database (the
  stub's `proto/`, or `-O`) keep their editions too: they and the
  database agree.
- **S2.** The flag itself stays. Given explicitly, it applies to the
  schema database as to the rest of the output, which is G3's way back
  to the proto2 form. Its help text loses "--schema-db-out forces this
  unconditionally".
- **S3.** The hidden `--prost-workaround` alias stays, so that scripts
  feeding prost-reflect keep working. The flag's help says whom it is
  for: "Use this for readers that do not support editions, such as
  prost-reflect."
- **S4.** The CHANGELOG says, under the release: a schema database from a
  corpus with editions files now holds them as editions; prototext and
  protolens 0.3.4 and older, on prost-reflect, panic loading one, so a
  database for them needs `--force-proto2-for-editions`.

## Alternatives considered

### Keep converting by default, add a flag to keep editions

Backwards compatible, but the default would stay the lossy one for every
reader that can do better, and the graph and the descriptors would keep
disagreeing unless someone knew to pass the flag.

### Convert only when an old reader is detected

There is no reader at build time to ask: a database is written once and
read by whatever version opens it.

## Test plan

1. reproto's tests: a schema database built from a corpus with an
   editions 2023 and an editions 2024 file holds them with `syntax =
   "editions"` and their editions; with `--force-proto2-for-editions`,
   as proto2, as today.
2. A reproto test: prototext loads that database and decodes a message
   of each editions file, delimited and packed fields included, as
   protoc does; no packing-mismatch annotation where the bytes follow
   the declaration. protolens opens the same database, by hand.
3. The googleapis database rebuilt: the pinned corpus has no editions
   file, nor do the well-known types it pulls in (checked 2026-10-24),
   so it is byte-identical to today's.
4. `--prost-workaround` gives the same database as
   `--force-proto2-for-editions`.

## Measured outcome

Implemented 2026-10-24, for v0.3.5.

- **S1–S3:** `EDITIONS_COMPAT_REQUIRED` and its two readers are gone;
  `--force-proto2-for-editions`'s help names its readers, and
  `--prost-workaround` stays its hidden alias.
- **Plan 1, 2, 4** (`test_schema_db_editions.py`, 3 tests): a DB from
  an editions 2023 and an editions 2024 file holds both as editions,
  as do the `.proto` files beside it; prototext decodes a message of
  each, delimited and packed fields included, with no mismatch. With
  `--force-proto2-for-editions`, the same bytes decode both delimited
  messages as `TYPE_MISMATCH`: the loss G1 removes. The flag and its
  alias give the same DB. The fixtures are inline: prototext-upb's
  `editions2023.proto` sets `repeated_field_encoding` on a message,
  which protoc 34 rejects.
- **Plan 2, protolens:** `protolens export` of the same two messages,
  from the same DB, renders them as prototext does.
- **Plan 3:** the googleapis DB built from this change and from the
  commit before it: byte-identical.
- reproto: 293 tests pass; `nix-build -A ci` passes.
