<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0446 — a report of the build: `--report-out FILE`

Status: draft
App: protoback (not reproto: N3)
Refs: docs/specs/0438-warning-codes-and-fail-on.md (the codes reported);
        docs/specs/0445-filter-fixups-move.md (the rewrites
        reported)

## Background

A downstream tool turns a build's problems into actions: which inputs are
stale, and which of its sources to extract again. Today it can only parse
stderr, which is written for people: warnings squashed into summaries
(`(N occurrences)`), silenced by `-q`, their facts embedded in prose
(which import W19 blames, which file W5 is missing from). protoback knows
nothing of where its inputs came from, and should not; it only needs to
report the facts, completely, in a form a program can rely on.

## Goals

- **G1.** One JSON file per run, written whatever `-q` and the squashing
  show, with every warning, every rewrite done, every directive that did
  nothing, and the run itself.
- **G2.** Each warning with structured fields, not only its message.
- **G3.** A versioned schema, stated here, that downstream can depend on.

## Non-goals

- **N1.** Advice. Which inputs to refresh is the downstream tool's
  conclusion, from these facts.
- **N2.** Replacing stderr. The human-readable output does not change.
- **N3.** reproto. A new feature, not a fix, as spec 0445 (its N4).

## Specification

- **S1.** `--report-out FILE`: the report, never stale.
  - Written at exit, `--fail-on`'s exit 3 included, atomically: to a
    temporary file beside `FILE`, then renamed over it.
  - A run that fails after its options are parsed (an I/O error, a
    collision that stops it) writes what it knows, `"complete": false`,
    with the exit status and the error.
  - A previous `FILE` never outlives a run that names it: removed once the
    options are parsed, before any work. Only a usage error found before
    `--report-out` itself is read can leave one, and it exits 2 with no
    run at all.
- **S2.** The top level:

  ```json
  {
    "report_version": 1,
    "complete": true,
    "tool": {"name": "protoback", "version": "0.3.7"},
    "run": {"argv": ["…"], "cwd": "…", "variant": "proto2",
            "inputs": ["…"], "exit_status": 0, "error": null},
    "warnings": [ … ],
    "rewrites": { "prunes": [ … ], "moves": [ … ] },
    "idle_directives": [ … ]
  }
  ```
- **S3.** A warning: `{"code": "W19", "message": "…", "fields": {…}}`, one
  per occurrence, unsquashed, in stderr's order: per file, the same from
  run to run whatever the threads, so that two reports diff. `fields` per
  code, for a start:

  | Code | Fields |
  |---|---|
  | W3 | `pruned`, `kept` (file → symbols, extension numbers) |
  | W4 | `file`, `type` |
  | W5 | `importer`, `import` |
  | W6 | `file`, `field`; `reason` when a refusal explains it (spec 0438 S8) |
  | W19, W20 | `file`; `import`, when refused for its import (the knock-on form); else `reason`, upb's (the root) |
  | W22 | `file`, `options`, `number` |
  | W23 | `importer`, `import` |
  | W24 | `importer`, `definition` (a type's full name, or `{options, number}`), `move` (its `OLD`) (spec 0445) |

  A code not listed has `fields: {}` (T3 and T4 next, when wanted).
  `report_version` is an integer, a major version: adding a field keeps
  it, so a reader ignores fields it does not know; changing or removing
  one bumps it. `argv` is verbatim, and `cwd` resolves its relative
  paths and `inputs`'.
- **S4.** `rewrites`:
  - `prunes`: each file pruned, with what caused it: a `file:` or `path:`
    pattern (which one), W3 (the duplicate kept), a move (spec 0445 S5);
    and each `desc:` pattern with the definitions it matched, one entry
    per pattern. A file is what a downstream tool can act on; a `desc:`
    prune is the user's own filter, listed so that nothing pruned goes
    unreported, without one entry per definition.
  - `moves`: each move applied (spec 0445), its targets, and every
    importer it re-pointed, with its imports before and after.
- **S5.** `idle_directives`: each prune or seed pattern matching nothing
  (W11, W12), and each move that did not apply as written (spec 0445's
  W25, idle, and W26, broken), with its exact reason.

## Prerequisite

W19's, W20's and W6's `import` and `reason` (S3) are the refusal reason
spec 0438 S8 gives on stderr since 2026-11-18: the report carries the same
facts, split into fields.

## Alternatives considered

- **Parse stderr.** Lossy by design (squashing, `-q`), and a format for
  people that cannot stay stable.
- **JSON Lines on stderr.** Mixes the two audiences, and loses the run's
  summary fields unless repeated per line.

## Test plan

1. `report_lists_every_occurrence` — five W4 squashed on stderr, five in
   the report; also under `-q`.
2. `report_fields_per_code` — each code of S3's table, its fields.
3. `report_written_on_fail_on` — exit 3, the report complete.
4. `report_never_stale` — a previous report removed by a run that fails;
   that run's own written incomplete, `complete: false`, with its error;
   written through a rename, never seen half-written.
5. `report_schema` — the file validates against S2's shape.

## Measured outcome

(At implementation.)
