<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0185 — the override preview is an overlay, not a splice

Status: implemented
Implemented in: 2026-07-26
App: protolens
Refs: docs/protolens/rendering-flaws.md (P3),
      docs/specs/0135-protolens-override-raw-tag-rewrap.md (G1),
      docs/specs/0160-protolens-render-overrides-batch-scaling.md (G2),
      docs/specs/0167-protolens-nested-patch-scope.md,
      docs/specs/0174-preview-interior-truncation-and-node-budget-removal.md,
      docs/specs/0183-prune-the-override-walk.md (N2, L2),
      docs/specs/0184-packed-records-are-the-addressable-unit.md

## Background

The override *selection* pane shows a live preview of the highlighted
candidate. It does so by **actually applying the override**:
`preview_override_highlight` (`override_select.rs:786-858`) calls
`splice_override(idx, tentative, true, None)` (`:854`) — the same
function a confirmed `Enter` calls, with one flag flipped.

A splice is a document-wide operation. With `override_batch_depth == 0`
— which is always the case here, because the preview calls
`splice_override` directly rather than through `render_overrides` — it
runs `finalize_override_batch`, and that means, per keystroke:

1. `materialize_line_patches` rebuilds `self.lines` and
   `self.line_styles`, deep-cloning every `String` in the document;
2. a `doc_next` walk shifts `text_range` on every node after the target;
3. `line_to_node` and `footer_line_to_node` are cleared and reinserted
   for the whole document;
4. `rebuild_visible_rows` re-scans `visible_rows`.

On the 1.1 MB `FileDescriptorSet` (622,922 nodes / 193,072 lines) that
is the machinery behind the measured 10.6 s commit — and the preview
pays it **on every `j`/`k` in the candidate list**.

### Undoing it is worse than doing it

Because the preview mutates the real document in place, backing it out
needs its own machinery:

- `preview_tree_watermark: Option<usize>` (`mod.rs:725`) records
  `tree.len()` before the first preview, so `tree` and `heat_states` can
  be truncated back;
- `folded`, `line_to_node` and `footer_line_to_node` are each `retain`ed
  against that watermark;
- `tree[idx].first_child`/`last_child` are nulled by hand;
- `tree[idx].rendered_as` is force-set to `None` so that the eventual
  revert re-splices unconditionally;
- and the revert itself, `close_override` (`override_select.rs:271-273`),
  is a **full `render_overrides(idx)` pass** — the same O(document) walk
  spec 0183 is about.

Six pieces of state exist only to undo something that should not have
been done. Every one of them is a place where the document can be left
subtly wrong, and none of them would exist if the preview had never
touched the document.

### What is actually needed

A preview needs to change **what the user sees**. It does not need to
change what the document *is*. Nothing downstream of the preview reads
the spliced state: the candidate list is already computed, scoring runs
against raw byte ranges, and confirming the override re-derives
everything from the entry anyway.

So the preview can be a **render-time overlay**: a block of rendered
lines held beside the committed document, substituted for the target's
rows while drawing, and dropped by assignment.

## Goals

- **G1** — a preview costs only the decode and render of the target's
  own (already byte-bounded) interior. Nothing document-sized is
  touched: not `lines`, not `line_styles`, not `tree`, not
  `line_to_node`, not `visible_rows`.
- **G2** — a preview is discarded by `self.preview_overlay = None`.
  `preview_tree_watermark` and the five `retain`/null/force-`None`
  fix-ups go with it, and `close_override` stops calling
  `render_overrides`.
- **G3** — the preview looks exactly as the committed splice would:
  same lines, same indentation, same colorization, same truncation
  marker. A user must not be able to tell, from the main pane, that the
  preview is not real.
- **G4** — the main pane can still be **panned** while a preview is up,
  vertically and horizontally, by keyboard and by wheel.
- **G5** — while the selection pane is open, focus cannot leave it. Not
  by `Tab`, not by clicking in the main pane.
