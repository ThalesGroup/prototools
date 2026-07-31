<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0220 — the heat state is three numbers

Status: implemented
Implemented in: 2026-07-31
App: protolens
Refs: docs/specs/0154-protolens-heat-cue-progressive-display.md (the
        two-independent-halves design this preserves exactly),
      docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (made
        `heat_states` a fixed-length array sized to the arena, which is
        what turns its per-slot width into a fixed cost)

## Background

`heat_states: Vec<HeatState>` is allocated once in `App::new`
(`mod.rs:1557`) with one entry per arena slot. On googleapis that is
**4 737 284 slots** (measured 2026-07-31 by
`the_arena_covers_a_real_corpus`), and `size_of::<HeatState>()` is
**40** (measured the same day with
`const _: [u8; 0] = [0u8; size_of::<HeatState>()];`, which reports the
real number in the error). The array therefore costs **189.5 MB** —
the largest single per-slot term left after spec 0216 brought the node
slot itself down to 44 bytes.

Of those 40 bytes, 24 are payload and 16 are the `Option` machinery:

| field | width | why |
|---|---|---|
| `best: Option<RangeHeatStats>` | 24 | folds into `RangeHeatStats`'s own `Option<i64>` tag |
| ↳ `best_score: Option<i64>` | 16 | 1 tag byte + 7 padding + 8 payload |
| ↳ `best_count: usize` | 8 | |
| `current: Option<Option<i64>>` | 16 | 1 tag byte + 7 padding + 8 payload |

So 14 bytes are pure alignment padding and 2 are tag bytes, to carry
three numbers.

## Goals

- **G1.** Cut the per-slot width without changing what a heat state can
  express. Spec 0154's design — `best` and `current` arrive
  independently, and a *vetoed* current is distinct from a genuine
  score of `0` — is the reason the type is shaped the way it is, and it
  survives unchanged.
- **G2.** Any loss of precision is confined to what is *displayed*, and
  is unreachable outside a pathological blob. It must not reach
  `settled()`, which gates work scheduling.

## Non-goals

- **N1.** Making `heat_states` sparse. A `HashMap` keyed on node index
  would be far smaller still — the reader only ever visits a fraction
  of 4.7M nodes — but it puts a hash on the per-frame render path and
  on `recheck_pending_heat_states`' loop, and spec 0216 deliberately
  made the array fixed-length and index-parallel so that a splice
  resets an entry rather than resizing (`rendering-flaws.md` A2). Not
  reopened here.
- **N2.** Keeping `heat_states` authoritative for anything. It is a
  per-node *display* cache. The scores that decide candidate ordering
  in the override pane live in `heat_caches`
  (`RangeHeatEntry.top_n`) and stay `i64`. S2's narrowing is sound
  only because of this, so do not start reading `heat_states` for a
  decision that is not about what to draw.
