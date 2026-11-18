<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0446 — a report of the build: `--report-out FILE`

Status: draft
App: protoback (not reproto: N3)
Refs: docs/specs/0438-warning-codes-and-fail-on.md (the codes reported);
        docs/specs/0445-filter-fixups-move-and-split.md (the rewrites
        reported)

## Background

A downstream tool turns a build's problems into actions: which inputs are
stale, and which of its sources to extract again. Today it can only parse
stderr, which is written for people: warnings squashed into summaries
(`(N occurrences)`), silenced by `-q`, their facts embedded in prose (W19
says "this file or one of its imports"). protoback knows nothing of where
its inputs came from, and should not; it only needs to report the facts,
completely, in a form a program can rely on.

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

- **S1.** `--report-out FILE`: the report, written at exit, `--fail-on`'s
  exit 3 included; on a usage error (exit 2) none is written.
- **S2.** The top level:

  ```json
  {
    "report_version": 1,
    "tool": {"name": "protoback", "version": "0.3.7"},
    "run": {"argv": ["…"], "variant": "proto2", "inputs": ["…"],
            "exit_status": 0},
    "warnings": [ … ],
    "rewrites": { "prunes": [ … ], "moves": [ … ], "splits": [ … ] },
    "idle_directives": [ … ]
  }
  ```
- **S3.** A warning: `{"code": "W19", "message": "…", "fields": {…}}`, one
  per occurrence, unsquashed, in emission order. `fields` per code, for a
  start:

  | Code | Fields |
  |---|---|
  | W3 | `pruned`, `kept` (file → symbols, extension numbers) |
  | W4 | `file`, `type` |
  | W5 | `importer`, `import` |
  | W19, W20 | `file` (the file refused), `reason` (the pool's) |
  | W22 | `file`, `options`, `number` |
  | W23 | `importer`, `import` |

  A code not listed has `fields: {}`; adding fields is compatible,
  changing one bumps `report_version`.
- **S4.** `rewrites`: each prune with the pattern or rule that caused it
  (`-p`, `-C`, W3); each move and split (0445) with every importer's
  imports before and after, and the symbols removed from a split file.
- **S5.** `idle_directives`: each prune or seed pattern matching nothing
  (W11, W12), and each fixup doing nothing (0445's W25).

## Prerequisite

W19's `file` and `reason` (S3) need the pool's refusal reason carried to
the warning, which today says "this file or one of its imports". That is
the same change as making W19 and W6 say why on stderr, done first, on its
own.

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
4. `report_schema` — the file validates against S2's shape.

## Measured outcome

(At implementation.)
