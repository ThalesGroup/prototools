<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0178 — out-of-range is a penalty, not a veto; and coefficients rank by suspicion

Status: implemented
Implemented in: 2026-07-26
App: prototext-graph, prototext
Refs: docs/scoring-flaws.md (C12),
      docs/specs/0176-open-enums-have-no-range.md,
      docs/specs/0077-varint-range-veto.md,
      docs/specs/0090-cli-review.md (§3, F5),
      docs/specs/0049-groups-and-labels.md (superseded score expectations)

## Background

The governing principle for veto decisions (`docs/scoring-flaws.md`,
cross-cutting section) is: **veto only for what the wire format makes
impossible; score everything that is merely unlikely.**

After spec 0176 removed open-enum ranges, the `Range` leaf in
`check_varint_value` reaches exactly two kinds of field, and under
`strict_ranges` an out-of-range value on either one **vetoes**:

- **`bool`**, range `(0, 1)`. A `bool` is decoded as `value != 0` in every
  generated parser, so `2` is a legal `bool` on the wire; it parses to `true`.
- **A closed enum.** Closed-enum semantics move an unrecognized number to the
  *unknown-field set* rather than failing the parse, so the message still
  round-trips.

Neither value is impossible. Both are strong evidence against the candidate.
That is the definition of a penalty, and flaw C12 records it.

### The performance objection, and why it does not hold

A veto is not only a verdict, it is a **prune**. `walk.rs:1075` clears
`ae.entries`, `:1092` drops the emptied `ActiveEntry`, and `:1043-1045` returns
early once `active` is empty — so a veto that kills the last candidate stops the
byte scan outright. Veto is the only mechanism that makes scoring sublinear in
blob size. Demoting one therefore trades accuracy for work, and that trade has
to be argued rather than assumed.

It holds here, for three reasons:

1. **protolens has always run without this prune.** `ScoringOpts::default()` is
   `strict_ranges: false` (`walk.rs:222`), and the only writers of `true` are
   `prototext/src/run.rs:423`, `:500`, `:532`. protolens never overrides the
   default, so the most latency-sensitive consumer in the workspace — an
   interactive TUI whose `heat_worker` calls `score_all` with no lock held — has
   never had the range prune, and it has never been reported as a problem. This
   spec makes the CLI behave the way protolens already does.
2. **The prune is narrow.** It requires a conjunction: the field number matches,
   the wire type is varint, the leaf is `bool` or a closed enum, *and* the value
   is out of range. The dominant pruner is the wire-type mismatch at
   `walk.rs:1030`, which this spec does not touch, and `Verdict::Unknown`
   candidates are not pruned by anything today.
3. **The sound veto in the same arm survives.** `walk.rs:696-698` vetoes a value
   in the gap between "too big for `u32`" and "smallest sign-extended `i32`" —
   neither encoding of any 32-bit number, hence genuinely impossible. It is not
   gated on `strict_ranges` and it still prunes.

### The ranking power is already there

The score (`walk.rs:60-65`) is:

```rust
matches - 10 * unknowns - 10 * mismatches - 20 * non_canonical
```

`non_canonical` is the heaviest term at `-20` — twenty times the `+1` a matched
field earns — so one such value cancels twenty matches and three cancel sixty.
On any realistic message that sinks the candidate far below the true type. The
demotion gives up the hard prune, not the ranking.

This is deliberate posture, not an oversight: no typical SDK emits a
non-canonical encoding, so its presence is extremely suspicious. `-20` says so.

### The four signals are two kinds of evidence

Adding a fifth counter forces the question of how the existing four rank, and
the current weights do not survive it. `mismatches` has exactly **one**
increment site — `walk.rs:785`, label 1 (required), `count == 0` — so it means
precisely *a proto2 `required` field this schema declares is absent from the
blob*. The name is misleading: it reads like a wire-type mismatch, which
**vetoes** instead (`walk.rs:1030`) and never touches this counter.

That splits the four:

- **Writer-conformance evidence** — `non_canonical`, `out_of_range`. The schema
  still fits; the writer was weird.
- **Schema-fit evidence** — `unknowns`, `mismatches`. The schema itself is
  contradicted by the data.

Schema fit is what the scorer is trying to measure, so `mismatches` belongs
*above* `non_canonical`, not at half its weight. And the asymmetry between the
two schema-fit signals is justified rather than arbitrary: an unknown field has
a benign explanation — a newer sender talking to an older schema, which is the
whole point of forward compatibility — while an absent `required` field has
none, since `required` is exactly the thing a conformant writer cannot omit.

