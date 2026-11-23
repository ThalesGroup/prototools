<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0468 — advise --report: a W4 on a dependent of a refused import

Status: implemented
Implemented in: 2026-11-23
App: git-seriate-fdp
Refs: docs/specs/0454-advise-what-a-history-of-descriptors-cannot-hold.md
        (S4: a file refused for its import, reported under it),
      docs/specs/0455-advise-report-what-the-build-said-and-advise-did-not-foresee.md
        (S3, S4: knock-on refusals and their W4s attached under the root)

## Background

A downstream build's `advise --report --strict` fails on one finding:

```
Not foreseen:
   - build            a/service.pb: W4: unresolvable type .a.Row
       build: W4: unresolvable type .a.KeyAndPreconditions
       build: W4: unresolvable type .a.PersistedKey
```

`a/service.proto` imports `a/offline.proto`, which `advise` reports
`refused`. protoback writes `service.proto` with W4s but doesn't refuse
it (no W19), so 0455's rules, which follow knock-on refusals by
protoback's own `import` field, don't apply. Yet `advise`'s pool does
refuse `service.proto`, as a dependent of `offline.proto`, and reports it
once, under that root (0454 S4): it knows, and drops what it knows
(`refusals`, `problems.rs`, filters out the pool's `… was skipped`).

## Goals

- **G1.** A W4 on a file `advise`'s pool refused only because an import
  was refused attaches where that import's root refusal attached.

## Non-goals

- **N1.** Other codes on such a file: a W19 or W6 on it carries
  `import`, and 0455 S3 already follows it.

## Specification

- **S1.** `refusals` keeps, beside the files refused on their own, each
  file refused because an import was: the file and that import, followed
  to its root (a chain of skipped imports, a loop stopping it, as 0455
  S3 does).

- **S2.** In `--report`, a W4 on such a file attaches where its root's
  refusal attached (0455 S4's step), after protoback's knock-on chains:
  first protoback's `import` field, then `advise`'s own map. A root no
  problem of `advise` holds leaves the W4 where it would have gone.

## Test plan

1. `a_w4_on_a_dependent_of_a_refused_import` — `b.proto` refused for a
   type it lacks, `c.proto` importing it and using a type of it; the
   report holds a W19 on `b` and a W4 on `c`, no W19 on `c`: the W4
   under `b`'s refused problem, `--strict` passing.
2. The 0455 tests unchanged.

## Measured outcome

2026-11-23. `a_w4_on_a_dependent_of_a_refused_import` passes (a W4 on
`grand.proto`, refused by advise's pool for `child.proto`, itself for
`user.proto`, goes under `user.proto`'s refusal; nothing left as
`build`); the 0455 tests are unchanged. The downstream case is not run
here.
