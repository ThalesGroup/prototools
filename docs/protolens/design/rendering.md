<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Pipeline: rendering

*last verified: 2026-07-31*

## Executive summary

"Rendering" in protolens is not one operation but five, and almost every
misunderstanding of the codebase comes from conflating two of them:

| | when | scope |
|---|---|---|
| 1. decode-to-text | startup, once | whole document |
| 2. the arena | startup, once | whole document |
| 3. the window | every frame | viewport |
| 4. the splice | on an override | one subtree, plus one line-buffer merge |
| 5. heat cues | concurrent | one node at a time, byte-keyed |

Stage 1 is eager and materializes one `String` per line of the entire
blob; that choice is what makes startup O(total document) and it has not
changed. Everything that used to *join* it at whole-document scope has
since left: syntax highlighting moved into stage 3 (spec 0187), line
positions stopped being stored so a commit no longer walks the tail of
the document (spec 0210), and the arena became a function of the bytes
so a splice allocates nothing (spec 0216).

The second thing to hold in mind is that three **coordinate systems**
are live at all times, and every bug in the splice machinery has been a
desynchronization between two of them:

| Coordinate | Where it lives | Stability |
|---|---|---|
| **Byte offset** into `App::blob` | `NodeSpan::raw_range` | *Permanent* — never changes for the life of a session |
| **Node index** into `App::tree` | arena slot | *Permanent* — the arena is built from the bytes and never mutated |
| **Line index** into `App::lines` | derived, never stored | Volatile — shifts whenever any earlier subtree is re-rendered |

Only the line index moves, and the reason it is safe is that no node
records one. A node records the *size* of its own subtree
(`lines_total`, and `lines_visible` with folds applied); a position is
recovered on demand by `tui/lines.rs`'s root-path descent. A size does
not care what happens above it, which is why a splice owes its ancestors
a size correction and owes the rest of the document nothing.

## Technical detail

### The five stages, and who calls whom

```
                 startup, once                    on an override
  ┌──────────────────────────────────────┐  ┌──────────────────────────┐
  │ 1. decode()          whole document  │  │ 4. splice_override()     │
  │      ↓  lines: Vec<String>           │  │      one subtree         │
  │ 2. build_arena()     whole document  │  │        ↓ LinePatch       │
  │      ↓  arena: Arena  (immutable)    │  │ finalize_override_batch  │
  │      ↓  tree: Vec<TreeNode> (overlay)│  │   materialize_line_patches│
  └──────────────────────────────────────┘  └──────────────────────────┘
                    │                                    │
                    └────────────────┬───────────────────┘
                                     ↓
                 ┌──────────────────────────┐   ┌────────────────────────┐
                 │ 3. render()   viewport   │◄──┤ 5. heat worker thread  │
                 │   build_window + styles  │   │    byte-range keyed    │
                 └──────────────────────────┘   └────────────────────────┘
```

Stage 1 is `decode::render_resolved`, stage 2 `decode::
resolve_root_type_and_arena`; `main` calls them separately so it can
announce each — on a large blob both are multi-second and one message
spanning both makes whichever is running look like a hang in the other.
Stage 3 is `tui/render.rs`, stage 4 `tui/override_apply.rs`, stage 5
`tui/heat_worker.rs` + `tui/heat_cue.rs`.

### Stage 1: decode-to-text is eager, whole-document, and doubles the text

`decode()` wraps the blob in a synthetic single-field envelope (see
[target-blob.md](target-blob.md)) and hands the wrapped bytes to
`prototext_core::decode_and_render_indexed`, which returns **the entire
rendered document as one byte buffer** plus a flat `Vec<NodeSpan>`. Two
whole-document materializations follow and are both resident at peak:
`String::from_utf8(text)`, then `text.lines().map(str::to_string)` — a
second complete copy, one heap allocation per line. `text` drops only
when `decode` returns, so peak text residency is ~2x the rendered size
plus a `String` header per line.

Two `DecodeRenderOpts` choices are load-bearing and easy to misread.
`annotations: true` is unconditional: annotations are a pure *display*
concern, so the rendered text always carries the full `#@ ...` suffix
and the `a` key merely hides it per line via `render::annotation_start`
— no re-decode. And `expand_any`/`expand_message_set` are both **off**,
because protolens implements that expansion itself, as ordinary
overrides — see [document-tree.md](document-tree.md)'s recursion-gate
section.

