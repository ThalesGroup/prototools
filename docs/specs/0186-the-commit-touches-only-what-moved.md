<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0186 — a commit touches only what moved

Status: draft
App: protolens
Refs: docs/protolens/rendering-flaws.md (P3),
      docs/protolens/rendering-scaling-roadmap.md (S2, S3),
      docs/specs/0160-protolens-render-overrides-batch-scaling.md,
      docs/specs/0163-protolens-render-budget.md (N3),
      docs/specs/0167-protolens-nested-patch-scope.md,
      docs/specs/0183-prune-the-override-walk.md,
      docs/specs/0185-the-preview-is-an-overlay.md (N2)

## Background

Committing an override costs **10.6 s** on a 1.1 MB fixture (622,922
nodes / 193,072 lines) and 8.2 s for the second commit. That is flaw
[P3](../protolens/rendering-flaws.md).

Two specs have already taken bites out of it, and neither touched the
part that dominates:

- **Spec 0183** pruned `render_overrides`'s walk to the subtrees that can
  actually change, so the *walk to find the splices* is no longer
  O(document).
- **Spec 0185** made the live preview an overlay, so the passes below are
  no longer paid per keystroke. Its Non-goal N2 says so explicitly:

  > **N2** — P3's four O(document) passes themselves. They stay, and stay
  > wrong, for the committed path; this spec removes the preview as one of
  > their callers. Fixing them is a separate spec.

This is that spec. What remains is `finalize_override_batch`
(`override_apply.rs:1561-1605`), which runs **once per batch** — so once
per commit — and does four passes over the whole document to apply a
change that is, by construction, confined to one contiguous line range
and everything after it.

### The four passes, as they stand

Numbered as in 0185's background, against current line numbers.

**Pass 1 — `materialize_line_patches` (`:1565`) deep-clones the
document.** The merge builds fresh `new_lines`/`new_line_styles` with
`extend_from_slice` (`:1673-1681`). On a `Vec<String>` that is a *clone
per element*: one `malloc` plus one `memcpy` per line, 193,072 times, to
apply a patch that replaces a handful of them. `Vec<LineStyles>` is
worse — each element is itself a `Vec`.

**Pass 2 — the `doc_next` shift walk (`:1577-1581`).** Walks every live
node strictly after `idx`'s subtree, adding `delta` to its `text_range`.
This one is **inherent** to storing absolute line numbers on every node,
and this spec does not attempt it (N1).

**Pass 3 — the line-map rebuild (`:1589-1602`).** `line_to_node` and
`footer_line_to_node` are `clear()`ed and refilled by walking the entire
`doc_next` chain from `first_node`. Spec 0163 N3 already flagged this as
deferred work. Every entry before the patch point is reinserted with the
value it already had.

**Pass 4 — `rebuild_visible_rows` (`:1604`, `navigation.rs:60-77`).**
Allocates `vec![false; total]`, marks folded ranges, and filters
`0..total` into a fresh `visible_rows`. 193 kB of allocation and a
193 k-iteration filter, to change visibility at or after one line.

### Why "only what moved" is the right frame

A splice replaces the lines in one range and shifts every line index
after it by a constant. Therefore:

- Lines **before** the earliest patch keep their content *and* their
  index. Nothing about them changes, in any of the four structures.
- Nodes **before** the batch origin `idx` in document order keep their
  `text_range` — **except** `idx`'s ancestors, whose `text_range.end`
  grows (`:1582-1587`). An ancestor's *footer* line moves; its *header*
  line does not.

That exception is the whole subtlety of this spec, and it is why passes 3
and 4 cannot simply be keyed off "the batch origin's subtree".

## Goals

- **G1**: A commit's cost is proportional to *the size of the replaced
  content plus the number of nodes and lines after it*, not to the size
  of the document. Passes 1, 3 and 4 stop being O(document).
- **G2**: Pass 1 performs **no per-line heap allocation** and no
  `String`/`Vec` clone. Every surviving line is *moved*, exactly once.
- **G3**: **The incremental result is bit-identical to the full
  rebuild.** This is the acceptance criterion, asserted directly rather
  than inferred: for every fixture and every splice, running the
  incremental path and then a from-scratch rebuild must produce equal
  `line_to_node`, `footer_line_to_node` and `visible_rows`.
