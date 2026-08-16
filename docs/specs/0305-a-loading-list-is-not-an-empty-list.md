<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0305 — a loading list is not an empty list

Status: implemented
Implemented in: 2026-08-16
App: protolens
Refs: docs/specs/0139-….md (the A/B/C ladder `toggle_override` walks to
        pick an initial sort mode and highlight),
      docs/specs/0152-….md (spec 0152 G7: the shared heat cache, the
        pending flags, and `poll_pending_override_work`),
      docs/specs/0114-….md (the override selection pane; §3.2's sort
        modes and §6's complete-list upgrade),
      docs/specs/0137-….md (§G1/§G4: the fixed lexicographic universe —
        `none`, then the keywords, then every FQDN),
      docs/specs/0299-….md (`message` joins that universe beside `none`)

## Background

Open the override pane with `t` on a node for which spec 0139's ladder
finds no candidate type — a root with no schema, most obviously — and
`open_override_on_default` runs. It asks for `Inferred` order, and on a
cold shared cache `heat_lookup` answers with a queued request rather
than a list. The pane comes up empty.

Empty is the wrong thing to show. The lexicographic universe (spec 0137
§G1) is fixed, known synchronously, and always applicable: `none`,
`message`, the fifteen primitive keywords, then every FQDN in the pool.
The reader could be choosing from it while the scoring runs. Instead
they get a blank pane and a `Scoring candidates…` message, and the only
way out is to know that `i` toggles the sort mode.

The existing code half-handles this: when there is *no* scoring graph at
all it falls back to `Lexicographic` outright. The cold-cache case is
the gap — a graph exists, so the fallback is skipped, but the answer has
not arrived yet.

### The clobber

A first attempt at this — set `override_candidates` to the lexicographic
list, keep `override_sort = Inferred` so the arriving scores can replace
it — does not work, and the reason is worth recording because it will
catch the next attempt too.

`upgrade_active_override_to_complete` ends with:

```rust
if self.override_sort == SortMode::Inferred {
    self.override_candidates = self
        .override_inferred_raw
        .iter()
        .map(|(f, s)| (f.clone(), Some(*s)))
        .collect();
}
```

On a cold cache `override_inferred_raw` is empty, and the sort mode is
`Inferred` by construction — so the call made immediately after
installing the placeholder overwrites it with an empty list. The pane is
blank again. Reproduced by a test that installs the placeholder, calls
`upgrade_active_override_to_complete`, and finds `override_candidates`
empty.

The sync is unconditional today because, absent a placeholder, it is a
no-op on a miss: `override_candidates` was derived from the same
`override_inferred_raw` a moment earlier. It only becomes destructive
once something else has written to `override_candidates`.

This is the second bug of its shape in this file. Spec 0152 G7's
`poll_pending_override_work` had the same one — see
`poll_pending_override_work_does_not_clobber_a_non_inferred_sort_mode`,
fixed 2026-07-20 by guarding on the sort mode. That guard is not enough
here, because here the sort mode is deliberately still `Inferred`.

## Goals

- **G1.** Opening the override pane on a cold cache shows the
  lexicographic list immediately, instead of nothing.
- **G2.** When the scored list arrives it replaces the placeholder, and
  the pane is in `Inferred` order as the user asked.
- **G3.** A cache fetch that returns nothing never blanks a list that is
  already on screen.

## Non-goals

- **N1.** Do not block on the scoring request. The pane opens on the
  keystroke; spec 0257's posture — show a screenful now, finish later —
  is the house rule.
- **N2.** Do not change the no-graph path. Falling back to
  `Lexicographic` outright is right when no answer is ever coming: the
  sort mode should report what the user is actually looking at.
- **N3.** Do not show a spinner or a "loading" row among the candidates.
  The rows are selectable types; a non-selectable row among them would
  have to be excluded from every motion, search and confirm path.
- **N4.** Do not reorder the placeholder to guess at likely types. It is
  the lexicographic universe exactly as `i` would produce it, so a user
  who starts reading it and a user who pressed `i` see the same thing.

## Specification

- **S1.** `open_override_on_default`, on an empty `Inferred` list, splits
  on `override_candidates_pending`:

  - **pending** — a request is in flight, so an answer is coming. Leave
    `override_sort = Inferred`, fill `override_candidates` with the
    lexicographic universe as a placeholder, and call
    `upgrade_active_override_to_complete` so the full scored list is
    already being fetched when the first page lands.
  - **not pending** — no graph, or the graph had nothing. Nothing is
    coming; fall back to `Lexicographic` as today, so the sort mode
    names what is on screen.

- **S2.** A private `lexico_candidates()` helper returns that universe —
  `none`, `message`, `ALL_PRIMITIVE_KEYWORDS`, then `all_type_fqdns`,
  each paired with `None` for the score column — without touching
  `override_sort`. It is the same sequence `recompute_override_candidates`
  builds for `SortMode::Lexicographic`; both call the helper, so the
  order cannot drift between the placeholder and the real thing.

- **S3.** `upgrade_active_override_to_complete` syncs
  `override_candidates` from `override_inferred_raw` **only when the
  lookup returned candidates** — the sync moves inside the `Some` arm of
  the `heat_lookup` match. On a miss it leaves the on-screen list alone.

  This is what makes S1's placeholder survive. It is also correct
  independently of S1: on a miss the sync could only ever rewrite
  `override_candidates` with what it already held, so nothing else
  depends on it running.

- **S4.** `poll_pending_override_work` needs no change. Its own sync is
  already inside `if let Some(top_n) = lookup`, so a miss cannot blank
  the placeholder; and when the first page arrives it replaces the
  placeholder wholesale and resets the highlight to row 0, which is the
  behavior G2 asks for.

  Its `override_complete_pending` branch calls
  `upgrade_active_override_to_complete`, which S3 has made safe.

- **S5.** An inferred answer that arrives *empty* is not an answer.
  Wherever a `heat_lookup` hit yields an empty candidate list while
  `override_sort == Inferred`, the pane does what S1's not-pending
  branch does: clear the message, set `override_sort = Lexicographic`,
  recompute. Two sites — `upgrade_active_override_to_complete`'s `Some`
  arm and `poll_pending_override_work`'s.

  This is N2's rule applied to a late answer rather than an absent one:
  nothing further is coming that could fill an empty inferred list, so
  the sort mode must name what is on screen. It is also what makes the
  first `t` and the second `t` agree — the second, against a now-warm
  cache, already takes S1's not-pending branch and lands in
  `Lexicographic`.

  Without it, the reported symptom: the placeholder appears on the
  keystroke and is then wiped by the arriving empty list. Every
  candidate really is vetoed on a truncated root (spec 0266), so this
  is the common case there, not a corner one.

## Alternatives considered

### Fall back to `Lexicographic` on a cold cache, as the no-graph path does

The one-line version: drop the `override_candidates_pending` guard so
every empty `Inferred` list falls back. Rejected: it discards the
in-flight fetch's result. The user asked for inferred order by opening
the pane; when the scores arrive the pane should be showing them, and
after a `Lexicographic` fallback nothing ever puts it back — spec 0152
G7's sort-mode guard, added to fix the converse bug, deliberately parks
the arriving data instead of applying it.

### Keep the pane empty but improve the message

`Scoring candidates…` is accurate. Rejected: it tells the reader to
wait when there is a complete, usable list available synchronously. The
message is worth keeping only if there is genuinely nothing to show.

### Guard the sync in `upgrade_active_override_to_complete` on a
`placeholder_installed` flag

Instead of S3's "only sync on a hit", track whether the current
`override_candidates` is a placeholder and skip the sync if so.
Rejected: a second piece of state that every future writer of
`override_candidates` would have to maintain, to express something the
`Some`/`None` of the lookup already says.

## Test plan

1. `cold_cache_open_shows_the_lexicographic_list` — with a graph and an
   empty cache, opening the pane on a default target leaves
   `override_candidates` non-empty and `override_sort == Inferred`.
2. `the_placeholder_survives_the_complete_fetch` — the regression that
   pinned the clobber: install the placeholder, call
   `upgrade_active_override_to_complete` against a cold cache, and
   require `override_candidates` to be unchanged.
3. `arriving_scores_replace_the_placeholder` — populate `by_range`, call
   `poll_pending_override_work`, and require the list to be the scored
   one with the highlight back at row 0.
4. `no_graph_still_falls_back_to_lexicographic` — unchanged behavior:
   with `ctx.graph == None` the pane ends in `SortMode::Lexicographic`.
5. `the_placeholder_is_the_same_list_the_i_toggle_gives` — the
   placeholder equals `recompute_override_candidates`'s
   `Lexicographic` output for the same target, pinning S2's shared
   helper.
6. `an_empty_scored_answer_falls_back_to_lexicographic` — S5: cache a
   `RangeHeatEntry` with an empty `top_n`, call
   `poll_pending_override_work`, and require `override_sort ==
   Lexicographic` with the full universe on screen rather than a blank
   pane.
