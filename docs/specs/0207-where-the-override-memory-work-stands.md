<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0207 — where the override memory work stands

Status: informational
App: protolens
Refs: docs/specs/0162-protolens-tree-node-reclamation.md (A2, the first
        free-list rejection),
      docs/specs/0174-preview-interior-truncation-and-node-budget-removal.md
        (the preview byte budget, which is why preview renders are
        small),
      docs/specs/0183-prune-the-override-walk.md (the descent marks),
      docs/specs/0185-the-preview-is-an-overlay.md (the preview stopped
        splicing speculatively),
      docs/specs/0186-the-commit-touches-only-what-moved.md (the
        per-batch verification hook; the first measurement of the
        `render_cache` clone),
      docs/specs/0188-the-batch-updates-what-changed-not-what-exists.md
        (N4, the `render_cache` deep clone as a backlog item),
      docs/specs/0202-an-override-is-refused-rather-than-fatal.md
        (implemented: the refusal guard),
      docs/specs/0203-the-override-arena-is-compacted.md (implemented:
        incremental mark-compact),
      docs/specs/0204-a-long-batch-says-so-before-it-blocks.md (draft),
      docs/specs/0205-the-batch-runs-off-the-input-thread.md (draft),
      docs/specs/0206-the-arena-reuses-its-dead-slots.md (draft: slot
        reuse)

## What this document is

This is not a specification. It is a wrap-up, written at the point
where the override-memory work was paused, so that resuming it does not
mean rediscovering what was already settled and — more importantly —
does not mean re-adopting conclusions that were already found to be
wrong.

It records four things: where the defect stands today, what has been
verified against the code and what has not, which questions are open,
and which specs would have to be written to close them. Nothing here is
to be implemented. Every actionable item points at a spec, existing or
prospective.

## Where things stand

Protolens still runs out of memory on a large enough document, and it
does so **by design rather than by accident**: `App::tree` is an
append-only arena and `splice_override` is an unbounded producer of
garbage into it. Two specs have landed against that and neither closes
it.

- **Spec 0202 (implemented)** turns the OOM kill into a refusal. The
  override does not run. The user sees a message instead of a dead
  process. This is a guard, not a fix, and it says so.
- **Spec 0203 (implemented)** makes the arena flat *between* batches.
  Six `t`/`Enter`/`o`/`d` cycles on a doubled `googleapis.desc` now
  start from the same 4 499 336 nodes every time, where before they
  went 4 501 014 → 9 000 349 → 13 499 684 → OOM. Its own N2 concedes
  that the peak *during* a batch is untouched: every batch still climbs
  to 8 998 671 nodes and 3.9–4.0 GiB before the compaction pass runs.

So the standing situation is: the arena no longer grows without bound,
but a single batch on a large document still needs roughly four
gigabytes, and whether it survives is a question about the machine.

## The one measurement everything now hangs on

A root retype on the 4 499 336-node corpus has **three** large
transients live simultaneously. Attributing the 3.9–4.0 GiB to the
arena alone was the central error of the drafting session, and it
survived a first review before being caught.

| transient | size | addressed by |
|---|---|---|
| arena's superseded copy | ~1.37 GB (8 998 671 × ~305 B, half of it garbage) | spec 0206 |
| `local_tree` (`override_apply.rs:2566`) | 4.5 M × 264 B = 1.19 GB | prospective, below |
| `render_cache`'s clone of the render | 4.5 M `NodeSpan` = 432 MB, plus a second `Vec<String>` of every rendered line | prospective, below |

The arena at 0203's peak (2.74 GB) plus `local_tree` (1.19 GB) is
3.93 GB, against a measured 3.9–4.0 GiB. The two largest account for
the whole observed figure by themselves, which leaves the render
cache's 432 MB unexplained — either the ~305 B/node estimate is
generous, or the sampling window missed the insert, or the entry had
been evicted. Spec 0206's Background flags this as something its own
measurement should settle. It does not change the conclusion, which is
that **each of the three specs below removes exactly one term**, and no
single one of them makes the batch cheap.

Two mechanical facts are what make these transients concurrent rather
than sequential, and both are easy to forget:

- `Vec` does not release its buffer incrementally, so
  `for node in local_tree` at `:2567` holds all 1.19 GB until the loop
  *ends* — which is precisely when the arena is at its widest.
- The render cache pays its copy on **every** path, not just on a miss:
  `:2908` clones on insert, and `render_cache.rs:77` clones on a hit.

## What has been verified against the code

Recorded so that resuming does not mean re-reading these call sites.