- **G4**: Measured, on the same fixture and with the same harness that
  produced P3's table. A commit that is merely *believed* faster does not
  close P3.

## Non-goals

- **N1 — pass 2, the `doc_next` shift walk.** Every node stores absolute
  line numbers, so a splice genuinely must touch every following node.
  Removing that walk means storing offsets relative to the parent, which
  is roadmap item S8/S12's territory (`TreeNode` shrinking and lazy
  text) and a far larger change. It stays O(nodes after the splice),
  which is already within G1's bound.
- **N2 — arena reclamation.** Two commits leave ~2.1 M orphaned nodes
  resident. That is [spec 0162](0162-protolens-tree-node-reclamation.md),
  a goals-only draft, deferred by worklist decision D-c. It is a
  *memory* problem; this spec is a *time* problem. They share the
  observation that a commit's blast radius is smaller than the document,
  but not an implementation.
- **N3 — making the maps self-healing.** Today they are rebuilt from
  scratch, so a bug elsewhere heals on the next batch. After this spec a
  missed insert persists. That is a real loss and it is accepted
  deliberately; G3's equivalence assertion is the mitigation, and S2
  named this trade explicitly.
- **N4 — the fold path.** `rebuild_visible_rows` is also called by fold
  toggles, which would benefit from the same incrementalization. This
  spec gives the incremental entry point a `from` parameter and lets the
  fold path keep passing `0`; wiring the fold toggle's own
  `text_range.start` through is a follow-up, not because it is hard but
  because it changes a different caller's behavior and wants its own
  test.

## Specification

### S1. `materialize_line_patches` moves instead of cloning

Replace the `extend_from_slice` merge with a consuming one. `old_lines`
is already `std::mem::take`n, so it is owned:

```rust
let mut old_lines = old_lines.into_iter();
let mut old_styles = old_line_styles.into_iter();
let mut cursor = 0usize;
for idx in top_level {
    let range = /* as today */;
    assert!(range.start >= cursor, /* ... as today ... */);
    // Everything between the previous patch and this one: moved, not
    // cloned.
    new_lines.extend(old_lines.by_ref().take(range.start - cursor));
    new_line_styles.extend(old_styles.by_ref().take(range.start - cursor));
    let (lines, styles) = Self::resolve_line_patch(&mut patches, &children_of, idx);
    new_lines.extend(lines);
    new_line_styles.extend(styles);
    // Drop the replaced lines.
    for _ in 0..(range.end - range.start) {
        old_lines.next();
        old_styles.next();
    }
    cursor = range.end;
}
new_lines.extend(old_lines);
new_line_styles.extend(old_styles);
```

This is still one pass over the document, but the per-line work drops
from *allocate + copy the bytes* to *move 24 bytes of `String` header*.
That is G2, and it is the single largest item in this spec: it is the
only one of the four passes that touches the heap per line.

`resolve_line_patch`'s inner merge (`:1725-1738`) has the same shape
against a patch's own `lines`, which is bounded by the patch, not by the
document. Convert it too, for consistency and because it costs nothing —
but do not claim it as a win.

**Note on ordering.** This relies on the patches being sorted, which
`materialize_line_patches` now guarantees itself (flaw C2 / worklist W2,
fixed 2026-07-26). Before that fix a consuming iterator would have been
unsound in a way the cloning version was not: `by_ref().take(n)` with a
`range.start` *behind* `cursor` would underflow rather than panic on a
reversed slice. The sort is a precondition of this section.

### S2. The batch records where it starts

`finalize_override_batch` needs one number the batch does not currently
carry: the earliest line index any patch in the batch touches.

Add `App::pending_patch_min_line: Option<usize>`, set in
`splice_override` wherever a `LinePatch` is pushed, as
`min(existing, global_start)`. `global_start` is already computed there
(`:1887`) in the batch-corrected frame, which is the frame
`materialize_line_patches` resolves against — so it is the right
coordinate with no conversion.

Reset it to `None` alongside `pending_shift = 0` at `:1603`.

