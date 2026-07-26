<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# protolens rendering pipeline — flaws report

*last verified: 2026-07-25*

Findings from a fresh-eyes review of the rendering pipeline
([design/rendering.md](design/rendering.md)). Ranked by severity within
four bands: **correctness bug** (produces a wrong result, a panic, or a
hang), **perf cliff** (correct but degrades superlinearly or blocks the
UI), **architectural smell** (an invariant enforced only by convention),
and **minor / doc drift**.

Items marked ✔ **fixed in this review** were corrected as part of it (docs
only — no source was modified).

**Scope.** This document covers `protolens` itself. Two companion reports
cover the libraries it decodes and scores with, and several of their
findings are on protolens's own critical path — in particular an
uncapped decode recursion that `SIGSEGV`s on `googleapis.desc`:

- [../prototext/decode-flaws.md](../prototext/decode-flaws.md) —
  `prototext-core` decode and sinks.
- [../scoring-flaws.md](../scoring-flaws.md) — `prototext-graph`'s
  `score_all`.

All three feed one worklist,
[rendering-worklist.md](rendering-worklist.md).

---

## Correctness bugs

Numbered in discovery order, not impact order. By impact, **C3 (a)** is
the worst of the four: it is the only one whose failure is a
process-level `SIGSEGV` — no unwinding, no panic hook, no message —
rather than a panic or a stuck terminal, and it fires on the most
ordinary action there is (quitting) against the largest inputs
(`googleapis.desc`), which are exactly the inputs whose sweep is slow
enough to still be running.

### C1. The live-preview watermark truncation leaves the seam node's `doc_prev` dangling on the `Err` path

**Where:** `protolens/src/tui/override_select.rs:815-841`

**What happens.** `preview_override_highlight` recomputes `idx.doc_next`
before truncating (the 2026-07-25 fix), and defensively nulls
`idx.first_child`/`last_child` so that a `splice_override` returning
`Err` cannot leave them pointing past `tree.len()`. But the *other* end
of the same seam is not repaired. The previous preview's splice executed
`self.tree[after].doc_prev = Some(last_new)` (`override_apply.rs:1728`)
where `last_new >= watermark`. After `self.tree.truncate(watermark)`, the
seam node `after` still holds that index in `doc_prev`.

On the success path this is invisible: `splice_override` overwrites
`tree[after].doc_prev` (`override_apply.rs:1728` or `:1733`). On the
`Err` path — reachable, and explicitly handled at
`override_select.rs:840` (`"cannot preview override: {e}"`), e.g. an
unparseable target — the stale index survives. The very next backward
navigation or backward search dereferences it:

```rust
// navigation / override_select.rs:902
cur = self.tree[cur].doc_prev.unwrap_or(self.last_node());
let line_idx = self.tree[cur].span.text_range.start;  // index out of bounds
```

This is the identical failure class as the 2026-07-24 `line_to_node`
crash the block at `override_select.rs:813-814` was written to prevent,
and the identical class as the 2026-07-25 cycle — a truncation that
repairs the pointers *out of* `idx` but not the pointers *into* the
truncated range from outside it.

**Proposed correction.** In the same pre-truncation block that already
recomputes the seam, close the loop by restoring `idx` as a childless
node in the chain — the state a failed splice should leave behind
anyway:

```rust
let seam = self.doc_next_after_subtree(self.tree[idx].doc_next, &old_descendants);
self.tree[idx].doc_next = seam;
if let Some(a) = seam {
    self.tree[a].doc_prev = Some(idx);   // ← missing today
}
```

This must go *before* the truncation for the same reason the `doc_next`
recomputation does. It also makes the invariant local and total: after
the truncation the chain is fully consistent regardless of whether the
subsequent splice succeeds, rather than consistent only if it does.

---

### C2. ✔ The line-patch ordering invariant is enforced only by `debug_assert!`, which this project never compiles in

**Where:** `protolens/src/tui/override_apply.rs:1216`,
`protolens/src/tui/override_apply.rs:1261`

> ✔ **Fixed 2026-07-26**, taking the stronger of the two options below.
> `materialize_line_patches` now sorts both `top_level` and each
> `children_of` entry by range start, so ordering is the merge's own
> business rather than a caller's obligation, and both `debug_assert!`s
> became real `assert!`s covering *overlap* only — the one condition a
> reordering cannot repair — each naming the offending pair. Three tests
> in `tests/override_apply.rs` queue patches back-to-front (top-level and
> nested) and demand the correct merge, plus a `should_panic` case for
> overlap; all run under `--release`, which was the point.

**What happens.** `materialize_line_patches` merges the batch's queued
`LinePatch`es into `lines`/`line_styles` in one forward pass, which is
only correct if the patches arrive in strictly ascending,
non-overlapping `global_start` order. That precondition is checked with
`debug_assert!`.

This project's build convention is `cargo build --release` universally
(it is what the dev-shell puts on `PATH`). `debug_assert!` compiles to
nothing there. So the one guard standing between "the invariant broke"
and "we index a slice with a reversed range" is absent in every binary
anyone actually runs — and a reversed or overlapping range in a slice
operation panics with `slice index starts at N but ends at M`, pointing
at the merge loop rather than at whichever splice queued the bad patch.