The wrapper's placeholder field name is patched into `lines[0]` after
the fact (`patch_synthetic_field_name`). That patch is now all there is:
there is no parallel style buffer to repair, so the line simply gets
highlighted in its real context the next time it is drawn.

### Stage 2: the arena

Built by `build_arena` from the blob's bytes alone, with no schema and
no descriptor pool — described in full in
[arena-and-batch.md](arena-and-batch.md) and not repeated here. For
rendering purposes three facts matter: it is in **level order**, so
sibling and child relations are arithmetic rather than stored links; it
is **immutable**, so a node index is valid for the life of the document;
and it is a **superset** of any one rendering, so `TreeNode::
is_rendered` is what tells structure from interpretation.

`build_tree` then lays the render's spans over it as a per-slot overlay,
setting each node's `lines_total`/`lines_visible` as it goes — which is
why the row numbering is consistent with `lines` and `folded` the moment
the tree exists, with nothing to precompute.

`App::new` then does something easy to miss and expensive: because
Any/MessageSet expansion is *not* done by stage 1, it runs one full
`render_overrides(cursor)` pass over the entire document just to find
and expand those nodes before the first frame.

### Stage 3: the frame is a window, and highlighting is part of it

`render()` touches only the rows on screen. `build_window(from, count)`
resolves the first row with one root-path descent and then takes `count`
O(1) steps along the visible-line sequence (`next_visible`), so a
viewport costs one descent, not one per row. Each row is then drawn from
`lines[i]` plus this frame's `window_styles`, with horizontal pan, fold
margin and heat glyph applied.

**Highlighting is recomputed per frame, over the window only** (spec
0187). `window_styles_for` cannot simply parse the window: a window
scrolled into the middle of the document typically opens on a nested
field and contains a bare `}` with no matching `{`, which drives
tree-sitter into error recovery — and error recovery in this grammar
swallows *following* siblings, losing their captures. So the window is
first wrapped in as many synthetic `_ {` openers and `}` closers as its
own indentation implies, parsed as a complete document, and the
synthetic rows' buckets dropped.

That repair is only cheap because the rendering is deterministic in
indentation: `render_text` writes exactly `indent_size * level` spaces
via `push_indent` and emits no wrapped or continuation lines, so a
window's opening depth is readable off its first line. Change that and
this stage breaks.

The fold marker and the heat glyph are inserted as *separate spans at
draw time* rather than edited into `lines[i]`. That is what keeps stage
1's coordinate system authoritative: `lines` is always exactly what
`extract.rs` would hand back and what search matches against.

### Stage 4: the override splice

The data model and the batch architecture are in
[override-collection.md](override-collection.md); the arena side is in
[arena-and-batch.md](arena-and-batch.md). What matters *as rendering* is
the division of labour between three functions:

- **`splice_override`** re-wraps `idx`'s payload bytes under the new
  target and decodes them in isolation, consulting a byte-range keyed
  `RenderCache`. It does **not** touch `lines`: it records a `LinePatch`
  instead. It fixes its own target's line counts and carries the change
  up the ancestors as it goes, because the rest of the batch derives its
  patch positions from them.
- **`render_overrides_inner`** is the tree walk that decides *which*
  nodes need splicing. It descends only where `compute_descend_marks`
  says the rendering can change (spec 0183), and derives each child's
  positional path incrementally from the parent's plus a running ordinal
  (`child_path`) rather than recomputing it — `positional_path` is
  O(depth) hops each paying an O(k) sibling walk, which on a ~600k-node
  document with sibling groups in the hundreds made a single pass take
  minutes.
- **`finalize_override_batch`** runs **once per batch** and is now only
  the text: it merges the queued `LinePatch`es into `lines` in a single
  pass (`materialize_line_patches`), bumps `structural_version` and
  re-clamps the pan. There is nothing else left to repair.