- **N3.** Shrinking `RangeHeatEntry` (the cache's own entry type) or
  `RangeHeatStats`. Those are held per *scored range*, capped at
  `HEAT_CACHE_MAX_ENTRIES` = 8192, so their width is not a scaling
  term. `RangeHeatStats` keeps its exact present shape — `Option<i64>`
  and `usize` — because it is what `HeatState`'s accessors hand back
  and what `derive_stats`, `RangeHeatEntry` and `HeatCueKind::tie_count`
  already speak. Narrowing it would *add* a cast at each of those four
  boundaries to remove one here.

## Specification

- **S1.** `HeatState` becomes three plain numbers with private fields:

  ```rust
  pub(super) struct HeatState {
      best_score: i32,
      current: i32,
      best_count: u32,
  }
  ```

  12 bytes, no padding.

- **S2.** Two sentinels, shared by both score fields and taken from the
  bottom of the `i32` range, plus a floor that no real score may reach:

  | constant | value | meaning |
  |---|---|---|
  | `UNSCORED` | `i32::MIN` | not scored / not computed yet |
  | `VETOED` | `i32::MIN + 1` | scored; every candidate vetoed, or the current type is vetoed |
  | `SCORE_FLOOR` | `i32::MIN + 2` | the most negative *real* score representable |

  Scores are stored with `clamp(SCORE_FLOOR, i32::MAX)` — **both ends,
  unconditionally**. The positive side is not reachable today (S2a),
  but a one-sided clamp would silently become wrong the day somebody
  changes a coefficient in `EntryScore::score`, and the second compare
  is free next to the memory it saves. `best_count` saturates at
  `u32::MAX` the same way.

  **Clamping to `SCORE_FLOOR` rather than to `i32::MIN` is load-bearing,
  not defensive.** A score that saturated onto a sentinel would be read
  back as "not scored", `settled()` would answer `false` forever, and
  `mod.rs:2007`'s prefetch skip would stop firing — the node would be
  re-scored on every worker progress event. That is a scheduling
  defect, not a display one, and it is the single way to get this spec
  wrong.

- **S2a.** Saturation is reachable but pathological, and its blast
  radius is the cue. `EntryScore::score` is
  `matches - 10·unknowns - 15·out_of_range - 20·non_canonical -
  30·mismatches` (`prototext-graph/src/score/walk.rs:82`); every
  counter is bounded by the blob's record count, which
  `MAX_INDEXED_BUFFER` (`u32::MAX / 8`, 512 MiB) caps at ~2.7·10⁸. So
  the floor is ≈ −8.1·10⁹ against an `i32` floor of −2.147·10⁹: to
  saturate at all you need a blob of hundreds of megabytes that is
  almost entirely unknown or `required`-violating records. The positive
  side cannot saturate — `matches` is bounded by the same record count,
  an order of magnitude below `i32::MAX`.

  When it does happen, two things are visibly wrong and nothing else
  is: the printed score is the floor rather than the true value, and —
  because a saturated `current` and a saturated `best` compare equal —
  a `Mismatch` may render as a `Tie`, or as nothing at all if
  `best_count` is 1. Both are consequences of the display path in
  `heat_display` and reach no other subsystem (N2).

- **S3.** `Default` is written by hand, not derived. All-zero would
  mean "scored, best score 0, current score 0", i.e. every node in a
  fresh document would report `settled()` and show a stale cue. This is
  the one way to get S1 wrong silently, so `HeatState` must **not**
  derive `Default`.

- **S4.** The fields are private; the type gains

  ```rust
  fn new(best: Option<RangeHeatStats>, current: Option<Option<i64>>) -> Self
  fn best(&self) -> Option<RangeHeatStats>
  fn current(&self) -> Option<Option<i64>>
  ```

  so that every existing call site keeps reading and writing the same
  `Option`s it does today and the sentinel encoding stays inside this
  one file. `settled()` keeps its signature and becomes two integer
  comparisons.

- **S5.** The `usize` ↔ `u32` conversion for `best_count` happens
  inside `new`/`best()` and nowhere else (N3). It counts candidates
  sharing the top score, bounded by the root count of the graph —
  49 255 on googleapis against a `docs/schema-match.md` target of
  100 000+ — so the saturating cast is unreachable in practice, but it
  is written saturating rather than truncating because a truncated
  count could turn a real tie into `best_count == 0` and lose a cue
  entirely.

- **S6.** The clamp is a write-path cost only, and a negligible one:
  two comparisons and a truncating cast per `HeatState::new`, which
  runs once per node per cache read — not per frame and not per byte.
  Reads get *cheaper*: an `i32` load and an integer compare where today
  there is a 16-byte `Option` to inspect. The width change dominates
  either way.

## Alternatives considered

### `i64` scores — 24 bytes instead of 12

Keeping `i64` removes saturation entirely and still deletes all 14
padding bytes: 113.7 MB rather than 56.8 MB, so 57 MB worse than S1.

Rejected once the blast radius of saturation was established (S2a):
it is confined to the cue, needs a hundreds-of-megabytes adversarial
blob to trigger, and — with S2's floor — cannot reach `settled()`. 57 MB
out of a ~1 GiB working set is worth more than exactness in a case
that does not arise.

Mixed widths (an `i64` best and an `i32` current, 16 bytes) were
dismissed on asymmetry: two score fields that print side by side in one
cue should have one range.

### Keeping the `Option`s and only narrowing `best_count`

`usize` → `u32` alone changes nothing: `RangeHeatStats` stays 24 bytes
because `Option<i64>` forces 8-byte alignment. Padding, not field
width, is what costs here.

## Test plan

1. `a_fresh_heat_state_is_unsettled` — S3: `HeatState::default()` is
   not `settled()`, and both halves read back as `None`. This is the
   test that fails if `Default` is ever re-derived.
2. `a_heat_state_round_trips_every_shape` — S2/S4: every combination of
   `best` ∈ {`None`, all-vetoed, `Some(0)`, `Some(-7)`} and `current` ∈
   {`None`, `Some(None)`, `Some(0)`, `Some(-7)`} survives
   `new` → `best()`/`current()` unchanged. `Some(0)` is in the list on
   purpose: a genuine score of `0` must not read back as a vetoed half,
   which is the distinction spec 0154 G5 exists for.
3. `a_saturated_score_is_still_a_score` — S2, the load-bearing one: a
   `best` and a `current` built from `i64::MIN` read back as
   `Some(Some(SCORE_FLOOR as i64))`, **not** as `None`, and the state
   reports `settled()`. Same at `i64::MAX` against `i32::MAX`. This is
   the test that fails if the clamp is ever written against
   `i32::MIN`, or made one-sided.
4. S1: a `const _: () = assert!(size_of::<HeatState>() == 12)` next to
   the struct, the way `decode.rs` pins the node slot at 44 — in the
   production module, not in `tests`, so a plain `cargo build` catches
   a regression.
5. The existing `heat_cue` suite is the real regression test: it
   already covers the progressive `[?]` → `[?/{best}]` → cue sequence
   and the vetoed-vs-zero distinction, and must pass with no change to
   what it asserts — only its `HeatState { best, current }` literals
   become `HeatState::new(best, current)`, S4's whole point.

## Measured outcome

`size_of::<HeatState>()` **40 → 12**, so `heat_states` on googleapis
(4 737 284 slots, re-confirmed at `App::new`) goes **189.5 MB →
56.8 MB**.

Resident set, `protolens --descriptor-set googleapis.desc
googleapis.desc exit` with `/proc/self/status` read at the end of the
`Exit` arm — i.e. after every startup phase, with the whole session's
state live:

| | before | after | Δ |
|---|---|---|---|
| `VmRSS` (at rest) | 1 004 036 kB | 874 676 kB | **−129 360 kB (−12.9%)** |
| `VmHWM` (peak) | 1 189 456 kB | 1 189 276 kB | unchanged |

The at-rest delta is the array's own 132.6 MB and nothing else, which
is the check that it is really the array that shrank.

**The peak does not move**, and that is expected rather than
disappointing: on this path the high-water mark is set by an earlier
startup phase (decode/render), and `heat_states` is allocated after
that phase's temporaries are freed, so it has always fit underneath.
What this spec buys is 129 MB of *steady-state* footprint for the whole
life of the session — the number that matters for a reader who leaves
protolens open — not a lower peak. A future peak reduction has to
attack the decode/render phase, not this array.
