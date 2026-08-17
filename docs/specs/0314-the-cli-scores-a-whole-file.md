<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0314 — the CLI scores a whole file

Status: implemented
Implemented in: 2026-08-17
App: prototext
Refs: docs/specs/0310-….md (`ScoringOpts::end_undeclared`, the −5 charge,
        and the paragraph deferring exactly this change),
      docs/specs/0238-….md (the scan policy, which stays excluded),
      docs/specs/0178-….md (the suspicion order the reported counters
        are printed in)

## Background

Spec 0310 stopped a cut tail from vetoing every candidate, but made the
demotion conditional on `ScoringOpts::end_undeclared` — the caller's
assertion that *this range ends because the bytes ran out, not because a
length prefix said where it ends*. The walk cannot derive that: at depth 0
it holds a buffer and cannot tell a file's last byte from a payload's.

protolens sets the flag. The `prototext` CLI never did — 0310 S7 says so
outright, on the grounds that it is "a CLI contract, not this defect". The
result is that the two tools disagree about the same bytes:

```
$ prototext --descriptor-set bobapp1.desc score -t bobapp.v1.Log boblog
  vetoed: true
$ prototext --descriptor-set bobapp1.desc list-schemas boblog
  types:                                  # empty
```

while protolens, on the same file and the same database, infers
`bobapp.v1.Log` and opens it. `prototext decode boblog` with no `--type`
fails to name a type that protolens names.

That is not a defensible contract. It is the omission 0310 left behind.

## Goals

- **G1.** The CLI reaches the same verdict as protolens on the same bytes
  and the same database.
- **G2.** The cut is visible in the output. Without it `score: 19,
  matches: 24` does not add up and the reader has no way to learn why.

## Non-goals

- **N1.** `fdp-scan-pyo3`. It runs under `Policy::Scan`, where the flag is
  refused by a `debug_assert` (0310 N2): Scan reads an overrun as evidence
  that the candidate root does not start here, and owns the
  termination-offset contract. Unchanged, deliberately.

- **N2.** A score floor, or a minimum-evidence rule that refuses to list
  candidates when the cut left fewer than N scored tags. `list-schemas`
  has never had a floor and a truncated input is not the place to
  introduce one; it is 0310's fourth alternative and still wants the
  multi-depth measurement nobody has taken.

- **N3.** Re-tuning the −5 charge. It is levied once per range on every
  survivor equally, so it cannot reorder a candidate list; only
  cross-input comparisons see it, and this spec adds no new one.

## Specification

- **S1.** All three `ScoringOpts` literals in `prototext/src/run.rs`
  (`decode`, `list-schemas`, `score`) set `end_undeclared: true`.

  Unconditionally, because every buffer this CLI scores is a whole input:
  a file named on the command line, or stdin read to EOF. None of them is
  a byte range carved out by a length prefix. The three are one rule, not
  three judgements — `decode`'s copy feeds `infer_type` and nothing else,
  so it scores the same whole file the other two do.

  The flag does not leak downwards. A LEN payload recursion passes `false`
  itself (`walk.rs:2147`); only a group recursion inherits, which is
  correct, since a group that ran out of bytes ran out of the same bytes
  its parent did.

  Three literals rather than a shared helper: the whole change is one
  field, and wrapping the three sites put `expand_any: !no_expand_any`
  into a function signature where it read worse than the duplication it
  removed.

- **S2.** `truncated` is reported.

  `InferredType` and `run_score`'s `Breakdown` gain the field.
  `write_type_entry` prints `truncated: <bool>` under `--detailed-score`,
  last, after `mismatches` — 0178's suspicion order puts the mildest
  charge at the top, but this is not a counter and printing it among them
  would invite summing it.

  In the YAML it is printed always, so the shape is stable for a reader
  that parses it. In `decode`'s `# Score:` header it appears **only when
  set**, as the bare word `, truncated`: that header sits at the top of
  every inferred decode and `truncated: false` on all of them is noise.

## Alternatives considered

**Change `score` and `list-schemas` but not `decode`'s inference.** What
was literally asked for, and wrong. The input is the same shape in all
three, so the flag is the same value; leaving `decode` out would keep the
case that surfaced this — a capture protolens can name and `prototext
decode` cannot.

**Derive the flag per input, e.g. `true` for a file and `false` for
stdin.** There is no difference to derive. Both are read to their end
before scoring.

**Report the cut by writing `vetoed: true` when truncated.** Keeps the
old output shape and destroys the information: the counters are
meaningful after a cut and meaningless after a veto, which is the whole
distinction 0310 introduced.

## Test plan

1. `list_schemas_scores_a_truncated_capture` — drives the binary, as
   `score_still_honors_no_expand_any` does and for the same reason: the
   three literals are `..Default::default()`, so a field that stops being
   wired up takes its default silently instead of failing to compile.
   A payload cut mid-token lists its type and reports `truncated: true`;
   the same payload intact reports `truncated: false`.
2. The four existing gates, since the change moves published scores.

## Measured outcome

`grpconf/stage/boblog` (20 243 B, three intact log entries and a fourth
cut by 1 024 B), against `grpconf/stage/bobapp1.desc`:

| | before | after |
|---|---|---|
| `score -t bobapp.v1.Log` | `vetoed: true` | `score: 19` (24 matched, cut) |
| `list-schemas` | empty | `bobapp.v1.Log` **+19**, next `google.protobuf.BytesValue` −2 |
| `decode`, no `--type` | all entries vetoed | `# Type: bobapp.v1.Log` |

Twenty-one points clear of the runner-up, which is what protolens's own
inference has been reporting since spec 0310.

No regression on complete inputs:

- `bobshark` (84 B, well-formed) — bit-identical, `−16 / −55 / −59`, and
  now says `truncated: false`.
- A complete 70-byte PNG and a line of prose still list **nothing**,
  under both `bobapp1.desc` and `grpconf/stage/googleapis.desc`. They
  fail on a veto — a garbage tag that met its terminator — not on running
  out of bytes.

What *did* change, and is correct: a **truncated** non-protobuf file now
lists candidates instead of nothing. A PNG cut mid-chunk lists three
types tied at −115 under `bobapp1.desc` and −104 under googleapis. That
is the demotion working as designed — the scores are deeply negative and
tied, so nothing is inferred from them — but a reader who expected
"non-protobuf implies empty" should expect "non-protobuf implies
worthless scores" instead.
