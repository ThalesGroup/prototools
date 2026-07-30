<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# protolens rendering — large-blob scaling roadmap

*last verified: 2026-07-25*

## Executive summary

Target sizes today are `FileDescriptorSet`s of **1 MB to 30 MB**
(`/tmp/pdb.desc` at 1.1 MB, `googleapis.desc` at 24.5 MB). Measured
against the 1.1 MB fixture (`--release`, single-core sandbox — treat
ratios as meaningful, absolute numbers as indicative):

| Step | Cost | Scope |
|---|---|---|
| `DescriptorContext::load` | 78 ms | pool + graph |
| `decode()` | 1.65 s | whole document |
| `App::new` | **5.22 s** | whole document |
| `t` (open override pane) | 533 ms | one node |
| `Down` (preview candidate) | 11 ms | one splice + finalize |
| `Enter` (commit override) | **10.6 s** | whole document |
| **draw a frame** | **0.5 ms** | viewport |

193,072 lines / 622,922 nodes, growing to 2,709,031 nodes after two
override commits.

Three conclusions frame everything below.

**First, the per-frame draw is already correct and is not the problem.**
At 0.5 ms it is four orders of magnitude below any whole-document step.
Nothing in this roadmap needs to touch `render.rs`. The instinct to
"virtualize rendering" has already been satisfied where it matters.

**Second, the largest single cost is not decoding — it is walking the
document after decoding it.** `App::new` costs 3× `decode()`, and on a
blob with no `Any` and no `MessageSet` (the normal case for a
`FileDescriptorSet`) that entire 5.22 s produces no change to the output
at all. This is the cheapest large win available and it is purely local.

**Third, the root cause the incremental specs have been circling is
real, but it has been mislocated.** Specs 0160/0161/0162/0163/0167 each
shaved a constant off a whole-document step; spec 0160 N2 and 0167 N2 both
explicitly declined viewport-scoped rendering on the grounds that
folding, navigation, search and export need whole-document state. That
reasoning is correct about the ***tree*** and incorrect about the
***text***. Folding needs subtree extents; navigation needs the
`doc_next` chain; export needs bytes. None of those needs `lines[i]` to
exist for an `i` nobody is looking at. Separating "the tree is eager" from
"the text is lazy" is the structural fix, and it is the only proposal here
that would obsolete rather than tune the 0160-0167 series.

Items are ordered by (impact ÷ risk). S1-S5, S10 (1)-(2) and S11 are
**safe, local, do these**. S6-S7 and S10 (3) are **moderate, worth
scheduling**. S8-S9 **require rearchitecting a stated assumption** and
are worth it only under the conditions named. S10 and S11 are numbered
last because they were added after the first three conclusions were
drawn, not because they rank last — S10's first two parts and S11 both
belong near the top of the sequencing.

A fourth conclusion, added by the heat-cue audit: **the one cache in the
stack that is not byte-bounded is the one holding unbounded values.**
`render_cache.rs` and `CandidateCache` both bound by bytes;
`TieredBounded` bounds by entry count, and its entries grew a
`Vec<(String, i64)>` two specs after that bound's "well under 1MB"
justification was written. See S10.

A fifth: **the table's most expensive row is not an interaction, it is
part of startup.** The 10.6 s `Enter` figure is the root-override commit
— and startup performs exactly that commit unconditionally, from a
detached thread, moments after `App::new` returns. So the real
time-to-first-correct-render on the 1.1 MB fixture is not ~7 s but ~17 s,
and the document is decoded and rendered twice to get there. See S11 and
[spec 0168](../specs/0168-protolens-resolve-root-type-before-decode.md).

A sixth, added on 2026-07-25 once 24.5 MB was confirmed as a hard
requirement rather than an aspiration: **the whole-document `Vec<String>`
is not the binding constraint at 24.5 MB — the arena is.** `TreeNode` is
280 bytes, so the fresh arena for `googleapis.desc` is ~3.9 GB against
~275 MB of text, and ~16.9 GB after two override commits. S8 makes the
text lazy and deliberately keeps the tree eager; it therefore removes the
smaller half. S12 shrinks `TreeNode` to ~72 bytes by narrowing indices and
interning FQDNs, and S12 + S7 + S8 together — not S8 alone — are what the
24.5 MB target requires. See S12.

---

## S1. Replace the startup full-document walk with a seeded candidate list

**Fixes:** ~5.2 s of a ~7 s startup on the 1.1 MB fixture; the dominant
startup cost at every size. On the 24.5 MB fixture, extrapolating
linearly, on the order of two minutes.

**What's wrong.** `decode()` disables `prototext_core`'s Any/MessageSet
expansion because protolens implements it as ordinary overrides
(`decode.rs:783-784`). Nothing has expanded them when `App::new`
finishes, so `App::new` walks the **entire** document via
`render_overrides(cursor)` (`tui/mod.rs:1232-1234`) purely to find them.

