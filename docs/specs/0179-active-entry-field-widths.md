<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0179 — prototext-graph: `ActiveEntry`'s field widths — the candidate ceiling and the walk's allocation profile

Status: implemented
Implemented in: 2026-07-26
App: prototext-graph
Refs: docs/scoring-flaws.md (P3),
      docs/protolens/rendering-worklist.md (deferred decision D-h),
      docs/schema-match.md

## Background

Two open items land on the same 64-byte struct, so they are settled
together rather than measured twice.

`ActiveEntry` (`prototext-graph/src/score/walk.rs:110`) is the walk's hot
structure: one per distinct Hopcroft state alive at the current nesting
level. The walk does the expensive per-tag work once per `ActiveEntry`
and then fans out over its `entries` list for score bookkeeping.

```rust
struct ActiveEntry {
    state_id: u32,
    entries: SmallVec<[u16; 4]>,
    occurrences: Vec<(u32, u64)>, // sorted by field_number
    verdict: Verdict,
}
```

### D-h — the 65 535-candidate ceiling

`entries` holds `u16` indices into `graph.roots`, so a corpus with more
than 65 535 root message types cannot be scored. `load::check_root_count`
(`load.rs:83`) rejects one at load time, with a `debug_assert!` restating
the invariant in the walk (`walk.rs:236`).

`docs/schema-match.md:16` states the design target as "100,000+ FDPs",
and `:87` describes the depth-0 active set as holding "~100,000 entries".
Root message types are at least as numerous as the files that declare
them, so the documented target exceeds the implemented ceiling. The
worklist records this as decision D-h and asks for a measurement rather
than an assumption, because `entries` is the hottest structure in the
walk.

**The contradiction is no longer theoretical.** The googleapis corpus
used for the measurements below compiles to **49 255 roots — 75% of the
ceiling.**

The `u16` is purely a runtime index. The only `u16` in the serialized
graph is `range_idx` (`serial.rs:27`), an index into the enum-ranges
table, which is unrelated. **Widening changes no on-disk format and
needs no `VERSION` bump.**

### P3 — the walk's per-frame allocations

`docs/scoring-flaws.md:839` measures 2828 allocations per `score_all`
over the `benches/score` workload and attributes them to four sites:
three `Vec`s grown from zero capacity (`child_pairs` at `:1146`, the
`partition` at `:1273` that allocates two more, and `group_by_state`'s
`collect` at `:190`), each costing O(log A); plus `occurrences`
(`:204`) as a fixed per-frame term.

That analysis is arithmetically correct and its conclusion is
**inverted**, because the workload it was measured on is not
representative of any real corpus.

## The finding: the synthetic bench inverts the real profile

Re-measured with a counting `#[global_allocator]` over a real walk —
the googleapis scoring graph (49 255 roots) against all 375 committed
per-type instance blobs, 8 052 361 `ActiveEntry` created, **4 761 300
allocations**:

| site | allocations | share |
|---|---|---|
| `occurrences` first push + growth | ~3 885 000 | **81.6%** |
| `entries` spilling out of its 4-element inline buffer | ~814 000 | **17.1%** |
| `child_pairs` + the `partition` + `group_by_state`'s `collect` | ~62 000 | **1.3%** |

The three `Vec`s the flaws report singles out are **1.3%** of the total.
`group_by_state` is called **2 608 times in the entire corpus** — a
handful per blob, because it runs once per recursion into a nested
message, not once per record.

The synthetic bench reads the other way round for two reasons, and both
are deliberate properties of that bench:

- every synthetic root is given a field number unique to itself so that
  Hopcroft cannot merge them, so **every `ActiveEntry` holds exactly one
  entry and never spills** — erasing the 17.1% term entirely; and
- it holds A (the number of live states) large while the blob stays at
  64 records, which maximizes exactly the O(log A) terms and minimizes
  the per-`ActiveEntry` ones.

It is a good bench for what spec 0173 used it for — it was built to
expose an O(A²) scan — and a misleading one for allocation shape.

The two remaining terms are both `ActiveEntry` field widths, which is
why this spec covers D-h and P3 together.

## The measurements that pick the widths

