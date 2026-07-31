<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0165 — protolens: CLI-configurable heat-cue pool sizes and exit-time stats

Status: draft. Its byte-budget half (G2/G3) is open flaw P5 in
        `docs/protolens/rendering-flaws.md` — the result cache is still
        bounded by entry count only, and `RangeHeatEntry` is still
        variable-size.
App: protolens
Refs: docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md
      (introduces `TieredBounded`, the type this spec adds a byte
      budget and statistics to),
      docs/specs/0191-the-read-ahead-walk-is-bounded-and-the-activity-dot-stops-flickering.md
      (G2, whose `PREFETCH_WALK_MAX_ROWS <= HEAT_CACHE_MAX_ENTRIES`
      compile-time assertion G1 would have to become a runtime check),
      `protolens/src/tui/heat_worker.rs`,
      `protolens/src/tui/heat_cue.rs`, `protolens/src/main.rs`,
      `protolens/src/tui/mod.rs`

## Background

Spec 0164 grounded the request queue's and result caches' current
capacities: `HEAT_REQUEST_QUEUE_MAX_ENTRIES` (`heat_worker.rs`, `512`
when this spec was written, since raised to `2048`) and
`HEAT_CACHE_MAX_ENTRIES = 8192`
(`heat_cue.rs`), both hardcoded constants, both sized well before
main-pane prefetch (0164's other half) existed to generate any real
background-traffic volume. Discussion during that spec's review
concluded (a) both numbers likely need to grow substantially now
that prefetch can queue thousands of speculative entries, (b) the
right values are a guess until observed in practice, and (c) the
result caches specifically need a byte-budget bound, not just an
entry-count one — `RangeHeatEntry::top_n: Vec<(String, i64)>`
(`heat_worker.rs`) is genuinely variable-size (arbitrary candidate
count, arbitrary type-name length), so a pure entry-count cap either
under-bounds memory (few entries, each with a huge `top_n`) or
over-bounds it (many entries, each tiny) depending on what's actually
cached, neither of which an entry count alone can express.

This spec makes both pools' capacities CLI-overridable (with
data-informed defaults) and adds an opt-in exit-time statistics
summary, so real usage can feed back into tuning those defaults —
explicitly split out from spec 0164, which only defines the tiering/
prefetch mechanism itself, not how its pools are sized or observed.

## Goals

- **G1**: New top-level CLI flags (`main.rs`'s `Cli` struct, alongside
  existing session-wide flags like `--indent`/`--theme` — accepted
  regardless of subcommand, meaningful only for the interactive TUI
  path, same as those two):
  - `--prefetch-queue-capacity <N>` (`usize`, default `100_000`) —
    replaces `HEAT_REQUEST_QUEUE_MAX_ENTRIES` as `HeatRequestQueue`'s
    entry-count cap.
  - `--heat-cache-max-entries <N>` (`usize`, default `200_000`) —
    replaces `HEAT_CACHE_MAX_ENTRIES` as each result cache's
    entry-count cap. Deliberately generous relative to the byte
    budget below (see G5) — expected to rarely be the binding
    constraint in practice, but still bounds slab/index overhead
    if a workload happens to produce many small entries.
  - `--heat-cache-max-mb <N>` (`usize`, default `100`) — new: each
    result cache's byte budget, converted to bytes (`* 1_048_576`)
    before use (G2).
  - `--heat-cue-stats` (boolean flag, default off) — prints the
    exit-time summary (G4) to stderr after the session ends. Off by
    default so ordinary interactive use isn't cluttered by it.
