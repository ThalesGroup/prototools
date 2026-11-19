<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0437 — two files rewritten onto one path

Status: superseded (implemented 2026-11-17, superseded 2026-11-19 by
        docs/specs/0447-output-targets-and-profiles.md: variants gone,
        --target protoc the one renaming)
Implemented in: 2026-11-17
App: reproto, protoback
Refs: docs/specs/0086-canonize-output-names.md (the import
        rewrites that give a file its output path),
      docs/specs/0158-reproto-schema-db-canonical-name-collision-error.md (the same
        collision, caught in the schema database only, after rendering),
      docs/specs/0436-text-input-names-never-dropped-unsaid.md (W4, which
        says a value the kept file cannot hold)

## Background

A variant's `import_rewrites` give each file its output path
(`canonize_dependency`). Two input files can end up with the same one. A
variant that renames `internal/proto2/` to `google/protobuf/` maps its
`internal/proto2/descriptor.proto` onto `google/protobuf/descriptor.proto`,
and an input that also holds protobuf's own `descriptor.proto` has two
files for that path. Both are rendered, the last one overwrites the
other, and both go into the schema database under one name; nothing is
said. Reproduced: the output's `google/protobuf/descriptor.proto` was the
stock one, without the fields the variant's adds, while the other files
set those options.

## Goals

- **G1.** No collision is resolved silently.
- **G2.** When the variant's `descriptor_proto` is one of the files, it is
  kept: every rewritten reference targets it.

## Non-goals

- **N1.** Merging the two files. Their packages are the same after the
  rewrite; which declarations each holds is the user's input to sort out,
  by pruning one (`-p`).

## Specification

- **S1. Detected before rendering.** In phase 7, before any file is
  rendered or written, the summoned files are grouped by output path.
- **S2. The variant's descriptor.proto kept.** A group holding the
  variant's `descriptor_proto` keeps it; each other file of the group is
  neither rendered, written nor put in the schema database, and a
  warning names it: "'OTHER' and 'DESCRIPTOR' are both written as 'PATH':
  'DESCRIPTOR', the variant's descriptor.proto, is kept".
- **S3. Any other collision is an error.** The run stops before rendering,
  exit status 1: "'A' and 'B' are both written as 'PATH'; prune one (-p)",
  the files in input order, one line per path.
- **S4. Both tools alike**, the same text on stderr.

- **S5. Spec 0158's check stays**, as a backstop: S3 now stops the run
  before the schema database is built, so its message ("schema-db name
  collision") is no longer the one a user sees. Two byte-identical files
  are not a collision: deduplicated at load, one is summoned (0158 G2).

## Alternatives considered

- **A warning for every collision, the first file kept.** No principled
  winner outside S2: keeping either silently changes the output with the
  input's order.

## Test plan

1. `tests/test_rewrite_collisions.py`, against both tools: a renaming
   variant, the input holding the variant's descriptor.proto and
   protobuf's own: the warning, and the variant's written, its added
   field present; a text using a field only protobuf's copy declares,
   W4.
2. The same file: two files rewritten onto one path, neither the
   variant's descriptor.proto: exit status 1, the error, nothing written.
3. The harness: both tools identical.

## Measured outcome

- Test plan 1 and 2: `test_rewrite_collisions.py`, 3 tests, pass on both
  tools, stderr and output identical; spec 0158's
  `test_G1_colliding_names_different_content_is_fatal` now sees this
  spec's error. reproto's suite 339 passed on binary input, on text
  input and against protoback.
- Test plan 3: 1,668 cases, identical.
