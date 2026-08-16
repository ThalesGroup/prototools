<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0310 — a cut file is not a wrong file

Status: implemented
Implemented in: 2026-08-16
App: prototext-graph, protolens
Refs: docs/specs/0299-….md (the `message` override, and the two fixes it
        rejected — one of them was a version of this one, for reasons
        that do not bind here; see Background),
      docs/specs/0303-….md (`TRUNCATED_MESSAGE; MISSING: N`, and its
        note anticipating a scoring tier that penalizes truncation),
      docs/specs/0238-….md (the scan policy, the termination offset, and
        the per-boundary snapshot deferred there),
      docs/specs/0178-….md (the score coefficients and the suspicion
        ordering they are ranked by),
      docs/specs/0266-….md (`ProbeSink`: any invalid token disqualifies —
        the *render* verdict, which this spec does not touch)

## Background

Open a truncated protobuf in protolens and the override selection pane's
inferred list is empty. Not short — empty. `grpconf/stage/boblog` (20 198
bytes, three intact log entries and a fourth cut by 1 024 bytes) offers
no candidate for any node, including the document root, so
`determine_root_type` resolves nothing and the file opens untyped.

The cause is one line. `walk.rs:1650`:

```rust
let Some(end) = payload_end(pos, lr.value, buflen) else {
    veto_all(active, ws, "LEN body extends past end of buffer");
    return buflen;
};
```

`veto_all` sets the veto bit on every entry still active at that frame,
and `sweep::rank` (`protolens/src/sweep.rs:721`) filters `!r.vetoed`
before ranking. Every candidate that had survived twenty kilobytes of
matching evidence is discarded by the last token in the file.

That is the wrong verdict by the scorer's own governing rule
(`docs/scoring-flaws.md`): *veto only for what the wire format makes
impossible; score everything that is merely unlikely.* A message cut by
a capture is not impossible. It is a complete message that the reader
does not have all of.

**Why the veto looked cheap, and is not.** A veto clears `active`, and
the loop's `if pos == buflen || active.is_empty()` then returns at once;
this pruning is what keeps the walk sublinear in blob size, so demoting
a veto normally costs wall clock. Truncation is the exception, and
provably so:

> For a range cut at its tail, the overrun is always detected in the
> outermost frame. If every enclosing declared length had fit inside the
> available bytes, then by induction the innermost declared end is also
> within them and no overrun exists.

So this veto fires at the last token, after every earlier byte has
already been scored. There is nothing left to prune. It is the one veto
in the walk that buys no pruning at all.

**Why spec 0299 rejected a version of this, and why that does not
bind.** 0299 gave three reasons. The first — *the scorer is never
consulted for the untyped render* — was true and decisive **for the
render**, which is what 0299 was fixing; it says nothing about the
candidate list, which is the scorer. The second — *it would break the
gRPConf beat that opens boblog untyped* — is a real consequence and is
addressed in N3 below. The third — *"penalize but keep the score" needs
0238's deferred per-boundary snapshot* — does not apply: that snapshot
exists to unwind counters polluted by bytes past a record's true end,
and at a tail cut there are no bytes past it.

## Goals

- **G1.** A range whose end is the end of the available bytes scores the
  bytes it has, rather than refusing to score at all.
- **G2.** No pruning is lost. Every veto that can still prune keeps
  vetoing; only the one that fires at the last token is demoted.
- **G3.** The cut is visible in the result — a caller can tell a
  complete reading from a provisional one without re-parsing.

## Non-goals

- **N1.** Changing the default render. `ProbeSink` (spec 0266) is
  schema-free, never consults a score, and still disqualifies a parent
  containing a truncated field; spec 0303's N2 says so and stays true.
  A truncated node still renders as `TRUNCATED_BYTES` until the reader
  overrides it as `message` (spec 0299). No new automatic
  bytes-versus-message criterion is introduced, and none is needed:
  the reader is the criterion.

- **N2.** Changing `Policy::Scan`. Scan hunts records in a haystack,
  where an overrun is exactly the evidence that the candidate root does
  not start here; it also owns the termination-offset contract (spec
  0238), which a demoted overrun would have to define a value for.
  S1's flag is refused under Scan. `fdp-scan-pyo3`, which accepts a
  candidate only when `!vetoed && unknowns == 0 && mismatches == 0`, is
  therefore unaffected.

