<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0365 — heat-suffix hover popup reads top_n, not a synchronous sweep

Status: implemented
Implemented in: 2026-08-26
App: protolens
Refs: docs/specs/0284-hovering-the-heat-suffix-names-the-top-scorers.md
        (the hover popup this spec fixes),
      docs/specs/0152-heat-cue-worker.md (the worker architecture),
      docs/specs/0250-complete-candidate-list-cache.md
        (the `complete` cache and its write policy),
      docs/specs/0151-heat-cue-cross-population.md
        (G6: the `override_list_height` cap on `top_n`)

## Background

Hovering the heat-suffix token (` [2@85]`, ` [3/47]`, etc.) opens a
popup listing the types that share the top score for that range (spec
0284). Two defects affect this popup:

**Defect 1 — blocking sweep on the main thread.** When no background
worker is running, `heat_cue_resolve` falls through to a synchronous
`inferred_candidates` call on the render thread. Against a large
descriptor set (googleapis.desc, ~25 MB, thousands of types) this
blocks rendering for a perceptible duration. The fallback exists only
because at the time it was written there was no worker; the worker
(spec 0152) now always runs whenever a scoring graph is loaded, making
the synchronous path dead code in production.

**Defect 2 — popup capped at 8 candidates.** `heat_cue_resolve` calls
`heat_lookup(&range, …, 0, HEAT_CUE_PREVIEW, tier)` where
`HEAT_CUE_PREVIEW = 8`. The worker satisfies this request and writes
exactly 8 entries into `by_range.top_n`. The popup then reads those 8
entries and shows at most 8 types — even when `best_count` says there
are more tied winners. Pressing `t` (the override pane) implicitly
widens the request to `[0, override_list_height)`, which is why a
hover *after* a `t` press shows more types: the `top_n` entry has grown
behind the scenes.

The two defects share a root cause: `heat_cue_resolve` requests only
`HEAT_CUE_PREVIEW` entries from the worker instead of
`override_list_height`, and maintains a synchronous fallback that is no
longer needed.

## Goals

- **G1.** `heat_cue_resolve` requests `[0, override_list_height)` from
  the worker (the same window the override pane uses), not
  `[0, HEAT_CUE_PREVIEW)`. After the worker responds, `top_n` holds
  enough entries to fill a full terminal height of popup lines.
- **G2.** The synchronous `inferred_candidates` block in
  `heat_cue_resolve` is removed. The function never blocks the render
  thread on a graph sweep.
- **G3.** The hover popup reads all tied-top entries from `top_n`
  (up to `best_count`). If `top_n` holds fewer entries than
  `best_count` (the worker has not yet responded, or `top_n` is
  narrower than `best_count` for any other reason), the popup shows
  what it has and appends a `…` line so the reader knows the list is
  incomplete.
- **G4.** If nothing is in the cache yet (the worker has not responded
  at all), the popup shows a "retrieving…" message instead of a blank
  or a crash.

## Non-goals

- **N1.** Removing the `complete` cache or changing its write policy.
  `complete` remains the override pane's source for its unbounded
  `[0, usize::MAX)` request. This spec does not change that contract.
- **N2.** Widening `top_n` to `usize::MAX`. The cap at
  `override_list_height` is correct: a full screen of candidates is
  more than enough to display, and an uncapped `top_n` is exactly the
  oversized-entry problem `docs/rendering-flaws.md` P5 describes.
- **N3.** Removing `HEAT_CUE_PREVIEW`. It is still used by the
  prefetch walk and by `read_heat_state`'s settled-check. Only
  `heat_cue_resolve`'s outgoing request is widened.
- **N4.** Changing the no-graph path. When no scoring graph is loaded,
  `heat_worker` is `None` and `heat_cue_resolve` returns
  `HeatDisplay::None` immediately — that path is unchanged.

## Specification

- **S1.** In `heat_cue_resolve`, replace the `heat_lookup` call's `end`
  argument from `HEAT_CUE_PREVIEW` to `self.override_list_height.max(1)`:

  ```rust
  self.heat_lookup(
      &range,
      current_key.as_deref(),
      0,
      self.override_list_height.max(1),
      tier,
  );
  ```

  This is the only change to `heat_cue_resolve`'s worker path. The
  window now matches what `upgrade_active_override_to_complete` requests,
  so the worker fills the same `top_n` slice whether the override pane
  has been opened or not.

- **S2.** Remove the synchronous fallback block from `heat_cue_resolve`
  — everything from `let Some(graph) = self.ctx.graph.clone()` through
  `caches.complete.insert(range.clone(), candidates)`. The guard that
  precedes it (`if state.settled() || self.heat_worker.is_some()`)
  already returns early for every production configuration; what remains
  after the guard is dead in production and misleading in tests.

  After removal, `heat_cue_resolve` ends at:

  ```rust
  self.record_heat_state(idx, state);
  heat_display(state, self.heat_anchor)
  ```

