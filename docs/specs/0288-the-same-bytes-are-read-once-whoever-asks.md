<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0288 — the same bytes are read once, whoever asks

Status: implemented
Implemented in: 2026-08-14
App: prototext-graph, prototext-core
Refs: docs/prototext/walk-profiles.md (the profiles this spec acts on);
        docs/specs/0175-packed-and-expanded-repeated-scalars.md (packed and
        expanded encodings score alike — the rule the element check
        implements); docs/specs/0179-active-entry-field-widths.md (the walk
        allocates nothing in steady state — the constraint on the scratch
        buffer); docs/specs/0262-the-machine-shares-a-screenful-not-a-query.md
        (the 24 sweep parts that N1 defers)

## Background

`score_message_multi` derives facts from a LEN payload inside
`for ae in active.iter_mut()`, once per active entry group.  Three of those
facts depend only on the payload bytes and not on `ae` at all:

| fact | status |
| --- | --- |
| `packed_varints_terminate(payload)` | memoized, in `packed_varints_ok` |
| the per-element packed varint check | recomputed per entry group |
| `std::str::from_utf8(payload)` at the `is_string` test | recomputed per entry group |

The first was hoisted; the other two were not.  This spec finishes the job.

`callgrind` over
`protolens --descriptor-set googleapis.desc googleapis.desc quit`,
140 959 265 308 instructions, whole process:

- `score_message_multi` calls `parse_varint` **900 142 298** times (34.85%
  inclusive) and `check_varint_value` **875 123 235** times (19.27%).  Both
  edges leave the element-check loop.  **54.1% of the process.**
- The blob is 25 660 332 bytes, so it holds at most 25.7 M varint positions.
  900 M decodes over it is **at least 35 decodes of every byte in the file** —
  and only bytes inside a packed varint payload enter that loop, so the true
  factor over those bytes is higher.  The redundancy is arithmetic, not
  inference.
- The size of the prize is already measured *in situ*: the memoized
  `packed_varints_terminate` scans the same payloads in the same loop shape
  for **61 064 771** instructions, against **1 761 775 748** for the
  unmemoized element check beside it — **28.9x**.

Separately, `parse_varint` costs **54.6 instructions per call** and its
out-of-line body is **563 bytes**, well past LLVM's inline threshold, so its
`#[inline]` is declined and every one of those 900 M decodes is a real call
returning a 56-byte struct through a hidden pointer.

**Every number above is descriptor-set-shaped.**  `googleapis.desc` carries
`SourceCodeInfo`, whose `path` and `span` are long packed `int32` runs, and
the element check is gated on exactly that element type.  A blob without them
would rank these sites differently, and one with neither would not enter the
loop at all.  The percentages must not be read as properties of the walk.  The
*redundancy* is a property of the walk: re-deriving a payload-only fact once
per active entry is wasted work on every input, which is what justifies this
change independently of any corpus.

## Goals

- **G1.** A payload-only fact is computed at most once per token, whatever the
  size of the active set.
- **G2.** `parse_varint`'s common case is small enough that `#[inline]` is
  taken, so the surviving decodes fold into their caller.
- **G3.** `benches/score.rs` can reach the packed-scalar and string paths, so
  a change to them is gateable there rather than only on protolens.
- **G4.** No score changes, on any input.  This is a pure work-removal.

## Non-goals

- **N1. A blob-level cache shared across protolens' sweep parts.**  `SWEEP_PARTS`
  is 24 (protolens, spec 0262), so a partitioned query walks the same bytes 24
  times, and that redundancy is real.  It is not addressed here: `score_subset`
  is handed a root subset and cannot see its 23 siblings, so exploiting it means
  a new `Sync` parameter on `prototext-graph`'s public entry point owned by
  protolens — a wider change than all of this spec.  Do it after G1, when the
  per-walk decode has already shrunk and the cache's true value is measurable.
