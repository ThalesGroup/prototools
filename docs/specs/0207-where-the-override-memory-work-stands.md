<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0207 — where the override memory work stands

Status: informational — rewritten 2026-07-31 against the code, after
        spec 0216 dissolved most of what the first version described
App: protolens
Refs: docs/specs/0116-tree-sitter-textproto-highlight-captures.md §8
        (the render cache this document's one surviving item is about —
        the spec's title names only its §7/§9 half),
      docs/specs/0174-preview-interior-truncation-and-node-budget-removal.md
        (the preview byte budget, which is why preview entries are
        small),
      docs/specs/0185-the-preview-is-an-overlay.md (the preview stopped
        splicing speculatively — it is now a read-only overlay),
      docs/specs/0188-the-batch-updates-what-changed-not-what-exists.md
        (N4, the `render_cache` deep clone as a backlog item),
      docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (what
        actually closed the defect, and what superseded 0202/0203/0206),
      docs/specs/0204-a-long-batch-says-so-before-it-blocks.md (draft),
      docs/specs/0205-the-batch-runs-off-the-input-thread.md (draft)

## What this document is

A wrap-up, not a specification. Nothing here is to be implemented
directly; every actionable item points at a spec.

**The first version of this document is gone.** It was written while the
override-memory defect was still open and it recorded a three-part plan
— slot reuse, in-place subtree construction, and an unowned render — of
which the first two were overtaken and are now wrong. Keeping them would
have meant a future reader planning against a tree that no longer
exists. What follows is what remains true.

## Where things stand

**The defect is closed.** Protolens no longer runs out of memory on a
large document, and not because a guard refuses the work: spec 0216 made
the arena a function of the blob's bytes, so a splice re-labels slots
instead of allocating them. Measured on `googleapis.desc`, the root
retype that used to OOM now peaks at 1.66 GiB and holds flat to 0.2 MiB
across repeated `t`/`Enter`/`o`/`d` cycles.

| | before the narrowing specs | today |
|---|---|---|
| peak, root retype | 4.18 GiB | **1.66 GiB** |
| at rest | 1.87 GiB | **0.94 GiB** |

Three specs got there: 0211-0213 narrowed the node slot 272 → 76 B, and
0216 replaced the dynamic arena outright. Specs 0202 (refuse rather than
crash), 0203 (compact the arena) and 0206 (reuse dead slots) are all
superseded by 0216 and their code is deleted — 0202's crash is gone
rather than guarded.

## The one item still outstanding

**The render is cloned rather than owned.** `RenderCache::get` clones
the value on every hit (`render_cache.rs:74-80`) and `splice_override`
clones again on insert (`override_apply.rs:2449`). On a full-document
render that is a copy of every rendered line plus a `Vec<NodeSpan>`.

The obvious fix does not work, which is why the shape is recorded here.
Making `RenderValue` an `Arc` alone fails: the confirmed splice **moves**
the rendered lines into `self.lines`, which must own its `String`s, so a
shared `Arc` forces `make_mut` and copies anyway. The caller mutates the
render too — it patches line 0 and may insert a truncation marker.

The resolution is to split by caller, because the two are not symmetric:

- **Confirmed splice — own it.** Never insert; take on a hit. The render
  is built once and moved into `self.lines`. Zero copies. The cost is
  one re-render on a revert-and-re-apply, which is user-paced.
- **Preview overlay — share it.** `PreviewOverlay.lines` becomes an
  `Arc` held jointly with the cache. A refcount bump, no copy, and the
  cache keeps serving the workload it was built for (spec 0116 §8),
  where entries are byte-budgeted by spec 0174 and small.

Free alongside: the preview path renders a `Vec<NodeSpan>` and throws it
away — `preview_override_highlight` takes only `rendered.lines`
(`override_select.rs:825`). `RenderedAs` is really two products and the
preview wants one of them.

This needs a spec. It is independent of everything else here.

## Open questions

1. **Is spec 0204 still worth implementing at all?** Spec 0205's own
   Open question asks this: 0205's measured 150 ms trigger is strictly
   better than 0204's. Neither is implemented. Settle it before either
   is started; the likely answer is to fold 0204 into 0205 and close it.
2. **How much of the batch cost is still the render itself?** With the
   arena flat, the remaining per-batch transient is the render and its
   clone. Nobody has measured what is left after 0216.

## Housekeeping carried over

- **The `probe` module** (`override_apply.rs:17-37`, `VISITS` /
  `SPLICES` / `NODES` counters) is temporary instrumentation, committed
  deliberately. Strip it when this work concludes.
- **No fuzz target** anywhere in the workspace.
- **`override_select.rs:262`** still describes a preview that "rebuilt
  the tree". Spec 0185 replaced that with a non-mutating overlay; the
  comment reads as current and has cost time twice.