- **The splice frees before it allocates.** `mark_dead` runs at
  `override_apply.rs:2436`; the push loop starts at `:2555`. This is
  the fact that makes slot reuse bound `tree.len()` to the live
  high-water mark, and it is the fact spec 0203's N2 asserted the
  opposite of before being corrected.
- **`splice_override` has exactly one production caller**,
  `resettle_node` (`:1069`); every other call site is a test.
  `render_node_as` has two, the splice and
  `preview_override_highlight` (`override_select.rs:810`).
- **`resettle_node` re-splices only on a provenance change**
  (`:1064`, `current != rendered_as`), so a node is not re-rendered
  while its target is unchanged.
- **A confirmed render is essentially never re-read from the cache.**
  `is_preview` is part of the key (`render_cache.rs:30`), so a
  confirmed entry can only be hit by another *confirmed* splice of the
  identical `(range, target)` — i.e. the user reverting and
  re-applying. The preview path is where the cache is re-hit, on every
  arrow keystroke.
- **`PreviewOverlay.lines` is read-only** (`mod.rs:753`, read at
  `render.rs:333` and `:275`/`:290`/`:313`). It is never moved into
  `self.lines`. The splice path is the one that genuinely needs
  ownership.
- **The preview path discards `rendered.spans` entirely**
  (`override_select.rs:823-827` takes only `lines`). It renders a span
  vector it never looks at.
- **`descend.len() == tree.len()` is not a standing invariant.**
  `descend` is `Vec::new()` at `mod.rs:1564` while the arena is
  already full. It is raised to `tree.len()` by the resizes at
  `:1343`/`:1379`, so the two agree only after a batch has run.
- **`mark_fresh_subtree`'s `tree.len() <= base` guard (`:1376`) would
  fire on every splice under reuse**, since `tree.len()` stops
  growing. Under reuse it is also the *only* thing that marks a fresh
  node.
- **The local root's slot is read after it is freed** — marked dead at
  `:2638`, dereferenced at `:2639`, `:2646-2647` and `:2689`.
- **Freed nodes' links are read after the free**, by
  `doc_next_after_subtree` (`:2479`) and the holder repair
  (`:2454-2467`), both before the first allocation at `:2555`. Correct
  today, by ordering that nothing states.
- **The preview no longer splices speculatively.** Spec 0185 replaced
  it with a non-mutating overlay; the comments at
  `override_select.rs:258-269` describing a preview that "rebuilt the
  tree" are historical and read as current, which cost time twice.
- **`TreeNode` derives only `Debug`** (`decode.rs:479`). No `Default`,
  no `Clone`. This is why a slot cannot simply be `push`ed.

## Open questions

1. **Does `descend`'s watermark survive reuse in full?** Spec 0188's
   claim that two of three per-node target sources need no
   re-examination was checked against `compute_descend_marks`'s own
   reasoning only, **not** against the override-activation path. Spec
   0206 S4 now records this as a precondition of implementing it.
2. **What is `alloc_slots`' contract at the tail?** Reserving slots
   past `tree.len()` separates "reserved" from "initialized", because
   there is no `TreeNode` to write yet. Spec 0206 S1 states the
   problem and offers a `Default`-based alternative; it does not
   decide.
3. **How many extents does a real batch actually produce?** The
   argument for slot granularity over extent granularity rests on
   address-ordered first fit clustering holes well enough in practice.
   That is a measurement nobody has taken, and it also decides whether
   `slots`' 18 MB random gather is worth removing.
4. **Is spec 0204 still worth implementing at all?** Spec 0205's own
   Open question asks this: 0205's measured 150 ms trigger is strictly
   better than 0204's. Neither is implemented. This should be settled
   before either is started, and the likely answer is to fold 0204
   into 0205 and close it.
5. **What is the +128 MiB settled drift?** Spec 0203's measured
   outcome records 2594 → 2722 MiB across six batches and could not
   attribute it to the arena. Nothing since has explained it, and no
   spec claims it.

## The three specs this would take, in dependency order

Numbers are not assigned. Each is a separate change to a separate
thing, and each removes one term of the measurement above.

### A. Slot reuse — spec 0206, drafted

Ready to implement. It has been reviewed adversarially once and the
review's findings are folded in: the peak claim is corrected, `G3`'s
worst case is admitted, `alloc_slot` is replaced by `alloc_slots`, the
two intra-splice ordering constraints are stated, `descend`'s
watermark argument is rebuilt on a true premise, and
`reset_compaction`'s promotion from optimization to correctness
requirement is called out.

