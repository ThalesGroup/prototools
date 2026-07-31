<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0151 — protolens: heat-cue cache redesign and startup progress messaging

Status: implemented
App: protolens
Implemented in: 2026-07-19
Refs: docs/specs/0138-protolens-main-pane-inference-heat-cue.md (introduces
      `App::heat_cache`/G1, whose caching strategy this spec supersedes),
      docs/specs/0114-protolens-range-type-override.md (override pane's
      own `candidate_cache`, the correct capped-preview precedent this
      spec's redesign follows), `prototext-graph/src/score/walk.rs`
      (`score_all`, unchanged by this spec)

## Background

Live investigation of a real-world stall (`--descriptor-set
googleapis.desc`, ~40k message types across ~7,800 files) root-caused
two independent problems, confirmed with a live `perf` profile and a
direct in-process measurement (not committed, used for diagnosis only):

**1. The heat-cue cache doesn't actually cache anything useful.**
`App::heat_cache` (spec 0138 G1) is a `CandidateCache` instance — the
same type `override_pane.rs`'s own `candidate_cache` uses — keyed by a
node's byte range, storing the *entire* `Vec<(String, i64)>` that
`inferred_candidates` returns (every non-vetoed candidate type, sorted).
For `override_pane.rs`'s own `candidate_cache`, this is fine, because
the only caller that inserts into it caps the list to
`override_list_height` first (`tui/override_select.rs:209-211`) before
calling `insert` — matching `CandidateCache`'s own doc comment ("never
holds a range's *complete* ranked list — only a preview capped...").
`heat_cue_for` (`tui/heat_cue.rs:100-101`) never applies that cap — it
inserts `inferred_candidates`'s raw, complete result directly.

Measured against the real corpus and a real 719-byte blob: a single
range's candidate list held **15,995 non-vetoed entries, ≈1,012,849
bytes** — on its own, essentially the entire `HEAT_CACHE_MAX_BYTES`
budget (`1 << 20` = 1,048,576 bytes). Since `CandidateCache::insert`'s
eviction loop (`override_pane.rs:143`) evicts down to a single
surviving entry whenever the budget is exceeded, and a typical viewport
shows a few dozen distinct node ranges, **the cache can never hold more
than about one entry at a time** — inserting the second visible line's
entry already evicts the first. In practice this means every visible
line's heat cue is recomputed (a full `score_all` call, measured at
≈1.35s for this blob/corpus) on every single `render()` call, whether
or not the viewport actually changed — indistinguishable from having no
cache at all. This is the mechanism behind the observed ~15-25s stall
per keypress, including keypresses (e.g. `Down` with no scroll) that
don't change the visible window at all.

The actual root design flaw: `heat_cue_from_candidates`
(`tui/heat_cue.rs:115-150`) only ever reads three small facts out of
the cached list — `best` (the top score), `current`'s score (a lookup
by FQDN), and `tie_count` (candidates sharing the top score) — yet the
cache retains the entire multi-thousand-entry list indefinitely, at a
memory cost utterly disproportionate to what's actually consumed.

A second, smaller latent bug in the same area: `heat_cue_for`'s current
population path derives `RangeHeatStats` via `derive_stats(&candidates)?`,
which short-circuits to `None` (skipping the cache insert entirely)
whenever `candidates` is empty — i.e. whenever every candidate for a
range is vetoed. A permanently-vetoed range is never cached at all, so
it gets re-scored (another full `score_all` call) on every single visit
forever, not just before its first visit. Relatedly, the gating
function's `current.unwrap_or(0)` conflates "current type not found in
the candidate list" (vetoed) with "current type genuinely scored `0`" —
if `best_score` also happens to be `0`, a vetoed current type silently
fails to trigger the `Mismatch` cue it should.