This is not hypothetical: the 2026-07-25 `doc_next` cycle bug's
*observable* symptom was described as "out-of-order, overlapping line
patches". The assertion that would have named the problem was compiled
out; the bug presented as a hang and a panic instead.

**Proposed correction.** Promote both to real, always-on checks. They are
O(1) per patch against an O(N_final) merge, so the cost is
unmeasurable. Either:

- `assert!` with a message naming the offending pair
  (`prev.global_start`, `prev` length, `patch.global_start`), which turns
  a mystery slice panic into a directed one; or, better,
- make the ordering unrepresentable: have `materialize_line_patches`
  `sort_by_key(|p| p.global_start)` a scratch index and `assert!` only
  non-overlap. Sorting is O(k log k) in the number of patches in the
  batch (small — one per splice), not in document length, and it removes
  the requirement that *callers* queue in order, which is precisely the
  requirement that turned out to be violable at a distance.

---

### C3. ✔ The detached root-type thread outlives the mmap it reads, and runs the deep scoring recursion on the default stack

**Where:** `protolens/src/tui/mod.rs:1673-1684`

> ✔ **Both halves fixed 2026-07-26** by
> [spec 0180](../specs/0180-own-the-scoring-graph-by-arc.md), which took
> the `Arc` route rather than waiting for spec 0168's deletion of the
> spawn site (see the Note below — that spec is still unimplemented, and
> "will be deleted eventually" is not a reason to leave a
> use-after-unmap live). S1/S2 made `LoadedGraph::graph` private and
> `DescriptorContext.graph` an `Option<Arc<LoadedGraph>>`, so (a) is
> closed by the type system rather than by the field order; S4 moved the
> stack-size constant to `tui/mod.rs` as `SCORING_THREAD_STACK_SIZE` and
> gave this spawn a `thread::Builder`, closing (b) exactly as proposed.

Two independent defects at one spawn site.

**(a) Use-after-unmap.** The thread captures `graph_ref`
(`mod.rs:1660`), a `&'static ArchivedCompiledGraph`. That `'static` is
not a real static: `prototext-graph/src/score/load.rs:84-89` fabricates
it with `std::slice::from_raw_parts` over an `Mmap`, and the safety
comment there is explicit that its validity depends on the `Mmap`
outliving it — "enforced by keeping both in `LoadedGraph`". The `Mmap`
is `LoadedGraph._backing`, owned by `App.ctx`.

The spawn's own comment justifies never joining with "it holds only
`'static`/`Arc`-owned data". That is false in exactly the way the
lifetime extension makes it easy to be false: `graph_ref`'s type says
`'static`, its actual validity ends when `App.ctx` drops. Two mechanisms
protect the *heat worker* from this — the explicit `worker.shutdown()`
at `mod.rs:1701-1703`, and `App`'s field order (`heat_worker` declared
before `ctx`, `mod.rs:727-742`). The detached root-type thread is
covered by **neither**. Quitting during the sweep — which is the whole
point of not joining, and is most likely precisely on the large blobs
where the sweep is slow — drops `App`, unmaps the file, and leaves a
live thread dereferencing unmapped pages. SIGSEGV, not a panic; no
unwinding, no message.

This is the same signature as the field-order hazard spec 0152 G9 was
written to close, and the fix that closed it did not reach this call
site because this call site did not exist yet.

**Proposed correction.** Make the lifetime real rather than asserted:
change `DescriptorContext.graph` to `Option<Arc<LoadedGraph>>` and hand
both threads an `Arc` clone. The heat worker's `&'static` disappears
too, which retires the field-order dependency as a load-bearing
invariant (it becomes merely tidy). This is the same change A5 proposes
from the library side; doing one without the other leaves the hole open.

If an `Arc` is genuinely unwanted, the fallback is to join this thread
alongside the worker — but that reintroduces exactly the quit latency
the comment is trying to avoid, so the `Arc` is the right answer.

**(b) Stack size.** `HEAT_WORKER_STACK_SIZE = 16 MiB`
(`heat_worker.rs`) exists because `score_all`'s recursion overflows the
default stack on deeply nested blobs. This thread calls
`decode::resolve_root_winner_fqdn` → the same `score_all`, over the
*whole* blob (the deepest possible input), on a bare `thread::spawn`
with the platform default (2 MiB on Linux). A stack overflow here aborts
the entire process.

**Proposed correction.** `thread::Builder::new().stack_size(
HEAT_WORKER_STACK_SIZE)`, and move the constant somewhere both spawn
sites can name it — the fact that it currently lives in `heat_worker.rs`
is why the second spawn silently didn't get it.

**Note.** Both defects are *deleted* rather than fixed by
[spec 0168](../specs/0168-protolens-resolve-root-type-before-decode.md),
which removes this spawn site entirely by resolving the root type on the
main thread before the single decode. That is the preferred resolution:
(a) and (b) are both artifacts of running this one computation late and
off-thread, and neither has a reason to exist once it runs early and
in-line. The `Arc<LoadedGraph>` change is still worth making on its own
merits (see A5), because the heat worker's `&'static` remains.

---

### C4. Two `?` early returns in `run` sit above the terminal-restore block

**Where:** `protolens/src/tui/mod.rs:1631`, `protolens/src/tui/mod.rs:1694`