Hence the order, from mildest to most damning:

```
matched  <  unknown  <  out_of_range  <  non_canonical  <  mismatches
```

**Caveat worth recording:** these coefficients — the pre-existing ones included
— are folklore. No spec states a derivation and there is no labeled corpus
behind them. S1 below is an *order-preserving* adjustment, not a fitted model.
See "Later" for the idea that would make them measurable.

### Why `strict_ranges` goes away entirely

`strict_ranges` has exactly **one** reader, `walk.rs:716`, gating only this
veto. Once the veto is gone the flag selects nothing, and a flag that silently
does nothing is worse than a flag that no longer exists.

Note also that `--relax-ranges` was the only escape from the false veto, so its
removal costs nothing: the behavior it selected is now unconditional.

## Goals

1. An out-of-range `bool` or closed-enum value is penalized, never vetoed.
2. The penalty is **counted separately** from `non_canonical` in reports, so
   consumers can tell "value outside the declared range" from "sloppy
   encoding".
3. The coefficients rank in the same order the reports display, so the line a
   user reads explains the number beside it.
4. `strict_ranges` and `--relax-ranges` are removed, not left inert.
5. The impossible-value vetoes in the same arm are untouched.

## Non-goals

- **Fitting the coefficients to a corpus.** S1 preserves the existing relative
  order and two existing anchors; it does not claim the numbers are optimal.
- **A replacement prune** (score-bound / branch-and-bound over the best
  achievable future score). Reason 1 above says the lost prune is not
  load-bearing; if a measurement ever says otherwise, that is its own spec, and
  the answer is a sound prune rather than a reinstated unsound veto.
- **Any compiled-graph format change.** `EntryScore` is internal; `ranges` and
  `range_idx` keep their meaning. No `PTSGRAPH` version bump.
- **Changing which fields get a `Range` leaf.** Settled by spec 0176.

## Specification

### S1 — a separate `out_of_range` counter, and suspicion-ordered coefficients

Add to `EntryScore` (`walk.rs:52-57`):

```rust
pub out_of_range: u64,
```

and rewrite `score()` so the coefficients rank in the order of § "The four
signals are two kinds of evidence":

```rust
pub fn score(&self) -> i64 {
    self.matches as i64
        - 10 * self.unknowns as i64
        - 15 * self.out_of_range as i64
        - 20 * self.non_canonical as i64
        - 30 * self.mismatches as i64
}
```

| signal | before | after | one occurrence cancels |
|---|---|---|---|
| `matches` | `+1` | `+1` | — |
| `unknowns` | `-10` | `-10` | 10 matches |
| `out_of_range` | — | `-15` | 15 matches |
| `non_canonical` | `-20` | `-20` | 20 matches |
| `mismatches` | `-10` | `-30` | 30 matches |

Only `mismatches` changes, and only upward. Two anchors are preserved
deliberately: `unknowns` at `-10` and `non_canonical` at `-20`. A tidier
progression (`10/20/30/40`) would have moved `non_canonical` and rewritten every
documented score in `docs/tutorial.md` for no behavioral gain — see S5.

Initialize the new field at all three construction sites (`walk.rs:253`,
`:293`, and the test helper at `:1534`).

The five counters stay public on `EntryScore`. protolens is expected to surface
the breakdown on hover over a consolidated score, so a later cleanup must not
privatize them or fold them into `score()`.

### S2 — demote the veto

In `check_varint_value`, replace the `strict_ranges` branch (`walk.rs:716-718`)
with an `out_of_range` increment, so the out-of-range path is unconditional:

```rust
for &e in &ae.entries {
    ws.scores[e as usize].out_of_range += 1;
}
false
```

Everything else in the `0 if ri != 0xFFFF` arm stays exactly as it is — in
particular the impossible-gap veto at `:696-698` and the `non_canonical` charge
at `:699-703` for a negative written in the non-canonical 5-byte form.

A value that is *both* non-canonically encoded *and* out of range keeps
accruing both charges, as it does today (`-40`). That is two independent
suspicions and is intended.

### S3 — delete `strict_ranges` and `--relax-ranges`

Remove, in order:

- `WalkState::strict_ranges` (`walk.rs:120`) and its initializer (`:137`)
- `ScoringOpts::strict_ranges` (`walk.rs:196`), its `Default` (`:222`), and the
  test-helper literal at `:1491`