What used to stand in the finalizer was three walks — the spliced
subtree, *every node after it* in document order, and the ancestors —
each rewriting a stored absolute line number and re-filing two line-map
entries. The second is the one that made overriding the document's first
field cost a second: 4.5 M stored positions all wrong at once. Storing
sizes instead of positions is what deleted it, and with it
`line_to_node`, `footer_line_to_node`, `visible_rows` and `hidden_mask`
(85 MB of maps on the reference corpus, repaired on every commit).

The **live preview does not splice** (spec 0185). It renders into a
read-only overlay, so a preview mutates nothing and a failed one leaves
the committed document on screen; `committed_row_of` maps display rows
around it by arithmetic.

### Stage 5: heat cues are a second pipeline, keyed on bytes not indices

Heat cues annotate a rendered line with how well the bytes it describes
score against the node's *currently assigned* type versus the best
candidate type — an inference-mismatch cue. Producing one requires a
`prototext_graph` scoring sweep, far too expensive for the render
thread, so the work is split:

- **`heat_cue_for(line)`** runs on the render thread, once per visible
  row per frame, and calls `heat_cue_resolve`.
- **`heat_cue_resolve(idx)`** short-circuits if
  `heat_states[idx].settled()`. Otherwise it computes the node's payload
  byte range and type key, calls `heat_lookup` (which either hits the
  shared cache or pushes a `HeatRequest` and returns `None`), then
  re-reads the `best` and `current` halves **independently** — either
  may be known when the all-or-nothing lookup missed, which is what
  makes the progressive `[?]` → `[?/best]` → final display possible.
- **`heat_worker_loop`** runs on a dedicated 16 MiB-stack thread, pops
  the highest-priority request, re-checks the cache under the lock, runs
  `inferred_candidates` **with no lock held**, writes results back and
  sends `AppEvent::HeatWorkerProgress` — except for `Tier::Prefetch`
  completions, which write silently so a read-ahead burst doesn't cause
  thousands of no-op redraws.
- **`recheck_pending_heat_states`** runs on worker-progress wakeups over
  the `pending_heat_recheck` set only — deliberately *not* over
  `0..heat_states.len()` — and reads the cache without re-pushing.

`heat_states` is one entry per **arena** slot, not per rendered node —
4.7M of them on the reference corpus — so its per-slot width is a fixed
cost of the document, and `HeatState` is sized accordingly (spec 0220):
three private integers, `best_score: i32`, `current: i32`,
`best_count: u32`, read and written through `new`/`best()`/`current()`
so that every call site still speaks the two independent `Option`s
above. Two things about that encoding are load-bearing rather than
tidy, and both have a test named after them:

- **`Default` is hand-written; never re-derive it.** All-zero would
  decode as "scored, best 0, current 0", so every node in a fresh
  document would report `settled()` and draw a stale cue.
- **Scores clamp to `SCORE_FLOOR` (`i32::MIN + 2`), not to `i32::MIN`,
  at both ends.** `i32::MIN` and `i32::MIN + 1` are the not-scored and
  vetoed sentinels; a score saturating onto one would read back as "not
  scored", leaving `settled()` false forever and re-scoring the node on
  every worker-progress wakeup. Saturation is otherwise display-only
  and needs an adversarial blob of hundreds of megabytes to reach — the
  ordering scores the override pane actually uses live in `heat_caches`
  and stay `i64`.

The design property that matters is the **key discipline**: the shared
caches (`HeatCaches::by_range`, `current_score`) are keyed on the
payload's *byte offset*, while `heat_states` is indexed by node index.
Cross-thread staleness is impossible for the same reason: the worker
only ever writes byte-keyed entries and holds `Arc<Vec<u8>>` of the
blob, never a tree index.

Both caches, and the request queue, are `TieredBounded` (spec 0164):
entries carry a `Tier` (`User` > `Visible` > `Prefetch` >
`PrefetchPrevious`), pushes are merged by key rather than duplicated, a
cache *hit* promotes the entry's tier, and eviction always prefers the
lowest tier — so a prefetch wave can never displace what the user is
looking at, and an over-capacity prefetch push is `Rejected` (which is
how `prefetch_step` learns to stop walking).