- **N3.** Changing `pick_winner`. It takes rank 1 unless the top two tie,
  with no score floor, so after this change a truncated file **can**
  auto-resolve a root type at startup. That is the intended outcome —
  boblog opening typed is the point — but it also means a file cut after
  two tags could resolve on almost no evidence. S4's constant is the
  brake (a cut range scores negative below five matched fields), and it
  is deliberately only a ranking brake, not a floor. If a floor turns
  out to be wanted it belongs in `pick_winner`, applies to more than
  truncation, and is a separate spec.

  The gRPConf beat that opens boblog untyped is the thing this breaks.
  Re-pinning it is tracked with the other beat work; the scripts already
  carry `STALE` headers.

- **N4.** The per-boundary snapshot deferred by spec 0238. Not needed
  here — see Background.

- **N5.** Charging per missing byte, or scaling the charge by how much
  was cut. The reader has no way to know how much is missing when the
  file simply ends, and `MISSING: N` (spec 0303) exists only where a
  length prefix survived to say so.

## Specification

- **S1.** `ScoringOpts` gains `end_undeclared: bool`, default `false`:
  *this range ends because the bytes ran out, not because a length
  prefix said where it ends.*

  It is a static property of where the range came from, not a discovered
  one, which is what makes it answerable before the walk: the whole file
  always has it; a byte range carved out by a declared length never
  does; a truncated node's available bytes have it again. A well-formed
  file never reaches a demoted site, so the flag costs it nothing.

  Every existing `ScoringOpts` literal uses `..Default::default()`
  (`prototext/src/run.rs:436,514,545`, `fdp-scan-pyo3/src/lib.rs:172`,
  `prototext-graph/src/score/tests.rs:2391`), so adding the field breaks
  no caller.

  Setting it together with `Policy::Scan` is a `debug_assert` failure
  (N2).

- **S2.** With `end_undeclared` set, the veto sites that mean *the bytes
  ran out* charge S3's counter instead of vetoing. They still return
  immediately — the range is over either way.

  | site | condition |
  |---|---|
  | `walk.rs:1390` | EOF while inside a group |
  | `walk.rs:1421` | wire tag ran out |
  | `walk.rs:1566` | varint body ran out |
  | `walk.rs:1611` | I64 body ran out |
  | `walk.rs:1635` | LEN length prefix ran out |
  | `walk.rs:1650` | LEN payload extends past the end |
  | `walk.rs:2022` | I32 body ran out |

  The three remaining `veto_all` sites are untouched, and none of them
  is about running out of bytes: `:1381` recursion depth, `:1948`
  malformed unknown group, `:1999` `END_GROUP` outside a group.

  The four bounds checks are unambiguous. The three varint sites are
  not: `VarintResult::garbage` is `Option<()>` and flattens *truncated*
  together with *overflowed past ten bytes* (`walk.rs:659,669,678`).

  **`next_pos` does not separate them either** — an earlier draft of
  this spec said it did, and was wrong. The overflow path sets
  `next_pos = buflen` deliberately, to match Python's `varint_gar`
  content (`prototext-core/src/helpers/varint.rs:220-228`), exactly as
  the truncated path does. Had that gone in, *every* overlong varint
  would have been read as a cut, and a non-protobuf file would have
  stopped vetoing.

  What does separate them is that a truncated varint never met a
  terminator: every byte from the parse's start onwards carries the
  continuation bit. `varint_ran_out(buf, pos)` is that test, at the
  three sites, rather than a widened result type — the distinction is
  wanted only where the walk is already returning, and the flattening
  is on the hot path. The scan it costs is bounded by the rest of the
  buffer and runs at most once per walk.

  An overlong varint that runs to the end of the buffer without a
  terminator is then read as a cut. It is one: the terminator may well
  have been among the bytes the cut removed.

- **S3.** `EntryScore` gains `truncated: bool`, and
  `protolens::ScoreBreakdown` mirrors it.

  A `bool` rather than a counter: the demoted sites return, so it can be
  set at most once per walk, and a count that is only ever 0 or 1 would
  invite readers to sum it. The hover popup (spec 0280) shows per-
  category counts, where "cut" reads correctly and "truncated: 1" does
  not. `EntryScore` is constructed only inside `prototext-graph` (three
  literals) and is not a serialized structure, so no graph version bump.

  It also carries G3: `vetoed` used to be the only way a caller could
  tell that a reading was incomplete, and the demotion takes that away.