**What happens.** `run` enables raw mode, enters the alternate screen and
enables mouse capture at `mod.rs:1618-1621`, and restores all three at
`mod.rs:1708-1709`. Between them, two fallible calls propagate with `?`:

```rust
app.override_list_height = terminal.size()?.height.max(1) as usize;  // :1631
...
warm_up_heat_cues(&mut terminal, app)?;                              // :1694
```

Either `Err` returns from `run` without running `restore_terminal()`,
leaving the user's shell in raw mode inside the alternate screen with
mouse reporting on — i.e. wedged, with the error message printed
somewhere invisible. The panic hook installed at `mod.rs:1637-1641`
covers *panics* only, and in the `:1631` case is not even installed yet.

Spec 0152 G9 describes this region as having an "unconditional cleanup
block". It is unconditional with respect to `run_loop`'s result, which
is captured into `result` and returned *after* cleanup — but not with
respect to these two.

**Proposed correction.** Give both the same treatment `run_loop` already
gets: capture, then clean up, then return. Better, wrap the whole
fallible middle in a closure so that *future* `?`s are covered
structurally rather than by review — [spec 0168](../specs/0168-protolens-resolve-root-type-before-decode.md)
adds two more fallible calls to this region, which is the argument for
fixing the shape and not the two instances.

```rust
let result = (|| -> io::Result<()> {
    app.override_list_height = terminal.size()?.height.max(1) as usize;
    ...
    warm_up_heat_cues(&mut terminal, app)?;
    run_loop(&mut terminal, app, &rx, &mut input_reader, &tx)
})();
```

Wrapping the whole fallible middle in one closure is preferable to
hand-restoring at each `?`, because it makes the property structural:
any future `?` added between the setup and the cleanup is covered
automatically. That is the same reasoning that put the panic hook there
rather than a `catch_unwind` at each site.

---

## Perf cliffs

### P1. `App::new` runs a full-document `render_overrides` pass at startup, and it is the single largest startup cost

**Where:** `protolens/src/tui/mod.rs:1232-1234`

**What happens.** `decode()` deliberately disables `prototext_core`'s
Any/MessageSet expansion, because protolens implements it as ordinary
overrides. The consequence is that nothing has expanded them by the time
`App::new` finishes building state — so `App::new` runs
`render_overrides(cursor)` over the **entire** document purely to
discover them.

Measured on a 1.1 MB `FileDescriptorSet` (622,922 nodes / 193,072
lines), `--release`:

| | |
|---|---|
| `decode()` (stages 1-3) | 1.65 s |
| `App::new` | **5.22 s** |

The walk to find Any/MessageSet nodes costs **3× the decode of the whole
document**. On a blob containing no Any and no MessageSet at all — the
common case for a `FileDescriptorSet` — the entire 5.22 s produces
literally no change to the rendering.

**Proposed correction.** Make the candidate set known without a walk.
`render_overrides_inner`'s recursion gate is
`is_message || is_auto_expand_candidate(span) || <has active override
entry> || rendered_as.is_some()`; `is_auto_expand_candidate` is a purely
*structural* predicate over a single `NodeSpan` (spec 0120's deliberately
narrow "matches exactly the two known Any/MessageSet field shapes"). So:
have `build_tree` (`decode.rs:301-369`) — which already visits every span
once — collect `auto_expand_seeds: Vec<usize>` of the nodes satisfying
that predicate, and have `App::new` call `render_overrides` once per seed
instead of once on the root. At startup the only override entry that can
exist is the seeded root type (already reconciled by the `rendered_as`
write at `mod.rs:1223`), so no other node can need resettling, and the
targeted set is provably complete.

Cost becomes O(number of Any/MessageSet nodes) instead of O(document).
For the measured fixture that is zero seeds and a ~0 ms startup pass.

**Invariant touched:** none of the coordinate systems; `build_tree` gains
one output field. Each seed's `render_overrides` is its own batch, so `k`
seeds pay `k` finalizations — bound that by wrapping the loop in a single
explicit batch (increment `override_batch_depth` around it and finalize
once) if a document turns out to have many seeds.

**Risk:** low. Local, and directly testable: assert that the seed list
equals the set of nodes the current full walk would have spliced.

---

### P2. Every unsettled visible row recomputes a positional path and linearly scans the override list, per frame

**Where:** `protolens/src/tui/heat_cue.rs:243` →
`override_apply.rs:724` → `override_apply.rs:650` →
`navigation.rs:446` / `navigation.rs:425`

**What happens.** `heat_cue_resolve` calls `current_type_key(idx)`, which
for a non-message node calls `resolve_active_override(idx)` →
`resolve_active_override_entry(idx)` → `positional_path(idx)`. That walks
up the parent chain calling `sibling_position` at every level, and
`sibling_position` (`navigation.rs:425-433`) walks `prev_sibling`
**one node at a time from the node back to its first sibling**:

```rust
let mut pos = 1;
let mut cur = idx;
while let Some(prev) = self.tree[cur].prev_sibling { pos += 1; cur = prev; }
```

It is therefore O(ordinal position among siblings). A
`FileDescriptorSet`'s repeated `file`/`message_type`/`field` runs have
thousands to tens of thousands of siblings, so a node late in such a run
costs tens of thousands of pointer chases — and the path has one such
walk *per level*. Then
`resolve_active_override_entry_index_by_path` performs up to **three
separate linear scans** over `self.overrides.entries()`
(`override_apply.rs:682`, `:690`, `:698`).