- **N2. Hand-written assembly for `parse_varint`.**  A `naked_asm!`/`global_asm!`
  body can never be inlined, which locks in the call overhead G2 exists to
  remove; an inline `asm!` block is opaque to LLVM, which defeats S7's reliance
  on SROA and DCE.  It would also be three implementations — x86_64, aarch64,
  portable fallback — of semantics that must agree.
- **N3. A hand-written lean `VarintResult` for the walk.**  The adapter in
  `walk.rs` already discards the borrow and flattens both `Option`s.  Once G2
  lets the body inline, SROA breaks the 56-byte struct into registers and DCE
  removes exactly what the adapter drops — the optimizer *derives* the lean
  struct from the caller's context.  Hand-writing it buys the same thing at the
  cost of a second implementation of truncation, overflow and overhang.
- **N4. A min/max/first-violation summary answering every child in O(1).**  It
  would remove the per-child scan as well as the per-child decode, but it only
  pays when runs are long — which is the `SourceCodeInfo` shape, i.e. the one
  workload we have.  Designing for it is the overfitting this spec's Background
  warns about.  Revisit only if S3's per-child scan measures as a cost on a
  blob that is not a descriptor set.
- **N5. Narrowing or removing `overflow-checks`.**  Measured by interleaving two
  protolens builds in one `hyperfine` invocation: 4.738 s with, 4.644 s without
  — 2%, against a command whose cross-invocation reproducibility is worse than
  that.  It buys a real class of silent bug.  Keep it.
- **N6. Any change to the scoring formula, the veto set, or the `non_canonical`
  accounting.**  S4 is explicit that the observable behaviour is byte-identical.

## Specification

- **S1.** `packed_varints_terminate` already decodes every element of a packed
  varint payload, once per token, and discards the values.  Make it fill a
  scratch buffer with them instead of throwing them away.  There is then no
  second pass: the decode G1 wants is the pass the walk already pays for.
- **S2.** The scratch buffer lives on `WalkState` and is cleared, not
  reallocated, per token.  Spec 0179's steady-state-no-allocation posture is a
  constraint on this spec, not a suggestion; the buffer reaches the high-water
  mark of the largest packed run and stays there.
- **S3.** The element-check loop reads decoded values from the buffer instead of
  calling `parse_varint`.  `check_varint_value` keeps its present signature and
  runs per child: the *decode* is what is shared, not the verdict.  Keying a
  memo on `child` instead was considered and rejected — see Alternatives.
- **S4.** Iteration order and the break-on-first-veto are preserved exactly, so
  the `non_canonical` tally remains the count of offending elements *before* the
  break.  This is the reason S1 buffers values rather than summarizing them: a
  summary cannot reproduce a prefix count without also recording where each
  predicate first fires.
- **S5.** The buffer needs no error variant.  `run_ok` is tested first and
  `continue`s on failure, so by the time the element check is reached every
  varint in the payload is known to terminate inside it.  This is load-bearing;
  a future reordering of those two tests breaks it.
- **S6.** Memoize `from_utf8(payload)` in an `Option<bool>` declared beside
  `packed_varints_ok`, consulted at the `is_string` test.  Validity is a
  property of the bytes; `is_string` stays per child.
- **S7.** Split `parse_varint` in `prototext-core`: a small `#[inline]` entry
  handling the common case — terminator inside the buffer, value fits in 64
  bits, no overhang — tail-calling an `#[inline(never)] #[cold]` continuation
  for truncation, overflow, over-long varints and the backwards overhang scan.
  No varint semantics are duplicated; the cold path is the existing code, moved.
- **S8.** Add packed-scalar and string-field density knobs to the `blob` helper
  shared by `benches/score.rs` and `examples/profile_score.rs`.  Today's
  generator holds the active set at the full root count and emits neither, which
  is why `parse_varint` is 0.37% of the bench and 35.99% of a startup.  Report
  before/after at a few corners of (packed density, string density, root count),
  never as a single percentage.

