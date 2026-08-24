<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0351 — a poor fit is not a call to action

Status: implemented
Implemented in: 2026-08-24
App: protolens
Refs: docs/specs/0336-the-square-is-the-scale.md (the three hues and
        the N3 deferral this spec resolves),
      docs/specs/0337-the-scale-learns-its-top.md (S1's floor at
        score 1, S7's measurement mandate)

## Background

Spec 0337 S1 defined `t = 0` for any `best_score < 1`. Spec 0336 N3
deferred the hue question for negative best scores, pending
measurement. The measurement is now in. Running protolens against a
payload where the schema only partially matches produces:

```
■ ▼ 1 {  #@ message [-/-35]
■ │ ▶ 1 { ... }  #@ message [-/-37]
■ │ ▶ 1 { ... }  #@ message [-/-37]
■ │ ▶ 1 { ... }  #@ message [-/-37]
■ │ ▶ 1 { ... }  #@ message; TRUNCATED_MESSAGE; MISSING: 1024 [-/-32]
```

All five squares are green and all five suffix scores are deeply
negative. The `Mismatch` cue has two hues — green and amber — and the
choice between them is `best_score`: green means the best candidate
scores well enough to act on; amber means the best candidate is itself
a poor fit. Today both are green, making the distinction invisible.

Spec 0336 S4's cue table for the second display mode (mismatches and
ties):

| display | square | word |
|---|---|---|
| `Cue(Mismatch)` | green, graded | flat green |
| `Cue(Tie)` | blue, graded | flat blue |
| `Settled { None }` | amber, flat | flat amber |

This spec splits `Cue(Mismatch)` on the sign of `best_score`, leaving
`Cue(Tie)` and `Settled { None }` unchanged.

## Goals

- **G1.** `Cue(Mismatch)` with `best_score >= 0` remains green
  (graded square, flat green suffix) — the best candidate scores
  well; the reader should act.
- **G2.** `Cue(Mismatch)` with `best_score < 0` becomes amber
  (graded square, flat amber suffix) — the best candidate is itself
  a poor fit; acting on it is unlikely to help.
- **G3.** The LHS square brightness (`t`) is computed the same way
  for amber as for green: `heat_fraction(best, anchor)`. A negative
  `best` yields `t = 0` (the floor) by the existing formula — no
  change to `heat_fraction`.
- **G4.** `Cue(Tie)` stays blue. `Settled { None }` stays amber.
  Neither is touched.

## Non-goals

- **N1.** *A graded amber ramp for negative scores.* The suffix number
  already expresses the magnitude. `t = 0` (the floor) for all
  negative bests is correct: the scale runs from small-positive to
  large-positive, and everything below it is simply "not on the
  scale."
- **N2.** *Changing the hover box color.* The box text already conveys
  the score; adding color there compounds the number of hues the
  reader must track.

## Specification

- **S1.** In `heat_chrome`, the hue for a `Mismatch` cue is derived
  from `best`:

  ```
  if best >= 0 { HeatHue::Green } else { HeatHue::Amber }
  ```

  This hue is applied to both the LHS square (`heat_style(t, hue,
  theme)`) and the RHS suffix word (`heat_label_style(hue, theme)`).
  The `t` computation (`heat_fraction(best, anchor)`) is unchanged.

- **S2.** Spec 0336 S4's table becomes:

  | display | square | word |
  |---|---|---|
  | `Cue(Mismatch)`, `best >= 0` | green, graded | flat green |
  | `Cue(Mismatch)`, `best < 0` | amber, graded | flat amber |
  | `Cue(Tie)` | blue, graded | flat blue |
  | `Settled { Some(n) }` | none | flat blue |
  | `Settled { None }` | amber, flat | flat amber |
  | `PendingCurrent` | none | comment |
  | `Unknown` | none | comment |

## Alternatives considered

**Keeping green for all mismatches.** The status quo. Green at `t = 0`
is visually quiet but semantically wrong: it still says "a better type
exists" when the best candidate scores below zero.

**A graded amber ramp for negative scores.** Would require a second
anchor for the negative tail and duplicate information already in the
suffix number. See N1.

## Test plan

1. `a_negative_best_mismatch_is_amber` — `heat_chrome` on a
   `Mismatch { best: -35, current: Some(-35) }` cue yields an amber
   square and an amber suffix.
2. `a_zero_best_mismatch_is_green` — `Mismatch { best: 0, .. }` yields
   green. The boundary is explicit.
3. `a_positive_best_mismatch_is_green` — existing behavior pinned.
4. `a_tie_is_always_blue` — `Cue(Tie)` is unaffected by this spec.

## Measured outcome

One line changed in `render.rs` (`heat_chrome`'s hue derivation for
`Mismatch`). Four tests added to `tui/tests/heat_cue.rs`. 1218
protolens tests, 25 theme tests, 3 batch tests — all pass.
