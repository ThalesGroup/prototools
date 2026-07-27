<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0186 — a commit touches only what moved

Status: draft — implemented, then measured; see `Measured outcome`.
        The performance claims in G1 and G4 are **withdrawn**: the four
        passes this spec removes are 1.4% of a commit, and 85% is one
        whole-document tree-sitter parse in `colorize::colorize` that no
        part of this spec touches. Read `Measured outcome` before the
        Goals below, which are left as originally written so the record
        of what was believed, and on what evidence it was overturned,
        stays legible.
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

## Measured outcome

**Summary: this spec's premise is wrong, and the measurement says so.**
The four passes it set out to remove are together **1.4%** of a commit.
**85% is one whole-document tree-sitter parse** in `colorize::colorize`,
which no part of this spec touches.

Measured 2026-07-27 on `/tmp/pdb.desc` (146,511 nodes / 193,072 lines at
load), `cargo test --release --bin protolens`, under `flock -n -x
/tmp/prototools-bench.lock taskset -c 4`. Per-phase wall-clock
accumulators were added temporarily to `override_apply.rs`; logs in
`/tmp/0186-phases2.log`, `/tmp/0186-nested3.log`, `/tmp/0186-noop.log`.

### Regime 1 — the document-wide commit

`profile_override_pane_enter_on_pdb`. One splice, whose replaced content
is essentially the whole document.

| phase | 1st Enter | 2nd Enter |
|---|---|---|
| `compute_descend_marks` (spec 0183) | 0.016 s (0.1%) | 0.042 s (0.3%) |
| `splice_override` **total** | 14.558 s (97.7%) | 13.020 s (96.4%) |
| ↳ `render_node_as` | 13.969 s (93.7%) | 12.130 s (89.8%) |
| ↳↳ `decode_and_render_indexed` | 0.464 s (3.1%) | 0.340 s (2.5%) |
| ↳↳ **`colorize::colorize`** | **12.976 s (87.1%)** | **11.325 s (83.8%)** |
| ↳↳ `render_cache` insert (deep clone) | 0.177 s (1.2%) | 0.160 s (1.2%) |
| ↳↳ `hints_by_line` | 0.250 s (1.7%) | 0.231 s (1.7%) |
| **S1** `materialize_line_patches` | 0.036 s (0.2%) | 0.135 s (1.0%) |
| **S3** line-map repair | 0.166 s (1.1%) | 0.196 s (1.4%) |
| **S4** `rebuild_visible_rows_from` | 0.005 s (0.03%) | 0.002 s (0.02%) |
| walk overhead | 0.084 s (0.6%) | 0.081 s (0.6%) |
| **total** | **14.904 s** | **13.510 s** |

`from` is **0** in both. The batch's earliest patch is the document's
first line, so S3's retained prefix is empty and S4's is empty: the
incremental path degenerates to the full rebuild it replaced. This is not
a harness artifact — see regime 3.

End-to-end before/after (baseline worktree at `02b61a8`,
`/tmp/0186-before.log` vs `/tmp/0186-after.log`): 14.75 s / 13.67 s
baseline against 14.89 s / 13.67 s with S1–S4. **Parity.** G4 is not
satisfied and cannot be by this spec.

### Regime 2 — the nested commit

`profile_nested_commit_on_pdb` (added by this spec). Root type resolved
first (382,032 nodes / 276,515 lines), then an override confirmed on
`google.protobuf.SourceCodeInfo` at lines 276,247..276,513 — 2 splices,
757 lines re-rendered, `from = 276,023 of 276,515`.

| phase | | |
|---|---|---|
| **S3** line-map repair | 0.069 s | **55.1%** |
| `render_cache` insert (deep clone) | 0.026 s | 20.8% |
| `compute_descend_marks` (spec 0183) | 0.017 s | 13.5% |
| `mark_fresh_subtree` | 0.003 s | 2.4% |
| **S1** `materialize_line_patches` | 0.003 s | 2.2% |
| `colorize::colorize` | 0.004 s | 3.3% |
| `decode_and_render_indexed` | 0.000 s | 0.2% |
| **S4** `rebuild_visible_rows_from` | 0.000 s | 0.01% |
| **total** | **0.125 s** | |

Here the boundary is as favorable as it can get — 99.8% of the document
is below it — and **S3 is still the single largest item**. The reason is
structural and fatal to the approach:

> `line_to_node.retain(|&line, _| line < from)` is **O(map size)
> regardless of `from`**. `HashMap::retain` visits every occupied bucket
> to decide what to drop. S3 replaced an O(document) clear-and-refill
> with an O(document) scan. The re-insertion was removed; the traversal
> was not. No choice of boundary makes it sublinear in a `HashMap`.

`compute_descend_marks` (spec 0183, not this spec) has the same shape —
`vec![false; tree.len()]` plus a scan of every node, per batch — and is
13.5% here for the same reason.

### Regime 3 — why `from` is 0 in regime 1

A confirmed override becomes an `OverrideOrigin::PathField { path:
positional_path(parent), field }` — "field N of *that* parent", which
matches **every** child of that parent bearing that field number. In
regime 1 the target is a `file` entry whose parent is the document root,
so the rule matches all 465 `FileDescriptorProto`s and the commit is
document-wide by construction, not by accident. An intermediate run that
picked the same shape confirmed it directly: **465 splices, all of
pre-existing nodes, all with an override entry, 276,513 lines
re-rendered** — the whole document — in 1.97 s, of which `colorize` was
1.50 s (76%).

This also disposes of a hypothesis raised while diagnosing: that a commit
re-splices every previously-overridden node. It does not.
`resettle_node` (`override_apply.rs:1141`) splices only on `(target,
field_name) != rendered_as`, and two consecutive no-op batches on a
settled 382,032-node document splice **zero** nodes each, in 69 ms and
72 ms. The batch machinery is idempotent.

### What the numbers imply

- **S1 stands on its own.** It removes a `malloc` + `memcpy` per line
  from a pass that still runs unconditionally. Small, local, no
  invariant surrendered. Nothing here argues against it.
- **S2/S3/S4 do not deliver G1 and cannot.** In regime 1 the boundary is
  0; in regime 2 the boundary is irrelevant because the drop step is
  linear anyway. They also cost N3 — the maps stopped being
  self-healing — for no measured return.
- **G3 earned its keep regardless.** Asserting bit-identity against a
  full rebuild after *every* splice in the suite found a real
  pre-existing bug on its first run: `App::new` called `render_overrides`
  before the first `rebuild_visible_rows`, so the first batch of every
  session ran against a `visible_rows` that had never been built. The
  full rebuild masked it; that is exactly the class of latent
  inconsistency a self-healing structure hides.
- **P3 is misdiagnosed and stays open.** Its four O(document) passes are
  1.4% of the commit. The real item is
  [roadmap S6](../protolens/rendering-scaling-roadmap.md) — highlight
  lazily, per viewport — which the roadmap currently files as a
  *startup and memory* item marked "gate it on measurement, it may not
  pay off". The measurement is now in: it is also the commit path's
  bottleneck, at 84–87%.
- **A note for S6's own design.** Coloring in one large chunk was not
  cheaper per line than coloring in many small ones: 12.2 µs/line for a
  single 1,067,034-line parse, versus 5.4 µs/line across 465 parses
  totalling 276,513 lines. Batching the viewport into one joined parse is
  therefore a convenience, not a performance argument.

**Decision pending.** Whether S2/S3/S4 are reverted, or kept as
correct-but-inert code, is not settled by this section and is deliberately
left open here rather than assumed. What *is* settled: the performance
claim in G1/G4 is withdrawn.

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