## Alternatives considered

**Memoize the verdict, keyed on `child`.**  Caches one bit plus a count per
distinct child rather than the decoded values.  Rejected because its hit rate is
the number of active groups sharing a child, whereas S1's is unconditional — one
decode per token, the measured 28.9x — and because the decode is the 34.85%
while the verdict is the 19.27%.  S1 removes the larger term with the better hit
rate and no key.

**A blob-wide array of decoded varints, built once and shared.**  The
generalization of S1 to the whole input.  Rejected on memory and locality: ~9
bytes per varint over a 25 MB blob is 100–230 MB, four to nine times the input,
in a process that has already had an arena OOM; and it replaces a decode of
bytes that are hot in L1 — the loop re-reads one payload back to back — with a
load from a table that fits in no cache.  Sharing it across the sweep's threads
makes this worse, not better: per-core re-decode from L1 is embarrassingly
parallel, a shared multi-hundred-megabyte table is bandwidth-bound.  N1 keeps
the *scoped* version of this idea alive; the unscoped one is dead.

**Trust the compiler to fix `parse_varint`.**  LLVM handles the shift-or chain
well.  What it cannot do is decide that a 563-byte body should be split hot from
cold — `#[inline]` is a hint and it is being declined on size.  The split is a
source change or it does not happen.

**Do S7 and not S1.**  Makes 900 M calls cheaper instead of making them 30 M
calls.  Bounded by the call count, and the call count is the defect.

## Test plan

1. The existing `prototext-graph` score suite — unchanged, and it is the
   correctness gate for G4.  Every assertion about `matches`, `non_canonical`
   and vetoes must hold with no edit.
2. `packed_run_scores_identically_when_buffered` — a differential test over the
   `grpconf` anomalies fixture and the self-describing descriptor: score with
   the buffer path and with a debug-only reference path that re-decodes per
   entry, and assert the two `Score` structs are equal field for field.  This is
   what protects S4's prefix count, which no existing test targets directly.
3. `packed_scratch_does_not_grow_after_warmup` — assert the buffer's capacity is
   unchanged across the second and later tokens of a run, for S2.
4. `bin/profile startup` before and after — expect the `parse_varint` and
   `check_varint_value` edges out of `score_message_multi` to fall by roughly
   the 28.9x the memoized neighbour already achieves.
5. `bin/bench` at the S8 corners, against the same-binary noise floor
   (`docs/prototext/bench-process.md`), pinned per that document.

## Measured outcome

`bin/profile startup` — the real binary on the real corpus, whole process,
same command and same `googleapis.desc` bytes as the Background's profile:

| | before | after | |
| --- | ---: | ---: | --- |
| process total Ir | 140 959 265 308 | **88 247 832 711** | **−37.4%** |
| `parse_varint` self | 35.99% / 50.73 G | 1.54% / **1.355 G** | **37.4x** |
| `score_message_multi` → `parse_varint` | 900 142 298x | **25 019 063x** | **36.0x** |
| `check_varint_value` self | 17.31% / 24.40 G | 27.65% / **24.398 G** | unchanged |
| `score_message_multi` → `check_varint_value` | 875 123 235x | **875 123 235x** | unchanged |

**G4 has an exact witness.**  `check_varint_value`'s call count is identical to
the digit, and its cost moved by 0.01%.  S3 kept the check per child and shared
only the decode, so the only thing that could have changed the score did not
change at all.  Its share rose to 27.65% because the denominator shrank.

**G1 confirmed, and the Background's arithmetic was right.**  It reasoned from
the blob size that the loop had to be decoding every byte "at least 35" times.
The measured factor is 875 123 235 / 25 019 063 = **35.0**.  That was a lower
bound derived without profiling the element loop specifically, and the decode
count landed on it.

**G2 confirmed.**  The instruction ratio (37.4x) exceeds the call ratio (36.0x),
which is only possible if the surviving calls each got cheaper — i.e. the
`#[inline]` that was being declined on the 563-byte body is now taken.  The
synthetic corner measured it directly: 54.08 → **27.1 Ir per call**.

