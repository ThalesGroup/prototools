<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0473 — protoback: --repair-skew's repairs reach the pool and the schema database

Status: draft
App: protoback
Refs: docs/specs/0469-protoback-repair-skew.md (the repairs, S7 here
        revised),
      docs/specs/0446-build-report.md (W19, W20 in the report)

## Background

0469 repairs the text protoback writes: an import added (W33), a field
or method of no written type (W34) or an extension out of its extendee's
ranges (W35) commented out. The descriptor pool, though, loads the input
descriptors as they were (`render::options::pool_loads`), before any
repair: a repaired file is still refused (W19), its importers with it
("the descriptor pool refused its import …"), and none of them reaches the
schema database (W20).

On a downstream corpus of 16,166 written files: 34 files refused, of
which 6 roots; the repairs make 4 of those roots, and the files that are
refused only through them, compile with protoc, while the run still
reports all of them refused and leaves them out of the schema database.

## Goals

- **G1.** Under `--repair-skew`, the pool loads each repaired file as the
  text has it: what protoc would build from the written `.proto`.
- **G2.** So a file the repairs make whole is neither refused (no W19, no
  W20) nor left out of the schema database, and neither are the files
  refused only through it.

## Non-goals

- **N1.** Without the option: the pool, the warnings and the schema
  database as today.
- **N2.** Repairs beyond 0469's.

## Specification

- **S1. The order.** Under `--repair-skew`, the repairs (0469) are
  computed before the pool is loaded, from what they need (the graph, the
  definitions, the symbols), not from the pool.

- **S2. The repaired descriptor.** A file with repairs is given to the
  pool as: its dependencies with the imports added (W33); without the
  fields, methods and extensions commented out (W34, W35); a oneof left
  with no field removed with them. Nothing else changes.

- **S3. What follows the pool.** Everything that reads the pool reads the
  repaired descriptor: W19 is said only of a file still refused; the
  schema database's entries and scoring graphs (no W20 for it). A refused
  file's options are rendered whatever (0475).

- **S4. The descriptor written beside the text** (`--emit-binary`) is the
  repaired one too (0469 S7 revised): what protoc builds from the text.
  Its SourceCodeInfo paths count the fields kept, not the input's.

- **S5. Reported:** W33 to W36 as today, each still saying what was done.
  The report (`--report-out`) gains nothing else: a repaired file no
  longer refused is simply not in W19 or W20.

## Alternatives considered

**W19 left out for a repaired file, the pool unchanged** (as first
asked, then withdrawn by the downstream). It hides the refusal without
making the file usable: the schema database would still lack it and its
importers.

**The repairs applied to the input database itself** (before phase 3).
The graph, the summoning and every warning before rendering would then
see repaired files: a larger change than the pool needs, and the
graph's findings (W4 among them) would no longer say what the input
holds.

## Test plan

1. `a_repaired_file_is_not_refused` — 0469's shape (a type not imported,
   W33): no W19 nor W20 for it, nor for a file importing it; both in the
   schema database; its options rendered.
2. `a_field_commented_out_leaves_the_pool` — a W34 field: the file
   accepted, the field absent from its schema-database entry and its
   `.pb`, which protoc's descriptor of the written text matches.
3. `still_refused_is_still_reported` — a file a repair doesn't make
   whole: its W19 and W20 as today.
4. The self-check without the option: no change.

## Measured outcome

Filled in at implementation.
