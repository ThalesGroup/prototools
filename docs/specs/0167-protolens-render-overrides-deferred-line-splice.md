<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0167 — protolens: defer `render_overrides` batch line-buffer splices to a single materialization pass

Status: implemented
Implemented in: 2026-07-25
App: protolens
Refs: docs/specs/0160-protolens-render-overrides-batch-scaling.md
      (N1 non-goal this spec addresses), `protolens/src/tui/
      override_apply.rs`, `protolens/src/tui/mod.rs`

## Background

Spec 0160 fixed an O(M×N) blowup in `render_overrides`'s
whole-*document* bookkeeping (forward doc-chain shift, ancestor
closing-brace shift, `line_to_node`/`footer_line_to_node` full
rebuild, `rebuild_visible_rows()`) but explicitly left one cost
unaddressed as its N1 non-goal: the `self.lines.splice`/
`self.line_styles.splice` calls themselves
(`override_apply.rs::splice_override`, current lines ~1293-1300),
each an unavoidable-looking `Vec::splice` — genuinely O(document
length), since inserting/replacing a range in the middle of a
`Vec<String>`/`Vec<Vec<(Range<usize>, SyntaxRole)>>` must memmove
every element after the splice point. `render_overrides_inner` calls
`splice_override` once per qualifying node during its single
whole-document walk (spec 0160's own measurement: 47,342 calls on a
147,342-message-node document) — M splices, each O(N), is exactly
the residual O(M×N) cost spec 0160 measured at ≈8.3 s on that fixture
after G1/G2 landed, left as "the residual cost behind the measured
8.3 s figure ... would need a deeper 'collect patches, materialize
once' redesign to eliminate."

This residual cost has since been confirmed to dominate three
separate real workflows, not just spec 0160's own `App::new` startup
case, once a separate `positional_path`-recompute blowup was fixed
(see this repo's `protolens_tui_patterns.md` memory note, "Hot
O(n)-over-the-document walks must never recompute `positional_path`
per node") and both fixes were validated on `/tmp/pdb.desc`
(622,922 tree nodes):

- **Interactive override confirm** (`t` then `Enter` on the first
  candidate): ≈5.1 s, growing the tree by 622,922 new nodes.
- **Activating a different override for an already-overridden node**
  (reopen the manage pane, pick a different candidate, confirm):
  ≈8.5 s, growing the tree by 1,018,680 new nodes.
- **Automatic root-type-resolved render** — `App::new` spawns a
  detached background thread (`decode::resolve_root_winner_fqdn`)
  that, on completion, sends `AppEvent::RootTypeResolved` through the
  ordinary event loop; handling that event calls
  `apply_resolved_root_type` → `render_overrides(first_node)`
  **synchronously on the main/UI thread**, without any keypress —
  the same code path as the interactive case above, just triggered
  automatically. This is the ≈10 s "FileDescriptorSet-typed
  rendering" delay observed after `/tmp/pdb.desc` startup's initial
  (faster, `None`-typed) render.

All three measured at a consistent ≈8-9 µs/new-node rate, matching
`App::new`'s own initial expansion rate — i.e. genuinely proportional
per-splice work, not a remaining algorithmic bug on top of spec
0160's fix, but still large enough (minutes, cumulatively, across a
session on a document this size) to be worth reducing.

**Narrower scope than spec 0160's own N1 wording suggested.** Spec
0160's Background speculated the fix would need the recursion to
"walk a scratch/local tree representation for not-yet-materialized
splice results rather than immediately `self.tree.push`-ing them."
Re-examining `splice_override` for this spec shows that's broader
than necessary: `self.tree.push` only ever *appends* a freshly
decoded local subtree at the array's end (`base = self.tree.len()`)
and re-links it into the live tree purely via stored indices
(`parent`/`first_child`/`next_sibling`/`doc_next`, etc.) — no other
tree entry's position needs to move or be memmoved when a new node is
pushed. This is already O(1) amortized per node and is *not* part of
the O(M×N) blowup. Only `self.lines`/`self.line_styles` — flat,
line-indexed buffers where a splice's position determines where
*every later line* in the whole buffer must shift — have this
property. No change to `self.tree`'s push mechanics is needed; this
spec is narrower than spec 0160's Background anticipated.