- **G6** — 0183's N2 becomes true rather than anticipated: the preview
  is a different path, and the committed walk is the only thing 0183
  has to reason about.

## Non-goals

- **N1** — the committed path. `Enter` still splices, through
  `render_overrides`, exactly as today. This spec changes the *preview*
  only.
- **N2** — P3's four O(document) passes themselves. They stay, and stay
  wrong, for the committed path; this spec removes the preview as one of
  their callers. Fixing them is a separate spec.
- **N3** — the byte budget. `OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT`
  (`override_apply.rs:1489`) keeps its value and its meaning: it bounds
  the renderer's *input*, and it is what makes G1's cost claim true.
- **N4** — candidate computation, ordering, scoring, or the selection
  pane's own layout and key set (beyond the additions in S5).
- **N5** — previewing more than one node at a time. The overlay is
  singular by construction, which is what makes S2's arithmetic O(1).
- **N6** — making overlay content selectable, foldable, navigable, or
  addressable by path. It has no `NodeSpan`s in `self.tree` and
  therefore no identity. See S4.

## Specification

### S1. The overlay

```rust
struct PreviewOverlay {
    /// Index into `visible_rows` of the first row the committed
    /// target's `text_range` covers.
    first_row: usize,
    /// How many `visible_rows` entries that `text_range` covers.
    covered_rows: usize,
    lines: Vec<String>,
    line_styles: Vec<LineStyles>,
}
```

held as `App::preview_overlay: Option<PreviewOverlay>` — replacing
`preview_tree_watermark`.

*(As implemented: the `target` and `committed` fields this spec first
sketched are not carried. Nothing reads them — `target` is consumed at
construction time to find the anchor, `committed` only to compute it —
and a field no code reads is a field that can drift out of agreement
with the two that matter.)*

`first_row` is found by binary search: `visible_rows` is sorted
ascending, so `partition_point(|&l| l < text_range.start)` gives it, and
`covered_rows` is the run of entries below `text_range.end`. Both are
computed once, when the overlay is built.

**Spec 0184 interaction.** If `target` is an element of a packed run,
`committed` must be the **record's** extent (`packed_record_extent`'s
`text_range`), not the element's, and `target` its leader — because that
is what a commit would splice. Getting this wrong shows as a preview
that replaces one line of a run and leaves the rest.

### S2. Render-time composition

Exactly one contiguous span of rows is substituted, so the display-row
map is arithmetic, not a rebuilt vector. With
`delta = lines.len() - covered_rows`, the display row `d` resolves as:

| range of `d` | resolves to |
|---|---|
| `d < first_row` | committed line `visible_rows[d]` |
| `first_row <= d < first_row + lines.len()` | overlay line `d - first_row` |
| otherwise | committed line `visible_rows[d - delta]` |

and the total display row count is `visible_rows.len() + delta`.

`App::render` (`render.rs:230`) builds its window from
`visible_rows[scroll_offset..]` (`:309-315`); it instead walks display
rows through the map above. `render_line_spans` (`:79-120`) grows a
sibling that takes `(&str, &[(Range, SyntaxRole)])` directly, so both
committed and overlay rows reach the same span-building, `pan_spans`
(`:340`), cursor/selection `REVERSED` (`:405-412`) and `Paragraph`
(`:416`) code. **There must not be a second line-rendering path** —
that is how G3 is met, and reviewing it is how G3 is checked.

`scroll_offset` is clamped against the composed row count, not
`visible_rows.len()` — which also bounds keyboard and wheel panning
(G4), since both go through `pan_vertical`.

**Two cursor-row spaces, kept apart.** `cursor_display_row()` is a
binary search into `visible_rows`, so it answers in *committed* row
space, and that is the space `last_cursor_row` and `clamp_pan_offset`'s
guard compare against. The render loop's `REVERSED` comparison needs
*composed* space instead, because it compares against rows it is
drawing. So it uses `cursor_composed_row()`, which carries the same
answer across the substitution; a cursor inside the block the overlay
stands in for — the usual case, since the preview's target *is* the
node the cursor is on — resolves to the overlay's own first row.
Conflating the two shows as the highlight bar drifting by `delta` rows
while a preview is up.

