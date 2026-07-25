<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Pipeline: rendering

*last verified: 2026-07-25*

## Executive summary

"Rendering" in protolens is not one operation but a **five-stage
pipeline**, and almost every misunderstanding of the codebase comes from
conflating two of its stages. Stage 1 (*decode-to-text*) runs once at
startup and is **eager and whole-document**: it produces one `String` per
line of the entire blob. Stage 4 (*the per-frame draw*) is **lazy and
viewport-scoped**: it touches only the ~50 lines currently on screen.
Stages 2, 3 and 5 sit between them — a whole-document syntax-highlight
pass, the navigable arena, and the in-place re-render that an override
triggers. Stage 6 (*heat cues*) is a separate, concurrent pipeline that
annotates already-rendered lines from a background thread.

The pipeline's structural cost profile follows directly from that split:
per-frame drawing is cheap and stays cheap (measured at 0.5 ms on a
620k-node document), while *anything that has to re-establish
whole-document state* — startup, an override commit, a fold toggle — is
O(total document), not O(visible). Every scaling spec in the 0151-0167
range is an attempt to shrink the constant on one of those
whole-document steps; none of them changes the exponent, because the
exponent is set by stage 1's choice to materialize the whole document up
front.

The second thing to hold in mind is that three **independent coordinate
systems** are live at all times, and every bug in the splice machinery
has been a desynchronization between two of them:

| Coordinate | Domain | Stability |
|---|---|---|
| **Byte offset** into `App::blob` | `NodeSpan::raw_range` | *Permanent* — never changes for the life of a session |
| **Line index** into `App::lines` | `NodeSpan::text_range` | Shifts whenever any earlier subtree is re-rendered |
| **Node index** into `App::tree` | arena slot | Stable for a live node, but the array only ever grows |

Byte offsets are the only stable key, which is why the heat-cue caches
are keyed on them (stage 6) and the render cache is keyed on them
(stage 5), while `line_to_node`/`text_range` must be rebuilt after every
splice batch.

## Technical detail

### The five stages, and who calls whom

```
                 startup, once                        per keystroke
  ┌──────────────────────────────────────┐    ┌──────────────────────────┐
  │ 1. decode()          whole document  │    │ 5. splice_override()     │
  │      ↓  text: String                 │    │      one subtree         │
  │      ↓  lines: Vec<String>           │    │        ↓ LinePatch       │
  │ 2. colorize()        whole document  │    │ finalize_override_batch  │
  │      ↓  line_styles: Vec<Vec<..>>    │    │        ↓ whole document  │
  │ 3. build_tree()      whole document  │    └──────────────────────────┘
  │      ↓  tree: Vec<TreeNode>          │                 │
  └──────────────────────────────────────┘                 │
                    │                                      │
                    └──────────┬───────────────────────────┘
                               ↓
                    rebuild_visible_rows()   whole document
                               ↓
                 ┌──────────────────────────┐   ┌────────────────────────┐
                 │ 4. render()   viewport   │◄──┤ 6. heat worker thread  │
                 │   visible_rows[scroll..] │   │    byte-range keyed    │
                 └──────────────────────────┘   └────────────────────────┘
```

Stages 1-3 all live in `decode::decode` (`decode.rs:733-816`) and run
before the first frame is ever drawn. Stage 4 is `tui::render::render`
(`render.rs:230-546`). Stage 5 is `tui/override_apply.rs`. Stage 6 is
`tui/heat_worker.rs` + `tui/heat_cue.rs`.

### Stage 1: decode-to-text is eager, whole-document, and doubles the text

`decode()` takes the blob, wraps it in a synthetic single-field envelope
(see [target-blob.md](target-blob.md)), and hands the wrapped bytes to
`prototext_core::decode_and_render_indexed`, which returns **the entire
rendered document as one byte buffer** plus a flat post-order
`Vec<NodeSpan>`. Three whole-document materializations follow, in
sequence, all resident simultaneously at peak:

1. `String::from_utf8(text)` (`decode.rs:789`) — the document as one
   contiguous `String`.