**Implementation shape.** `is_auto_expand_candidate`
(`tui/override_apply.rs:522-542`) is a local predicate: it reads the
node's own `field_number` plus its parent's (for MessageSet, its
grandparent's) `span.type_fqdn`, plus one `ctx.pool()` lookup on the
MessageSet branch (`is_message_set_typed`, `override_apply.rs:449-463`).
It needs no line state, no override state and no descendants — only
ancestry and the pool, both of which `build_tree` already has in hand as
it goes. Spec 0120 deliberately kept it narrow ("matches only the two
known Any/MessageSet field shapes", after an earlier bug where widening
it demoted ordinary string fields to raw dumps). So the candidate set is
knowable without a second walk:

1. `build_tree` (`decode.rs:301-369`) already visits every span exactly
   once. Have it also collect
   `auto_expand_seeds: Vec<usize>` — the nodes satisfying that predicate
   — and return it on `Decoded`.
2. In `App::new`, replace `render_overrides(cursor)` with a loop over the
   seeds. Wrap the loop in one explicit batch (increment
   `override_batch_depth`, run all seeds, decrement, finalize once) so
   `k` seeds cost one finalization rather than `k`.
3. Completeness argument, worth writing into the code as a comment: at
   `App::new` time the only override entry that can exist is the seeded
   root type, and `mod.rs:1223` has already reconciled the root's
   `rendered_as` against it. So no node other than an auto-expand
   candidate can need resettling on this first pass — the seeded set is
   provably the full set the current walk would splice.

**Invariant touched.** None of the three coordinate systems.
`Decoded`/`build_tree` gain one output field. The one behavioral
difference is batch shape (one batch over `k` roots instead of one batch
over the document root), which `finalize_override_batch` already supports
— but note it takes a single origin `idx` for its downstream walk, so the
batch must finalize from the *earliest* seed in document order, not the
last one processed. Getting that backwards would leave every later node's
`text_range` stale (see A3 in
[rendering-flaws.md](rendering-flaws.md)).

**Risk: low.** Directly testable without measuring anything: assert the
seed list equals the set of nodes today's full walk actually splices, on
the existing Any/MessageSet fixtures.

---

## S2. Make `finalize_override_batch`'s line-map rebuild incremental

**Fixes:** two of the three whole-document passes inside every batch
finalization — so it hits override commits (10.6 s), previews (paid per
highlighted candidate), and fold toggles.

**What's wrong.** `finalize_override_batch` clears
`line_to_node`/`footer_line_to_node` entirely and rebuilds them by
walking the whole `doc_next` chain from the first node
(`override_apply.rs:1144-1160`). Spec 0163 N3 already named this as
deferred work.

**Implementation shape.** Every entry whose line index is *before* the
batch's earliest patch is unchanged by construction — line indices only
shift at or after a patch's `global_start`. So:

1. Track `min_patch_global_start` across the batch (it is already
   computed implicitly by `materialize_line_patches`' merge).
2. `retain(|&line, _| line < min_patch_global_start)` on both maps.
3. Walk `doc_next` forward from the batch origin's own subtree seam
   (`doc_next_after_subtree`) — the same walk step 2 of the finalization
   already performs for `text_range` shifting — inserting map entries as
   it goes, and additionally insert entries for the newly spliced subtree
   itself.

This makes the map rebuild O(nodes after the splice point) instead of
O(all nodes), and — importantly — it *merges into the walk step 2 already
does*, so it costs no additional traversal.

**Invariant touched.** The maps' totality: today they are rebuilt from
scratch, so a bug elsewhere is self-healing on the next batch. After this
change a missed insert persists. Mitigate with a debug-only (or
test-only) full-rebuild-and-compare assertion, and note this interacts
with the preview truncation's `retain(|_, idx| *idx < watermark)` scrub
(`override_select.rs:813-814`), which must keep working — it removes
entries by *node* index while this change filters by *line* index, so the
two are independent but both must run.

**Risk: low-medium.** Local, but the maps feed `heat_cue_for`'s
line→node lookup, and a stale entry there is exactly the 2026-07-24
out-of-bounds crash.

---

## S3. Make `rebuild_visible_rows` incremental

**Fixes:** an O(total lines) allocation + filter on every fold toggle,
every preview keystroke, and every batch finalization.

**What's wrong.** `rebuild_visible_rows` (`navigation.rs:37-54`)
allocates a fresh `vec![false; total_lines]`, filters `0..total_lines`
into a new `visible_rows`, and bumps `structural_version`. For 193 k
lines that is a 193 kB allocation and a 193 k-iteration filter, per
keystroke.

**Implementation shape.** A fold toggle changes visibility only *within*
the toggled node's `text_range`; a splice changes it only at or after the
patch point. So expose two entry points:

- `rebuild_visible_rows_from(line: usize)` — retain the
  `visible_rows` prefix `< line` (a `binary_search` + `truncate`, since
  `visible_rows` is sorted ascending by construction) and re-filter only
  from `line` onward.
- Fold toggle passes the node's `text_range.start`; batch finalization
  passes `min_patch_global_start` (already needed by S2).

Reuse the hidden-mask buffer across calls instead of reallocating it —
keep it as an `App` field sized to `lines.len()` and `resize` rather than
`vec![]`.

**Invariant touched.** `visible_rows` must remain sorted ascending — true
today by construction, and the prefix-retain relies on it. Assert it in
tests. `structural_version` must still bump on every call, since it is
the heat prefetch walk's invalidation signal (`mod.rs:1336`); a partial
rebuild is still a structural change.

**Risk: low.** Self-contained in `navigation.rs` with a trivial
equivalence test (incremental result == full rebuild result) over the
existing fold fixtures.

---

## S4. Share the blob and stop double-materializing the text

**Fixes:** ~3× blob residency (74 MB on the 24.5 MB fixture) and ~2×
peak rendered-text residency at startup.

**What's wrong.** Detailed as P4 in [rendering-flaws.md](rendering-flaws.md):
`App::blob: Vec<u8>` plus `Arc::new(app.blob.clone())` for the heat
worker (`mod.rs:1661`) plus `app.blob[wrapper_offset..].to_vec()` for the
root-type thread (`mod.rs:1679`); and `decode()` holding `text: String`
and `lines: Vec<String>` simultaneously (`decode.rs:789-791`).

**Implementation shape.**

1. `App::blob: Arc<Vec<u8>>`. Hand `Arc::clone` to the worker; hand the
   `Arc` + `wrapper_offset` to the root-type thread and let it slice.
   Every existing `&self.blob` use site is unaffected through `Deref`.
2. Split `decode_and_render_indexed`'s returned `Vec<u8>` on `b'\n'`
   directly into `lines`, without the intermediate contiguous `String`.

**Invariant touched.** (1) touches nothing. (2) touches invariant 1 in
[design/rendering.md](design/rendering.md) — `lines.len()` must be
*exactly* what `str::lines()` produced, or `lines`, `line_styles` and
every `text_range` desynchronize by one for the rest of the session. Two
specific traps: `str::lines()` strips a trailing `\r`, and a buffer
ending in `\n` yields no trailing empty element from `lines()` but does
from a naive `split(b'\n')`. Match both explicitly and assert
`lines.len()` against a `bytecount`-of-newlines in tests.

Note that step 2 only removes the *duplication*; the contiguous buffer is
still needed by `colorize::colorize` (see S6), so realizing the full
saving depends on that item.

**Risk:** (1) low, mechanical, compiler-checked. (2) low-but-sharp —
small diff, catastrophic-and-silent if the line-count edge cases are
wrong. Land it with the line-count assertion, not without.

---

## S5. Cache the per-node type key instead of recomputing it per frame

**Fixes:** P2's per-visible-row-per-frame O(sibling ordinal) path walk
plus three linear override-list scans — i.e. frame cost during the
progressive heat-cue window, which is exactly the post-startup and
post-scroll period.

**What's wrong.** `heat_cue_resolve` → `current_type_key` →
`resolve_active_override` → `positional_path` → `sibling_position`, where
`sibling_position` (`navigation.rs:425-433`) walks `prev_sibling` one
node at a time. A `FileDescriptorSet`'s repeated `field`/`message_type`
runs put tens of thousands of siblings in that walk, once per level, once
per unsettled visible row, per frame.

**Implementation shape.** Add `type_key_cache: Vec<Option<String>>`
parallel to `tree`, plus a `type_key_cache_version: (u64, u64)` stamped
with `(structural_version, overrides_version)`. `current_type_key` checks
the stamp, clears on mismatch, and memoizes.
`structural_version` already exists and already bumps on exactly the
right events (`navigation.rs:53`); `overrides_version` is a counter
incremented by every `OverrideCollection` mutation and is trivial to add.

Separately (and independently landable), replace the three linear scans in
`resolve_active_override_entry_index_by_path`
(`override_apply.rs:682`/`:690`/`:698`) with three `HashMap`s keyed by
origin, rebuilt on `overrides_version` bump.

**Invariant touched.** Introduces a new cache with a new invalidation
obligation — the thing this codebase has historically got wrong. Mitigate
by deriving the stamp from counters that already exist and are already
bumped centrally, rather than inventing new invalidation points. Do *not*
take the alternative route of storing `ordinal` on `TreeNode` first: that
adds a maintenance obligation inside `splice_override`'s packed-run
absorption (`override_apply.rs:1705-1715`), which is the highest-risk
code in the pipeline.

**Risk: low.** Purely additive; worst case on a stale stamp is a
recompute, not a wrong answer, provided the stamp comparison is
conservative (clear on *any* mismatch).

---

## S6. Highlight lazily, per line, on first draw

> **Superseded by [spec 0187](../specs/0187-highlighting-is-a-property-of-the-viewport.md).**
> Two corrections, both established by measurement or by reading the
> grammar, not by argument:
>
> - The "Caveat — measure first" below has been answered. One
>   whole-document parse is **85% of an override commit** (12.98 s of
>   14.90 s on a 1.1 M-line document) and is the largest single cost in
>   protolens. Batching is not even cheaper *per line*: 12.2 µs/line for
>   one 1,067,034-line parse against 5.4 µs/line spread over 465 smaller
>   parses. The only lever is parsing fewer lines.
> - The soundness argument below — that `hints_by_line`'s newline
>   clipping makes a per-line parse equivalent to a document parse — is
>   **wrong**. Clipping constrains the *output*; it says nothing about
>   the parse seeing the right *context*. A window that starts mid-message
>   or contains an unmatched `}` drives tree-sitter into error recovery,
>   which this repo already has a regression test showing will swallow
>   following siblings. This is the same effect [D4](rendering-flaws.md)
>   documents.
>
> Spec 0187 therefore highlights the **window**, not the line, and wraps
> it in a synthetic enclosing context reconstructed from indentation, so
> the parse is complete. It also deletes `line_styles` outright rather
> than making it `Option`-valued, which the per-line shape below could
> not do.

**Fixes:** the whole-document tree-sitter parse at startup, and the
`Vec<Vec<(Range, SyntaxRole)>>` residency (193 k inner `Vec`s = ~4.6 MB
of headers alone before content).

**What's wrong.** `colorize::colorize` parses the *entire* rendered
document (`colorize.rs:134-163`) and `hints_by_line` buckets the result
per line (`colorize.rs:171-198`), all before the first frame — even
though `render.rs` reads `line_styles[i]` for ~50 values of `i` per
frame.

**Implementation shape.** `hints_by_line` already **clips any hint that
crosses a newline to the line it starts on**, which means the per-line
style vectors are, by the highlighter's own construction, independent of
each other. That is the property that makes laziness sound here and it is
already relied upon by the draw. So:

- Change `line_styles` from `Vec<Vec<..>>` to
  `Vec<Option<Vec<..>>>` (or a byte-bounded MRU keyed by line index,
  mirroring the existing `RenderCache`/`CandidateCache` shape — see
  [design/caches.md](design/caches.md)).
- `render.rs` fills a `None` slot on demand by highlighting that line.
- Splices invalidate by setting the patched range's slots to `None`
  instead of recomputing hints for them.

Because of the clipping, single-line highlighting *is* the semantics the
current code already produces — with one exception that S6 actually
*improves*: the root header line is today highlighted in isolation as a
special case (`decode.rs:798-803`, D4 in
[rendering-flaws.md](rendering-flaws.md)); under S6 it stops being
special.

**Caveat — measure first.** Whether a per-line tree-sitter parse is
cheaper *in aggregate* than one whole-document parse is not obvious:
tree-sitter's per-parse setup is not free, and 50 parses per frame during
fast scrolling could be worse than one parse at startup. Batch by
viewport (parse the window's lines as one joined chunk, which the
clipping rule makes equivalent) and measure both before committing.

**Invariant touched.** "the whole document has per-line styles" — read in
exactly two places (`render.rs:16` in `annotation_start` and
`render.rs:82` in `render_line_spans`, both already viewport-scoped) and
written in two (`decode.rs:792`, and the splice's patch path via
`override_apply.rs:1517`/`:1230`). Four narrow sites. This is the
*smaller half* of the
"whole document is materialized" assumption and is a good rehearsal for
S8.

**Risk: medium.** Correctness risk is low (the clipping rule makes it
sound); the risk is that it does not pay off, so gate it on measurement.

---

## S7. Implement spec 0162 (arena reclamation)

**Fixes:** monotonic arena growth — 622,922 → 1,690,153 → 2,709,031 nodes
over two override commits, none of it reclaimed. This is a *session
lifetime* memory problem, not a latency one, and it compounds with S8's
value (a smaller arena makes every whole-document walk cheaper too).

**What's wrong.** "Always append, never renumber or compact" is the
discipline that keeps every live index valid across a splice
([design/document-tree.md](design/document-tree.md)), and it is the right
call. But it was accepted as "a cheap, session-scoped cost", and at 30 MB
with a few overrides applied it is neither cheap nor bounded. Spec 0162
scoped the fix and was never implemented.

**Implementation shape.** Deliberately not restated here — read spec 0162
first. The one observation to add from this review is that the preview
path already implements a *special case* of reclamation
(`preview_tree_watermark` truncate-and-retry, spec 0161), and its bug
history is instructive: two of the three known truncation bugs
(`doc_next` cycle, `line_to_node` staleness) were about pointers *into*
the reclaimed range from outside it, and C1 in
[rendering-flaws.md](rendering-flaws.md) is a third one still open. Any
general reclamation must start from an explicit enumeration of every
index-holding structure — `cursor`, `folded`, jumplist, override-entry
origins, `line_to_node`, `footer_line_to_node`, `heat_states`,
`pending_heat_recheck`, `prefetch_walk`, `override_target`,
`preview_tree_watermark` — rather than from the arena.

**Invariant touched.** The central one: "a node's array index is stable
for the life of the session". Any compaction breaks it for orphans and
must prove it never breaks it for live nodes.

**Risk: high.** This is the item most likely to produce a subtle,
intermittent, index-corruption bug. Prefer a *conservative* variant
first: reclaim only when nothing can hold a stale index — e.g. truncate
trailing orphans when the arena's tail is entirely orphaned, which
requires no renumbering at all and captures the preview case generally.

---

## S8. Make the tree eager and the text lazy

**Fixes:** the root cause. Startup becomes O(structure) instead of
O(structure + rendered text + highlight); an override commit stops
rewriting a 193 k-element `Vec<String>`; memory stops being dominated by
per-line `String` allocations.

**This is the item that would obsolete rather than tune specs
0160/0163/0167.** Those specs exist to make whole-document line-buffer
maintenance affordable; if the line buffer is not whole-document, the
maintenance largely disappears.

**Why the earlier rejection was too broad.** Spec 0160 N2 and spec 0167
N2 both declined viewport-scoped rendering because "folding, navigation,
search and export depend on global document structure". Taking those one
at a time against what they actually read:

| Consumer | Actually needs | Needs `lines[i]` for unviewed `i`? |
|---|---|---|
| Folding | subtree line extents (`text_range`) | **No** — extents come from the tree |
| Navigation | the `doc_next`/`doc_prev` chain | **No** |
| `extract.rs` (export) | blob bytes + `text_range` | **No** for binary; yes for text export |
| `max_visible_line_len` | viewport rows only (`navigation.rs:210-219`) | **No** — already viewport-scoped |
| Statusline | `positional_path`, `display_range` | **No** |
| Search (`jump_to_match`) | `self.lines[node.text_range.start]` per node | **Yes** |

So the objection reduces to exactly two consumers — search, and text
export — and neither needs the text to be *resident*, only *obtainable*.

**Implementation shape.** Keep stages 1's *span* output and stage 3
entirely eager: `prototext_core` continues to produce the full
`Vec<NodeSpan>`, and `build_tree` continues to build the whole arena.
Change only *text materialization*:

- Replace `lines: Vec<String>` with a chunked store: the document is
  divided into fixed-size line windows (say 4096 lines), each window
  either materialized or absent, held in a byte-bounded MRU exactly like
  the existing `RenderCache` ([design/caches.md](design/caches.md)).
- A window is materialized by re-rendering the byte range its spans
  cover — which is the *same operation `splice_override` already
  performs*, via the same `wrap_blob` + `decode_and_render_indexed` +
  render-cache path. No new rendering primitive is needed; this is the
  synthetic-wrapper trick applied at window granularity.
- `render.rs` faults in the windows its viewport touches. Search faults
  in windows as it walks, and discards them behind itself (a search is
  already a full-document operation the user waits for).
- Overrides no longer splice into a global line buffer at all: they
  invalidate the affected windows. `LinePatch`,
  `materialize_line_patches`, `pending_shift`, and the batch-end
  downstream `text_range` correction walk all shrink dramatically or
  disappear — the line-count delta still has to propagate to `text_range`s,
  but it no longer has to move any text.

**Invariants this breaks.** Named explicitly, because they are assumed
widely:

1. **`self.lines: Vec<String>` is whole-document and directly
   indexable.** Every `self.lines[i]` becomes a faulting accessor. Call
   sites are concentrated (`render.rs`, `jump_to_match`, `extract.rs`,
   `override_apply.rs`'s patch machinery) but there are enough that this
   should be a compiler-enforced type change, not a convention.
2. **`lines.len()` is the document's line count.** Must become a stored
   counter maintained separately, since no window need be resident.
3. **`line_styles` is whole-document** — subsumed; do S6 first, as the
   rehearsal.
4. **`line_to_node`/`footer_line_to_node` are whole-document maps.**
   Would become per-window, built when a window is faulted in. This is
   also what makes S2 unnecessary rather than merely faster.
5. **Text export reads resident text.** Must fault windows in, which is
   fine (export is not interactive) but must be explicit.

**Preserves the core promise.** Worth stating plainly, since this is the
proposal most likely to look like it threatens it: laziness changes *when*
a byte range's text exists, never *whether* it can. Any window is
materializable on demand from the blob plus the tree, by exactly the
mechanism overrides already use, and schema knowledge still only ever
improves a window's rendering.

**Risk: high, but tractable — and it is the only proposal that changes
the exponent.** Sequence it as S6 (lazy styles, small blast radius) →
window store behind a faulting accessor with *every* window pre-faulted
(a pure refactor, no behavior change, fully testable against current
output) → actually allow windows to be absent. The middle step is the
one that de-risks the whole thing: it converts the assumption into a type
before changing any behavior.

**Necessary but not sufficient.** ~~Only worth it if the 24.5 MB class of
input is a real target~~ — confirmed real on 2026-07-25, so this item is
in scope. At 1 MB, S1-S5 alone should bring startup from ~7 s to well
under 2 s and an override commit from 10.6 s into the low seconds, which
may be enough. At 24.5 MB, S1-S5 leave startup at tens of seconds and
commits at minutes — arithmetic that no constant factor fixes. But note
what this item *keeps* eager: the `Vec<NodeSpan>` and the arena. At
24.5 MB those are ~3.9 GB against the ~275 MB of text this item makes
lazy, so S8 landed alone would still not open `googleapis.desc`. Pair it
with **S12** (per-node size) and **S7** (node count).

---

## S9. Bound the initial decode by a horizon (rejected as a shortcut)

Recorded to be dismissed, because it is the obvious cheap version of S8
and it is a trap.

The tempting shortcut is: decode only the first N bytes at startup, show
that, and extend on demand. It appears to give S8's startup win for a
fraction of the work. It does not, because it breaks the one thing S8
carefully preserves: **the tree would no longer be complete**. Every
consumer that walks the document — `finalize_override_batch`'s downstream
correction, `rebuild_visible_rows`, search, the heat prefetch walk, the
statusline's `positional_path` — would need a "and the rest is not
decoded yet" case, and the fold/navigation invariants would become
horizon-dependent. That is a far larger and far more invasive change than
S8's, which keeps the tree whole and touches only text residency.

If startup must be cut *before* S8 is affordable, the right lever is S1
(which removes ~75% of it for almost no risk), not a horizon.

---

## S10. Bound the heat cache by bytes, and stop cloning it per frame

**Fixes:** [rendering-flaws.md](rendering-flaws.md) P5, P6, and the
`covers_window` half of A4.

**Shape.** Three independent changes, in increasing order of size:

1. Clamp the worker's `top_n_len` (`heat_worker.rs:384-388`) to the same
   `max(override_list_height, HEAT_CUE_PREVIEW)` bound the synchronous
   path uses (`heat_cue.rs:330`), so `req.end = usize::MAX` from
   `upgrade_active_override_to_complete` can no longer size a cache
   entry. The full list still lands, in the `complete` slot that exists
   for it.
2. Split `TieredBounded::peek` into `touch(&mut self, key, tier)` and
   `get(&self, key) -> Option<&V>`, so `HeatCaches::window` clones only
   the `[start..end)` slice it returns rather than the whole
   `RangeHeatEntry`. Confined to `tiered.rs` plus its two callers.
3. Change `TieredBounded`'s capacity from an entry count to a byte
   budget, matching `render_cache.rs` and `CandidateCache`
   ([design/caches.md](design/caches.md)) — the only one of the four
   caches that is *not* byte-bounded is the one holding the unbounded
   values.

**Invariant touched.** None of the three coordinate systems. (2) removes
a `Clone` bound on `V` only if all callers migrate; keep `peek` for any
that don't. (3) needs a size estimator per value type — `RangeHeatEntry`
is `top_n.iter().map(|(s, _)| s.len() + 24).sum()` plus a header
constant, the same shape `render_cache.rs` already uses.

**Scale.** (1) alone turns a ~1 MB-per-entry worst case into a
~10 KB one, and with it (2)'s per-row-per-frame cost. (3) is the durable
fix and can wait for measurement.

**Risk.** (1) is three lines and directly testable (assert `top_n.len()`
never exceeds the cap after a `usize::MAX` request). (2) is mechanical.
(3) is a contained rewrite of one type, but it is the type spec 0164's
tier semantics live in, so its existing tests are the acceptance
criterion.

**Rider.** The same "share, don't copy" reasoning as S4 (1) applies to
`DescriptorContext.graph`: making it `Arc<LoadedGraph>` removes the last
`&'static`-over-mmap in the stack and closes
[rendering-flaws.md](rendering-flaws.md) C3/A5. It is a correctness fix
rather than a scaling one, but it touches the same lines as S4 (1) and
should be done in the same pass.

---

## S11. Resolve the root type before decoding, not after

**Fixes:** the ~10.6 s root-override splice that follows every startup on
the 1.1 MB fixture — a second full decode and re-render of the whole
document, on top of the ~1.65 s `decode()` and ~5.2 s `App::new` that S1
addresses. Also deletes [rendering-flaws.md](rendering-flaws.md) C3
outright.

**What's wrong.** Startup renders the document twice. `decode()`'s
`defer_root_type` flag (`decode.rs:733-752`) skips `determine_root_type`,
so the first render is raw; a detached thread (`tui/mod.rs:1673-1684`)
then runs the sweep and posts `RootTypeResolved`, whose handler applies a
root override — which re-decodes and re-renders everything. The
asynchrony was introduced to fix a symptom (a black screen while the
sweep ran) and paid for it with a second full render.

Worse, the sweep itself is duplicated: `resolve_root_winner_fqdn`
(`decode.rs:188-199`) and `override_pane::inferred_candidates`
(`override_pane.rs:56-73`) are the same `score_all` call with the same
comparator over the same bytes, and the resolver's result is written
nowhere — so opening the override pane on the root re-pays for it.

**Implementation shape.** See
[spec 0168](../specs/0168-protolens-resolve-root-type-before-decode.md)
for the full design. In outline: run the sweep on the main thread before
`decode()`, behind a progress frame; pass the winner straight into
`decode()`; delete `defer_root_type`, `root_type_deferred`,
`root_type_pending`, `AppEvent::RootTypeResolved`, `apply_resolved_root_type`
and the spawn; and seed `HeatCaches` with the sweep's candidate list so
the pane's first open on the root is a hit.

**Invariant touched.** None of the three coordinate systems — this is
strictly a removal of a splice, not a change to one. The winner rule is
byte-identical; only *when* it is known changes. The user-visible
difference is that the first frame drawn is already correctly typed,
rather than raw-then-corrected.

**Scale.** Removes one of the two full decodes at every blob size, so it
scales exactly as the document does — and it composes with S1 rather than
overlapping it (S1 removes a walk, this removes a decode).

**Risk: low-to-moderate**, and gated on one measurement the spec calls
out: the sweep must be materially cheaper than the splice it replaces
before the change is worth making. The margin is expected to be very
wide, but it has not been measured. If it somehow is not, the fallback is
to keep the sweep asynchronous and have `RootTypeResolved` trigger a
re-decode rather than a root splice, since the splice is the expensive
half.

---

## S12. Shrink the arena — S8 makes the text lazy, but the tree is the larger half

**Fixes:** the memory ceiling that S8 does *not* touch. S8's own subtitle
is "make the tree **eager** and the text lazy", and it keeps
`Vec<NodeSpan>` and `Vec<TreeNode>` whole-document by design. At 24.5 MB
that eager half is an order of magnitude larger than the lazy one, so S8
alone does not deliver the 24.5 MB capability it was written to deliver.

**The arithmetic.** `NodeSpan` (`prototext-core/src/serialize/render_text/
sink.rs:960-1034`) and `TreeNode` (`protolens/src/decode.rs:269-289`) are
plain `repr(Rust)` aggregates; compiling their field lists standalone
gives:

| Type | `size_of` when this was written | today |
|---|---|---|
| `NodeSpan` | **120 B** | **32 B** (spec 0212) |
| `TreeNode` (= `NodeSpan` + 7 × `Option<usize>` + `rendered_as`) | **280 B** | **76 B** (specs 0211, 0212, 0213) |

Everything below in this section is the arithmetic as it stood before any
of those specs landed; it is kept because the *ratios* are what motivated
the work. Only a hot/cold column split remains, and it saves no bytes — so
76 B is where the slot stops.

Applied to the measured node counts (S7's figures, same 1.1 MB fixture),
and extrapolated at the observed density of 0.566 nodes/byte:

| | 1.1 MB fixture | 24.5 MB (`googleapis.desc`) |
|---|---|---|
| nodes, fresh decode | 622,922 | ~13.9 M |
| arena, fresh | 174 MB | **~3.9 GB** |
| nodes, after two override commits | 2,709,031 | ~60 M |
| arena, after two commits | 758 MB | **~16.9 GB** |
| rendered lines | 193,072 | ~4.3 M |
| `Vec<String>` for those lines (headers + text) | ~13 MB | **~275 MB** |

The last two rows are what S8 removes. The rows above them are what it
leaves in place, and they are **14×** larger. Excluding the `String` heap
behind `type_fqdn` and `natural_annotation`, which is on top of all of it.

**There is also a transient peak.** `build_tree`
(`protolens/src/decode.rs:301-315`) is
`spans.into_iter().map(|span| TreeNode { .. }).collect()`. The element
sizes differ (120 → 280), so the allocation cannot be reused: `collect`
allocates the full `Vec<TreeNode>` up front (the size hint is exact) while
the source `Vec<NodeSpan>` stays alive until the iterator drops. Peak at
24.5 MB is ~3.9 GB + ~1.7 GB = **~5.5 GB**, for a document whose steady
state is 3.9 GB.

**What is *not* the problem.** The append-only discipline and the
whole-document tree are both correct — see S7 and
[design/document-tree.md](design/document-tree.md). Nothing here proposes
making the tree lazy; navigation, folding and `doc_next` genuinely need
it resident. The problem is only that each resident node costs 280 bytes
when it could cost roughly a quarter of that.

**Implementation shape.** Per-field, all mechanical, all independently
landable:

| Field | Now | Proposed | Saves |
|---|---|---|---|
| ~~7 × link (`parent`…`doc_prev`)~~ | ~~`Option<usize>` = 16 B ea~~ | ✔ **done 2026-07-29** ([spec 0211](../specs/0211-the-arenas-links-are-half-as-wide.md)) — `type NodeIdx = u32` + `NO_NODE` | 84 B, **banked** |
| ~~`rendered_as`~~ | ~~`Option<(Option<Option<String>>, String)>` = 48 B~~ | ✔ **done 2026-07-30** ([spec 0213](../specs/0213-the-provenance-is-one-word.md)) — ~~side `HashMap<u32, _>`~~ → one interned `ProvenanceId` for the *whole pair*, not one per half; `design/arena-and-batch.md` trap 1 has why the side table was rejected and why the halves could not be interned separately | 44 B + up to two heap allocs per spliced node, **banked** |
| ~~`field_number`~~ | ~~`u64`~~ | ✔ **done 2026-07-30** ([spec 0212](../specs/0212-the-span-is-a-third-as-wide.md)) — `u32`; the wire format bounds it at 2²⁹ − 1, so no saturation was needed | 4 B, **banked** |
| ~~`raw_range`, `text_range`~~ | ~~`Range<usize>` = 16 B ea~~ | ✔ **done 2026-07-30** (spec 0212) — `Range<u32>` both. `text_range` was *not* deleted: spec 0210's "no production reader" applies to the arena's stale copy, not to the flat list the library returns | 16 B, **banked** |
| ~~`level`~~ | ~~`usize`~~ | ✔ **done 2026-07-30** (spec 0212) — `u16`; `MAX_WIRE_DEPTH` = 1000 is enforced at decode | 6 B, **banked** |
| ~~`type_fqdn`~~ | ~~`Option<String>` = 24 B~~ | ✔ **done 2026-07-30** (spec 0212) — an interned `FqdnId` into a caller-owned `FqdnTable`, plus a `NO_FQDN` sentinel | 20 B + one heap alloc per message node, **banked** |
| ~~`natural_annotation`~~ | ~~`Option<String>` = 24 B~~ | ✔ **deleted 2026-07-26** (spec 0181) — it had zero production readers | 24 B + heap, **banked** |
| ~~`is_message` + `wire_type`~~ | ~~`bool` + `u32`, 8 B padded~~ | ✔ **done 2026-07-30** (spec 0212) — kept as a `bool` + a `u8` rather than a flag byte; spec 0169's `is_elision` can have its own `bool` and still fit | 6 B, **banked** |
| ~~`packed_record_start`~~ | ~~`Option<usize>` = 16 B~~ | ✔ **done 2026-07-30** (spec 0212) — `u32` + `NO_PACKED_RECORD`; offset 0 is legal, so not a `NonZeroU32` | 12 B, **banked** |

Landing all of it gives `NodeSpan` ≈ 40 B and `TreeNode` ≈ 72 B: **~1.0
GB** at 24.5 MB fresh, ~4.3 GB after two commits (which is the argument
for doing S7 as well, not instead). The `u32` node index also becomes the
natural key type for `line_to_node`, `heat_states` and the S8 window maps.

**One of those rows was free, and is now banked.** `natural_annotation`
was written at three sites in `sink.rs` and read at **none** — a
repo-wide grep found only producers, `: None` initializers, and
prototext-core's own tests
([decode P2](../prototext/decode-flaws.md)). ✔ Deleted 2026-07-26 by
[spec 0181](../specs/0181-delete-natural-annotation.md): ~330 MB at
24.5 MB plus one allocation per container node, at the cost of nothing.
It went first because it was the only row with no design question
attached; **every remaining row still has one.**

~~**Do the interning first** of the rows that remain.~~ ✔ **Done**, in
spec 0212 alongside the scalars, because both cross the crate boundary
and their call-site churn almost entirely overlaps. `type_fqdn` was the
single highest-value remaining entry: 20 B *and* one small heap
allocation per message node removed. The interning also introduced the
`FqdnTable` that `rendered_as` was expected to reuse — in the event it
could not, and got a table of its own; see spec 0213.

**Measured, not projected.** `NodeSpan` is now **32 B** and `TreeNode`
**76 B**, both pinned by compile-time equality assertions. On
`googleapis.desc` (4 501 014 nodes) the three specs together took peak RSS
from 4.18 GiB to **2.09 GiB** (−50.0%) and at-rest from 1.87 GiB to
**1.01 GiB** (−45.7%). Each spec's Measured outcome has its own
breakdown; the short version is that a *span*-shaped change pays the
constant ≈3.06 times at the peak and a *slot*-shaped one ≈2.26, because
the render cache holds `NodeSpan`s and not `TreeNode`s. Spec 0213, which
touches the slot only, came in at 2.24 and so confirms the split.

**Invariant touched.** "A node's array index is a `usize`." Narrowing to
`u32` caps the arena at ~4.29 G nodes — 7.6 GB of blob at the observed
density, well outside the stated target. Make it a type alias
(`type NodeIdx = u32;`) so the cap is one line to revisit, and make the
sentinel a named constant rather than a bare `u32::MAX`.

**Also a time cost, not just memory.** `build_tree`'s loop allocates a
fresh `Vec` per node with children (`decode.rs:324`, `let mut children =
Vec::new();`). Hoist it out of the loop and `clear()` it — leaves are
already free, since `Vec::new` does not allocate, but every message node
pays today.

**Risk: low-to-moderate, and it is mostly typing.** No invariant of the
rendering pipeline changes; the compiler finds every site. The one item
that needs care is `rendered_as`, because moving it to a side table means
splice and reclamation paths must keep that table in sync with the arena.

**Sequence it before S8, not after.** S8's window store wants a node-index
key type and a `text_range` representation to build on, and it is far
easier to pick those once than to change them under a half-migrated
window store.

---

## Suggested sequencing

1. **S1** — biggest single win, lowest risk, no invariant touched.
2. **S11** — measure the sweep first, then delete the second startup
   decode. Independent of S1 and roughly its equal in size; doing it
   second means the two startup measurements don't confound each other.
3. **S4 (1)** plus its S10 rider — `Arc<Vec<u8>>` for the blob and
   `Arc<LoadedGraph>` for the scoring graph; mechanical, immediate
   3×→1× blob memory. (S11 already removed C3's spawn site; the `Arc`
   still closes A5 and the heat worker's own `&'static`.)
4. **S10 (1)** and **S10 (2)** — three lines and a mechanical accessor
   split; together they remove the heat cache's unbounded entry and the
   per-row-per-frame deep clone of it.
5. **S5** — removes the progressive-window frame cliff; purely additive.
6. **S3**, then **S2** — the two incremental-rebuild items, S3 first
   because it is self-contained in `navigation.rs`.
7. **S4 (2)** — line-split without the intermediate `String`, with the
   line-count assertion.
8. **S6** — lazy per-line styles, gated on measurement; doubles as the
   rehearsal for S8.
9. **S10 (3)** — byte-bounded `TieredBounded`, once there is a
   measurement showing the entry cap is the binding constraint.
10. **S12** — shrink `TreeNode` from 280 B to ~72 B. Sequenced here
    because it is mechanical and because S8 should be built on the final
    node-index and `text_range` types, not migrated onto them afterwards.
11. **S7** — arena reclamation. Its value is proportional to the arena's
    per-node cost surviving S12, and at 24.5 MB the post-commit arena is
    the largest number anywhere in this document.
12. **S8**. No longer conditional: 24.5 MB was confirmed as a real target
    on 2026-07-25 (see the sixth conclusion). Re-measure first to size the
    remaining gap, not to decide whether to proceed.

Steps 10-12 are one campaign, and the order matters: S12 fixes the
per-node constant, S7 fixes the node count, S8 fixes the text. Doing S8
first would leave a 3.9 GB floor at 24.5 MB and make the result look like
a failure of S8.