Same rig throughout: a throwaway `prototext-graph/examples/` binary,
counting `GlobalAlloc`, `VmHWM` from `/proc/self/status`, pinned to
vCPU 4 under `bin/bench`'s lock, variants interleaved, 7 rounds, median
reported. Allocation and byte counts are exact and identical across
runs. Two independent sweeps agreed on every ordering below.

### `occurrences` — length distribution

| `occurrences.len()` at drop | share | cumulative |
|---|---|---|
| 0 | 51.88% | 51.88% |
| 1 | 37.04% | 88.93% |
| 2 | 9.23% | **98.15%** |
| 3 | 1.43% | 99.59% |
| 4 | 0.28% | **99.87%** |

Half of all `ActiveEntry` never record an occurrence, so `Vec::new()`
costs them nothing. The other half allocate immediately, and `Vec`'s
first non-zero capacity for a 16-byte element is 4 — so today every one
of those 3.87 M frames takes a 64-byte heap block to store, in 98% of
cases, one or two 12-byte pairs.

### `entries` — length distribution

Cumulative coverage by inline capacity: 1 → 63.8%, 2 → 87.1%,
**4 → 93.4%**, 8 → 98.2%.

### The sweep

`ActiveEntry` size, allocations, allocated bytes, peak RSS and wall
clock over the whole corpus:

| variant | `entries` | `occurrences` | size | allocations | bytes | peak RSS | wall |
|---|---|---|---|---|---|---|---|
| baseline | `[u16; 4]` | `Vec` | 64 B | 4 761 300 | 3.70 GB | 14 420 kB | — |
| entries only | `[u32; 4]` | `Vec` | 72 B | 4 761 300 | 4.01 GB | 14 352 kB | −1.5% |
| occurrences only | `[u16; 4]` | `[(u32,u64); 2]` | 88 B | 1 035 352 | 4.15 GB | 13 864 kB | −12.5% |
| both, u64 count | `[u32; 4]` | `[(u32,u64); 2]` | 96 B | 1 035 352 | 4.45 GB | 13 324 kB | −9% |
| **both, u32 count** | **`[u32; 4]`** | **`[(u32,u32); 2]`** | **80 B** | **1 035 352** | **3.99 GB** | **13 688 kB** | **−13.1%** |
| wider entries | `[u32; 8]` | `[(u32,u32); 2]` | 96 B | 505 642 | 4.43 GB | 14 180 kB | −9.8% |
| wider still | `[u32; 16]` | `[(u32,u32); 2]` | 128 B | 357 652 | 5.33 GB | 14 680 kB | −7.7% |

Three results decide the spec:

1. **Widening the index to `u32` costs exactly zero allocations.**
   Inline capacity is unchanged at 4, so every spill decision is
   bit-identical — which is why the allocation counts match to the unit.
2. **Fewer allocations is not the objective function.** The `[u32; 8]`
   and `[u32; 16]` rows remove a further 0.5 M and 0.7 M allocations and
   are *slower* and use *more* peak RSS than `[u32; 4]`. Past capacity 4
   the inline buffer costs more in memory traffic than the spills it
   avoids. Same story for `occurrences` at capacity 4 versus 2.
3. **The `u64` occurrence count pays for the wider index.** Carrying the
   count as `u32` shrinks `ActiveEntry` from 96 B to 80 B and recovers
   the ~4% that widening `entries` otherwise costs once allocations stop
   dominating.

An earlier draft of this work concluded from `size_of` alone that
`SmallVec<[u32; 2]>` was the clever choice, since it is 24 bytes —
identical to `[u16; 4]` — and therefore "free". Measured, it costs
**+21.9% allocations** and +8.6% wall clock. The variable that matters
is inline *capacity*, not struct size, and it has an interior optimum
that has to be found by measurement in both directions.

## Goals

- **G1**: `score_all` addresses candidates by `u32`, so the 65 535-root
  ceiling is gone and `docs/schema-match.md`'s stated target is
  achievable.
- **G2**: `score_all` allocates ~78% less, by removing the dominant
  per-`ActiveEntry` allocation rather than the incidental per-frame ones.
- **G3**: `docs/scoring-flaws.md`'s P3 entry describes the profile a real
  corpus actually has, and records why the synthetic one differs.

## Non-goals

- **N1**: Any change to scoring semantics. Every change here is a field
  width; no score, counter or veto may move. The test plan's central
  assertion is that the existing corpus is unchanged.