- **S4.** The coefficient is **−5**, and `EntryScore::score` gains
  `- 5 * self.truncated as i64`.

  Spec 0178's ladder is ordered by what the evidence is *about*:
  `unknowns` −10 is mildest because forward compatibility is a benign
  explanation; `out_of_range` −15 and `non_canonical` −20 are about
  writer conformance; `mismatches` −30 is about schema fit. A cut is
  about the *capture* — it says nothing about the writer and nothing
  about whether the schema fits — so it belongs below the mildest rung
  already there. `score_coefficients_rank_by_suspicion` gains it at the
  bottom.

  Why not zero: the score is compared across ranges
  (`determine_root_type`, the heat cues), and a cut reading is
  provisional in a way a complete one is not.

  Why five: a fixed constant is self-scaling against the evidence, and
  five is the number of matched fields a cut range must show before it
  scores positive at all. boblog has thousands and the charge is noise,
  correctly. A node cut after one tag goes negative, correctly.

  **The charge cannot reorder a candidate list.** It is levied once per
  range, on every survivor equally, so within one pane it is a constant.
  Its only effects are cross-range. That bounds how much getting the
  number wrong can cost, and is the reason not to agonize over it.

- **S5.** The cut frame runs no end-of-frame cardinality pass, which is
  what the code already does (`veto_all` returns before `walk.rs:1394`)
  and is now the stated rule rather than a side effect: a cardinality
  check is a statement about a frame that ended, and this one did not.

  Without the rule the demotion would half-work. `apply_cardinality_multi`
  charges `mismatches += 1` per `required` field with zero occurrences
  (`walk.rs:1170-1174`) — that is −30 for a field that may well have
  been in the bytes the cut removed, which on a schema with two required
  fields would swamp everything the demotion just recovered.

- **S6.** `EntryScore::termination` is `pb.len()` under `Policy::Score`
  unconditionally, so the demotion introduces no new value for it. Its
  doc comment's "meaningless on a vetoed entry" gains "and on a
  truncated one".

- **S7.** protolens sets the flag from one predicate,
  `override_pane::ends_where_the_bytes_end(range, blob_len)`, used by
  every candidate query it makes: the startup sweep (whose range is the
  whole document, so always true), the heat worker's part walks and its
  single-score fast path, the no-worker synchronous arm of
  `heat_cue_resolve`, and the score popup.

  The predicate is `range.end >= blob_len`. A range carved out by a
  declared length that fitted ends *before* the blob does; a document,
  and a truncated node's available bytes (spec 0302), end exactly where
  the blob does. The one case it reads generously is a node whose
  declared length happens to end at the file's last byte — telling that
  apart from a cut needs the arena, which none of these callers holds,
  and the cost of being wrong is a candidate list charged five points
  where an empty one would also have been defensible.

  `sweep::ranked`, `sweep::ranked_with` and `Partition::walk` take it as
  an argument and fold it into a local copy of the stored `ScoringOpts`
  (which gains `Clone` for the purpose), rather than `Partition` caching
  it: the partition is shared for the life of the process and the flag
  is per query.

  The `prototext` CLI is deliberately not changed. `list-schemas` on a
  truncated file still lists nothing; whether it should is the same
  question for `score` and for `fdp-scan-pyo3`, and it is a CLI
  contract, not this defect.

- **S8.** The score popup (spec 0280) prints the cut as a term, last —
  it is the mildest charge and the box reads worst-last — and without a
  count, since `truncated` is a bool and `1 ×` would invite the reader
  to wonder what two of them would mean. Its weight sits in the same
  column as every other term's, so the lines still visibly sum to the
  total beneath them. Unlike the vetoed box, which prints no counts at
  all, the cut box prints all of them: they are still true of everything
  before the cut.

## Alternatives considered

**Demote unconditionally, with no caller flag.** Then a nested field
whose declared length overruns its parent's declared end — a frame
contradicting itself, mid-buffer, unresyncable — would also stop
vetoing, and that veto *does* prune. The flag is what separates "the
file ended" from "a length lied".

**Derive the flag inside the walk instead of taking it from the
caller.** The walk cannot: at depth 0 it has a buffer and no idea
whether its last byte is a file's last byte or a payload's. Trying to
infer it is the chicken-and-egg the flag exists to break.