- **S3.** In `heat_chrome_hit` (the hover-hit builder), replace the
  current `top_n.take(best_count)` read with a read that:

  1. Reads `entry.top_n` up to `entry.best_count` entries.
  2. If `top_n.len() < best_count` — i.e. the worker has not yet
     widened the entry to cover all tied winners — sets a flag
     `truncated: true` on the hit.
  3. If `top_n` is empty (cache miss), sets `heat_top` to an empty
     `Vec` as before (the "retrieving…" branch in `doc_lines` already
     handles this).

  `truncated` is carried in `DocHit` (a new `bool` field, `false`
  everywhere except heat-suffix hits).

- **S4.** In `doc_lines`, the `HeatSuffix` arm:

  - If `heat_top` is empty: emit `"retrieving…"` (was `"still scoring
    these bytes"` — the new wording is more accurate now that this
    state means the worker hasn't answered yet, not that scoring hasn't
    started).
  - If `heat_top` is non-empty and `hit.truncated` is `false`: emit
    all entries unchanged (the complete tied set fits).
  - If `heat_top` is non-empty and `hit.truncated` is `true`: emit all
    entries, then emit `"…"` as the last candidate line, before the
    fixed "double-click" tail.

  The `tail`-based truncation added to `PopupBody::Doc` in spec 0364's
  fix session handles terminal-height overflow for the full list; S4
  handles the case where the *source* is incomplete.

- **S5.** The `complete`-first fallback introduced in the spec-0364
  session (`heat_chrome_hit` reading `complete` before `top_n`) is
  removed. With S1 in place, `top_n` holds `override_list_height`
  entries — the same number the popup can display — making `complete`
  redundant as a popup source. `complete` reverts to its documented
  role: the override pane's unbounded list only.

- **S6.** `CompleteLists::get` is reverted to `fn` (private) — the
  `pub(super)` promotion introduced for the now-removed `complete`
  fallback is no longer needed.

## Alternatives considered

### Keep the synchronous fallback, protect it with a timeout

The synchronous path exists so that test fixtures without a worker can
still exercise `heat_cue_resolve` end-to-end. A timeout would bound
the block but not eliminate it, and the test value is better served by
tests that deliberately exercise the no-worker path. The fallback is
removed cleanly; tests that need synchronous scoring call
`inferred_candidates` directly.

### Read from `complete` as the popup source

This was the approach in the spec-0364 fix session: `heat_chrome_hit`
tries `complete` first (all tied winners, unbounded) and falls back to
`top_n`. It fixes the "only 8" defect but not the "blocking" defect, and
it introduces a state dependency (the popup shows more after `t` than
before, because `t` populates `complete`). With S1 in place `top_n` is
already wide enough; reading `complete` is unnecessary and reverts the
principle that `complete` is the override pane's cache, not the popup's.

### Widen `top_n` to `usize::MAX`

This would guarantee all tied winners are always present, but at the
cost of holding the entire ranked candidate list in a cache designed for
a screenful — exactly the rendering-flaws.md P5 problem.

## Test plan

1. `heat_suffix_popup_shows_all_tied_winners` — fixture with
   `override_list_height` set and multiple tied top-scorers; assert
   the popup lists all of them without `…`.
2. `heat_suffix_popup_truncates_when_top_n_is_narrower_than_best_count`
   — fixture where `best_count > top_n.len()`; assert `…` appears and
   no panic.
3. `heat_suffix_popup_shows_retrieving_when_cache_is_empty` — cache
   cold; assert the "retrieving…" message.
4. `heat_cue_resolve_does_not_block` — confirm the synchronous branch
   is gone: call `heat_cue_resolve` on a node with a graph but no
   worker; assert it returns `HeatDisplay::None` without scoring.

## Measured outcome

Four files changed, no new state:

- `heat_cue.rs`: `heat_cue_resolve` widened its `heat_lookup` request
  from `HEAT_CUE_PREVIEW` to `override_list_height.max(1)`; the
  synchronous `inferred_candidates` fallback removed entirely.
- `popup_doc.rs`: `DocHit` gains `heat_truncated: bool`; `heat_chrome_hit`
  reads `top_n.take(best_count)` and sets `truncated` when shorter;
  `doc_lines` uses `"retrieving…"` for an empty cache and appends `"…"`
  when `heat_truncated`.
- `heat_worker.rs`: `CompleteLists::get` reverted to `fn` (private).
- `tests/popup_doc.rs`: updated `"still scoring these bytes"` →
  `"retrieving…"`. All 1261 tests pass.