`None` means "no patches this batch", in which case
`materialize_line_patches` already returns early and passes 3 and 4 have
nothing to do beyond what pass 2's shift may have moved. Note that
`pending_shift` can be non-zero with no patches — a batch may shift
without replacing — so passes 3 and 4 must key off the *shift*, not off
the patch count, when deciding whether to run at all. **The safe rule:
if either `pending_shift != 0` or `pending_patch_min_line.is_some()`,
run; and use `pending_patch_min_line.unwrap_or(0)` as the boundary.**

### S3. The line maps are repaired, not rebuilt

Replace `:1589-1602` with three steps. Let `from =
pending_patch_min_line.unwrap_or(0)`.

1. **Drop what may have moved.**
   `self.line_to_node.retain(|&line, _| line < from);` and the same for
   `footer_line_to_node`. Filtering by *line* index is what makes this
   correct in the presence of the ancestor exception: an ancestor's
   header line is `< from` and stays; its footer line is `>= from` and
   goes.
2. **Reinsert from the walk that pass 2 already performs.** Pass 2 walks
   `doc_next` from the seam after `idx`'s subtree. Extend it to start at
   `idx` itself and to insert map entries as it goes, so the traversal is
   shared rather than repeated. Entries for `idx`'s own freshly spliced
   descendants come from the same walk — they are in the chain.
3. **Reinsert the ancestors' footers.** The `parent` walk at
   `:1582-1587` already visits exactly the nodes whose
   `text_range.end` changed. Insert each one's footer entry there, under
   the same `has_children`-equivalent guard the full rebuild uses
   (`node.span.text_range.end - 1 > node.span.text_range.start`).

Steps 2 and 3 must run **after** all `text_range` mutation for the
batch, since they key on the final line numbers. Today's code shifts
first and rebuilds second; keep that order and simply fold the inserts
into the two existing walks.