**Deferring the line-buffer write is safe.** `self.lines`/
`self.line_styles` are read in exactly three places in the codebase
outside of `override_apply.rs` itself: `render.rs` (drawing to the
terminal), `mouse.rs` (click-to-column mapping), and
`override_select.rs` (search). None of these run while a
`render_overrides` batch is in progress (`override_batch_depth > 0`)
— they're driven by the main event loop between key events, after
any active batch has already returned. Nothing inside
`override_apply.rs`'s own splice/render walk reads `self.lines`/
`self.line_styles` either (`splice_override`'s own header-patch logic
operates on its local `new_lines`, never on `self.lines`). So
`self.lines`/`self.line_styles` can be left untouched for the
duration of an entire batch and reconstructed once at the very end,
with no other code observing the stale intermediate state.

**Recovering each patch's pre-batch position in O(1).** By the time
`splice_override` is called, `self.tree[idx].span.text_range` is
already corrected for every earlier splice in the *current* batch
(spec 0160 G2's carried-down `inherited` correction) — but
`self.lines` itself, under this spec, is deliberately left in its
*pre-batch* state until the final materialization pass, so a patch
recorded using the already-corrected `text_range` would target the
wrong offset into the still-unmodified `self.lines`. For a node
whose position is still relative to `self.lines` directly, this is
recoverable in O(1) without a second walk: `render_overrides_inner`
visits nodes in a strict pre-order, left-to-right (sibling-order)
walk — the same ordering spec 0160 G2 already relies on for its
`child_owed` accumulation — so every `splice_override` call within
one batch occurs at a document position that is monotonically
non-decreasing across the whole walk. Consequently,
`self.pending_shift`'s value *immediately before* a given call's own
`self.pending_shift += delta` already equals the cumulative delta of
every earlier splice in the batch, all of which lie at or before the
current node's position. Subtracting that value from the
already-corrected `text_range` recovers the exact pre-batch position:
`original_start = corrected_start - pending_shift_before_this_call`
(and likewise for `end`).

**Patches nest — a flat list isn't sufficient.** The paragraph above
covers only nodes that already existed before the batch began. A
node freshly created *by this same batch's own splice* (`splice_
override`'s `self.tree.push` for a decoded subtree's nodes) can
itself immediately need its own re-splice within the very same
batch — e.g. a nested submessage whose own natural-type or
active-override resolution differs from how its just-decoded parent
rendered it — and this turned out, once implemented and exercised
against real fixtures, to be the *common* case for nested submessages,
not a rare edge case. Such a re-splice's content cannot be recorded
as a patch against `self.lines` directly (which, for the whole
duration of the batch, still holds only pre-batch content): its true
position lies inside its parent's own freshly decoded, not-yet-
materialized content. Patches therefore form a tree, keyed by a
`patch_scope: Option<usize>` threaded top-down through `render_
overrides_inner` (mirroring how `inherited`/`path` are already
threaded) recording the nearest still-open ancestor patch a node's
own position currently lies within (`None` = still directly in
`self.lines`).

A second, subtler wrinkle surfaced once nesting was implemented:
`render_overrides_inner`'s `inherited`-shift propagation (spec 0160
G2's `child_owed` accumulation) keeps every node's `text_range`
tracking its true *final* document position — needed by `finalize_
override_batch`'s own `line_to_node` rebuild — which keeps growing
every time an earlier-processed sibling *within the same not-yet-
materialized parent* itself gets spliced. But a nested patch's
parent's own recorded `lines` is a frozen snapshot, captured once at
the parent's own splice time and never touched again until
materialization — a nested child's local offset into that frozen
snapshot must specifically *exclude* this growth, or it drifts out of
bounds relative to the parent's actual (still pre-growth-sized)
content. Each `LinePatch` therefore also records `children_base_
shift`: `App::pending_shift`'s value at the exact moment the patch's
own freshly decoded children were translated into the tree. A nested
child's local offset then subtracts out exactly the growth
accumulated since that moment (`pending_shift_before_this_call -
parent.children_base_shift`), recovering the offset that's valid in
the parent's frozen snapshot, still in O(1).

## Goals

- **G1.** Replace `splice_override`'s two `self.lines.splice`/
  `self.line_styles.splice` calls with an append to a new per-batch
  patch list (`App::pending_line_patches`); materialize
  `self.lines`/`self.line_styles` exactly once per outer
  `render_overrides` batch — in `finalize_override_batch`, before its
  existing `rebuild_visible_rows()` call (which reads
  `self.lines.len()`) — via a single O(N_final) linear merge pass
  instead of M separate O(N) `Vec::splice` calls.
- **G2.** No observable behavior change: for any given sequence of
  splices, the final `self.lines`/`self.line_styles`/`self.tree`/
  `line_to_node`/`footer_line_to_node`/`visible_rows` state is
  identical to what today's eager-splice code already produces — a
  "same result, less redundant work" characterization, matching spec
  0160 G4's framing.
- **G3.** A standalone (non-batch) `splice_override` call
  (`override_batch_depth == 0` — e.g. `override_select.rs`'s
  live-preview splice) sees no behavior or cost change: it pushes one
  patch and immediately materializes it via the same unified code
  path (`finalize_override_batch` already runs immediately for a
  standalone call), equivalent to today's single `Vec::splice`.
- **G4.** Any batch with many qualifying splices — `App::new`'s
  startup pass, the automatic `RootTypeResolved`-triggered render,
  interactive `Enter`-to-confirm, and activating a different override
  — drops the line-buffer materialization step specifically from
  O(M×N) to O(N) (informal target, not a hard SLA, matching spec 0160
  G5's framing: the remaining per-node decode/render cost, spec 0160
  N4 below, is untouched and still scales with the number of new
  nodes produced).

## Non-goals

- **N1.** Changing `self.tree`'s push mechanics or introducing a
  scratch/local-tree representation — confirmed in Background to be
  unnecessary: `self.tree.push` is already O(1) amortized and plays
  no part in this cost.
- **N2.** Making `render_overrides`/`splice_override` viewport-scoped
  or lazy. Same reasoning as spec 0160 N2: override/type resolution
  determines `self.lines`/`self.tree`'s actual structure, which
  folding, cursor navigation, search, and export all depend on
  globally.
- **N3.** Any change to *what* gets spliced or *when* (auto-expand
  seeding, `resettle_node`'s re-splice-trigger condition, the
  recursion's descent condition into children). Same scope boundary
  as spec 0160 N3.
- **N4.** The per-node decode/render cost itself
  (`decode_and_render_indexed`, `render_cache` lookups/misses) —
  unaffected by this spec, which targets only the bookkeeping cost of
  writing already-decoded content into `self.lines`/`self.line_styles`.
  This is the ≈8-9 µs/node cost noted in Background and remains after
  this spec lands.

## Specification

### `protolens/src/tui/mod.rs`

- `App` gains a new field, alongside `pending_shift`:
  ```rust
  /// Spec 0167 (N1 follow-up to spec 0160): line-buffer patches
  /// collected by `splice_override` calls during the currently-active
  /// `render_overrides` batch — see `override_apply::LinePatch`'s own
  /// doc comment, and `render_overrides_inner`'s `patch_scope`
  /// parameter, for why a patch can be nested inside another,
  /// not-yet-materialized one. Always empty outside of an active
  /// batch; drained and applied to `self.lines`/`self.line_styles` in
  /// one pass by `finalize_override_batch`.
  pending_line_patches: Vec<override_apply::LinePatch>,
  ```
  Initialized to `Vec::new()` in `App::new`, next to
  `pending_shift: 0`.

### `protolens/src/tui/override_apply.rs`

- New types, replacing the flat `(Range<usize>, Vec<String>,
  Vec<...>)` tuple originally proposed:
  ```rust
  /// Where a single `splice_override` call's freshly decoded content
  /// ultimately belongs once the active `render_overrides` batch
  /// finishes.
  pub(super) enum LinePatchTarget {
      /// A range in `self.lines`/`self.line_styles` as they stood
      /// before this batch began.
      Original(Range<usize>),
      /// `(parent_patch_index, local_range_within_that_patch's_own_lines)`.
      Nested(usize, Range<usize>),
  }

  /// One collected line-buffer patch. `global_start` is the patch's
  /// own start position in the batch's corrected coordinate frame.
  /// `children_base_shift` is `App::pending_shift`'s value right
  /// after this patch's own delta was folded in — both exist solely
  /// so a further-nested child patch can compute its own local
  /// offset in O(1) (see Background's "Patches nest" discussion).
  pub(super) struct LinePatch {
      pub(super) target: LinePatchTarget,
      pub(super) global_start: usize,
      pub(super) children_base_shift: isize,
      pub(super) lines: Vec<String>,
      pub(super) styles: Vec<Vec<(Range<usize>, SyntaxRole)>>,
  }
  ```
- `render_overrides_inner` gains a `patch_scope: Option<usize>`
  parameter, threaded top-down exactly like `inherited`/`path`: `None`
  at the outermost call (`render_overrides`'s own entry, `idx` is
  always pre-existing); at each node, `child_scope = spliced_patch.
  or(patch_scope)` (`spliced_patch` = the index of the patch just
  recorded for `idx` itself, if any) is passed down to every child —
  a spliced node's fresh children live inside its own new patch; an
  unspliced node's children remain wherever it itself was found.
- `resettle_node` gains the same `patch_scope` parameter, threads it
  straight through to `splice_override`, and returns `Option<usize>`
  (the freshly recorded patch's index, if `idx` was actually
  re-spliced) instead of `()`.
- `splice_override` gains the same `patch_scope: Option<usize>`
  parameter and returns `Result<usize, String>` (the patch's index)
  instead of `Result<(), String>`. In place of the two `Vec::splice`
  calls, it captures `pending_shift`'s value before its own delta is
  folded in, then builds a `LinePatchTarget` depending on `patch_scope`:
  ```rust
  let pending_shift_before = self.pending_shift;
  self.pending_shift += delta;
  // ...
  let global_start = old_span.text_range.start;
  let target_range = match patch_scope {
      None => {
          let original_start =
              (old_span.text_range.start as isize - pending_shift_before) as usize;
          let original_end =
              (old_span.text_range.end as isize - pending_shift_before) as usize;
          LinePatchTarget::Original(original_start..original_end)
      }
      Some(parent_idx) => {
          let parent = &self.pending_line_patches[parent_idx];
          let parent_start = parent.global_start as isize;
          let extra_growth = pending_shift_before - parent.children_base_shift;
          let local_start =
              (old_span.text_range.start as isize - parent_start - extra_growth) as usize;
          let local_end =
              (old_span.text_range.end as isize - parent_start - extra_growth) as usize;
          LinePatchTarget::Nested(parent_idx, local_start..local_end)
      }
  };
  let patch_idx = self.pending_line_patches.len();
  self.pending_line_patches.push(LinePatch {
      target: target_range,
      global_start,
      children_base_shift: self.pending_shift,
      lines: new_lines,
      styles: new_line_styles,
  });
  ```
  (returns `Ok(patch_idx)` at the end, instead of `Ok(())`). All
  standalone/live-preview and test call sites pass `None` for
  `patch_scope`.
- `finalize_override_batch`: calls a new `materialize_line_patches`
  first — before the existing `line_to_node`/`footer_line_to_node`
  rebuild and, critically, before `rebuild_visible_rows()` (reads
  `self.lines.len()`, which must already reflect the batch's final
  content).
- New method `materialize_line_patches`: groups patches by parent
  (`Original`-targeted ones are top-level; `Nested`-targeted ones are
  grouped by `parent_patch_index`), then, for each top-level patch,
  resolves it (and, recursively, all of its own nested descendants)
  via a new `resolve_line_patch` helper before splicing the resolved
  result into `self.lines`/`self.line_styles` at its `Original` range
  — a single O(document length) pass over the whole document, done
  exactly once.
- New method `resolve_line_patch(patches, children_of, idx)`:
  recursively flattens patch `idx`'s own content by splicing in each
  of its direct `Nested` children (each first resolved the same way)
  at that child's recorded local range — bounding each individual
  merge to the size of that patch's own content, not the whole
  document, so the nesting doesn't reintroduce O(M×N) behavior.

No change to `render_overrides_inner`'s existing `inherited`/
`child_owed` span-correction logic (spec 0160 G2) — it still keeps
every node's `text_range` tracking its true final document position,
needed by `finalize_override_batch`'s rebuild; `children_base_shift`
exists specifically to let `splice_override` undo that tracking's
side effect when it's not wanted (a nested patch's offset into its
parent's frozen snapshot).

## Test plan

- Existing `tui/tests/override_apply.rs` suite must pass unchanged —
  single-splice and multi-splice-batch behavior (G2/G3) is not
  expected to change observably.
- New regression test with a multi-splice batch (reusing/extending
  spec 0160's own multi-splice fixture, e.g. a message with several
  nested message-typed fields each needing their own splice on the
  initial pass) asserting final `self.lines`/`self.line_styles`
  content matches what today's per-call `Vec::splice` code already
  produces — the primary correctness guard for the
  `original_start`/`original_end` recovery math. Include at least one
  batch where an earlier patch's new content has a *different* line
  count than its old content (net growth), so a later patch in the
  same batch specifically exercises subtracting a nonzero
  `pending_shift_before`.
- A variant with a net-*shrinking* patch (new content shorter than
  old) followed by a later patch in the same batch — exercises
  negative `delta`/`pending_shift_before` arithmetic without
  underflow (mirrors spec 0160's own MessageSet negative-offset
  regression, `round_trip_extract_and_encode_preserves_message_set_
  group_framing` / `esc_closing_the_override_pane_restores_nested_
  message_set_auto_expansion`, which should also be re-run as-is
  here).
- **Nesting correctness** (found necessary only once implemented,
  not anticipated by the original flat-list design above): a node
  freshly created by a splice, within the same batch, itself needing
  its own re-splice — and specifically, at least two such nested
  siblings under the same not-yet-materialized parent, where the
  first sibling's own splice changes line count, so the second
  sibling's local-offset computation must correctly exclude that
  growth (the `children_base_shift` term). The existing
  `override_apply.rs`/`override_select.rs` suites already exercise
  this shape incidentally (deeply nested auto-expand scenarios, e.g.
  `message_set_group_items_auto_expand_through_render_overrides`) and
  serve as regression coverage; the bug this guards against was only
  actually caught by manual testing against a large real-world
  descriptor set (`/tmp/pdb.desc`), not by any committed fixture —
  small synthetic fixtures didn't happen to produce nested siblings
  with differing growth within the same parent.
- A standalone (non-batch) `splice_override` call regression test
  (G3) — e.g. `override_select.rs`'s live-preview path — confirming
  identical `self.lines`/`self.line_styles` content to today's,
  single-patch materialization.
- Manual/perf validation (not a regression test, depends on an
  external large fixture not committed to the repo): confirm
  `protolens /tmp/pdb.desc`-scale interactive `Enter`-to-confirm and
  startup `RootTypeResolved` timings (see Background's measured
  baselines: ≈5.1 s / ≈8.5 s / ≈10 s) drop measurably, though full
  elimination isn't expected — the remaining per-node decode/render
  cost (N4) is untouched by this spec.