All of this runs once per *unsettled* visible row, on every frame. The
code even documents the danger — "prohibitively expensive when called
once per node across an entire large document"
(`override_apply.rs:672-673`) — and `render_overrides_inner` was fixed to
avoid it by deriving paths incrementally. The heat-cue path was not.

The 0.5 ms measured frame time does not contradict this: by the time it
was measured the visible rows had *settled*, and `heat_cue_resolve`
returns at `heat_cue.rs:235` before any of this. The cliff is exactly
during the progressive-display window — the seconds after startup or
after a scroll into cold territory, i.e. precisely when responsiveness
matters most.

**Proposed correction.** Two independent fixes, either sufficient, both
cheap:

1. **Store the ordinal on the node.** Add `ordinal: usize` to `TreeNode`,
   set by `build_tree` (which already iterates children in order) and by
   `splice_override` for the appended local tree. `sibling_position`
   becomes a field read. The one subtlety is packed-run absorption
   (`override_apply.rs:1705-1715`), which removes `run_len - 1` siblings
   and so shifts every following sibling's ordinal — handle it by
   walking `next_sibling` from `idx` once, which is bounded by the run's
   own tail rather than by the document.
2. **Memoize `current_type_key` per node**, keyed on `(structural_version,
   overrides_version)`, in a `Vec<Option<String>>` parallel to `tree`.
   `structural_version` already exists (`navigation.rs:53`) and already
   bumps on exactly the right events; an `overrides_version` counter
   bumped by every `OverrideCollection` mutation is trivial to add.

Independently, replace the three linear `entries()` scans with three
`HashMap`s keyed by `OverrideOrigin` variant, rebuilt on
`overrides_version` bump. Entry counts are small today, but the scans are
inside the per-row-per-frame path, which is the wrong place for a linear
search on principle.

**Risk:** (1) is medium — it adds a field with a maintenance obligation
in `splice_override`, the exact place invariants have historically been
violated. (2) is low and purely additive. Prefer (2) first.

---

### P3. A single override commit costs 8-11 s, and the tree quintuples over two commits

**Where:** `protolens/src/tui/override_apply.rs:1122-1166`
(`finalize_override_batch`), `navigation.rs:37-54`
(`rebuild_visible_rows`)

**What happens.** Measured, same fixture:

| Action | Cost | `tree.len()` after |
|---|---|---|
| startup | — | 622,922 |
| `Enter` (commit override #1) | **10.6 s** | 1,690,153 |
| `Enter` (commit override #2) | 8.2 s | 2,709,031 |

Committing an override runs `render_overrides(first_node)` — a full
document walk — followed by a `finalize_override_batch` that itself does
three whole-document passes: the patch merge, the `text_range` shift
walk, and a **clear-and-rebuild of `line_to_node` +
`footer_line_to_node` over the entire `doc_next` chain**
(`override_apply.rs:1144-1160`, already flagged as a non-goal by spec
0163 N3). Then `rebuild_visible_rows` allocates a fresh
`vec![false; total_lines]` and refilters `0..total_lines`.

Separately, the "always append, never renumber" discipline means the
arena grows monotonically and nothing reclaims orphans — spec 0162
(tree-node reclamation) is a **goals-only draft with no implementation**.
Two commits leave ~2.1 M orphaned nodes resident for the rest of the
session.

**Proposed correction.** Detailed in
[rendering-scaling-roadmap.md](rendering-scaling-roadmap.md) (items S2,
S3, S4); in brief: make the `line_to_node` rebuild incremental (only
entries at or past the batch's first patch need touching — everything
before it is unchanged by construction), and make `rebuild_visible_rows`
incremental on the same range.

---

### P4. Startup holds the rendered text twice, plus three copies of the blob

**Where:** `protolens/src/decode.rs:789-792`,
`protolens/src/tui/mod.rs:1661`, `protolens/src/tui/mod.rs:1679`

**What happens.** Two independent duplications:

*Text.* `decode()` builds `text: String` (the whole rendered document,
contiguous), then `lines: Vec<String>` — a complete second copy, one heap
allocation per line — then keeps `lines` and drops `text` only on
return. Peak is 2× rendered text plus 24 B of `String` header per line
plus 24 B of `Vec` header per line in `line_styles`. At 193 k lines
that is ~9 MB of headers alone, before content.

*Blob.* `App` owns `blob: Vec<u8>` (the wrapped blob). Startup then makes
`Arc::new(app.blob.clone())` for the heat worker (`mod.rs:1661`) and
`app.blob[app.wrapper_offset..].to_vec()` for the detached root-type
thread (`mod.rs:1679`) — **three** resident copies. On the 24.5 MB
`googleapis.desc` fixture that is ~74 MB of blob, all identical.

**Proposed correction.** Both are near-trivial:

- Change `App::blob` to `Arc<Vec<u8>>` and hand clones of the `Arc` to
  the worker and to the root-type thread. The root-type thread wants the
  *unwrapped* blob; give it the `Arc` plus `wrapper_offset` and let it
  slice, rather than materializing a copy. Everything reading
  `self.blob` as `&[u8]` is unaffected by `Arc` deref.
- Build `lines` without the intermediate `String`: have `decode` split
  the `Vec<u8>` returned by `decode_and_render_indexed` on `b'\n'` and
  `String::from_utf8` each line. This also localizes UTF-8 validation
  failures to a line instead of failing the whole document, and drops
  peak text residency to 1×. `colorize::colorize` still needs the
  contiguous text — see roadmap item S5 for that half.

**Risk:** low for the blob change (mechanical, compiler-checked). Low for
the line split, with one caveat: `text.lines()` treats a trailing `\r`
as part of the line, and splitting on `b'\n'` yields a trailing empty
element if the buffer ends with `\n` — both need matching explicitly to
avoid an off-by-one in line count, which would desynchronize
`lines`/`line_styles`/`text_range` (invariant 1 in
[design/rendering.md](design/rendering.md)).

---

### P5. The heat cache's size bound is stated in entries, but one entry can hold the entire candidate list

**Where:** `protolens/src/tui/heat_cue.rs:120-124`,
`protolens/src/tui/heat_worker.rs:384-396`,
`protolens/src/tui/override_select.rs:485`

**What happens.** `HEAT_CACHE_MAX_ENTRIES = 8192` is documented as
costing "well under 1MB", because "both value types are small
fixed-size scalars". That was true when written. It is now false: the
`by_range` value is `RangeHeatEntry`, which gained
`top_n: Vec<(String, i64)>` — an owned, unbounded vector of FQDN
strings.

Two writers populate it, with very different bounds:

- `heat_cue_resolve` (`heat_cue.rs:330-331`) caps at
  `max(override_list_height, HEAT_CUE_PREVIEW)` — a terminal-height
  bound, so tens of entries.
- The worker (`heat_worker.rs:384-394`) uses
  `existing_len.max(req.end)`. And `upgrade_active_override_to_complete`
  requests `end = usize::MAX` (`override_select.rs:485`). So a range the
  user opened the override pane on gets its `top_n` set to the *entire*
  candidate list — every message type in the pool that scores at all.

Against `googleapis.desc`, spec 0151's own measurement was 15,995
entries totaling ~1,012,849 bytes for a *single* such range. The cap
counts entries; nothing counts bytes. 8192 entries of that shape is
gigabytes, not "well under 1MB".

**Proposed correction.** The `TieredBounded` cap should be on payload
bytes, not entry count — the same discipline `render_cache.rs` /
`CandidateCache` already apply (see [design/caches.md](design/caches.md):
both are explicitly *byte*-bounded, which is why this one being
entry-bounded reads as an oversight rather than a choice). Failing that,
stop letting `req.end` size `top_n` at all: clamp the worker's
`top_n_len` to the same `max(override_list_height, HEAT_CUE_PREVIEW)`
cap the synchronous path uses, and let the `complete` slot — which
already exists precisely to hold one unbounded list — be the only place
a full list is retained.

The second fix is smaller and is the one that matches the existing
design's intent: `top_n` is documented as a *preview* prefix; nothing
justifies it ever holding the full list.

Spec 0165 (`docs/specs/0165-protolens-heat-cue-pool-sizing-cli-and-exit-stats.md`,
draft) diagnoses this same sizing problem.

---

### P6. Each unsettled visible row deep-clones the whole candidate list, twice, per frame

**Where:** `protolens/src/tui/render.rs:330-333` →
`heat_cue.rs:216-228` → `heat_worker.rs:239-241` →
`tiered.rs:139-145`

**What happens.** `render` builds `heat_displays` by calling
`heat_cue_for` once per visible row, every frame. For an unsettled row
that reaches `heat_lookup`, `HeatCaches::window` does:

```rust
if let Some(entry) = self.by_range.peek(&range_start, tier) {   // clone #1
    if entry.top_n.len() >= end {
        return Some(entry.top_n[start..end].to_vec());          // clone #2
    }
}
```

`TieredBounded::peek` returns `V` by value via `.value.clone()`
(`tiered.rs:139-145`), so clone #1 is a deep copy of the entire
`RangeHeatEntry` including every `String` in `top_n` — and then all of
it but the requested window is dropped. Compounded with P5, on a range
the override pane has upgraded that is a ~1 MB allocate-and-free per
row per frame. Two further `to_string()` allocations happen on the
`current_score` key path (`(usize, String)`).

Note this shares the "only during the progressive window" caveat with
P2, and for the same reason: `heat_cue_resolve` short-circuits at
`heat_cue.rs:235` once a row settles. The two therefore stack — the
frames that pay P2's positional-path walks are exactly the frames that
pay these clones.

**Proposed correction.** Give `TieredBounded` a borrowing accessor
alongside `peek`. The tier promotion `peek` performs needs `&mut self`,
so the borrow can't outlive it as written; split it:

```rust
pub(super) fn touch(&mut self, key: &K, tier: Tier);          // promotion only
pub(super) fn get(&self, key: &K) -> Option<&V>;              // no clone
```

`window` then calls `touch` followed by `get`, and clones only the
`[start..end)` slice it actually returns. This is a mechanical change
confined to `tiered.rs` + its two callers, and it is worth doing
independently of P5 because it removes the clone whose size is the thing
P5 fails to bound.

---

## Architectural smells

### A1. The render cache key omits `initial_level` and `indent_size`

**Where:** `protolens/src/tui/override_apply.rs:1450`, with the fields
themselves at `override_apply.rs:1470-1471`

**What happens.** The `RenderCache` is keyed on
`(interior_range, target, is_preview)`. But the render whose output is
being cached is also parameterized by `initial_level` and `indent_size`,
both of which affect the produced *text* (leading indentation), and
neither of which is in the key. Two nodes at different tree depths, or
two sessions at different `--indent`, sharing a byte range and a target
would hit the same entry.

It is correct today for a reason that is nowhere near the cache: every
caller happens to pass the session-wide `indent_size`, and the spliced
text is re-indented to the target node's level afterwards. That is a
non-local argument protecting a cache key, and the code has no assertion
or comment binding the two together.

**Proposed correction.** Put both in the key. They are a `usize` and a
`usize`; the entry payload is a rendered subtree. If the extra key width
is genuinely unwanted, at minimum assert that `indent_size` matches the
session's at insert time, so the assumption is enforced where it is
relied upon rather than where it happens to be true.

**Severity note:** filed as a smell, not a bug, because no current caller
can violate it. It is on the list because the *next* caller can, silently,
and the failure mode (a subtree rendered at another node's indentation)
would look like a splice bug, not a cache bug.

---

### A2. `heat_states` parallel-array coupling is maintained by hand in three places

**Where:** `protolens/src/tui/override_apply.rs:1682-1684`,
`protolens/src/tui/override_select.rs:799`,
`protolens/src/tui/heat_cue.rs:375-378`

**What happens.** `heat_states: Vec<HeatState>` must stay index-parallel
to `tree: Vec<TreeNode>`. Three separate sites uphold this: a `resize` in
`splice_override`, a `truncate` in the preview path, and a defensive
`idx >= self.heat_states.len()` bounds check in
`recheck_pending_heat_states` that exists *because* the other two are not
sufficient (a queued index can outlive a truncation).

That third site is the tell: the invariant is known to be violable, and
the response was a check at the read site rather than making the write
sites total. Any fourth site that grows or shrinks `tree` — and
`splice_override`'s `tree.extend`/preview truncation are not the only
conceivable ones — silently breaks it.

**Proposed correction.** Make the pairing structural rather than
conventional. Either move `heat: HeatState` into `TreeNode` (it is two
`Option`s; `TreeNode` is already large, and this removes the class of bug
entirely), or wrap the pair in a small `Arena { nodes, heat }` type whose
only mutators are `push`/`truncate`/`resize` and which cannot desync by
construction. The former is a smaller diff; the latter keeps `TreeNode`
free of UI-derived state.

**Risk:** low-medium. Mechanical, but touches `TreeNode`'s layout and so
every construction site including the test fixtures.

---

### A3. `override_batch_depth` is a counter modeling nesting that cannot occur

**Where:** `protolens/src/tui/override_apply.rs:898-909`,
`override_apply.rs:1749-1751`

**What happens.** No call site invokes `render_overrides` from inside a
walk — the recursion is `render_overrides_inner` calling itself, which
never touches the counter. The counter therefore only ever holds 0 or 1.
What it actually encodes is a binary mode read by `splice_override`:
"inside a walk, defer finalization" vs. "standalone, finalize now".

Modeling that as a depth counter invites a reader (and a future author)
to believe nested batches are supported and tested. They are neither.
Worse, if a nested call *were* added, `finalize_override_batch(idx)` takes
the *inner* call's `idx` as its walk origin, so the outermost finalization
would start its downstream correction from whichever node happened to be
passed last — a subtle wrong-origin bug the counter's presence implies is
already handled.

**Proposed correction.** Either replace it with
`in_override_batch: bool` and `debug_assert!(!self.in_override_batch)` on
entry (documenting that nesting is *not* supported), or — if nesting is
wanted — thread the batch's origin node explicitly rather than relying on
the last `idx` to reach `finalize_override_batch`. The first is a
two-line change and states the truth; the second is real work and should
wait for an actual need.

✔ The design doc claim that nested calls occur has been corrected in
[design/override-collection.md](design/override-collection.md).

---

### A4. The `HeatCaches::complete` single slot is clobbered by prefetch work

**Where:** `protolens/src/tui/heat_worker.rs:402`, with the slot declared
at `heat_worker.rs:199-203`

**What happens.** `complete` holds "the most recently *fully* scored
range's complete candidate list", justified by "only one override pane
can be open at a time". But it is refreshed **unconditionally** by every
worker completion (`heat_worker.rs:402`), including `Tier::Prefetch`
completions for ranges the user has never looked at. So a prefetch wave
landing while the override pane is open evicts the pane's own complete
list from the slot.

The consequence is worse than "occasionally colder than it could be",
because for the override pane's own request the `complete` slot is not a
fallback — it is the *only* possible hit path. Both coverage tests
require `top_n.len() >= end` (`heat_worker.rs:348`,
`heat_worker.rs:240`), and the pane requests `end = usize::MAX`
(`override_select.rs:485`), which no finite `top_n` satisfies. So
`upgrade_active_override_to_complete` can only ever be answered by
`complete` holding this exact range.

That makes the clobber self-sustaining: the pane misses, pushes a
`Tier::User` request, the worker performs a full sweep and writes
`complete`; a prefetch completion lands and overwrites `complete` with
some other range; the pane's next `poll_pending_override_work` misses
again and re-requests the *same* full sweep. Under a sustained prefetch
wave this ping-pongs indefinitely, re-scoring a range that was fully
scored moments ago — the most expensive operation in the subsystem, on
repeat, while the user waits for a candidate list that is sitting
finished in memory each time it is thrown away.

The same unsatisfiable-`covers_window` shape also means the worker
recomputes `covers_window` as `false` on every such request, so the
early-out branch at `heat_worker.rs:345-348` never fires for pane
requests either.

**Proposed correction.** Do not refresh `complete` from a
`Tier::Prefetch` completion — a prefetch by definition is not the thing
the user is looking at:

```rust
if req.tier != Tier::Prefetch {
    c.complete = Some((req.range.clone(), candidates));
}
```

This matches the existing tier-based suppression two lines below
(`heat_worker.rs:405-410`), which already treats `Prefetch` as
"write the cache, don't disturb the foreground".

Separately, and orthogonally, make `covers_window` satisfiable so
`complete` stops being load-bearing: add `total_candidates: usize` to
`RangeHeatEntry` (the worker knows it — it is `candidates.len()` before
truncation), and test `top_n.len() >= end.min(entry.total_candidates)`.
An entry then reports coverage honestly for an unbounded request whose
real answer is shorter than the request, which is the actual question
being asked.

---

### A5. ✔ `LoadedGraph` publishes the `&'static` it exists to protect

**Where:** `prototext-graph/src/score/load.rs:21-25`, `:82-89`

> ✔ **Fixed 2026-07-26** by
> [spec 0180](../specs/0180-own-the-scoring-graph-by-arc.md) S1/S2. The
> field is private, `graph()` exists, and protolens holds an
> `Arc<LoadedGraph>`. One deviation from the correction below, recorded
> as the spec's non-goal N2: **the loader still returns a plain
> `LoadedGraph`**, not an `Arc`. The soundness property comes from
> privacy, not from `Arc`; `prototext`'s CLI is single-threaded and holds
> the graph in its own `DescriptorContext`, so forcing an `Arc` on it
> would buy an allocation and an indirection and nothing else. `Arc` is
> applied where a reference must outlive a borrow, which is protolens and
> only protolens.

**What happens.** The safety comment says the `'static` lifetime
extension is sound "as long as `_mmap` lives as long as `graph` —
enforced by keeping both in `LoadedGraph`". Co-location enforces
nothing. `graph` is a `pub` field of `Copy` type: any consumer can copy
it out, and `LoadedGraph` can then drop while the copy remains. That is
not hypothetical — it is exactly what `mod.rs:1660` does, and C3 is the
resulting live segfault.

The type's `Deref<Target = ArchivedCompiledGraph>` impl
(`load.rs:27-28`) already provides the intended borrow-checked access
path. The `pub` field is a second, unchecked one that defeats it.

**Proposed correction.** Make the field private and add
`pub fn graph(&self) -> &ArchivedCompiledGraph`, returning a reference
tied to `&self`. Consumers wanting to keep it across a thread boundary
must then hold an `Arc<LoadedGraph>`, which is the correct thing and is
what C3 needs anyway. Have the loader return `Arc<LoadedGraph>`
directly, so no caller has to know to wrap it.

**Risk:** low, and compiler-directed — every site that breaks is a site
that was relying on the unchecked path.

---

### A6. The heat cache key is justified by a property that is false

**Where:** `docs/specs/0151-protolens-heat-cue-cache-and-startup-progress.md:89-101`,
implemented at `protolens/src/tui/heat_worker.rs:192`

**What happens.** `by_range` is keyed on a bare `start: usize` rather
than the `Range<usize>` it describes. The recorded justification is that
"a node's byte range occupies a region disjoint from every other
node's", asserted twice and called "disjoint-by-construction".

Protobuf node ranges are **nested**, not disjoint — that nesting is the
document tree. The stated reason for the key being sound is therefore
simply wrong.

The key may well still be unique, but for a different and unwritten
reason: length-prefix framing means a child's payload starts strictly
after its parent's does, so no two nodes share a payload start. That is
a real argument, but it is not the one in the spec, it is not in the
code, and it is not obviously robust to the two places protolens
manufactures ranges rather than reading them off the wire (the synthetic
wrapper, and packed-run absorption).

Filed as a smell rather than a bug because no collision has been
demonstrated. It is on the list because a key protected by a false
argument is protected by nothing: the next person to change how ranges
are derived will check the recorded justification, find it satisfied in
spirit, and be wrong.

**Proposed correction.** Key on the whole `Range<usize>`. Hashing a
two-word struct instead of one word is not a measurable cost at this
call frequency, and it removes the need for any argument at all. If the
single-word key is kept, replace the justification with the framing
argument and say explicitly that it is what the synthetic wrapper must
not violate.

---

## Minor / doc drift

### D1. ✔ `document-tree.md` asserted an invariant that is false, and described the wrong fix

**Where:** `docs/protolens/design/document-tree.md:90-121` (as committed
earlier this session)

The section titled "The `doc_next` invariant splicing depends on" claimed
`idx.doc_next` "always points to a node outside `idx`'s own subtree", and
that the 2026-07-25 fix "was simply to leave `doc_next` alone on that
path." Both are false, and they are false in opposite directions:

- Document order is pre-order, so for any node *with* descendants,
  `idx.doc_next` is its own first child — *inside* the subtree. This is
  stated explicitly by the code the doc is describing
  (`override_apply.rs:105-119`), and is the entire reason
  `doc_next_after_subtree` exists.
- "Leave `doc_next` alone" is what the buggy code *did*
  (`override_select.rs`, per commit 60a1673's diff), and it is what
  caused the cycle. The actual fix was to *recompute* it before
  truncating.

A doc that inverts both the invariant and the remedy is worse than no
doc: it would lead a reader fixing the next occurrence of this bug to
reintroduce it. ✔ Rewritten as "The seam `doc_next` does *not* directly
give you".

### D2. ✔ `override-select-pane.md` carried the same inverted claim

**Where:** `docs/protolens/design/override-select-pane.md:76-81`

Said the truncation "only ever needs to invalidate the target node's own
`first_child`/`last_child` … its `doc_next` must be left untouched".
✔ Corrected, and the `line_to_node`/`footer_line_to_node` scrub (which
the doc omitted entirely) documented.

### D3. ✔ `design/README.md` undercounted the `tui/` module list

**Where:** `docs/protolens/design/README.md:64-68`

Said "eight sibling files (`navigation`, `mouse`, `override_select`,
`manage_pane`, `override_apply`, `key_dispatch`, `command_line`,
`render`)" — omitting `heat_worker`, `heat_cue`, `tiered`, `event`, and
`neovim`. The omission is not neutral: the three heat-cue modules are an
entire concurrent subsystem, and a reader orienting from this list would
not know a background thread exists. ✔ Corrected.

### D4. Patched header lines are syntax-highlighted out of context — in two places

**Where:** `protolens/src/decode.rs:798-803` and
`protolens/src/tui/override_apply.rs:1527-1532`

Both sites follow the same pattern: rewrite line 0's *text* (to patch in
a real field name over `register_wrapper`'s `"_"` placeholder, or a spec
0119 §G4 rename), then recompute that line's style hints by running
`colorize::colorize` on **that line alone**. A tree-sitter parse of one
line in isolation is not the same parse as that line within its
surrounding document — a lone `foo {` is an incomplete node — so the
patched header's highlighting can differ from what the enclosing pass
would have produced.

In practice it is one line per splice and the visible effect is small,
which is why this is filed as minor. The reason to fix it anyway is that
it is *two* copies of a workaround for a self-inflicted ordering problem.

**Proposed correction.** Patch the field name *before* highlighting, not
after. At `decode.rs:798` the patch target and replacement are both known
before the `colorize` call at `:792` (`wrapper_desc.is_some()` and the
literal `"1"`), so the fix is a reordering, not new logic. The splice site
is the same shape: compute `patched_header` first, apply it to
`new_lines[0]`, *then* call `hints_by_line`. Both special cases then
disappear, and the "highlight one line in isolation" primitive stops
existing — which also removes the only obstacle S6 in
[rendering-scaling-roadmap.md](rendering-scaling-roadmap.md) would have
had to preserve.

### D5. Spec 0162 (tree-node reclamation) is a goals-only draft with no implementation

**Where:** `docs/specs/0162-protolens-tree-node-reclamation.md`

Worth stating explicitly because the surrounding specs (0160, 0161, 0163,
0167) *are* implemented, so 0162's presence in the same numeric
neighborhood reads as "handled". It is not: nothing reclaims orphaned
arena nodes, which is why `tree.len()` grows 622 k → 1.69 M → 2.71 M over
two override commits (P3). Either implement it or mark its Status
prominently.

---

## Checked and clean

Recorded because they were plausible failure modes worth ruling out, and
a negative result is only useful if it is written down.

- **`TieredBounded` eviction cannot invalidate anything a caller holds.**
  Every public return is owned or cloned (`peek` clones the value,
  `upsert` returns an outcome enum), and the internal slot indices never
  escape the type. So the eviction that P6 proposes to stop cloning
  around is safe to make borrowing *only* if that borrow is confined to
  the accessor — which is why P6 splits `touch`/`get` rather than
  handing out a `&V` that outlives a subsequent `upsert`.
- **No `unsafe` anywhere under `protolens/src/tui/`.** The only unsafety
  reachable from the rendering and heat-cue paths is the mmap lifetime
  extension in `prototext-graph` (C3, A5). Fixing that removes `unsafe`
  from protolens's dependency surface for these paths entirely.
- **The `Mutex<HeatCaches>` is never held across a scoring sweep.** The
  worker locks to read coverage (`heat_worker.rs:343-355`), releases,
  runs `inferred_candidates` (`:377`), then re-locks to write (`:383`).
  The render thread's own scoring branch does the same
  (`heat_cue.rs:319` scores, `:325` locks). So the multi-second sweeps
  never block the other thread's cache access, and an "unsettled" row is
  a genuine cache miss rather than lock contention.
  - The render thread reaching that branch at all is separately gated on
    there being **no** worker (`heat_cue.rs:290-301`) — with a worker
    present, the render thread only ever does bounded cache reads and
    queue pushes. That gate, not the lock discipline, is what keeps a
    full sweep off the frame path.