### S3. Building the overlay

`splice_override` already computes exactly what the overlay needs, at
`override_apply.rs:1653-1779`: it slices `field_bytes` from the blob
(`:1653`), applies the byte budget (`:1667`), consults `render_cache`
(`:1685`), decodes and renders at `old_span.level`, colorizes, patches
the synthetic field name into the header, and appends spec 0174's
truncation marker. The splice proper begins at `:1780` with
`let delta = ...`.

Factor `:1653-1779` out as `render_node_as`. `splice_override` calls it
and proceeds; the preview calls it and stops. The `NodeSpan`s the
preview receives are **discarded** (N6).

As implemented the signature carries three things this spec's sketch
left out, each of them load-bearing:

```rust
fn render_node_as(
    &mut self,
    idx: usize,
    target: Option<&str>,
    is_preview: bool,
) -> Result<(usize, NodeSpan, RenderedAs), String>;

struct RenderedAs {
    lines: Vec<String>,
    line_styles: Vec<LineStyles>,
    spans: Vec<NodeSpan>,
    span_shift: usize,
}
```

- `is_preview` selects spec 0174's interior truncation. It has to be a
  parameter rather than "true for the preview, false for the splice",
  because both callers are now the same function and only the caller
  knows which it is.
- The returned `usize` is the **resolved** index. Spec 0184's packed
  normalization happens inside `render_node_as` (it is what decides
  which bytes to render), so it is what redirects a packed element to
  its run's leader and widens `old_span` to the record's extent — which
  is exactly the S1 interaction above, obtained rather than
  reimplemented.
- `span_shift` (spec 0174 §S3) is how far a truncated preview's interior
  moved left because the rewritten LEN framing is narrower. Only
  `splice_override` needs it, to correct `spans`' byte ranges; the
  preview discards it with the spans.

This is the one real refactor in this spec, and the risk is that the
extracted half quietly depends on state the splice half sets. It does
not appear to — the region reads `self.blob`, `self.ctx`,
`self.indent_size`, `self.render_cache` and the already-cloned
`old_span` — but "does not appear to" is what the test plan's
byte-equality check is for.

`render_cache` is shared between the two callers, unchanged: a preview
that the user then confirms renders the same bytes as the same type and
hits the cache.

### S4. Overlay rows have no node

Overlay lines are not in `line_to_node`, so:

- **heat cues**: `heat_cue_for` finds no node and returns
  `HeatDisplay::None`, leaving the gutter blank. This is correct rather
  than merely convenient — a cue reports how well a *committed* node's
  bytes score as its current type, and an overlay row has no committed
  node. Do not synthesize one.
- **the bold active-override hint** (`render.rs:341-344`): likewise
  absent on overlay rows.
- **fold markers**: absent. The overlay block always renders in full;
  `folded` is never consulted for it and never mutated by it. If the
  target was folded when the pane opened, it renders unfolded for the
  overlay's lifetime and returns to folded when the overlay is dropped
  — no saved state, because none was changed.
- **click and selection**: an overlay row cannot be clicked onto or
  drag-selected. S5 makes this moot by refusing main-pane clicks
  outright.

### S5. Focus is locked to the selection pane

While `override_target.is_some()`:

- `override_focus` is `true` and stays `true`.
  `handle_override_key`'s `KeyCode::Tab => self.override_focus = false`
  (`key_dispatch.rs:28`) becomes a no-op. `key_dispatch.rs:623`'s
  `Tab`-into-the-pane arm becomes unreachable, since the main pane never
  holds focus.
