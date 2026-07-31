<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0132 — protolens: override-pane live preview + smarter default target

Status: implemented — but the preview no longer works this way. G1's
        default-highlight ranking stands as written
        (`recompute_override_candidates`,
        `resolve_active_override_entry`). G2's live preview does not:
        it spliced the candidate into the tree and backed the splice
        out on `Esc`, via a helper `effective_render_target` that no
        longer exists. Spec 0185 made the preview a `PreviewOverlay` —
        rendered lines drawn over the viewport, mutating nothing, so
        there is nothing to back out. Read `override_select.rs`'s
        `preview_override_highlight` for what happens today.
Implemented in: 2026-07-15
Refs: docs/specs/0185-the-preview-is-an-overlay.md (replaces G2's
      splice-and-revert mechanic with an overlay),
      docs/specs/0114-protolens-override-selection-pane.md (§2 `Esc`-
      cancel semantics, §3.1/§3.2 default-highlight/candidate-ranking
      this spec's G1 refines),
      docs/specs/0117-protolens-override-collection.md (§2 per-kind
      origin, `OverrideCollection`/`activate`/`activate_auto`),
      docs/specs/0118-protolens-override-fidelity.md (§5/§6 render-
      cache and recursive `render_overrides` this spec reuses),
      docs/specs/0120-protolens-any-messageset-auto-overrides.md
      (auto-derived entries and their staleness, relevant to G3),
      protolens/todo.md (2026-07-15 feedback round 2, item 4 —
      discussion and decisions this spec formalizes)
App: protolens

## Background

Today, opening the override selection pane (`t`) shows a ranked
candidate list but the main pane keeps rendering the node exactly as
it did before the pane opened — moving the highlight through
candidates gives no visual feedback until `Enter` actually confirms.
Separately, the initial highlight on open only accounts for two of
three intended priority levels.

`recompute_override_candidates` (tui.rs 1172-1218) already implements
priorities (2)/(3) of the desired default-highlight order: it sets
`override_highlight = usize::from(!override_candidates.is_empty())`
— row 0 (`<raw / no type>`) when there are no ranked candidates, row 1
(the top-inferred candidate) otherwise. It does not yet consult
whether the target node already has an active override (priority (1)).
`resolve_active_override_entry(idx)` (tui.rs 1583-1622) resolves the
currently-active override entry for a node, checking `Path`, then
`PathField`, then `FqdnField` origin in that exact priority order —
exactly the lookup priority (1) needs.

For live preview, the real "confirm" path (`Enter` in
`handle_override_key`, tui.rs 2325+) is expensive and stateful: it
calls `self.overrides.activate(origin, new_fqdn)` (mutating the
persisted override collection) and then `self.render_overrides
(self.first_node)`, a recursive whole-tree walk that also handles
nested Any/MessageSet auto-expansion. Running that on every highlight-
move keystroke (arrow-key autorepeat can fire many events/second)
would be both wasteful and unsafe — it would mutate persisted state
before anything is confirmed.

`render_overrides` itself, per node, bottoms out in a cheaper
primitive: `splice_override(idx, target)` (tui.rs 1803+) re-decodes/
re-renders *one* node's payload under a given type (render-cache keyed
on `(payload_range, target, field_name)`), splices the new lines into
`self.tree`/`self.lines`, but does *not* touch `self.overrides` and
does not recurse into children. This is the right building block for
preview.

Critically, `render_overrides`'s own "effective type" computation
(tui.rs ~1742-1757) is more than a plain `resolve_active_override_entry`
lookup:

```rust
let stale_auto_entry = self.resolve_active_override_entry(idx).filter(|e| e.auto).cloned();
let target = match stale_auto_entry {
    Some(entry) if !self.auto_entry_in_scope(&entry) => None,
    _ => self.resolve_active_override(idx),
};
let field_name = self.field_name_for(idx);
let current = Some((target.clone(), field_name));
if current != self.tree[idx].rendered_as {
    let effective = match &target {
        Some(explicit) => explicit.clone(),
        None => self.natural_type(idx),
    };
    match self.splice_override(idx, effective) {
        Ok(()) => self.tree[idx].rendered_as = current,
        Err(e) => self.message = format!("cannot apply override: {e}"),
    }
}
```