- the three `strict_ranges: !relax_ranges` sites (`run.rs:423`, `:500`, `:532`)
  and the `relax_ranges` bindings that feed them (`:413`, `:486`, `:518`)
- the three clap definitions (`prototext/src/lib.rs:186-193`, `:264-268`,
  `:305-309`), including the `alias = "no-strict-ranges"`, which removes both
  spellings

`prototext --help` and the man page are generated from clap, so both follow
automatically; `prototext-gen-man` needs no edit.

Check every `ScoringOpts` construction site compiles after the field is gone —
notably protolens, which uses the `Default`.

### S4 — report `out_of_range` on both surfaces

There are two report surfaces, and both must show the new counter. Leaving it
off the summary line would print `non_canonical: 0` beside a score of `-60`,
i.e. counters that no longer explain the number next to them.

**Field order on both surfaces is `matched, unknown, out_of_range,
non_canonical, mismatches`** — increasing order of suspicion, which after S1 is
also increasing order of coefficient magnitude. The two agreeing is the point:
a reader can go left to right and see the penalties grow.

1. **The inference header** (`run.rs:250-258`):

   ```
   # Score: {}  (matched: {}, unknown: {}, out_of_range: {}, non_canonical: {}, mismatches: {})
   ```

2. **The detailed YAML breakdown** (`run.rs:264-273`, gated on
   `--detailed-score`), in the same order.

Carry the field through the structs that mirror `EntryScore`:
`InferredType` (`run.rs:154-161`) and its two populate sites (`:216-225`,
`:233-240`); `ScoreEntry::Scored` (`run.rs:1053-1060`) and its two push sites
(`:1105`, `:1127`).

While doing so, replace the `score_input` closure's return type
(`run.rs:1067`) — currently a 6-tuple of one `bool` and five `u64`s, destructured
positionally at `:1098` and `:1120` — with the existing `InferredType` plus the
`vetoed` flag. Widening it to a 7-tuple would make an already error-prone
positional signature worse; this is the minimum change that avoids that, not a
general refactor.

Update the `--detailed-score` doc comments that enumerate the dimensions
(`prototext/src/lib.rs:180-181`, `:257`).

### S5 — documentation

- `docs/schema-match-impl-notes.md:86` — the formula; `:73` — the prose listing
  what accumulates in `non_canonical`; `:79` and `:262` — worth naming there
  that `mismatches` counts *only* an absent `required` field, since the name
  suggests otherwise.
- `docs/tutorial.md` — the header line appears verbatim at `:105`, `:231`,
  `:263`, `:339`, `:408`. Only the **format** changes: `:105`, `:231`, `:263`
  and `:408` have all-zero counters, and `:339` is `9 - 20×1 = -11` with
  `mismatches: 0`, so every printed score stays correct under S1. Any diff to a
  number here means S1 was implemented wrong.
- `prototext/README.md:170` — same line, same reasoning (all-zero counters).
- `docs/scoring-flaws.md` — C12 becomes resolved, citing this spec; drop the
  `--relax-ranges` escape-hatch argument at `:579-585` and the CLI-default note
  at `:295`; extend the "do not fix a `non_canonical`" rule at `:772-782` to
  name the out-of-range case as the canonical example of a penalty that should
  *not* be promoted back to a veto.
- `docs/protolens/rendering-worklist.md:515` — same stale `relax_ranges` note.

## Test plan

### Flips from veto to penalty

- `tests.rs:945-961` (TC-77-01) — **`bool` wire value `2` is no longer vetoed**;
  assert `out_of_range == 1`, `non_canonical == 0`, and `!vetoed`.
- `tests.rs:964-974` (TC-77-02) — closed enum `99` outside `[0, 2]`: same shape.
  This subsumes TC-77-03 (`:977-988`), which asserted the old non-strict path;
  fold the two into one test now that there is one behavior.
- `tests.rs:723-742` — nested `Outer` with enum `99`: `Outer` is now penalized,
  not vetoed. Remove the `strict_ranges: true` literal at `:734`. `Inner`
  continues to take an unknown, unchanged.
- `tests.rs:1449-1466` — negative enum `-99` outside `[0, 3]`: penalized in
  both halves of the test; the `strict_ranges: true` arm disappears.
- `tests.rs:285` and `:1148-1151` already assert the penalty path; retarget the
  assertion from `non_canonical` to `out_of_range`.
- `tests.rs:764` — the `score_one`/`score_all` agreement check must compare
  `out_of_range` too, or the fast path could diverge on the new counter
  unnoticed.