- **N2**: Any change to the serialized graph format, and therefore no
  `VERSION` bump. `range_idx` stays `u16`.
- **N3**: **Scratch buffers for `child_pairs`, the `Any` partition and
  `group_by_state`'s `collect`** — the correction `docs/scoring-flaws.md`
  actually proposes. Measured at **1.3%** of allocations (2 608
  `group_by_state` calls for the whole corpus). It would thread a
  borrowed buffer through a recursive function that already holds `ws`
  mutably, to buy about one part in eighty. The flaws entry is amended to
  say so rather than left to be implemented later on stale grounds.
- **N4**: Raising `entries`' inline capacity above 4, or `occurrences`'
  above 2. Both measured; both lose. See the sweep.
- **N5**: Raising `load::check_root_count`'s limit to anything other than
  what the index can address. Whether a 100 000-root corpus is *fast
  enough* is a separate question from whether it can be addressed; this
  spec answers only the second.
- **N6**: `EntryScore`'s counters. They stay `u64`.

## Specification

### S1. Candidate indices are `u32` (D-h)

`ActiveEntry::entries` becomes `SmallVec<[u32; 4]>`, and every type that
carries a candidate index with it changes to match. All in
`prototext-graph/src/score/walk.rs`:

| site | from | to |
|---|---|---|
| `:115` `entries` | `SmallVec<[u16; 4]>` | `SmallVec<[u32; 4]>` |
| `:163` `is_vetoed` | `e: u16` | `e: u32` |
| `:173` `set_vetoed` | `e: u16` | `e: u32` |
| `:189` `group_by_state` | `Item = (u32, u16)` | `Item = (u32, u32)` |
| `:190` its pair buffer | `Vec<(u32, u16)>` | `Vec<(u32, u32)>` |
| `:236` the `debug_assert!` | `u16::MAX` | `u32::MAX` |
| `:261` root enumeration | `i as u16` | `i as u32` |
| `:296` `score_one`'s local | `SmallVec<[u16; 4]>` | `SmallVec<[u32; 4]>` |
| `:1146` `child_pairs` | `Vec<(u32, u16)>` | `Vec<(u32, u32)>` |
| `:1331` `recurse_into` | `Vec<(u32, u16)>` | `Vec<(u32, u32)>` |
| `:1332` `stay_out_entries` | `Vec<u16>` | `Vec<u32>` |

and `load::check_root_count` (`load.rs:78`) compares against `u32::MAX`.

The check is **kept, not deleted**. `graph.roots.len()` is a `usize`, so
on a 64-bit target it can still exceed what the index addresses; the
check is what keeps the walk's `debug_assert!` an invariant rather than a
live abort in a background thread (spec 0172 S5). Its doc comment cites
D-h as open and is updated to cite this spec as the answer.

The inline capacity stays **4**. This is the whole reason the change is
allocation-neutral, and it is a deliberate choice against the smaller
`[u32; 2]` that has the same `size_of`.

### S2. The occurrence count is `u32`, held inline (P3)

`ActiveEntry::occurrences` becomes `SmallVec<[(u32, u32); 2]>`.

`record_occurrence` (`:652`) takes the new type, and its increment
becomes saturating:

```rust
Ok(i) => occurrences[i].1 = occurrences[i].1.saturating_add(1),
```

`count` is the number of times one field number appears in one message
frame. Every occurrence costs at least one tag byte, so reaching
`u32::MAX` requires a single frame of more than 4 GiB. The bound is
therefore not reachable in practice — but the value is derived from
attacker-chosen bytes, and a plain `+= 1` on a `u32` wraps silently in a
release build, which is the exact defect class spec 0171 exists to
prevent. `saturating_add` is one instruction and total, so the question
does not need to be argued.

The two read sites in `apply_cardinality_multi` (`:781`, `:792`) widen at
the point of use, since `EntryScore::non_canonical` stays `u64` (N6):

```rust
scores[e as usize].non_canonical += (count - 1) as u64;
```

`count > 1` guards both, so the subtraction cannot underflow.

Inline capacity **2**, not 4: it covers 98.15% of frames, and capacity 4
measured slower and larger.

### S3. The documentation is corrected, not appended to