2. `text.lines().map(str::to_string).collect()` (`decode.rs:791`) — a
   *second* complete copy, this time as one heap allocation per line.
   `text` is only dropped when `decode` returns, so peak text residency
   is ~2× the rendered size plus 24 bytes of `String` header per line.
3. `colorize::hints_by_line(&lines, &colorize::colorize(&text))`
   (`decode.rs:792`) — stage 2, below.

Two `DecodeRenderOpts` choices here are load-bearing and easy to
misread. `annotations: true` is unconditional (`decode.rs:775`):
annotations are a pure *display* concern, so the rendered text always
carries the full `#@ ...` suffix and the `a` key merely hides it per
line via `render::annotation_start` — no re-decode. And
`expand_any`/`expand_message_set` are both **off** (`decode.rs:783-784`),
because protolens implements that expansion itself, as ordinary
overrides, in stage 5 — see
[document-tree.md](document-tree.md)'s recursion-gate section.

The consequence is that **startup latency is proportional to total blob
size, not to visible size** — with the additional twist that the
dominant cost is not stage 1 at all (see "Measured costs" below).

### Stage 2: highlighting is a whole-document tree-sitter parse, bucketed per line

`colorize::colorize(text)` (`colorize.rs:134-163`) runs
`tree-sitter-textproto` over the *whole* rendered document and walks the
highlight-event stream into a flat `Vec<StyleHint>` of
`(byte_range, SyntaxRole)`. `hints_by_line`
(`colorize.rs:171-198`) then buckets those into one `Vec` per line, using
a `partition_point` over precomputed line starts, and **clips any hint
that crosses a newline to the line it starts on**. That clipping is why
`line_styles` can be indexed per line independently at draw time with no
cross-line context — the invariant stage 4 relies on.

Highlighting the whole document up front is what makes stage 4 cheap: the
draw does no parsing at all, only `Vec` indexing. It is also why a
*re-render* has to re-highlight: stage 5's replacement lines get their
own `hints_by_line` pass over just the new lines.

One asymmetry worth knowing: the root header line is re-highlighted **in
isolation** (`decode.rs:798-803`) after `patch_synthetic_field_name`
rewrites the wrapper's placeholder field name. A single line parsed
alone is not the same tree-sitter input as that line in document context,
so this is a small, deliberate fidelity compromise on exactly one line.

### Stage 3: the arena

`build_tree(spans)` is described in full in
[document-tree.md](document-tree.md) and not repeated here. For
rendering purposes only two facts matter: the array is in **post-order**
(so array index carries no navigational meaning), and document order is
carried by an explicit `doc_next`/`doc_prev` chain. Every rendering-side
walk — `finalize_override_batch`'s downstream correction,
`rebuild_visible_rows`, search — uses the chain or the parent/child
pointers, never array order.

`App::new` then does something that is easy to miss and expensive:
because Any/MessageSet expansion is *not* done by stage 1, it runs one
full `render_overrides(cursor)` pass over the entire document
(`tui/mod.rs:1232-1234`) just to find and expand those nodes before the
first frame. On the measured fixture this single call dominates startup.

### Stage 4: the per-frame draw is genuinely virtualized

`render()` is the one stage that already behaves the way the whole
pipeline arguably should. It:

1. Windows the visible-row list — `self.visible_rows[scroll_offset..end]`
   (`render.rs:314-315`) — where `visible_rows` is the fold-filtered list
   of line indices maintained by `navigation::rebuild_visible_rows`.
2. Resolves heat-cue displays for **only** those rows
   (`render.rs:330-333`).
3. For each row, builds `ratatui` spans from `lines[i]` +
   `line_styles[i]` (`render_line_spans`), applies horizontal pan, then
   splices in the fold marker and the heat glyph/suffix via
   `spans_with_insertions` (`render.rs:128-156`), then applies
   cursor/selection `REVERSED` styling.

Nothing here is O(document) *except* two statusline calls —
`positional_path(cursor)` and `display_range(cursor)`
(`render.rs:432-455`) — which are O(depth + sibling ordinal) and
therefore cheap for one node but, as stage 6 shows, catastrophic when
called once per visible row.