### Coefficients

- A direct unit test on `score()`: one occurrence of each counter in isolation
  yields `-10`, `-15`, `-20`, `-30`, and the five weights are strictly ordered.
  Asserting the *order* as well as the values is what stops a future edit from
  silently reintroducing the contradiction this spec fixes.
- No Rust test currently asserts a numeric score with `mismatches > 0`
  (verified: the only score comparison in `tests.rs` is the inequality at
  `:1153`), so the `-10` → `-30` change breaks nothing and is *unguarded*. The
  unit test above is therefore the only thing that will hold it — write it
  first.
- `tests.rs:1148-1156` needs care: `bool = 802` moves from `non_canonical` to
  `out_of_range`, so its penalty goes `-20` → `-15`. The `int32_s.score() >
  bool_s.score()` assertion still holds, but the counter assertion at `:1150`
  must retarget or it will pass for the wrong reason.
- `docs/specs/0049-groups-and-labels.md:519` and `:525-532` tabulate expected
  scores that S1 invalidates (GL-05's `score=-10` becomes `-30`). Those are
  historical records of what 0049 implemented and should be **superseded, not
  retroactively edited** — the same treatment 0176 and 0175 gave the specs they
  changed.

### Must not change

- `tests.rs:991-1008` (TC-77-04) — a value in the impossible varint gap still
  **vetoes**. This is the guard proving only the right half was demoted; if it
  ever flips, the arm has been gutted.
- `tests.rs:1011-1035` — the `UINT32` and `INT32` vetoes are in different arms
  and are untouched.
- Every printed score in `docs/tutorial.md` and `prototext/README.md`, per S5.

### End to end

Per `docs/scoring-flaws.md`'s reproto recipe (scoring tests need a
`hopcroft.rkyv`, so they live in reproto's pytest suite):

- A blob whose `bool` field carries `2`, scored against the true type: appears
  in `list-schemas` (it used to be omitted), with a score reduced by `20`.
- The true type still outranks a decoy despite the penalty — the ranking claim
  in "The ranking power is already there" asserted, not just argued.
- `prototext decode --detailed-score` shows `out_of_range: 1` and the header
  line's five counters reconcile with the printed score.
- `prototext decode --relax-ranges` now **fails** with an unknown-argument
  error, as does `--no-strict-ranges`.
- `reproto/src/reproto/tests/test_open_enum_scoring.py:17` — the comment
  explaining the CLI/`ScoringOpts` default split is obsolete; the two now agree.

### Suite

- `cargo test --release --no-default-features --workspace` green.
- `cargo clippy --release --no-default-features --workspace -- -D warnings` and
  `cargo fmt --all --check` clean.
- `nix-build -A ci` green.

## Later — caller-provided coefficients

Recorded as a design idea, **not** part of this spec.

The weights above are hardcoded constants with no recorded derivation. The idea
is to let the caller supply them, one per counter, which would turn the
veto/penalty distinction from code into data — C12 would have been a
configuration change rather than a spec.

Two notes for whoever picks it up.

**Represent veto explicitly, not as a sign.** The tempting encoding is one
signed int per counter, where a negative value means "veto". That spends the
whole negative range to express one bit and reads as a sentinel; the next person
to add a sixth counter will miss it. Prefer:

```rust
enum Weight { Penalty(u32), Veto }
```

Same expressiveness, and the veto case becomes impossible to overlook at the
match site.

**Veto is a prune, not just a weight — so this makes performance
configuration-dependent.** `walk.rs:1075` clears the entry, `:1092` drops the
`ActiveEntry` and `:1043` returns early once none are left, so a veto shortens
the byte scan. Today that early exit is structural. Once vetoes are data, the
same blob and schema scan a different number of bytes under different
coefficients: any benchmark number would have to record the coefficient set
alongside it, and `bin/bench` workloads would need pinning to a fixed set.

**And it reopens a knob this spec deliberately closes.** S3 deletes
`--relax-ranges` on the grounds that a flag which silently selects nothing is
worse than no flag. A general coefficient mechanism would let a caller veto on
`unknowns` — i.e. veto ordinary forward-compatible traffic — which the governing
principle forbids. That is defensible for an expert tool, but it should be a
deliberate decision rather than drift.

**The strongest argument for it** is the folklore caveat in the Background:
coefficients supplied from outside can be *fitted* against a labeled corpus,
which turns the weights from asserted constants into a measured model. That is
the version worth building, and it wants the corpus first.
