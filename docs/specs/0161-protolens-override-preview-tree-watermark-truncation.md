<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0161 — protolens: bound live-preview tree/heat-state growth via a preview watermark

Status: superseded (implemented 2026-07-24, superseded 2026-07-31).
        Both premises below are gone: spec 0185 made the live preview a
        non-mutating overlay, so it no longer splices anything, and
        spec 0216 made the arena immutable, so a splice appends no
        nodes at all. `preview_tree_watermark` and the fix-ups it drove
        are deleted; the identifier survives only in a comment at
        `tui/mod.rs`. Nothing here describes current behavior.
Implemented in: 2026-07-24
App: protolens

## Background

The override pane's live-preview mechanism (`preview_override_highlight`,
`override_select.rs` ~773) re-renders the main pane every time the
highlighted candidate changes — on every `Down`/`Up`/`j`/`k`, mouse
hover, and search-jump (`jump_to_override_match`) while the pane is
open — by calling `self.splice_override(idx, tentative)` directly,
standalone (`override_batch_depth == 0`), and then deliberately
resetting `self.tree[idx].rendered_as = None` afterward so the preview
is never recorded as durable. By construction, a preview's spliced-in
content becomes disposable garbage the instant a new preview supersedes
it: nothing else in `App`'s state is meant to keep referencing it.

`splice_override` (`override_apply.rs`), however, never removes
anything from `self.tree: Vec<TreeNode>`. Every call — preview or real
— decodes a fresh local subtree and appends it to `self.tree`'s tail
(`let base = self.tree.len();` at `override_apply.rs:1180`, followed by
a `self.tree.push(TreeNode { .. })` loop), then rewires `idx`'s own
`first_child`/`last_child`/`span` to point at the new content. The
*previous* content — the old descendants being replaced, and the local
root's own pushed-but-unused copy at `new_self_idx` — becomes
unreachable via live traversal pointers but stays physically resident
in the `Vec` forever, per the code's own doc comment:

```rust
// The pushed copy of the local root (at `new_self_idx`) is left
// orphaned, never referenced again — its span/children are copied
// into the live `idx` entry instead, same "abandon in place"
// pattern already used for old descendants.
```
(`override_apply.rs:1223-1226`)

`self.heat_states: Vec<heat_cue::HeatState>` is kept parallel to
`self.tree` via:

```rust
self.heat_states
    .resize(self.tree.len(), heat_cue::HeatState::default());
```
(`override_apply.rs:1219-1220`)

called on *every* `splice_override`, including every preview. Because
`self.tree.len()` is the cumulative total of every node ever pushed
(dead or alive), not just the current splice's local subtree, this
resize's cost — and the append itself — scales with how much preview
garbage has already piled up in the session, not with the size of the
candidate currently being previewed.

### Confirmed by direct instrumentation

Using an interactive-path-faithful profiling harness
(`protolens/src/tui/tests/profiling.rs`, throwaway/`#[ignore]`d) against
a large real-world descriptor set (`/tmp/db3.desc`, decoding to 635,052
tree nodes):

- Launching `protolens --descriptor-set /tmp/db3.desc /tmp/db3.desc`
  lands the cursor, by `App::new`'s own default, on a field whose raw
  payload spans nearly the entire ~1.1MB document (index 149358).
  Pressing `t` there opens the override pane on that field.
- One of the resolved candidates' schemas mis-parses that payload —
  reinterpreting valid bytes under a structurally mismatched message
  type causes the recursive-descent decoder to spuriously parse
  garbage as deeply nested submessages, producing 1,083,626 spans from
  a *single* preview splice — more spans than the entire correctly
  decoded 635,052-node original document. (This blowup in the size of
  any *one* splice is a separate, orthogonal defect; see Non-goals.)
- Successive `Down` presses through the candidate list each preview a
  different candidate, so each one appends its own multi-hundred-
  thousand-node subtree without ever reclaiming the previous preview's.
  Measured per-keystroke cost escalates across the loop — roughly
  2.7s for the first `Down`, rising past 16s a few keystrokes later —
  consistent with `self.heat_states.resize` (and the `Vec::push` loop
  itself) scaling with the ever-growing cumulative `self.tree.len()`
  rather than any single candidate's own size.

This spec addresses the *accumulation across repeated previews*
specifically — the fact that cost keeps climbing the longer a user
lingers on the override pane moving the highlight around — independent
of how large any individual candidate's own parse happens to be.

## Goals

- **G1**: Bound `self.tree`/`self.heat_states` growth during a single
  override-pane session's live preview so that at most one outstanding
  preview's worth of appended nodes exists at a time, no matter how
  many times the user moves the highlight or how long the pane stays
  open.
- **G2**: Preserve the existing index-stability invariant — `idx` (the
  node under override) always keeps its fixed, pre-existing, low tree
  index across every preview; nothing in this fix may relocate or
  remap it.