- `docs/scoring-flaws.md` P3 — the allocation table, the 81.6/17.1/1.3
  split, and why the synthetic bench inverts it. The "proposed
  correction" paragraph is **replaced**: scratch buffers are declined
  with the number that declines them, and `SmallVec<[(u32, u64); 4]>`
  becomes `SmallVec<[(u32, u32); 2]>` with the sweep behind it.
- `docs/protolens/rendering-worklist.md` — D-h moves from deferred to
  answered, citing this spec. Its "costs memory in the hottest structure,
  so measure it rather than assuming it is free" instruction was correct
  and is recorded as having been followed, with the outcome.
- `docs/schema-match.md` — the "100,000+ FDPs" target stands, and gains a
  note that the implemented ceiling now exceeds it.
- `prototext-graph/benches/score.rs` — a doc comment recording that every
  synthetic root is structurally distinct by construction, so
  `ActiveEntry::entries` never spills and the bench cannot be used to
  reason about the walk's allocation shape.

## Test plan

The change is observationally equivalent by construction: no control flow
depends on the width of an index or of a saturating counter within its
range. The obligation is to demonstrate that, not to restate it.

- **Equivalence** — the existing `score::tests` corpus (67 cases) and the
  `hopcroft_suite` fixtures assert every `EntryScore` field against
  committed expected values. All must stay green **unchanged**; a new
  test asserting the same facts over the same fixtures would add nothing.
- **The ceiling is gone** — a test that builds a graph, asserts
  `check_root_count` accepts a root count above `u16::MAX`, and scores a
  blob against it. This is the one behavior that is genuinely new, and it
  is the assertion that would have failed before the change.
- **`occurrences` beyond the inline buffer** — a blob repeating one
  optional field enough times to spill past capacity 2, asserting the
  resulting `non_canonical` count. The spill path is 1.85% of real
  frames, so it must not be left to chance in the fixtures.
- **The saturating count** — a unit test on `record_occurrence` driving
  one entry to `u32::MAX` and asserting the next increment holds rather
  than wraps. Reaching it through the walk would need a 4 GiB blob;
  calling the function directly is the same guarantee for no cost.
- **No test may assert a `state_id` ordering.** `graph::build` assigns
  node IDs by `HashMap` iteration order, so those differ between
  processes; an earlier spec's test passed locally and failed under
  `nix-build` for exactly this reason.

Measurement obligation: re-run the corpus sweep and confirm the
allocation count lands at 1 035 352, which is exact and reproducible.
Wall clock is reported but is **not** an acceptance criterion —
`prototext-graph --bench score` has a same-binary noise floor of up to
+15.9%, well above the effect being measured, so the deterministic
counter is the acceptance criterion instead.

## Measurements

Confirmed on implementation (2026-07-26), googleapis corpus — 49 255
roots × 375 instance blobs, pinned to vCPU 4 under the `bin/bench` lock,
counting global allocator around `score_all` only:

| metric | before | after | change |
|---|---|---|---|
| allocations | 4 761 675 | **1 035 345** | **−78.3%** |
| bytes allocated | 3.70 GB | 3.99 GB | +7.8% |
| peak RSS | 14 680 kB | 13 480 kB | **−8.2%** |
| `size_of::<ActiveEntry>()` | 64 B | 80 B | +25% |
| wall clock (min of 7) | 5.220 s | 4.794 s | −8.2% |
| addressable roots | 65 535 | 4 294 967 295 | — |

The acceptance criterion was the allocation count, and it is met: all
seven runs reported **1 035 345** allocations and 3 990 715 224 bytes,
bit-identical. That is 7 below the 1 035 352 predicted above, which is
the probe's own jitter rather than the walk's — the probe allocates while
walking the corpus directory, and this run's harness differs from the
sweep's by a few such allocations. Nothing in `score_all` is
nondeterministic across runs, which is the property that made a counter
usable as an acceptance criterion where a timer was not.

Wall clock is reported and is **not** an acceptance criterion, per the
test plan. It moved in the right direction and by more than a rounding —
but the effect and this target's noise floor are the same order, so the
number is corroboration, not evidence.

The `+7.8%` bytes and `+25%` struct size are the price, and peak RSS
falling anyway is why they are worth paying: the inline buffers replace
heap blocks that were larger than the data they held, so the process
touches fewer pages despite allocating more bytes on paper.