It additionally (a) detects a *stale* auto-seeded entry via
`auto_entry_in_scope` (spec 0120's follow-up demotion logic — an
auto-derived entry whose ancestor context has since changed) and
treats it as `None` rather than as still-active, and (b) falls back to
`natural_type(idx)` (tui.rs 1385-1390, the node's plain schema type),
not just `None`, when nothing is active at all. Reverting a preview
(`Esc`) must reproduce this exact computation to restore precisely
what was on-screen before the pane opened — a plain
`resolve_active_override_entry`-only revert would, in the stale-auto
or nothing-active cases, splice the wrong type back in.

## Goals

### G1 — default-target priority: active override first

- `toggle_override` (tui.rs 1003+), before calling
  `recompute_override_candidates`: call
  `self.resolve_active_override_entry(self.cursor)`. If it returns
  `Some(entry)`, after `recompute_override_candidates` runs, look up
  `entry.r#type` in `self.override_candidates` (row 0, i.e.
  `override_highlight = 0`, if `entry.r#type` is `None`; otherwise the
  1-based row of the matching candidate FQDN, if present) and use that
  as the initial `override_highlight` instead of
  `recompute_override_candidates`'s own computed default. If
  `entry.r#type` is `Some(fqdn)` but `fqdn` is not present in
  `override_candidates` (e.g. `SortMode::Lexicographic` where every
  known FQDN is listed, so this should not normally happen, but
  `SortMode::Inferred`'s capped/incomplete candidate list could
  plausibly omit it), fall back to `recompute_override_candidates`'s
  own default rather than leaving `override_highlight` unset.
- If `resolve_active_override_entry` returns `None`, behavior is
  unchanged (priorities (2)/(3), already implemented).

### G2 — live preview on highlight move

- New shared helper (see `tui.rs` Specification below) that, given the
  currently highlighted row in the override pane, resolves the
  tentative target type: `None` for row 0 (`<raw / no type>`),
  otherwise `override_candidates[row - 1].0.clone()`.
- Every `override_highlight`-changing key in `handle_override_key`
  (`j`/`Down`, `k`/`Up`, `PageDown`, `PageUp`, `Home`, `End`) — after
  updating `override_highlight` as today — additionally calls
  `self.splice_override(override_target_idx, tentative_type)` for the
  newly-highlighted row's tentative type, splicing the preview
  directly into the main pane. This call does **not** update
  `self.tree[idx].rendered_as` (left exactly as `render_overrides`
  last set it) — so a later `Enter`-confirm, which runs the real
  `activate()` + `render_overrides()` path, still correctly re-
  evaluates from persisted state (its own `rendered_as` comparison
  sees a mismatch whenever the confirmed type differs from what was
  last non-preview-rendered, and a no-op whenever it doesn't) and is
  entirely unaffected by whatever preview happened to be showing.
- Errors from `splice_override` during preview (e.g. malformed
  payload under the tentatively-selected type) set `self.message`
  exactly as `render_overrides` does today — the preview simply fails
  to update for that candidate, main pane keeps its last successfully-
  spliced state, browsing further remains possible.
- On first opening the override pane (`t`), the initial highlighted
  row (per G1) is also live-previewed immediately, via the same
  helper — so the main pane shows the tentative type from the very
  first frame, not just after the first navigation keystroke.

### G3 — `Esc` reverts to the actual effective type

- New shared helper `effective_render_target(&mut self, idx: usize)
  -> Option<String>` (tui.rs), factored out of `render_overrides`'s
  existing target-computation block (`stale_auto_entry`/
  `auto_entry_in_scope`/`resolve_active_override`, falling back to
  `natural_type(idx)` when nothing is active) — the *exact* same
  logic quoted in Background, used by three call sites: (a)
  `render_overrides` itself (replacing its inline block, no behavior
  change), (b) G2's live-preview call sites, and (c) this goal's
  `Esc`-revert.
- `close_override` (tui.rs 1147+), when closing via `Esc`/`t` from
  `handle_override_key`'s cancel arm (not via the `Enter`-confirm
  path, which already calls its own `close_override` after
  `render_overrides` has correctly re-settled the node): before
  clearing `override_target`, compute
  `self.effective_render_target(idx)` for the target node and splice
  it back in via `self.splice_override(idx, effective)`, updating
  `self.tree[idx].rendered_as` to match (mirroring `render_overrides`'
  own bookkeeping) so a later full `render_overrides` pass sees
  "already matches, no-op" rather than redundantly re-splicing.
- This restores exactly what would be on-screen had the override pane
  never been opened — including the stale-auto-entry-demoted-to-`None`
  case and the nothing-active-falls-back-to-`natural_type` case, both
  of which a naive `resolve_active_override_entry`-only revert would
  get wrong.

## Non-goals

- No live preview of nested Any/MessageSet auto-expansion within the
  tentatively-previewed type — preview only re-splices the override
  target node itself (unchanged from `splice_override`'s existing
  single-node scope), not its children; a nested Any/MessageSet field
  inside the previewed type only auto-expands once `Enter` actually
  confirms and the real recursive `render_overrides` pass runs. Same
  limitation the override pane already has today for its flat
  candidate list (not new).
- No debounce or cancel-in-flight handling for rapid highlight-move
  events (e.g. arrow-key autorepeat). Deferred per the user's
  agreement in todo.md item 4 — `splice_override` is already cache-
  backed and no slower than what `Enter` already pays today; revisit
  only if/when a future "complete preview" (see Refs) proves too slow
  in practice, starting with a simple debounce (no threads) if ever
  needed.
- No background-thread/async rendering — the preview stays fully
  synchronous within the existing single-threaded event loop.
- No change to `SortMode`/candidate ranking/search (`/`, `?`, `n`)
  themselves — this spec only adds preview-on-move and fixes the
  initial highlight priority.

## Specification

### `tui.rs`

- New method `fn effective_render_target(&mut self, idx: usize) ->
  Option<String>`: extracted verbatim from `render_overrides`'s
  existing `stale_auto_entry`/`target` computation (the first half of
  the block quoted in Background, up to and including the `target`
  binding) — returns the type `render_overrides` would treat as
  "active" for this node, `None` meaning "no override active, use
  natural type."
- `render_overrides` (tui.rs 1699+): replace its inline
  `stale_auto_entry`/`target` computation with a call to
  `self.effective_render_target(idx)`; the subsequent
  `current`/`rendered_as`-comparison/`splice_override` logic is
  otherwise unchanged.
- `toggle_override` (tui.rs 1003+): per G1, resolve
  `resolve_active_override_entry(self.cursor)` before
  `recompute_override_candidates`, then adjust the resulting
  `override_highlight` per G1's lookup rule. After settling
  `override_highlight`, call the new preview helper (see next bullet)
  once for the initial row, per G2's "first frame" requirement.
- New private helper (name open to pick while coding, e.g.
  `preview_override_highlight(&mut self)`): resolves the tentative
  target type for the current `override_highlight` row (`None` for
  row 0, else `override_candidates[override_highlight - 1].0.clone()`)
  and calls `self.splice_override(override_target_idx, tentative)`,
  ignoring/reporting errors via `self.message` as described in G2 —
  does not touch `rendered_as`.
- `handle_override_key` (tui.rs 2245+): after each of
  `move_override_highlight`, the `Home` arm, and the `End` arm's
  `override_highlight` assignment, call the new preview helper.
- `close_override` (tui.rs 1147+): per G3, when called from the
  `Esc`/`t` cancel arm, compute and splice back
  `effective_render_target(idx)` for the (still-`Some`)
  `override_target` node before clearing it, updating `rendered_as`.
  The `Enter`-confirm call site (tui.rs ~2373, which already calls
  `close_override` *after* `render_overrides` has run and correctly
  re-settled `rendered_as`) is unaffected either way — re-splicing to
  the value `rendered_as` already reflects is the cheap "already
  matches, no-op" path in `splice_override`'s caller-side comparison,
  costing at most one avoidable `effective_render_target` call. No
  separate code path is needed for the two callers.

## Test plan

1. Cursor on a node with an already-active override; press `t` — the
   pane opens with that override's type pre-highlighted (priority
   (1)), and the main pane already shows that type's rendering (no
   navigation needed).
2. Cursor on a node with no active override but with inferred
   candidates; press `t` — top-inferred candidate is pre-highlighted
   and pre-previewed (priority (2), unchanged from before this spec).
3. Cursor on a node with no active override and no inferred
   candidates; press `t` — `<raw / no type>` is pre-highlighted
   (priority (3), unchanged).
4. Move the highlight (`j`/`k`/arrows/`PageUp`/`PageDown`/`Home`/`End`)
   through several candidates — main pane updates live to each
   tentative type without committing to `self.overrides`.
5. `Enter` after previewing several candidates confirms exactly the
   final highlighted candidate (not some intermediate one) and
   persists it correctly in the override collection/manage pane.
6. `Esc` after previewing several candidates restores the main pane to
   exactly what it showed before the pane opened, in each of: (a) a
   pre-existing active (non-stale) override, (b) a stale auto-derived
   entry (demoted to natural type), (c) no override active at all
   (natural type).
7. Reopening the pane after an `Esc`-cancel still shows correct
   priority-(1)/(2)/(3) defaults per tests 1-3 (no leftover state from
   the cancelled preview).
8. `reuse lint` passes.