- **G3**: Scope the fix strictly to the disposable live-preview call
  path (`preview_override_highlight` and its callers —
  `jump_to_override_match`, arrow/`j`/`k` navigation, mouse hover,
  `open_override_from_manage`'s initial preview). Real/confirmed
  override commits (`close_override`'s `render_overrides` finalization,
  batch operations from spec 0160) must keep their current full
  accumulation semantics unchanged — their appended content is not
  disposable and must never be truncated away.
- **G4**: No user-visible behavior change: identical rendered output,
  identical override outcomes, identical candidate-list/undo/redo
  semantics. This is purely an internal memory/performance fix.
- **G5**: Defensively guard against any other `App`-level index-keyed
  state that could end up referencing a truncated index range —
  specifically `self.folded: HashSet<usize>`, since a user could in
  principle fold a node that is part of a previewed subtree before the
  preview changes again — so that truncation can never leave a stale
  or dangling index reference behind.

## Non-goals

- **N1**: General reclamation/compaction of `self.tree` for *confirmed*
  overrides accumulated over a long session (many distinct nodes each
  overridden, or a batch operation touching many nodes). Deferred to
  spec 0162.
- **N2**: Fixing the separate mis-parse "blowup" defect, where a single
  splice under a structurally mismatched candidate type can produce a
  pathologically large subtree (observed: >1,000,000 spans from one
  splice). That is a render-size/parse-validation problem, independent
  of this spec's accumulation-across-previews problem, and is tracked
  as separate follow-up work.
- **N3**: Changing `render_cache` growth/eviction behavior — that cache
  is keyed by span/type content, not by tree index, and is unaffected
  by this fix.

## Specification

### protolens/src/tui/mod.rs

Add a new `App` field, initialized to `None`, tracking the tree length
just before the *first* preview splice of the current override-pane
session:

```rust
/// `self.tree.len()` captured just before the first live-preview
/// splice of the current override-pane session (spec 0161). `None`
/// when no preview has been spliced yet this session. Every
/// subsequent preview truncates `self.tree`/`self.heat_states` back
/// to this watermark before splicing its own candidate, so at most
/// one outstanding preview's worth of disposable nodes exists at a
/// time. Reset to `None` in `close_override`.
preview_tree_watermark: Option<usize>,
```

Initialized alongside the other override-pane fields in `App::new`:

```rust
preview_tree_watermark: None,
```

### protolens/src/tui/override_select.rs

`preview_override_highlight` truncates any previous preview's garbage
before splicing the newly highlighted candidate, and (re-)captures the
watermark on the first preview of a session:

```rust
pub(super) fn preview_override_highlight(&mut self) {
    let Some(idx) = self.override_target else {
        return;
    };
    match self.preview_tree_watermark {
        Some(watermark) => {
            self.tree.truncate(watermark);
            self.heat_states.truncate(watermark);
            self.folded.retain(|&i| i < watermark);
            // The truncation above just invalidated whatever `idx`'s
            // children/doc-chain pointer previously pointed at (the prior
            // preview's now-discarded subtree). `splice_override` below
            // unconditionally overwrites all three fields on success, but
            // if it returns `Err` before reaching that point (every error
            // path precedes any tree mutation), these must not keep
            // dangling, out-of-bounds indices around. Null them
            // defensively so a failed splice leaves `idx` merely
            // childless (harmless — the override pane, not the main
            // pane, has focus here) rather than referencing truncated
            // memory.
            self.tree[idx].first_child = None;
            self.tree[idx].last_child = None;
            self.tree[idx].doc_next = None;
        }
        None => self.preview_tree_watermark = Some(self.tree.len()),
    }
    let tentative = self
        .override_candidates
        .get(self.override_highlight)
        .map(|(fqdn, _)| fqdn.clone());
    match self.splice_override(idx, tentative) {
        Ok(()) => self.tree[idx].rendered_as = None,
        Err(e) => self.message = format!("cannot preview override: {e}"),
    }
}
```

`close_override` resets the watermark to `None` so the next
override-pane session starts fresh (its own first preview captures a
new watermark rather than inheriting this session's):

```rust
pub(super) fn close_override(&mut self) {
    if let Some(idx) = self.override_target {
        self.render_overrides(idx);
    }
    self.preview_tree_watermark = None;
    // ... existing body unchanged ...
```

No other call site needs changes: every path that opens the override
pane (`toggle_override`, `open_override_from_manage`) already calls
`preview_override_highlight` (directly or via `open_override_on_*`) as
its first action, so the `None` branch above always runs first and
establishes the watermark before any truncation is attempted.

## Test plan

- New unit test in `tests/override_select.rs`: open the override pane
  on a node with several candidates, preview candidate A (record
  `self.tree.len()`), preview candidate B, then preview candidate C —
  assert `self.tree.len()` after C is bounded by (watermark + C's own
  local size), not by the sum of A's + B's + C's sizes — i.e. it does
  not grow monotonically across previews.
- New unit test: preview many candidates in a loop (e.g. cycling
  through all available candidates twice) and assert `self.tree.len()`
  stays within a small constant bound across the whole loop, rather
  than climbing with each iteration.
- Regression test: after several previews, confirm the override with
  `Enter` and verify the final rendered content matches confirming
  directly (no interaction between truncation and the real commit
  path, which goes through `render_overrides`/`finalize_override_batch`
  unaffected by this change).
- Regression test: `Esc` after several previews still correctly
  reverts to the pre-preview content (unaffected — revert already goes
  through `render_overrides`, not through anything watermark-related).
- Regression test covering G5: fold a node, preview a candidate whose
  subtree includes that folded node, preview a different candidate
  (triggering truncation), and confirm no panic and no stale entry
  survives in `self.folded` referencing a truncated index.
- Manual/perf validation (external fixture, not part of the automated
  suite): re-run `tests/profiling.rs`'s `Down`-press loop against
  `/tmp/db3.desc` and confirm the escalating 2.7s → 16s per-keystroke
  cost becomes flat/bounded across repeated presses.