Its principal risk is unchanged and is the one thing three separate
specs have agreed on: reuse **fails dangerously rather than loudly**. A
stale index stops naming an abandoned-but-intact node and starts naming
a live, unrelated one. Spec 0206 S5 answers this with `verify_arena`
plus a requirement that the verifier become reachable from the shipping
binary behind an environment variable. That requirement is not
decoration — no fixture reaches the scale where the failure appears.

### B. In-place construction of the spliced subtree

Removes `local_tree`'s 1.19 GB by having `build_tree` write nodes
directly into their destination slots instead of into a private `Vec`
that is then copied out.

**Depends on A**, necessarily: you can only build in place once the
destination slots are known, which is exactly what spec 0206 S3
provides. It also deletes `slots`' 18 MB gather, since `build_tree`
would emit global indices in the first place and `translate` would
have nothing left to do.

Constraints already identified:

- Do not fork the linking algorithm at `decode.rs:544`. One
  implementation generic over the index mapping, monomorphized twice,
  or the two copies will drift.
- The coordinate translation (`byte_offset`, `text_range` rebasing,
  `packed_record_start` — `:2569-2589`) and the parent remap to `idx`
  (`:2596-2600`) fold into the same pass.
- The comment at `:2573-2586` records a real, expensive bug
  (`packed_record_start` left in local coordinates producing a cycle
  and unbounded recursion). Any rewrite of this loop must preserve
  that translation, and the comment should move with it.

### C. The render is owned, not cloned

Removes the render cache's copy. The shape was worked out and is worth
recording, because the obvious version does not work.

`RenderValue = Arc<...>` alone fails: the splice **moves** the rendered
lines into `self.lines` via `merge_line_patches` (`:2210`), and
`self.lines` must own its `String`s, so a shared `Arc` forces
`make_mut`, which copies anyway. The caller also mutates — `:2952`
patches line 0, `:2964` may insert a truncation marker.

The resolution is to split by caller, because the two are not
symmetric:

- **Confirmed splice — own it.** Never insert; take on a hit. The
  render is built once and moved into `self.lines`. Zero copies. The
  cost is one re-render on a revert-and-re-apply, which is
  user-paced.
- **Preview overlay — share it.** `PreviewOverlay.lines` becomes an
  `Arc`, held jointly with the cache. A refcount bump, no copy, and
  the cache keeps serving the workload it was built for (spec 0116
  §8), where entries are byte-budgeted by spec 0174 and small.

`RenderCache::get` should also stop returning by value
(`render_cache.rs:74-80`): promote, then hand back a reference or an
owned take, never a clone.

Free alongside: the preview path allocates spans it discards
(`override_select.rs:823-827`), so `RenderedAs` is really two products
and the preview wants one of them.

**Independent of A and B.** It is the smallest of the three and could
go first if a partial improvement is wanted before the harder work.

## Housekeeping carried over

Small, unrelated to the above, and easy to lose.

- **Spec 0201's paperwork.** Its code is committed. Status,
  `Implemented in` and Measured outcome are unfilled.
- **The `probe` module** (`override_apply.rs:29-37`, `VISITS` /
  `SPLICES` / `NODES` counters, 9 use sites) is temporary
  instrumentation, committed deliberately. Strip it when this work
  concludes.
- **`TieredBounded::peek` does not revive a cache entry** out of
  `prefetch_previous`. Noted, unspecified.
- **Per-node footprint narrowing** — `Option<usize>` → `Option<u32>`
  links (264 B → ~180 B/node, ~380 MiB at 4.5 M nodes) and `type_fqdn`
  interning. Spec 0203 N1 and spec 0206 N3 both defer it. Worth noting
  that it gets *more* attractive after A, not less: a ~100 B node fits
  several to a cache line, which is the one thing that would make
  contiguity worth more than spec 0206 concluded it is at 264 B.
- **No fuzz target** anywhere in the workspace.

## The judgment to resume with

The uncomfortable part of this position is worth stating plainly rather
than leaving implicit in three drafts.

There is no cheap fix. The four gigabytes are three separate mistakes
of roughly a gigabyte each, made in three different places, and each
one requires its own change to a different subsystem — the allocator,
the tree builder, the cache. Any single one of them leaves the batch
needing two to three gigabytes, which on a large enough document is
still a refusal by spec 0202.

What has actually been achieved is that the problem is now *bounded and
attributed* rather than open-ended. Spec 0203 stopped the growth across
batches, and the three transients above are measured, located and
individually addressable. That is a materially better position than
"protolens OOMs on big files", even though the user-visible behavior on
the largest documents has not yet changed.