**The cold path is cold, and this was an open question.**  S7's split is only
correct if truncation, overflow and overhang are rare *on the scoring path*,
where nonsense typings are tried deliberately and land on invalid varints by
construction.  Measured: `parse_varint_uncommon` is called **30 810** times
against ~31.8 M decodes — **0.097%**.  The mechanism is that an invalid varint
vetoes, so the framing that produced it dies rather than recurring.  Overhang
does not veto, but it is not frequent enough to matter either.  No change is
needed; in particular, do not move the overhang scan back onto the hot path.

`std::str::from_utf8` falls to 760 481 calls, and `score_subset` is 95.83% of
the process — under callgrind the walk essentially *is* the startup.

### Wall clock, and why it is a smaller number

`hyperfine`, 20 runs each, `taskset -c 0-11`, two release builds of the same
tree (HEAD in a worktree vs. this change):

| | pre | post | |
| --- | ---: | ---: | --- |
| wall | 3.846 s ± 0.225 | **3.268 s ± 0.153** | **1.18x ± 0.09** |
| user CPU | 19.142 s | **14.107 s** | −26.3% |
| instructions | 140.96 G | 88.25 G | −37.4% |

The three disagree, for two separate reasons, and both are worth recording
because they change what should be optimized next.

- **−37.4% instructions buys only −26.3% CPU.**  Once S7's `#[inline]` is
  taken, `parse_varint` is a tight register-resident loop — cheap instructions
  at high IPC.  What remains is not: `check_varint_value` 27.65%,
  `score_message_multi` 24.41%, `rkyv/string/repr.rs` 4.77%, `smallvec` under
  `propagate_vetoes` 3.35% — pointer chasing and comparisons.  Removing the
  cheapest instructions in the program returns less than their share, and any
  future estimate made from an Ir percentage must be discounted for this.
- **−26.3% CPU buys only −15.0% wall.**  Measured separately and precisely:
  `-j 1` pinned to one CPU is 24.580 s ± 0.267 against `-j 8` at 5.209 s ±
  0.103, i.e. **4.72x on 8 cores**.  The gap is the straggler (max part is
  2.4x the mean, and `partition_roots` balances on root count while splitting
  on group boundaries) plus the unchanged serial phases — the root render and
  the 15 575-line index.  This change makes every part 37% cheaper and
  rebalances nothing, so the imbalance ratio passes through intact.

The consequence for N4: `check_varint_value` is now the largest single term at
27.65%, and its 875 M calls against 25 M distinct elements mean the redundancy
this spec removed from the *decode* is still fully present in the *check*.  But
it is made of the low-IPC instructions above, so its wall-clock return is well
under its share — while the scheduling work already scoped in spec 0269
attacks the 5x-of-8 directly.  N4 stays declined, now on measured grounds
rather than only on the overfitting argument.

**Both wall-clock ratios are lower bounds, and the reason is a measurement
trap worth recording.**  `hyperfine` runs command 1 to completion and *then*
command 2, so the second is measured on a hotter package.  This is a 15 W
U-series part whose E-cores turbo to 3.8 GHz and settle ~1.6x lower under
sustained all-core load: the same binary measured 40 minutes apart differed by
1.6x in **both** wall and user CPU at unchanged parallelism (wall:CPU 4.32 vs
4.48).  `pre` ran first and `post` second, so the drift penalized the change.
The true improvement is ≥ 1.18x and ≥ 26.3%, not ≈.

Within a single block, by contrast, the run-to-run spread is small — CV 1.1%
at `-j 1` and 1.2–2.0% at `-j 8` — so the sweep's part hand-out is *not* a
significant source of variance here.  The instruction counts remain the only
exactly reproducible figures; quote those, and treat any wall-clock ratio as
a floor unless the two builds were interleaved.