**2. No feedback during the (still nontrivial, even after the fix
above) initial startup cost.** Loading a large descriptor set + scoring
graph, resolving the root type, and populating heat cues for the
initial viewport are all silent — `protolens` gives no indication that
it's working versus hung, which is exactly what triggered the original
report ("Pressing Down does not have any effect -> Seems already
stalled").

## Goals

### Caching redesign

- **G1.** Replace `App::heat_cache: override_pane::CandidateCache`
  with two new, small-valued caches, both living in `tui/heat_cue.rs`
  (co-located with their only consumer, distinct from
  `override_pane.rs`'s `CandidateCache`, which is untouched — see N1).
  Both are keyed by a bare `start: usize` offset rather than a
  `Range<usize>`, because comparing/hashing/cloning a `usize` is
  cheaper. What makes that sound is that within one fixed, immutable
  blob no two nodes share a payload *start*: every nested field is
  preceded by its own tag and length prefix, so a child's payload
  begins strictly after its parent's does. Node ranges are **nested,
  not disjoint** — an earlier draft of this spec justified the key by
  claiming disjointness, which is false and would not have implied
  uniqueness of the start anyway. The same argument covers the "outer"
  /raw range (`node.raw_range`, tag included), so either start would
  work as a key. `heat_cue_for` already computes the interior range's start to
  slice `self.blob` for scoring, so that's the value both caches key
  on, avoiding a second offset computation.
  - `heat_range_cache: usize -> RangeHeatStats`, where
    `RangeHeatStats { best_score: Option<i64>, best_count: usize }` —
    one entry per distinct range start whose heat cue has ever been
    computed this session. Independent of the node's current type
    (mirrors spec 0138 G1's existing "independent of override"
    property): `best_score`/`best_count` depend only on the range's
    bytes and the schema graph, never on which type is currently
    assigned to that node. `best_score` is `None` when every candidate
    for the range is vetoed (equivalently, `-inf`) — critically, this
    is now a valid, cacheable *value*, not a reason to skip the insert
    (fixing the "permanently-vetoed ranges never get cached" bug
    described above). `best_count` is the number of candidates sharing
    the top score, with the convention that a unique winner has
    `best_count == 1` (not `0`) — the cardinality of the set of types
    achieving `best_score`; meaningless (left `0`) when `best_score` is
    `None`.
  - `heat_current_score_cache: (usize, String) -> Option<i64>`, where
    the cached `Option<i64>` distinguishes "found in the candidates
    list with this score" (`Some(score)`) from "confirmed absent from
    the candidates list" — i.e. the current type is vetoed for this
    range (`None`), symmetric with `best_score`'s own `Option<i64>`.
    This is the same distinction spec 0138 G9 already needed, now
    represented precisely instead of via an implicit `unwrap_or(0)`
    (see the gating fix in G5 below). One entry per distinct
    `(range-start, current-type-FQDN)` pair actually looked up this
    session, since `current_score` genuinely depends on which type is
    currently assigned to that node, and an override edit can change
    that.

  Forward-compatibility note (see N7): making both values' "no data"
  case an explicit, first-class `Option<i64>` rather than an implicit
  default keeps a clean seam for a future third state ("requested,
  not yet answered") without disturbing this spec's `None` ("vetoed")
  meaning — a later `V` swap (e.g. wrapping in an enum), not a
  cache-shape change.
- **G2.** Population: on a `heat_range_cache` miss, call
  `inferred_candidates` exactly once (unchanged) and, while the full
  result is still in hand, derive and insert *all three*:
  - `best_score`/`best_count` into `heat_range_cache` —
    unconditionally, even when `candidates` is empty (`best_score:
    None`), so a permanently-vetoed range is cached too (see
    Background);
  - the current type's score (looked up in that same in-memory list)
    into `heat_current_score_cache`;
  - (G6) a capped copy into `App::candidate_cache`.

  A single `score_all` invocation feeds all three caches at once, same
  call count as today — only the derived scalars (plus one capped
  list copy for G6) are retained afterward, not the full list.
- **G3.** On a `heat_range_cache` **hit** but a
  `heat_current_score_cache` **miss** for the node's current
  `(range, type)` pair — only possible when the node's assigned type
  changed (an override edit) since that range was last scored — call
  `inferred_candidates` again to resolve the new current-type's score,
  insert it, and discard the list again. This is the *only* case that
  re-pays the `score_all` cost after the first visit; it's bounded by
  the number of override edits a user actually makes, not by
  render/frame count.
- **G4.** Eviction strategy for both new caches: MRU (get promotes to
  most-recently-used, mirroring `CandidateCache::get`/`insert`'s
  existing pattern), but bounded by **entry count** rather than byte
  budget — a deliberate departure from `CandidateCache`'s
  byte-accounting convention, justified because both new value types
  are fixed-size scalars (no per-entry size estimator like
  `candidates_bytes` is needed). Default cap: `HEAT_CACHE_MAX_ENTRIES
  = 8192` for both caches — generous headroom for any realistically
  browsed document tree (even fully populated, 8192 entries of a few
  dozen bytes each costs well under 1MB, the same order of magnitude
  as today's single-entry budget was *supposed* to cover many ranges).
  Forward-compatibility note (see N7): `BoundedMru`'s public surface
  is deliberately just `get`/`insert` over opaque `K`/`V` — every
  caller (`heat_cue_for`, tests) goes through this API, never touching
  `entries`/`max_entries` directly, so wrapping an instance in a
  `Mutex` for a future background scoring thread is a call-site-
  transparent change.
- **G5.** Gating logic (spec 0138 G4/G9, `Mismatch`/`Tie`) is
  unchanged in *intent* — same two cues, same mutual exclusivity —
  but is now `Option`-aware throughout instead of collapsing
  `current_entry` to `0` via `unwrap_or` before comparing: a `None`
  current score (vetoed current type) now unconditionally yields a
  `Mismatch` cue whenever `best_score` is `Some`, regardless of
  whether `best_score` happens to be `0`, fixing the latent conflation
  bug described in Background. A `None` `best_score` (every candidate
  vetoed) yields no cue at all, same as an empty candidate list did
  before. `heat_level` bucketing (spec 0138 G5) is unchanged, still
  applied to `best_score` whenever it is `Some`.
- **G6.** Opportunistic cross-population of `App::candidate_cache`
  (the override pane's own, separately-instantiated `CandidateCache`,
  untouched in type — see N1): whenever G2/G3 pays for a full
  `inferred_candidates` call and the resulting list is already in
  hand, also cap it to `self.override_list_height.max(1)` (mirroring
  `override_select.rs:209-211`'s existing capping expression exactly)
  and insert it into `self.candidate_cache` keyed by the same range —
  so that if the user later opens the override pane (`t`) on that same
  node, it's a cache hit instead of a redundant `score_all` call.
  Harmless if `override_list_height` was measured against a
  differently-sized terminal since the entry was written (the override
  pane simply displays a shorter or already-sufficient list; a later
  genuine `t` invocation re-caps and overwrites it exactly as
  `override_select.rs` already does today). This does not change
  `CandidateCache`'s type or its `get`/`insert` API — only adds a new,
  read-only-of-the-candidates-list caller of that existing API from
  `tui/heat_cue.rs`.

  `override_list_height` is `0` from `App::new` (`tui/mod.rs:1013`)
  until the override pane's own first render (`render.rs:562`) — i.e.
  for all of G8's warm-up pass and any ordinary browsing before the
  user's first `t` press, `.max(1)` would cap every G6 insert to a
  single entry, defeating most of its value. Fixed by G8: `tui::run()`
  sets `app.override_list_height` from `terminal.size()?.height` right
  after `Terminal::new` succeeds, before the warm-up pass runs (see
  Specification) — a safe upper bound (the real, render-computed value
  is always `<=` the raw terminal height, since it's an inner-area
  height net of borders/header rows) available from the very start of
  the session, not just after the first override-pane render.

### Startup progress messaging

- **G7 (pre-TUI phase).** `main.rs`'s setup sequence — descriptor/pool
  load, graph load, root-type resolution, terminal-capability probing
  — prints a short, unstyled, single-line status to stderr (matching
  this file's existing `eprintln!` convention for
  errors/warnings) immediately before each stage begins:
  - Before `DescriptorContext::load` (`main.rs:165`): `"protolens:
    loading descriptor set '<name>' (<N> MB)..."`, or, if a sibling
    `hopcroft.rkyv` exists (checked the same way
    `DescriptorContext::load` itself does, `decode.rs:87-89`):
    `"protolens: loading descriptor set '<name>' (<N> MB) and scoring
    graph (<M> MB)..."`.
  - Before `decode::decode` (`main.rs:173`), only if a graph was
    loaded (root-type resolution's `score_all` call is the only
    potentially-slow part of `decode`; skip the message entirely when
    `ctx.graph.is_none()`, since `decode` is then near-instant):
    `"protolens: resolving root type..."`.
  - Before terminal-capability probing (`main.rs:194-201`, the
    `cli.command.is_none()` branch only — batch mode never probes the
    terminal at all, unchanged): `"protolens: detecting terminal
    capabilities..."`.

  These messages are gated on `cli.command.is_none()` (i.e., only for
  the interactive TUI path — see N4) to avoid adding stderr noise to
  scripted/batch invocations. Since they're plain stdout-independent
  stderr writes issued before `EnterAlternateScreen` runs, they need no
  special handling around raw-mode/alt-screen setup — ordinary
  shell-visible output, gone once the alt screen takes over, exactly
  like any other pre-TUI diagnostic.
- **G8 (in-TUI phase — the "warm-up pass").** The one-time cost of
  populating heat cues for the initial viewport (G2, first visit to
  each initially-visible range) is moved out of the first `render()`
  call into an explicit warm-up pass in `tui::run()`
  (`tui/mod.rs`, between `Terminal::new(backend)?` at line 1330 and
  the `run_loop(&mut terminal, app)` call at line 1342), so it can
  drive its own incremental terminal redraws instead of computing
  silently inside a single `terminal.draw` callback:
  - Skipped entirely if `ctx.graph.is_none()` (no scoring graph — no
    `score_all` calls are possible either way) or if
    `app.heat_cues_hidden` (forward-compatible with a future
    default-off toggle — see N3).
  - Otherwise, iterates the same initially-visible line range
    `render()` would compute on its first call (an approximation
    derived from `terminal.size()`'s row count is sufficient — see
    Specification; any range the warm-up pass doesn't reach is simply
    populated normally, lazily, the first time `render()`'s own
    `heat_cue_for` visits it, same as today), calling the same
    population logic as G2/G3 for each line.
  - No progress frame is drawn at all if the whole pass completes
    before a first ~300ms elapsed-time checkpoint is reached (avoids
    any flicker for small/fast descriptor sets, where this pass is
    already near-instant today).
  - Once the pass has run for more than ~300ms, an intermediate
    `terminal.draw` call renders a placeholder using the existing
    global command/message row (`self.message`, `tui/render.rs`
    ~440-464): `"Computing inference cues for the initial view: <n>/<m>
    lines scored..."`, refreshed roughly every ~300ms of further
    progress (time-based, not a fixed line-count interval, so it stays
    responsive regardless of how expensive each individual line turns
    out to be).

  This is a plain, synchronous loop with periodic redraws driven only
  by elapsed time — not the async architecture sketched in N7 — but
  its "call `terminal.draw` from outside the normal input-driven
  cycle" shape is a natural precedent for it: a future step 2 could
  plausibly reimplement this same warm-up pass on top of the request
  queue (draw a frame, drain whatever's landed since the last one)
  without changing G7's messaging or this section's user-visible
  behavior.

## Non-goals

- **N1.** No change to `override_pane.rs`'s own `CandidateCache` type
  or its `get`/`insert` API — already correctly capped (spec 0114 §6,
  `override_select.rs:209-211`). G6 adds a second caller of that
  existing, unmodified API from `tui/heat_cue.rs`; it does not touch
  the type itself.
- **N2.** No change to `score_all`/`inferred_candidates`/`walk.rs`
  scoring internals — this spec is purely about what protolens caches
  and derives from their existing, unchanged output, not about making
  a single `score_all` call itself faster.
- **N3.** "Lazy heat cues" — starting a session with
  `heat_cues_hidden = true` by default, so *no* `score_all` call
  happens at all until the user explicitly opts in via `i` — is a
  valid, complementary idea, deliberately **deferred to a future
  spec**. This spec's caching fix (G1-G6) already eliminates the
  recurring per-frame cost, and G7-G8 already address the remaining
  one-time startup cost, so the tool is usable without it; a
  default-off toggle remains further, independent polish for a later
  session. G8's warm-up pass is written to skip cleanly on
  `heat_cues_hidden` so this later change is a small, additive delta,
  not a rework.
- **N4.** No progress messaging for batch/extract mode
  (`cli.command.is_some()`) — G7 is gated to the interactive path only;
  batch mode's existing plain stdout/error-only behavior (spec 0123)
  is unchanged.
- **N5.** No persistent (cross-session, on-disk) caching for either
  new cache — both remain in-memory, per-`App`-instance, cleared on
  exit, exactly like the `CandidateCache` instance they replace.
- **N6.** No spinner/animation/percentage-bar UI — G7/G8's messages
  are plain text, updated only in step with actual measured progress
  (time-based checkpoints), never by an independent timer/animation
  tick that would itself add wakeups/CPU use unrelated to real work.
- **N7.** A background "score" thread, communicating with the main
  thread via a LIFO, deduplicating request queue — deliberately
  **deferred to a future spec** ("step 2"). Recorded here only so
  step 1's design doesn't foreclose it (see the forward-compatibility
  notes under G1 and G4):
  - **Sketch** (not designed in detail, not implemented here):
    `heat_cue_for`'s cache-miss path — today always a synchronous,
    blocking `inferred_candidates` call — would instead enqueue a
    scoring request for that key and return an "unknown yet" result
    immediately. A dedicated worker thread drains the queue **LIFO**
    (answer the most recently requested range first, since that's
    most likely still on-screen), **de-duplicating** so a range
    requested again while its prior request is still queued replaces
    the old entry rather than queuing twice. The worker writes results
    directly into the caches (behind a mutex) once computed.
  - **Open question, explicitly not resolved by this spec:** how the
    render loop learns a pending answer has landed and should re-poll
    (a channel, a dirty flag checked once per frame/tick, something
    else) — left to that future spec.
  - **Consequence for callers**, once step 2 lands: `heat_cue_for`
    must distinguish "confirmed no cue" from "not yet known" and only
    cache/gate on the former, re-polling the latter rather than
    treating it as a negative result. Every caller already treats a
    cue as `Option<HeatCue>`, so this doesn't change call shapes —
    only what a cache `get` can return.
  - **Explicitly NOT part of step 1**: no thread, no queue, no
    "pending" state, no mutex. Step 1 (G1-G6) remains fully
    synchronous; its entire purpose is making that synchronous cost
    *rare* (paid once per range, not once per render), which should
    leave step 2 valuable mainly for the remaining warm-up-pass/
    first-visit cost (G8) rather than being needed for basic
    usability.

## Specification

### New types (`tui/heat_cue.rs`)

```rust
#[derive(Clone, Copy)]
pub(super) struct RangeHeatStats {
    /// `None` when every candidate for this range is vetoed
    /// (equivalently, `-inf`) — a real, cacheable value, not an
    /// absent-entry sentinel (see Background).
    best_score: Option<i64>,
    /// Cardinality of the set of candidates sharing `best_score`. A
    /// unique winner has `best_count == 1`, never `0`. Meaningless
    /// (left `0`) when `best_score` is `None`.
    best_count: usize,
}

/// Bounded-MRU cache, generic over a small, fixed-size value — the
/// shared shape behind both `heat_range_cache` and
/// `heat_current_score_cache`. Distinct from `override_pane::
/// CandidateCache` (byte-budget-bounded, sized for large `Vec<(String,
/// i64)>` previews) — entry-count-bounded instead, since `V` here is
/// always a small fixed-size scalar with no need for a per-entry size
/// estimator.
struct BoundedMru<K: PartialEq + Clone, V: Clone> {
    entries: Vec<(K, V)>,
    max_entries: usize,
}

impl<K: PartialEq + Clone, V: Clone> BoundedMru<K, V> {
    fn new(max_entries: usize) -> Self { ... }

    /// Promotes to most-recently-used on a hit, mirroring
    /// `CandidateCache::get`.
    fn get(&mut self, key: &K) -> Option<V> { ... }

    /// Replaces an existing entry for `key` in place (re-promoting it),
    /// or appends a new one; evicts the least-recently-used entry while
    /// over `max_entries`, always keeping at least the one just
    /// inserted — mirrors `CandidateCache::insert`'s shape with an
    /// entry-count bound instead of a byte budget.
    fn insert(&mut self, key: K, value: V) { ... }
}

const HEAT_CACHE_MAX_ENTRIES: usize = 8192;
```

### Derivation helper

```rust
fn derive_stats(candidates: &[(String, i64)]) -> RangeHeatStats {
    let Some(&(_, best)) = candidates.first() else {
        return RangeHeatStats { best_score: None, best_count: 0 };
    };
    let best_count = candidates.iter().filter(|(_, s)| *s == best).count();
    RangeHeatStats { best_score: Some(best), best_count }
}

fn score_of<'a>(candidates: &'a [(String, i64)], key: &str) -> Option<i64> {
    candidates
        .iter()
        .find(|(fqdn, _)| fqdn == key)
        .map(|(_, score)| *score)
}
```

`derive_stats` now always returns a value (no `Option`/`?`) — an empty
`candidates` list is itself informative (`best_score: None`) and must
still be cached, not treated as "nothing to insert."

### `heat_cue_for` (replaces `tui/heat_cue.rs:83-107`)

```rust
pub(super) fn heat_cue_for(&mut self, line_idx: usize) -> Option<HeatCue> {
    if self.heat_cues_hidden {
        return None;
    }
    let idx = *self.line_to_node.get(&line_idx)?;
    if !self.can_override(idx) {
        return None;
    }
    let range = {
        let node = &self.tree[idx].span;
        extract::message_payload_range(&self.blob, &node.raw_range, node.packed_record_start)
    };
    let start = range.start;
    let current_key = self.current_type_key(idx);

    let stats = match self.heat_range_cache.get(&start) {
        Some(s) => s,
        None => {
            let graph = self.ctx.graph.as_ref()?;
            let range_bytes = &self.blob[range.clone()];
            let candidates = override_pane::inferred_candidates(range_bytes, graph);
            let stats = derive_stats(&candidates);
            self.heat_range_cache.insert(start, stats);
            if let Some(key) = current_key.as_ref() {
                let score = score_of(&candidates, key);
                self.heat_current_score_cache.insert((start, key.clone()), score);
            }
            // G6: opportunistically prime the override pane's own
            // cache with this same, already-paid-for candidate list.
            let capped: Vec<_> = candidates
                .into_iter()
                .take(self.override_list_height.max(1))
                .collect();
            self.candidate_cache.insert(range.clone(), capped);
            stats
        }
    };

    let current_entry: Option<i64> = match current_key.as_deref() {
        None => None,
        Some(key) => {
            let cache_key = (start, key.to_string());
            match self.heat_current_score_cache.get(&cache_key) {
                Some(entry) => entry,
                None => {
                    let graph = self.ctx.graph.as_ref()?;
                    let range_bytes = &self.blob[range.clone()];
                    let candidates = override_pane::inferred_candidates(range_bytes, graph);
                    let score = score_of(&candidates, key);
                    self.heat_current_score_cache.insert(cache_key, score);
                    // G6: same cross-population as the G2 branch above —
                    // this call's candidate list is identical to what a
                    // fresh G2 call would have produced for this range
                    // (independent of current type), so it's just as
                    // worth caching for the override pane.
                    let capped: Vec<_> = candidates
                        .into_iter()
                        .take(self.override_list_height.max(1))
                        .collect();
                    self.candidate_cache.insert(range.clone(), capped);
                    score
                }
            }
        }
    };

    heat_cue_from_stats(stats, current_entry)
}
```

### `heat_cue_from_stats` (replaces `heat_cue_from_candidates`,
`tui/heat_cue.rs:115-150`)

Same two cues as spec 0138 G4/G9, but `Option`-aware throughout — no
`unwrap_or(0)` collapse before comparing, fixing the vetoed-vs-genuine-
`0` conflation described in Background:

```rust
pub(super) fn heat_cue_from_stats(
    stats: RangeHeatStats,
    current_entry: Option<i64>,
) -> Option<HeatCue> {
    let best = stats.best_score?; // every candidate vetoed — no cue possible
    match current_entry {
        None => Some(HeatCue {
            level: heat_level(best),
            // Display value only; the gating decision above already
            // treated `None` as strictly worse than any `best`,
            // independent of what's shown here.
            kind: HeatCueKind::Mismatch { current: 0, best },
        }),
        Some(current) if current < best => Some(HeatCue {
            level: heat_level(best),
            kind: HeatCueKind::Mismatch { current, best },
        }),
        Some(current) if current == best && stats.best_count > 1 => Some(HeatCue {
            level: heat_level(best),
            kind: HeatCueKind::Tie { tie_count: stats.best_count },
        }),
        _ => None,
    }
}
```

### `App` field changes (`tui/mod.rs`)

Replace the single `heat_cache: override_pane::CandidateCache` field
(~line 722, init ~line 1015) with:

```rust
heat_range_cache: heat_cue::BoundedMru<usize, heat_cue::RangeHeatStats>,
heat_current_score_cache: heat_cue::BoundedMru<(usize, String), Option<i64>>,
```

both initialized via `BoundedMru::new(heat_cue::HEAT_CACHE_MAX_ENTRIES)`.

### Pre-TUI messages (`main.rs`)

Inserted immediately before the three call sites named in G7, gated on
`cli.command.is_none()`. File sizes read via `std::fs::metadata` (best
effort — a metadata read failure simply omits the size suffix rather
than aborting or erroring, since these are pure UX decoration, not
load-bearing for correctness).

### Warm-up pass (`tui/mod.rs::run`)

New private function, called between `Terminal::new(backend)?` and
`run_loop`. `run()` itself gains one line right after `Terminal::new`
succeeds, *before* calling `warm_up_heat_cues` (and unconditionally —
not gated on `ctx.graph`/`heat_cues_hidden`, since ordinary browsing
benefits from a non-`1` G6 cap too, not just the warm-up pass):

```rust
let mut terminal = Terminal::new(backend)?;
app.override_list_height = terminal.size()?.height.max(1) as usize;
warm_up_heat_cues(&mut terminal, app)?;
```

```rust
fn warm_up_heat_cues<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    if app.ctx.graph.is_none() || app.heat_cues_hidden {
        return Ok(());
    }
    let rows = terminal.size()?.height as usize;
    let lines: Vec<usize> = app.visible_rows.iter().take(rows).copied().collect();
    let start = Instant::now();
    let mut last_draw = start;
    for (i, &line_idx) in lines.iter().enumerate() {
        app.heat_cue_for(line_idx); // populates the caches; return value unused here
        let now = Instant::now();
        if now.duration_since(start) > WARMUP_FIRST_DRAW_DELAY
            && now.duration_since(last_draw) > WARMUP_REDRAW_INTERVAL
        {
            terminal.draw(|frame| draw_warmup_progress(frame, i + 1, lines.len()))?;
            last_draw = now;
        }
    }
    Ok(())
}
```

`WARMUP_FIRST_DRAW_DELAY`/`WARMUP_REDRAW_INTERVAL` both default to
`Duration::from_millis(300)`. `draw_warmup_progress` renders only the
global command/message row's text (reusing its existing area/style —
no new layout), independent of `App::render`'s own splash/tree
rendering, so it composes cleanly with whatever the very next real
`render()` call draws.

## Test plan

**Caching (G1-G6):**
- `derive_stats`: empty candidates → `RangeHeatStats { best_score:
  None, best_count: 0 }` (and, separately, that `heat_cue_for` still
  inserts this value into `heat_range_cache` rather than skipping the
  insert — see the regression test below); single entry → correct
  `best_score`/`best_count == 1`; multiple entries tied at the top →
  correct `best_count`; ties elsewhere (not at the top) don't inflate
  `best_count`.
- `BoundedMru`: hit promotes to MRU (mirrors existing
  `candidate_cache_hit_promotes_to_most_recently_used`); eviction at
  exactly `max_entries + 1` distinct keys evicts the least-recently-used
  one (mirrors existing `candidate_cache_evicts_least_recently_used_
  past_byte_budget`, adapted to an entry-count bound).
- `heat_cue_from_stats`: port every existing `heat_cue_from_candidates`
  test case (spec 0138's Mismatch/Tie/absent gates) to the new
  `(RangeHeatStats, Option<i64>)` inputs — same boundary values
  (`best == current`, `best == current + 1`, tie counts of 1/2/3) —
  plus new cases exercising the fixed conflation bug: `current_entry:
  None` with `best_score: Some(0)` must yield `Mismatch` (previously
  silently dropped by `unwrap_or(0)`); `best_score: None` must yield
  no cue regardless of `current_entry`.
- **Regression test for the caching bug:** call `heat_cue_for` twice
  for the same `line_idx` (same range, same current type, no state
  change in between) against a real loaded graph, and assert the
  second call performs zero additional `inferred_candidates`/
  `score_all` work — observable either via an injected call-counting
  wrapper around `inferred_candidates` (test-only) or by asserting
  `heat_range_cache`/`heat_current_score_cache` each still hold exactly
  one entry after both calls (proving the second call was a pure cache
  hit, not a re-insert).
- **Regression test for the vetoed-range bug:** against a range whose
  every candidate is vetoed, call `heat_cue_for` twice; assert the
  second call is still a cache hit (zero additional `score_all` calls)
  — i.e. a `None` `best_score` was cached, not skipped.
- Override-edit interaction (G3): change the node's assigned type
  between two `heat_cue_for` calls on the same range; assert exactly
  one additional `inferred_candidates` call happens (for the new
  type's score), and that `heat_range_cache`'s entry for that range —
  `best_score`/`best_count` — is untouched (not recomputed) by the
  change.
- Cross-population (G6): after a `heat_range_cache` miss populates a
  range's stats, assert `App::candidate_cache` now also contains an
  entry for that same range, capped to `override_list_height` entries,
  matching the prefix of the same `inferred_candidates` result.

**Startup messaging (G7-G8):**
- G7: a subprocess integration test (mirroring existing
  subprocess-based tests, e.g. `reproto`'s `test_schema_db.py` pattern
  or protolens's own existing CLI integration tests) against a small,
  fast fixture descriptor set (not the real googleapis corpus — CI
  must stay fast), asserting the three staged messages appear on
  stderr, in order, before the process would otherwise exit/block —
  practically, run with `--extract`/batch mode disabled and a very
  short-lived TUI session (e.g. immediately dismissed), capturing
  stderr.
- G8: a unit test on `warm_up_heat_cues` directly (not a real
  crossterm `Terminal`, per existing `App`-test-harness conventions —
  a lightweight fake `Backend`/no-draw stub suffices) asserting: (a)
  it populates `heat_range_cache` for every line in the initial
  window against a real small test graph; (b) it returns immediately,
  with no draw calls at all, when `ctx.graph.is_none()` or
  `heat_cues_hidden`; (c) with an artificially slow test scorer (or a
  scaled-down `WARMUP_FIRST_DRAW_DELAY`/`WARMUP_REDRAW_INTERVAL` for
  test purposes), at least one intermediate progress draw occurs when
  the simulated pass exceeds the delay threshold, and none occurs when
  it doesn't.
- G6/G8 interaction regression: after `run()`'s eager
  `app.override_list_height = terminal.size()?.height.max(1)` line
  runs (simulated directly in a unit test, not via a real `run()`
  call) but before any override-pane render, assert a `heat_range_cache`
  miss's G6 cross-population caps `candidate_cache`'s entry to the
  terminal height, not to `1` — pinning the fix for the gap identified
  during spec review.
