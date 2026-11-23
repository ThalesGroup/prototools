<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0474 — reproto, protoback: a refusal said once

Status: draft
App: reproto | protoback
Refs: docs/specs/0438-warning-codes-and-fail-on.md (the squashed summary lines),
      docs/specs/0446-build-report.md (the report, every occurrence),
      docs/specs/0455-advise-report-what-the-build-said-and-advise-did-not-foresee.md
        (advise reads every W19, its `import` field)

## Background

A file the descriptor pool refuses is said twice on stderr: W19 when its
text is rendered, W20, with the same reason, when the schema database
leaves it out. And a file refused only because one of its imports is
gets a line of its own ("the descriptor pool refused its import …"),
down chains of up to five files. On a downstream corpus, 68 of 111
summary lines are W19 and W20, for 34 files with 6 roots.

## Goals

- **G1.** On stderr, a refusal is said once: no W20 repeating a W19, and
  the files refused only through an import counted under their root.
- **G2.** Nothing lost: every occurrence stays in the report and under
  `--detailed-warnings`; `--fail-on W20` and `--fail-on W19` fail as
  before.

## Non-goals

- **N1.** The report's content: `advise --report` (0455) reads every W19,
  its `import` field included; it stays.
- **N2.** Other codes: W4 and W5 share causes too, but each names a
  different missing thing.

## Specification

- **S1. W20 after W19.** A W20 whose file has a W19 with the same reason
  isn't printed on stderr; it is counted, recorded in the report, and
  printed under `--detailed-warnings`. A W20 with another cause (no
  scoring graph at all, a file refused for the schema database alone)
  prints as today.

- **S2. Knock-on refusals counted under the root.** A W19 whose reason is
  a refused import isn't printed on its own line; its root's line (the
  file refused for a reason of its own, followed through the chain of
  imports, a loop stopping it) says how many: `'a/b.proto' : the
  descriptor pool refused it: couldn't resolve name '.p.T' (and 8 files
  importing it)`. A knock-on whose chain leads to no root of its own
  prints as today. Under `--detailed-warnings`, every occurrence, as
  today.

- **S3. Both tools.** reproto and protoback say the same, byte for byte:
  the differential harness compares their stderr.

- **S4. The report** (`--report-out`) and the exit status are unchanged.

## Test plan

1. `a_refusal_is_said_once` — a file refused, two files importing it, one
   importing them: on stderr one W19 line, `(and 3 files importing it)`,
   no W20; the report holds every W19 and W20; `--detailed-warnings`
   prints them all.
2. `fail_on_still_fails` — `--fail-on W20` exits as before.
3. Both tools (the suite runs on reproto and on protoback); the harness,
   both modes.

## Measured outcome

Filled in at implementation.