**Keep the partial cardinality pass, charging over-counts but not
absences.** Truncation can only lower an occurrence count, so the
`count > 1` charges at `walk.rs:1164` and `:1175` remain sound and could
be kept. Rejected: it recovers a handful of `non_canonical` charges on
one frame, at the cost of a parameter on a function spec 0293 was
optimizing.

**A minimum-evidence rule in the pane — refuse to list candidates when
the cut left fewer than N scored tags.** Possibly right, but it is a
presentation decision about an already-correct score, and it needs the
measurement in test 5 before anyone picks N. Not this spec.

**Fold the charge into `unknowns` instead of a new counter.** It would
save a field and lie in the hover popup, which decomposes the score by
category; a cut is not an unknown field.

## Test plan

1. `a_cut_tail_scores_instead_of_vetoing` — the same blob with and
   without `end_undeclared`: vetoed and unlisted with it clear, scored
   and ranked with it set, and the score differing from the untruncated
   original by exactly 5 plus the matches the cut removed.
2. `only_the_ran_out_sites_are_demoted` — a nested LEN whose declared
   length overruns its parent's declared end still vetoes with
   `end_undeclared` set, because the range that ends is not the one that
   ran out.
3. `a_cut_frame_charges_no_absent_required_field` — a schema with a
   `required` field cut away; assert `mismatches == 0`, which fails by
   −30 if S5 is dropped.
4. `scan_policy_refuses_the_flag` — the `debug_assert`, and that
   `fdp-scan-pyo3`'s acceptance test is unchanged on a truncated FDP.
5. `boblog_offers_candidates` — the end-to-end claim, on the real
   fixture: the root's inferred list is non-empty and its winner is the
   type the intact entries support. This is also where the survivor
   count and the rank-1-to-rank-2 gap are measured, at several cut
   depths, for N3 and for the fourth alternative above.
6. Wall clock: `benches/score` and the googleapis startup before and
   after, to confirm the Background's claim that this veto prunes
   nothing. Expect no measurable change; a regression means the
   theorem is wrong and the spec should not land.

## Measured outcome

All against `grpconf/stage/googleapis` (58 777 roots), `taskset -c 4-7`.

| fixture | survivors before | survivors after | sweep before | sweep after |
|---|---|---|---|---|
| `boblog` (20 198 B, tail cut by 1 024 B) | **0** | **10 826** | 22.5 / 22.6 / 20.9 ms | 18.8 / 18.9 / 18.4 ms |
| `bobshark` (84 B, well-formed) | 8 987 | 8 987 | 22.9 ms | 17.7 ms |
| a PNG header | 0 | 0 | 18.6 ms | 15.9 ms |
| plain prose | 0 | 0 | 12.1 ms | 10.9 ms |

- **The Background's claim holds.** Not merely no regression — the
  demoted path is slightly *cheaper*, because it skips `set_vetoed`'s
  per-entry bookkeeping over ten thousand survivors. There was nothing
  left to prune.
- **`bobshark` is bit-identical**, ranking and scores both: a
  well-formed range never reaches a demoted site.
- **Non-protobuf files still list nothing.** This was N3's live risk and
  it did not materialize: a PNG and a prose file fail on a *veto* — a
  garbage tag that met its terminator, a wire-type contradiction — not
  on running out of bytes. It is also the check that caught the
  `next_pos` error in S2; with the wrong test both listed candidates.
- **`boblog` still opens untyped**, so the gRPConf beat N3 expected to
  break did not. Its top six candidates tie at −2 (three matches, one
  truncation charge) and `pick_winner` declines a tie. The pane is no
  longer empty, which was the goal; the auto-resolution N3 worried about
  needs a candidate that stands out, and a document of four opaque
  length-delimited records does not give one.

Tests added: five in `prototext-graph/src/score/tests.rs`
(`a_cut_tail_scores_instead_of_vetoing`,
`only_the_ran_out_sites_are_demoted`,
`a_cut_frame_charges_no_absent_required_field`,
`scan_policy_refuses_the_flag`,
`a_varint_overflow_before_the_end_is_not_a_cut`), the new coefficient in
`score_coefficients_rank_by_suspicion`, and
`a_cut_range_says_so_and_still_shows_its_terms` in
`protolens/src/tui/tests/popup.rs`.

Test 5 of the plan — the survivor count and the rank-1-to-rank-2 gap at
several cut depths — was measured at one depth only (the fixture's own),
and is the table above. The multi-depth sweep it also asked for is what
would let someone pick an N for the pane's minimum-evidence rule, and is
left with that alternative.