`spans_with_insertions` exists because the heat glyph and the fold marker
are *display-only* decorations that must not perturb the byte offsets
that `line_styles` uses. Inserting them as separate spans at draw time,
rather than editing `lines[i]`, is what keeps stages 1-3's coordinate
system authoritative: `lines` is always exactly what `extract.rs` would
hand back and what search matches against.

### Stage 5: the override splice, and why it costs a whole-document pass

The data model and the batch architecture are documented in
[override-collection.md](override-collection.md); the tree mutation in
[document-tree.md](document-tree.md). What matters *as rendering* is the
division of labour between three functions:

- **`splice_override(idx, target, is_preview, patch_scope)`**
  (`override_apply.rs:1335-1764`) re-wraps `idx`'s payload bytes under
  the new target and decodes them in isolation, consulting a byte-range
  keyed `RenderCache`. It appends the new nodes to the end of the arena,
  rewires pointers, and — crucially — **does not touch `lines` or
  `line_styles`**. It records a `LinePatch` instead (spec 0167) and adds
  its line-count delta to `App::pending_shift`.
- **`render_overrides_inner(idx, inherited, path, patch_scope)`**
  (`override_apply.rs:974-1107`) is the tree walk that decides *which*
  nodes need splicing. It carries the accumulated line shift down as
  `inherited`/`child_owed` so a node visited after its earlier siblings
  grew still reads a correct `text_range`, and derives each child's
  positional path incrementally (`child_path`) rather than recomputing
  it — the O(1)-per-node path derivation that replaced an O(depth ×
  ordinal) recomputation that made a full pass take minutes
  (`override_apply.rs:666-680`).
- **`finalize_override_batch(idx)`** (`override_apply.rs:1122-1166`) runs
  **once per batch** and is where the whole-document cost lives. It
  materializes all pending `LinePatch`es into `lines`/`line_styles` in a
  single O(N_final) merge (`materialize_line_patches`,
  `override_apply.rs:1183-1231`), then walks the doc-order chain forward
  from the batch root's last descendant shifting every later node's
  `text_range`, then **clears and rebuilds `line_to_node` and
  `footer_line_to_node` over the entire document**, then calls
  `rebuild_visible_rows()`.

The deferred-patch design (spec 0167) is what makes a *batch* of splices
cost one line-buffer rewrite instead of one per splice. But because a
live preview and a single override commit are each their own
one-splice batch (`override_apply.rs:1749-1751`), each of them still
pays a full `finalize_override_batch`: two whole-document map rebuilds
and one whole-document `rebuild_visible_rows`.