- `handle_mouse`'s unconditional `override_focus = false; manage_focus
  = false` on a left-click in `main_area` (`mouse.rs:134-135`) is
  suppressed. A main-pane click while the pane is open moves no focus,
  moves no cursor, and starts no selection.
- **Panning still works.** Wheel events already route by geometry
  rather than by focus (`mouse.rs:107-124`), so `wheel_pan_up`/`down`
  and the horizontal `wheel_pan_left`/`right` (`:87-91`) keep working
  unchanged. Keyboard panning does not, so `handle_override_key` gains
  arrow keys that delegate to the main pane's own
  `pan_vertical_up`/`down`/`pan_left`/`pan_right`
  (`navigation.rs:292-333`).

  **Deviation: Alt-arrows, not Ctrl-arrows.** This spec first said
  `Ctrl-Up`/`Down`/`Left`/`Right`, on the grounds that they are handled
  only in the main-pane branch. That is wrong — `handle_override_key`
  already binds all four to the *candidate list's* own pan
  (`key_dispatch.rs:37-54`), so taking them would have silently
  replaced a working binding. `Alt` was free (bound only in
  `handle_command_key`), so main-pane panning from the selection pane is
  `Alt-Up`/`Down`/`Left`/`Right` and the candidate list keeps `Ctrl`.
- The cursor does not move for the overlay's whole lifetime, so
  `render.rs:305-308`'s `last_cursor_row` guard never fires and
  `clamp_scroll_to_visible` never fights the user's panning.

**Why the lock is load-bearing, not just a simplification.** It is what
makes the overlay's anchor immutable. `first_row` and `covered_rows`
are positions in `visible_rows`; if the cursor could move, folds could
change, `visible_rows` could be rebuilt, and those two numbers would
silently address the wrong rows. Under the lock, the only inputs to
`visible_rows` — folding and splicing — are both unreachable, so the
anchor computed in S1 is valid until the overlay is dropped. A terminal
resize changes `pane_height` only, not `visible_rows`, so it is safe.

The lock is therefore stated as an invariant, not a preference: **the
overlay is only correct because nothing can move underneath it.**

### S6. Lifecycle

- **Built or rebuilt** by `preview_override_highlight`, on all nine of
  its current call sites (pane open `override_select.rs:128`, open from
  manage `:358`, background candidate arrival `:601`, highlight move
  `:623`, search jump `:888`, candidate click `mouse.rs:307`, and
  `key_dispatch.rs:19`/`:65`/`:72`). Rebuilding is a plain overwrite;
  there is nothing to undo first.
- **Dropped** on close, cancel, and confirm — `self.preview_overlay =
  None`. `close_override` (`override_select.rs:271-273`) drops it and
  **stops calling `render_overrides`**; there is nothing to revert.
- **Confirm** drops the overlay first, then applies the override through
  the ordinary committed path (N1). The overlay must not be alive while
  a splice runs, or S2's anchor would be stale.
- A candidate that fails to render leaves `preview_overlay` at `None`
  and the main pane showing committed content — the same outcome as
  today's failed preview, reached without a partial mutation.

### S7. Statusline

The main pane's statusline reports the **committed** document's line
count and the (frozen) cursor's committed line number, and gains an
indication that a preview is displayed. Overlay rows are not part of the
document and must not be counted as if they were.

While the override-selection pane is active it also carries the focus
lock's explanation (Q1): a message stating that the main pane is out of
reach until the override-selection pane is closed. This is the only
signal the lock gets — the cursor itself is not restyled. The message is
tied to the pane being active, not to an overlay existing, so it is
shown for a candidate that failed to render too, where the lock still
holds but no overlay does.

## Test plan

- **G3 is asserted by byte equality, and it is the acceptance
  criterion.** For each of several candidate types: build the overlay,
  then separately `splice_override` the same node for real and slice
  the committed `lines`/`line_styles` by its resulting `text_range`.
  They must be identical, styles included. This is the test that catches
  an S3 factoring that dropped the header patch, the truncation marker,
  or the indentation level. (The comparison is on the *lines*, not on
  `row_content`: display-time fold markers are S4's documented
  difference between an overlay row and a committed one, so comparing
  rendered rows would assert the opposite of what S4 specifies.)
- **G1 by assertion, not by timing:** after a preview, `lines.len()`,
  `tree.len()`, `line_to_node.len()` and `visible_rows` are all
  bit-for-bit what they were before it, and after dropping the overlay
  the whole `App` state compares equal to a never-previewed one. A
  timing test would pass for the wrong reasons on a small fixture.
- **The row map (S2) gets a direct test** at the three boundaries:
  a row before `first_row`, the first and last overlay rows, and the
  first row after — for `delta` negative, zero, and positive.
- **The lock's message (S7):** the main-pane statusline carries it
  whenever the override-selection pane is active, including for a
  candidate that failed to render, and drops it on close.
- **Focus lock (S5):** with the pane open, `Tab` leaves
  `override_focus` set; a synthesized left-click in `main_area` leaves
  focus, `cursor` and `select_anchor` untouched; `Alt-Down` and a wheel
  event both change `scroll_offset` and neither changes `cursor`. The
  management pane's own `Tab` still works, so that Q2's deliberate
  difference is pinned rather than merely intended.
- **Packed run (S1):** previewing an element of a run replaces the
  whole run's rows, not one.
- **Fold (S4):** previewing a folded target renders the overlay in
  full and leaves `folded` unchanged; dropping the overlay restores the
  folded rendering.
- `reuse lint` and `nix-build -A ci`.

## Measured outcome

`profile_preview_root_versus_first_child_on_db3`
(`tui/tests/profiling.rs`, `#[ignore]`d) on the 1.1 MB `db3.desc`
(149,359 nodes / 196,701 lines), 20 alternating previews each:

| target | per preview |
|---|---|
| root | 245 µs |
| first child | 32 µs |

with `tree.len()` and `lines.len()` unchanged across both runs — G1
observed on a real document, not just on a fixture.

This settles the 2026-07-26 report that "browsing types for the first
child is faster than for the root, but still noticeably slower". The
asymmetry was never in the decode or the render, both of which the byte
budget already bounds identically: it was `finalize_override_batch`'s
four O(document) passes, which the *root's* splice happened to make
cheap because replacing the root replaces the whole document, leaving
nothing for the passes to walk. Splicing the first child left the other
~196,000 lines in place, so all four ran at full size on every
keystroke. Removing the preview as their caller removes the asymmetry
with them; what is left is the render, where the root is now the slower
of the two.

## Resolved questions

- **Q1 — the cursor is left as it is; the lock is announced in words
  instead.** No dimming and no change of attribute, which keeps the
  main pane's appearance identical to a real splice as G3 requires.
  The user is told about the lock rather than shown it: while the
  override-selection pane is active, the main pane's statusline states
  that the main pane is out of reach until that pane is closed (S7).
  A sentence explains the restriction; a dimmed cursor would only hint
  at it.
- **Q2 — the manage pane does not get the lock.** It performs actual
  splices and renders, so its main-pane content is committed content;
  nothing there depends on an immutable anchor. The lock is specific to
  the overlay, and the two panes differing under `Tab` is deliberate.
- **Q3 — settled: `close_override` has nothing else to do.** Checked
  against the code. `close_override`'s own doc comment
  (`override_select.rs:255-270`) states the single reason that
  `render_overrides` call exists: the live preview's `splice_override`
  rebuilds the target's whole subtree from scratch with no overrides
  applied to any of the fresh descendants, so a `resettle_node`-only
  revert would leave every previously-auto-expanded `Any`/`MessageSet`
  descendant un-re-expanded, and only the full recursive pass re-seeds
  them. That descendant damage exists *because* the preview mutated the
  tree. An overlay never mutates it, so there is nothing to re-seed and
  nothing to revert. The same comment also records that the
  `Enter`-confirm call site already runs `render_overrides` itself, so
  this call is already a no-op on that path.

  Consequently: **cancel and close re-render nothing.** A re-render is
  triggered only by the events that change committed state — selecting
  a different type and confirming it, or activating an override that
  was deactivated.