**One asymmetry to get right.** Pass 2's shift walk is guarded by `if
delta != 0` (`:1567`). The map repair is *not* conditional on `delta`: a
splice that replaces N lines with exactly N lines shifts nothing but can
still change which node owns a line. Restructure so the walk runs when
the map needs it, with the `shift_span` call itself staying under the
`delta != 0` guard.

### S4. `visible_rows` is extended, not rebuilt

Split `rebuild_visible_rows` into a `from`-parameterized core:

```rust
pub(super) fn rebuild_visible_rows(&mut self) {
    self.rebuild_visible_rows_from(0)
}
pub(super) fn rebuild_visible_rows_from(&mut self, from: usize) { ... }
```

The core:

1. `visible_rows` is sorted ascending by construction, so the surviving
   prefix is `partition_point(|&l| l < from)` followed by `truncate`. No
   allocation, no move.
2. Recompute the hidden mask **only over `from..total`**. Iterating
   `self.folded` is O(folded), which is user-bounded and small; each
   folded range is clamped to `max(r.start + 1, from)..min(r.end, total)`
   before marking, so the marking cost is bounded by the tail's length,
   not by the ranges' full extents.
3. Filter `from..total` and `extend` onto the retained prefix.
4. `clamp_pan_offset()` and `structural_version += 1` run unchanged, on
   every call. `structural_version` is the heat prefetch walk's
   invalidation signal (`mod.rs`) and a partial rebuild is still a
   structural change — do not make the bump conditional.

**Reuse the mask buffer.** Keep it as an `App` field and `resize` it
rather than `vec![]`-ing per call. The buffer must be cleared over
`from..total` before marking, or a stale `true` from a previous call
hides a line that is now visible. This is the one place where reusing
the buffer is easy to get subtly wrong; clear the range you are about to
mark, not the whole buffer, and say so in a comment.

`finalize_override_batch` calls `rebuild_visible_rows_from(from)`. Every
other caller keeps calling `rebuild_visible_rows()` (N4).

## Test plan

- **G3 is the acceptance criterion, and it is asserted directly.** A
  helper that, given an `App`, clones the three structures, calls a
  from-scratch rebuild, and compares. Invoke it after every splice in
  the existing `override_apply` and `override_select` test suites — not
  in one new test, but as a post-condition of the ones already there,
  since the interesting cases (nested patches, packed runs, repeated
  overrides of the same node, folded targets, auto-expanded
  `Any`/`MessageSet` descendants) are already fixtured. This is what
  turns N3's accepted loss of self-healing into a bounded risk.
- **The ancestor-footer case gets its own test**, because it is the one
  the obvious implementation gets wrong: splice a *deeply nested* node
  whose growth moves several ancestors' footer lines, then assert
  `footer_line_to_node` maps each ancestor's new footer line to that
  ancestor — and that no entry survives at any of their *old* footer
  lines. A test that only checks "the new line resolves" passes with a
  stale duplicate still in the map.
- **The zero-delta case:** a splice replacing N lines with exactly N
  lines, where `pending_shift == 0` but ownership of lines inside the
  range changed. Assert the maps are still correct. This is what catches
  hanging the repair off the `delta != 0` guard.
- **The no-patch case:** a batch that shifts without patching (if
  reachable) or one with no patches at all — `pending_patch_min_line`
  is `None` and nothing must be corrupted.
- **`visible_rows` stays sorted ascending**, asserted, since S4's
  prefix-retain depends on it and 0185's overlay anchor
  (`partition_point` on the same vector) does too.
- **The fold path is unchanged**, pinned by the existing fold tests
  continuing to pass with `from = 0` (N4).
- **G4, measured** with `profile_override_pane_enter_on_pdb` in
  `tui/tests/profiling.rs` on `/tmp/pdb.desc`, before and after, with
  `tree.len()`/`lines.len()` reported so a "speed-up" that silently
  dropped content is visible. Record the numbers in a `Measured outcome`
  section of this spec, as 0185 did. `flock -n -x
  /tmp/prototools-bench.lock taskset -c 4 ...`.
- `cargo clippy --release --no-default-features --workspace -- -D
  warnings`, `reuse lint`, `nix-build -A ci`.

## Resolved questions

- ~~**Q1 — is pass 1 worth keeping as one pass at all?**~~ **Resolved
  2026-07-26: keep the one-pass merge; do not go back to `Vec::splice`.**
  The question assumed "a single patch, which is the commit case". That
  assumption is wrong, and the code says so at the point where the patch
  is queued (`override_apply.rs:1896-1901`): the patch mechanism exists
  because an eager per-splice `Vec::splice` is an "O(document length)
  memmove *per splice*, dominating a batch with **many qualifying
  splices**" (spec 0160 N1). A commit is not one splice — one override
  entry can match many nodes, and spec 0183's auto-expand tiers add a
  splice per qualifying node on top. So `top_level.len()` scales with the
  document, not with the number of user gestures, and back-to-front
  `splice` would reintroduce exactly the cost specs 0160 and 0167 were
  written to remove. No instrumentation run was needed to settle this;
  the premise was refutable from the existing invariant. S1 stands as
  written, and its gain over today is the allocation, not the memmove:
  moving 24-byte `String` headers instead of `malloc` + `memcpy` per
  line.
- ~~**Q2 — should the map repair be verified in production?**~~
  **Resolved 2026-07-26: no. Keep the check in the tests only (G3).**
  The proposed invariant — `line_to_node.len()` against a tracked live
  node count — is weak exactly where this spec's risk actually is. The
  failure mode the ancestor-footer exception invites (S3) is not a
  *missing* entry but a *stale* one: an entry surviving at an ancestor's
  old footer line while the correct entry is inserted at the new one.
  That leaves `len()` either unchanged or wrong by an amount that a
  second stale entry elsewhere silently compensates for, so the cheap
  check passes on the bug it was added to catch. Making it strong enough
  to be worth its keep means comparing the whole map against a rebuild,
  which is the full O(document) pass this spec exists to delete. Tracking
  a live node count also introduces a second source of truth for
  something `tree` already knows, and protolens has no existing
  verification-mode facility to hang it off (no `PROTOLENS_*` debug flag
  exists for this). G3's post-condition helper — assert the incremental
  result is bit-identical to a full rebuild, after *every* splice in the
  existing suites — is the stronger check and costs production nothing.