Thread lifetime is type-enforced (spec 0180): every thread that touches
the scoring graph holds an `Arc<LoadedGraph>` and calls `.graph()` at
the point of use, so the mmap cannot be unmapped while a scorer reads
it, whatever order anything drops in. This replaced two hand-maintained
mechanisms — an explicit join and a declaration-order-dependent drop —
and the reason it had to is worth keeping: neither covered the
**detached** root-type sweep thread, which is deliberately never joined.
A safety invariant expressed as a source-line ordering covers only the
threads that happen to have a handle to drop.

Every scoring thread also takes `SCORING_THREAD_STACK_SIZE` (16 MiB,
`protolens/src/sweep.rs`). It lives in the parent of all the spawn sites
on purpose: while it lived in `heat_worker.rs` the root-type sweep
silently ran the same deep recursion on `std`'s 2 MiB default.

### Measured costs

The per-frame draw was measured at **0.5 ms** on a 193k-line / 623k-node
document, four orders of magnitude below any whole-document step. That
ratio is the durable finding: stage 3 is not the problem and never has
been.

The whole-document numbers from the same 2026-07-25 profile are
**superseded and not reproduced here** — specs 0187, 0210 and 0216 each
removed one of the terms they were dominated by, and nobody has
re-measured since. What is known:

- Syntax highlighting was **85% of an override commit** before spec
  0187 moved it to the viewport; that spec's Background section has the
  per-phase table.
- Memory after spec 0216 is measured in
  [arena-and-batch.md](arena-and-batch.md): peak 1.66 GiB on
  `googleapis.desc`, flat across repeated override cycles.
- What remains of the per-batch transient is the render itself and the
  `RenderCache` deep clone — see
  [spec 0207](../../specs/0207-where-the-override-memory-work-stands.md),
  whose open question 2 is exactly this measurement.

### Invariants a reader must hold

Not enforced by the type system; every one has either already caused a
bug or is one refactor away from doing so.

1. **Nothing stores a line position.** A node stores `lines_total` and
   `lines_visible`; anything that changes a line count must correct its
   ancestors' counts, and only those. Reintroducing a stored position
   reintroduces the O(document) commit.
2. **A node's own line counts are stale only inside a batch.**
   `splice_override` fixes them as it goes, so by the time
   `finalize_override_batch` returns they are exact — checked over the
   whole document by `assert_line_counts_are_exact`, hung off the
   finalizer under `cfg(test)` so that *every* splice in the suite is a
   case. A count wrong by one puts every position after it out by one
   and nothing panics: the reported symptom is "the marker is on the
   closing brace".
3. **The render cache is keyed on the byte range and target only**, not
   on `initial_level`/`indent_size`. Correct today only because every
   caller passes the same `indent_size` and re-indents afterwards; a
   future caller that doesn't would silently get another node's
   indentation.
4. **Byte ranges and node indices are permanent; line indices are
   volatile.** Any new cache must pick its key accordingly. Stage 5 gets
   this right; anything keyed on a line index would not.
5. **`heat_states` is parallel to `tree`.** `recheck_pending_heat_states`
   bounds-checks because a queued index can outlive the state vector it
   was queued against.
6. **A window is highlighted inside a synthetic frame, never alone.**
   Parsing a fragment on its own is unsound in this grammar (see stage
   3). Flaw D4 recorded two "highlight a line in isolation" sites and
   spec 0187 removed both; a third would be silent, since the failure
   mode is lost captures on *following* rows rather than a panic. The
   preview's `...` truncation marker is the same hazard from the other
   direction — it is not valid prototext, so `window_text` blanks it
   before the parse.

## Cross-references

- [target-blob.md](target-blob.md) — the synthetic wrapper, payload
  extraction, `wrap_blob`.
- [arena-and-batch.md](arena-and-batch.md) — how the arena is built,
  what level order buys, the splice, measured memory.
- [document-tree.md](document-tree.md) — provenance, demotion detection,
  Any/MessageSet auto-expansion.
- [override-collection.md](override-collection.md) — the override data
  model and the render-pass/batch architecture.
- [main-pane.md](main-pane.md) — fold/unfold and highlighting from the
  pane's point of view.
- [caches.md](caches.md) — the `RenderCache`/`CandidateCache` pair.
- [override-select-pane.md](override-select-pane.md) — the live-preview
  overlay.