`rebuild_visible_rows` (`navigation.rs:37-54`) is itself
O(total lines): it allocates a fresh `vec![false; total]`, filters
`0..total`, reassigns `visible_rows`, and bumps `structural_version`
(which is stage 6's prefetch-walk invalidation signal). It is called on
every fold toggle and every preview keystroke.

Two guards in this path are enforced **only** by `debug_assert!`
(`override_apply.rs:1216`, `override_apply.rs:1261`): that patches
arrive in non-overlapping ascending order. Since this project always
builds `--release`, those assertions are compiled out, and a violated
ordering invariant surfaces as an opaque slice-index panic inside the
merge instead of a named assertion at the point of violation.

### Stage 6: heat cues are a second pipeline, keyed on bytes not indices

Heat cues annotate a rendered line with how well the bytes it describes
score against the node's *currently assigned* type versus the best
candidate type — an inference-mismatch cue. Producing one requires a
`prototext_graph` scoring sweep, far too expensive to do on the render
thread, so the work is split:

- **`heat_cue_for(line_idx)`** (`heat_cue.rs:216-228`) runs on the render
  thread, once per visible row per frame. It maps line → node via
  `line_to_node`, and calls `heat_cue_resolve`.
- **`heat_cue_resolve(idx)`** (`heat_cue.rs:234-356`) short-circuits
  immediately if `heat_states[idx].settled()`. Otherwise it computes the
  node's payload byte range, computes its current type key, calls
  `heat_lookup` (which either hits the shared cache or pushes a
  `HeatRequest` and returns `None`), then re-reads the `best` and
  `current` halves **independently** — either may already be known even
  when the all-or-nothing lookup missed, which is what makes the
  progressive `[?]` → `[?/best]` → final-cue display sequence possible.
- **`heat_worker_loop`** (`heat_worker.rs:335-412`) runs on a dedicated
  16 MiB-stack thread, pops the highest-priority request, re-checks the
  cache under the lock, runs `inferred_candidates` **with no lock held**,
  writes results back, and sends `AppEvent::HeatWorkerProgress` — except
  for `Tier::Prefetch` completions, which write silently so a read-ahead
  burst doesn't cause thousands of no-op redraws
  (`heat_worker.rs:405-410`).
- **`recheck_pending_heat_states`** (`heat_cue.rs:370-419`) runs on
  worker-progress wakeups over the `pending_heat_recheck` set only —
  deliberately *not* over `0..heat_states.len()` — and reads the cache
  directly without re-pushing requests.

The important design property is the **key discipline**: the shared
caches (`HeatCaches::by_range`, `current_score`) are keyed on the
payload's *byte offset*, which is permanent, while `heat_states` is
indexed by *node index*, which is not. That is exactly the right split —
a splice invalidates node indices but never byte ranges, so
`splice_override` only has to reset `heat_states[idx]`
(`override_apply.rs:1682-1684`) and the byte-keyed cache stays valid and
reusable. Cross-thread staleness is impossible for the same reason: the
worker only ever writes byte-keyed entries and holds `Arc<Vec<u8>>` of
the blob, never a tree index.

Both caches, and the request queue, are `TieredBounded` (spec 0164):
entries carry a `Tier` (`User` > `Visible` > `Prefetch` >
`PrefetchPrevious`), pushes are merged by key rather than duplicated, a
cache *hit* promotes the entry's tier, and eviction always prefers the
lowest tier — so a prefetch wave can never displace what the user is
looking at, and an over-capacity prefetch push is `Rejected` (which is
how `prefetch_step` learns to stop walking).

Thread lifetime is the one part of this that is not type-enforced, and
it is the pipeline's weakest joint. Both background threads hold
`graph: &'static ArchivedCompiledGraph` — but that `'static` is
fabricated by a `from_raw_parts` lifetime extension over an `Mmap`
(`prototext-graph/src/score/load.rs:82-89`), and its real validity ends
when `App::ctx` drops. The type system is not tracking this at all; two
hand-maintained mechanisms are:

- `run` joins the heat worker explicitly before returning
  (`tui/mod.rs:1698-1703`), and
- `App` declares `heat_worker` **before** `ctx`, so field drop order
  stops the thread before the mmap is unmapped (`tui/mod.rs:727-742`) —
  needed because the join is not reached on a panic-unwind path.
  Getting this order wrong was an intermittent segfault.

There is a **third** thread, and it is covered by neither: the detached,
one-shot root-type sweep spawned at `tui/mod.rs:1673-1684`. It copies
the same `&'static` graph out and is deliberately never joined, so
quitting mid-sweep unmaps the file underneath a running thread. See
[../rendering-flaws.md](../rendering-flaws.md) C3, and A5 for the `pub`
field that makes copying it out possible in the first place. That thread
exists only to run the root-type sweep *late*;
[spec 0168](../../specs/0168-protolens-resolve-root-type-before-decode.md)
proposes deleting it by running the sweep before the single decode
instead.

Independently of that, the right fix retires this whole subsection: hold
the graph as `Arc<LoadedGraph>` and the lifetime becomes real rather than
asserted, at which point neither the join nor the field order is
load-bearing.

### Measured costs

Measured 2026-07-25 via `tui::tests::profiling::
profile_override_pane_enter_on_pdb` (`--release`, single-core sandbox, so
treat absolute numbers as indicative and ratios as meaningful), against a
**1.1 MB** `FileDescriptorSet` yielding **193,072 lines / 622,922
nodes**:

| Step | Cost | Scope |
|---|---|---|
| `DescriptorContext::load` | 78 ms | pool + graph |
| `decode()` — stages 1-3 | 1.65 s | whole document |
| `App::new` — mostly the startup `render_overrides` | **5.22 s** | whole document |
| `t` (open override pane) | 533 ms | one node + candidate ranking |
| `Down` (preview next candidate) | 11 ms | one splice + finalize |
| `Enter` (commit an override) | **10.6 s** | whole document |
| second `Enter` | 8.2 s | whole document |
| **draw a frame** | **0.5 ms** | viewport |

Two things stand out. First, the per-frame draw is four orders of
magnitude cheaper than any whole-document step — stage 4 is not the
problem and never has been. Second, `App::new` costs 3× `decode()`,
i.e. **the largest single startup cost is not decoding the document but
walking it afterwards** to find Any/MessageSet nodes.

The arena also grows monotonically by design ("always append, never
renumber"): 622,922 nodes → 1,690,153 after one override → 2,709,031
after two. Two override commits nearly quintuple arena residency in a
session, and nothing reclaims the orphans (spec 0162 remains
unimplemented).

Extrapolating linearly to the 24.5 MB `googleapis.desc` fixture puts
startup in the minutes and a single override commit at a few minutes —
i.e. at that size the current pipeline is not merely slow but unusable,
and the binding constraint is whole-document work, not per-frame work.

### Invariants a reader must hold

These are the invariants the rendering pipeline depends on that are *not*
enforced by the type system, listed here because every one of them has
either already caused a bug or is one refactor away from doing so:

1. **`lines`, `line_styles` and `visible_rows` are always the same
   length as / index into one document.** Anything that changes line
   count must go through `pending_shift` + `finalize_override_batch`, or
   the three desynchronize silently.
2. **`text_range`s downstream of a splice are stale until batch end.**
   Reading any node's `text_range` between a `splice_override` and its
   `finalize_override_batch` gives a wrong answer. Inside a batch, only
   the `inherited`/`child_owed` shift bookkeeping is authoritative.
3. **`doc_next` may point *inside* `idx`'s own subtree.** Once an earlier
   splice has given `idx` descendants, `idx.doc_next` is its first child.
   Code that discards `idx`'s subtree must recompute the seam via
   `doc_next_after_subtree` *before* truncating, not null the pointer —
   see [document-tree.md](document-tree.md).
4. **The render cache is keyed on the byte range and target only**
   (`override_apply.rs:1450`), not on `initial_level`/`indent_size`. It
   is correct today only because every caller passes the same
   `indent_size` and re-indents afterwards; a future caller that doesn't
   would silently get another node's indentation.
5. **Byte ranges are permanent; node indices are stable-but-append-only;
   line indices are volatile.** Any new cache must pick its key
   accordingly. Stage 6 gets this right; anything keyed on a line index
   would not.
6. **`heat_states` is parallel to `tree` and must be resized/truncated
   with it.** `splice_override` resizes it; the live-preview watermark
   path truncates it; `recheck_pending_heat_states` additionally
   bounds-checks because a queued index can outlive a truncation.

## Cross-references

- [target-blob.md](target-blob.md) — the synthetic wrapper, payload
  extraction, `wrap_blob`.
- [document-tree.md](document-tree.md) — the arena, post-order storage,
  the document-order chain, the splice mechanic.
- [override-collection.md](override-collection.md) — the override data
  model and the render-pass/batch architecture.
- [main-pane.md](main-pane.md) — fold/unfold and highlighting from the
  pane's point of view.
- [caches.md](caches.md) — the `RenderCache`/`CandidateCache` pair.
- [override-select-pane.md](override-select-pane.md) — live preview and
  its watermark truncate/retry cycle.
- `docs/protolens/rendering-flaws.md` — current known defects in this
  pipeline.
- `docs/protolens/rendering-scaling-roadmap.md` — prioritized proposals
  for large blobs.