- **G2**: `TieredBounded<K, V>` (spec 0164) gains an optional byte
  budget, tracked alongside its existing entry-count cap:
  - New fields: `max_bytes: Option<usize>`, `total_bytes: usize`
    (running sum), `size_fn: Option<fn(&V) -> usize>` (a plain
    function pointer, not a boxed closure — size computation is
    always a structural, capture-free function of `V`, so this
    avoids forcing a trait bound onto every `V`/every caller,
    including `HeatRequestQueue`, which doesn't use this feature).
  - `TieredBounded::new(max_entries)` (existing, spec 0164)
    continues to mean "no byte budget" (`max_bytes`/`size_fn: None`)
    — used unchanged by `HeatRequestQueue`, whose `HeatRequest`
    payload is small and roughly fixed-size (N4).
  - New `TieredBounded::new_with_bytes(max_entries, max_bytes,
    size_fn)` — used by both of `HeatCaches`' maps (G3).
  - `upsert`'s over-capacity eviction loop (spec 0164 G6) becomes:
    evict via `evict_one` while `self.len() > self.max_entries ||
    self.total_bytes > self.max_bytes.unwrap_or(usize::MAX)` — either
    condition alone is enough to trigger eviction; `Rejected` (spec
    0164 G6) is unchanged in meaning, now checked against both caps.
  - `total_bytes` is maintained incrementally: `+= size_fn(&value)`
    on insert, `-= size_fn(&old_value)` (then `+= size_fn(&new_
    value)` if the payload actually changes) on in-place update,
    `-= size_fn(&value)` on eviction/removal — no full rescan.
- **G3**: `HeatCaches::new` takes `(max_entries: usize, max_bytes:
  usize)` instead of just `max_entries`, constructing both `by_range`
  and `current_score` via `TieredBounded::new_with_bytes`, sharing
  both caps (same "one pair of numbers for both maps" simplicity
  spec 0151 already used for the single existing cap). `size_fn` for
  `by_range` sums `RangeHeatEntry`'s fixed fields plus each `top_n`
  entry's `String` byte length; for `current_score`, the key's
  `String` length plus the fixed-size `Option<i64>` payload.
- **G4**: Exit-time statistics. `TieredBounded` tracks, alongside its
  existing fields: `high_water_entries: usize`, `high_water_bytes:
  usize`, `applied_count`/`evicted_count`/`rejected_count: u64`
  (from `upsert`/`evict_one`), `peek_hit_count`/`peek_miss_count:
  u64` (from `peek`) — all `u64` counters, updated inline at their
  existing call sites, no separate bookkeeping pass. A new `pub(super)
  fn stats(&self) -> TieredBoundedStats` returns a plain `Copy`
  snapshot. `App` gains `print_heat_cue_stats: bool` (from G1's CLI
  flag); `tui::run` (`mod.rs`), right before returning, calls a new
  `App::heat_cue_stats_summary(&self) -> String` (formatting the
  queue's and both caches' `stats()` snapshots into a multi-line
  block: final/high-water entry and byte counts, applied/evicted/
  rejected totals, hit/miss ratio) and `eprintln!`s it if the flag is
  set.
- **G5**: Default values (G1) are deliberately generous, informed by
  spec 0164's discussion, not measured: `100_000`-entry queue cap
  (up from today's `2048`), `200_000`-entry / `100MB` cache cap (up
  from `8192`, no prior byte cap at all). The exit-time summary (G4)
  exists specifically so a future spec can revisit these defaults
  against real high-water-mark data instead of another guess.

## Non-goals

- **N1**: Adaptive/dynamic pool resizing based on observed usage —
  fixed, CLI-configurable constants only.
- **N2**: Persisting statistics anywhere beyond one `eprintln!` at
  exit (e.g. to a file, or across sessions).
- **N3**: Per-tier breakdown of cache byte usage (e.g. "N MB is
  `User`-tier, M MB is `Prefetch`-tier") — `TieredBoundedStats`
  reports aggregate totals only; tier-level granularity isn't
  tracked.
- **N4**: A byte budget for `HeatRequestQueue`. `HeatRequest`'s
  shape (`Range<usize>`, `Option<String>`, two `usize`s) is small
  and roughly fixed-size regardless of tier — an entry-count cap
  alone is sufficient, as already established in spec 0164's sizing
  discussion.
- **N5**: Any change to `HeatCaches::complete` — stays the single,
  uncapped, unprioritized slot spec 0164's N2 already left alone.

## Specification

### `protolens/src/main.rs`

```rust
/// Spec 0165: entry-count cap for `HeatRequestQueue`.
#[arg(long = "prefetch-queue-capacity", default_value_t = 100_000)]
prefetch_queue_capacity: usize,

/// Spec 0165: entry-count cap for each of `HeatCaches`' maps.
#[arg(long = "heat-cache-max-entries", default_value_t = 200_000)]
heat_cache_max_entries: usize,

/// Spec 0165: byte-budget cap (megabytes) for each of `HeatCaches`'
/// maps.
#[arg(long = "heat-cache-max-mb", default_value_t = 100)]
heat_cache_max_mb: usize,

/// Spec 0165: print a heat-cue pool/cache usage summary to stderr
/// once the session ends.
#[arg(long = "heat-cue-stats")]
heat_cue_stats: bool,
```

Threaded into `App::new`'s call site (new parameters, or a small
`HeatCueLimits { queue_capacity, cache_max_entries, cache_max_bytes,
print_stats }` bundle — TBD during implementation, whichever reads
better at the call site).

### `protolens/src/tui/heat_cue.rs` (or `tiered.rs`, per spec 0164)

```rust
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct TieredBoundedStats {
    pub(super) entries: usize,
    pub(super) high_water_entries: usize,
    pub(super) bytes: usize,
    pub(super) high_water_bytes: usize,
    pub(super) applied_count: u64,
    pub(super) evicted_count: u64,
    pub(super) rejected_count: u64,
    pub(super) peek_hit_count: u64,
    pub(super) peek_miss_count: u64,
}

impl<K: Eq + Hash + Clone, V: Clone> TieredBounded<K, V> {
    pub(super) fn new_with_bytes(
        max_entries: usize,
        max_bytes: usize,
        size_fn: fn(&V) -> usize,
    ) -> Self { .. }

    pub(super) fn stats(&self) -> TieredBoundedStats { .. }
}
```

### `protolens/src/tui/heat_worker.rs`

`HEAT_REQUEST_QUEUE_MAX_ENTRIES`/`HEAT_CACHE_MAX_ENTRIES` constants
removed — both are now `App`-carried values sourced from CLI flags
(G1) with the same numeric defaults. `HeatCaches::new(max_entries,
max_bytes)` constructs `by_range`/`current_score` via `TieredBounded::
new_with_bytes`, passing `heat_cache_entry_bytes`/`current_score_
entry_bytes` size functions (new small free functions alongside
`HeatCaches`).

### `protolens/src/tui/mod.rs`

```rust
impl App {
    /// Spec 0165 G4: formats the request queue's and both caches'
    /// `TieredBoundedStats` snapshots into a human-readable summary.
    pub(super) fn heat_cue_stats_summary(&self) -> String { .. }
}
```

`run` (`mod.rs`), immediately before its final `result` return,
gains:

```rust
if app.print_heat_cue_stats {
    eprintln!("{}", app.heat_cue_stats_summary());
}
```

## Test plan

- `TieredBounded` unit tests (new, alongside spec 0164's): a byte
  budget triggers eviction before the entry-count cap when payloads
  are artificially large; the entry-count cap triggers when payloads
  are tiny and the byte budget is nowhere near reached; `stats()`
  after a scripted sequence of upserts/evictions/peeks matches
  expected counts exactly (high-water marks included).
- `HeatCaches::new_with_bytes` (via `HeatCaches::new`): confirms both
  maps share the configured caps and that `size_fn`'s accounting
  matches manually-computed sizes for a few representative
  `RangeHeatEntry`/`current_score` payloads.
- CLI parsing: `--prefetch-queue-capacity`/`--heat-cache-max-entries`/
  `--heat-cache-max-mb`/`--heat-cue-stats` parse to the expected
  `Cli` fields, including default values when omitted.
- Regression: existing `heat_cue`/`heat_worker` suites pass unchanged
  with the new defaults (100,000/200,000/100MB), same as they did
  with the hardcoded constants. Note that several of those tests fill
  the queue to `HEAT_REQUEST_QUEUE_MAX_ENTRIES` by iterating over it
  (`heat_worker.rs`, `tests/prefetch.rs`), so they must be re-pointed
  at the configured value or given their own small cap — at the G1
  default they would push 100,000 entries each.
- Manual (`tests/profiling.rs` against `/tmp/db3.desc`): run with
  `--heat-cue-stats`, confirm the summary prints exactly once, after
  the terminal is restored, with sane numbers (final counts at or
  below high-water marks, a plausible hit/miss ratio, no panic on a
  zero-graph/no-scoring session).
