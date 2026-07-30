<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Asset: the node arena and the override batch

*last verified: 2026-07-29*

This document exists to prepare a redesign of how protolens stores
decoded nodes and how it re-renders them, with the eventual-OOM defect
as the thing to fix. It is written to be read on its own: it repeats
what it needs from [document-tree.md](document-tree.md),
[rendering.md](rendering.md) and [caches.md](caches.md) rather than
sending you there first.

Everything factual here has been checked against the code as of
2026-07-29 (specs 0202 and 0203 landed; 0206 drafted, not implemented).
Measurements are labeled *measured* or *derived* so the two are not
confused.

## Executive summary

protolens keeps every decoded node of the document in one big flat
`Vec`, the **arena**. Applying (or removing) an override does not
rebuild that `Vec`; it **splices** — it re-decodes the affected subtree,
appends the new nodes to the end of the arena, and rewires a handful of
pointers so the new nodes take the old ones' place in the tree. The old
nodes are simply left behind, unreferenced.

That is the whole defect in one sentence: **a splice is an allocation
with no matching free**. A user action that retypes the document's top
level fires ~7 800 splices that between them re-decode every node in the
document, so one keystroke adds a second entire copy of the document to
the arena. Do it three times on a large file and the process is killed.

Two specs have been applied to this, and neither closes it:

- **0202** turns the kill into a *refusal* — before starting, a batch
  checks whether another arena's worth of nodes would fit in half of
  `MemAvailable`, and if not, prints a message and does nothing.
- **0203** makes the arena flat *between* actions — an incremental
  compaction pass runs in the event loop's idle time and walks the
  garbage back out, so batch N+1 starts from the same node count as
  batch N.

What is left is the **peak during a single batch**, which is unchanged
at 3.9–4.0 GiB on the reference corpus. That peak is not one mistake but
three roughly equal ones in three different places (the arena, the
tree builder, the render cache), and no single change removes more than
about a third of it.

## Vocabulary

Four words in this area are easy to confuse; they are used precisely
below.

| word | meaning here |
| --- | --- |
| **arena** | `App::tree: Vec<TreeNode>` — the flat array every node lives in, and whose *indices* are used as node identity everywhere else in the program. |
| **splice** | One call to `splice_override`. Replaces one node's rendering and its whole subtree. The only function in the crate that pushes onto the arena. |
| **batch** (a "run" of the walk) | One call to `render_overrides`. Walks the document top-down and performs zero, one, or many splices, then reconciles the document once at the end. Triggered by a single user action. |
| **packed run** | An unrelated meaning of "run": the several elements of one packed-repeated wire record. They share one wire record and are therefore addressed, overridden and replaced as a unit. Relevant here only because it is the reason a splice can abandon *sibling* nodes and not just descendants. |

## Part 1 — the arena

### What a node is

```rust
pub struct TreeNode {
    pub span: NodeSpan,          //  96 B
    pub parent: Option<usize>,   //  16 B each, seven of them
    pub first_child: Option<usize>,
    pub last_child: Option<usize>,
    pub next_sibling: Option<usize>,
    pub prev_sibling: Option<usize>,
    pub doc_next: Option<usize>,
    pub doc_prev: Option<usize>,
    pub sibling_ordinal: u32,    //   4 B
    pub rendered_as: Option<(Option<Option<String>>, String)>, // 48 B
}
```

*Measured*: `size_of::<TreeNode>() == 264`, `size_of::<NodeSpan>() ==
96`. Of the 264 bytes, **112 are the seven links** and 96 are the span.
`NodeSpan` in turn carries a `type_fqdn: Option<String>`, a separate
heap allocation for every node that has a resolved type — which is why
the *effective* cost measured against RSS is ~305 B/node rather than
264.

Three cheap observations follow, and all three matter for a redesign:

- The links are `Option<usize>`, i.e. **16 bytes to express a value that
  never exceeds a few million**. `Option<u32>` would be 8. Seven of them
  is 112 B → 56 B, roughly a 24% cut of the whole node, for a purely
  mechanical change. (Recorded as future work in specs 0203 N1 and 0206
  N3; not done.)
- `type_fqdn` is a per-node `String` holding a fully-qualified type name
  drawn from a small set — a document with 4.5 M nodes has on the order
  of tens of thousands of distinct type names. Interning it would remove
  ~41 B/node *and* 4.5 M small allocations.
- `TreeNode` derives **only `Debug`**. No `Clone`, no `Default`. This is
  why an unused arena slot cannot simply be "pushed" — there is no blank
  `TreeNode` to push. Any free-slot scheme has to deal with this.

### The arena's layout has no meaning, but its indices do

`build_tree` receives the decoder's flat span list, which is
**post-order**: a container's own entry comes *after* all its
descendants. protolens does not re-sort it. Array position therefore
tells you nothing about document position — index 0 is a leaf, not the
root — and document order is carried by an explicit `doc_next`/
`doc_prev` chain instead.

The important consequence is the opposite one: because position is
meaningless, an index is free to be used as a **stable name** for a
node, and it is, everywhere. A node's index survives its own retype on
purpose (spec 0118 §7), so that the cursor, the fold set and the
back-jump list keep pointing at the right thing across an override.

That makes "who is holding an index?" a question the redesign must be
able to answer exhaustively. It has been answered once, and the answer
is short:

**Holders of node indices** (must be remapped if a node moves):
`cursor`, `first_node`, `override_target`, `folded: HashSet<usize>`,
`pending_heat_recheck: HashSet<usize>`, plus the two line maps
`line_to_node` / `footer_line_to_node`, which are `Vec<Option<u32>>`
indexed *by line* and holding node indices as values.

**Arrays parallel to the arena** (must be permuted, not remapped):
`heat_states: Vec<HeatState>` (40 B/entry) and `descend: Vec<bool>`,
plus `dead: Vec<bool>`.

**Not affected**: anything keyed by *line* (`visible_rows`,
`select_anchor`, the prefetch walk) or by *byte range and type*
(`heat_caches`, `render_cache`, `active_override_range`).

### The one property compaction rests on

> **Every reference to a node is discoverable from that node.**

Its parent, siblings and document neighbors are named by its own link
fields, and each of them holds the reverse link. Its children are on its
own child chain. No line-keyed structure names it at all any more (spec
0210 S2 deleted both maps). The five remaining holders are index-keyed,
so a hash lookup or an equality test finds them.

This is what makes relocation *incremental*: one node can be moved in
time proportional to its own degree, with no arena-wide pass and no
remap table, and **the arena is fully consistent after every single
move**. That is why compaction can be sliced 4 096 nodes at a time into
the event loop and abandoned halfway with no cleanup.

A redesign is free to change a great deal, but if it breaks this
property it loses incrementality, and the alternative is a
stop-the-world pass measured in seconds on the reference corpus.

### Resident cost at rest

On the reference corpus (`googleapis.desc`, 25.6 MB, opened as its own
target blob — 4 499 336 live nodes, ~5.28 M rendered lines):

| structure | *derived* size | note |
| --- | --- | --- |
| `tree` | 1.19 GB struct + ~0.18 GB strings ≈ **1.37 GB** | 4.5 M × 264 B, plus `type_fqdn` |
| `heat_states` | 180 MB | 4.5 M × 40 B |
| `lines` (`Vec<String>`) | 127 MB of headers + the text itself | 5.28 M × 24 B |
| `line_to_node` + `footer_line_to_node` | 85 MB | 2 × 5.28 M × 8 B (`Option<u32>` is 8 B — no niche) |
| `visible_rows` | 42 MB | 5.28 M × 8 B, nothing folded |
| `dead` + `descend` | 9 MB | 1 B/node each |

*Measured* startup RSS for this document is 2045 MiB, which the above
accounts for to within the text bytes. The arena is about two thirds of
it.

## Part 2 — the batch (one "run" of the override walk)

### What starts one

`render_overrides(idx)` is called from exactly the places a user can
change what is displayed: the override selection pane confirming a type,
the management pane toggling/renaming/deleting an entry, the `:type-as`
and other commands, a keypress that removes an override, and once from
the event loop when the background root-type resolution lands. Every one
of these passes `self.first_node`, i.e. **almost every batch is a
whole-document walk**.

There is one other entry into `splice_override` — the live preview — but
since spec 0185 the preview is a non-mutating *overlay*; it renders into
a side buffer and never splices. Comments in `override_select.rs`
describing a preview that rebuilds the tree are historical.

### The five phases of a batch

```
render_overrides(first_node)
  1. compute_descend_marks()        — decide where the walk may go
  2. override_batch_refusal()       — the 0202 memory guard; may abort here
  3. render_overrides_inner(...)    — recursive walk; calls resettle_node,
                                      which calls splice_override
  4. finalize_override_batch(idx)   — materialize text, repair maps, reconcile
  5. (idle time, later) compact_slice() — the 0203 reclamation pass
```

**Phase 1 — the descent marks.** Descending into every message node cost
seconds on a 600 k-node document, so spec 0183 replaced the gate with a
precomputed `descend: Vec<bool>`. A node is *marked* if this batch could
change how it renders (it has an override entry, it has been rendered
before, or it is an auto-expand candidate) — **and so is every ancestor
of such a node**, which is the part that is easy to get wrong.
Over-marking merely wastes a descent; under-marking leaves stale text
with no panic and no failing assertion.

`descend.len()` does double duty as a *watermark*: it records how much
of the arena has already been examined, so a later batch only scans the
suffix. (The one exception is an `fqdn:field` origin — "field N of every
message of type T, anywhere" — which has no path to follow and forces a
full re-scan.)

**Phase 2 — the refusal guard.** 0202, verbatim in effect:

```
per_node = size_of::<TreeNode>() + 64      // 328 B
if tree.len() * per_node > MemAvailable / 2 { refuse the whole batch }
```

It deliberately does **not** try to predict this particular batch. A
marks-based predictor was written and thrown away: at startup `descend`
marks the root alone, so it charged the whole document for a pass that
splices nothing. The honest cost of not predicting is that once the
arena is large, *every* override is refused — including a one-node one
that would have been harmless.

Because that refusal is permanent for the session, it also undoes the
activation that triggered it (`OverrideCollection::revert_active`): the
entry stays listed but inactive, so the management pane never claims an
override the document does not have. Every batch that actually renders
drops the undo snapshot (`commit_active`), so a refusal can never reach
back past one that did.

**Phase 3 — the walk.** `render_overrides_inner` recurses in document
order through marked nodes only. At each node it calls `resettle_node`,
whose entire decision is one comparison:

```rust
let current = Some((target, field_name));
if current != self.tree[idx].rendered_as { splice }
```

`rendered_as` is the node's *provenance* — which override produced its
current text, and under what field name. If provenance is unchanged, no
splice happens. This is why a no-op batch is cheap, and it is also why
the first batch after startup is expensive: `rendered_as` is `None`
everywhere, so it does not match anything.

The walk carries nothing down the tree but the path and the patch scope.
It used to carry a running line-count correction as well (`inherited` /
`child_owed`), to keep every node's stored `text_range` naming its final
position — including, for a subtree the walk had *pruned*, a `doc_next`
run over the whole of it. That was the last piece of a commit whose cost
was proportional to the document rather than to the splice: an override
on the first of googleapis.desc's top-level records shifted 4.5 M spans,
402 ms of a 500 ms keystroke. Spec 0210 S11 deleted it. No node stores a
position now, so there is nothing to correct.

**Phase 4 — finalize.** Runs once per batch, not once per splice, and
merges the collected text patches into `self.lines` in a single pass.
That is now all it does: the two line maps and `visible_rows` are gone
(spec 0210 S2), and each splice fixes its own ancestors' line counts as
it goes, because the rest of the batch places its patches from them.

**Phase 5 — compaction.** In the event loop's idle branch, strictly
behind read-ahead, `compact_slice(4096)` moves up to 4 096 live nodes
down into holes. A pass only *starts* when at least 1/8 of the arena is
dead — expressed as a fraction on purpose, because refusing below a
garbage share of 1/N is exactly what bounds the arena at N/(N−1) times
its live size. When the pass finishes it truncates and `shrink_to_fit`s.

### What one splice actually does

This is the heart of the matter, so in order:

1. **Render.** `render_node_as` re-decodes the node's own tag+payload
   bytes under the target type and renders them to text + a flat span
   list. Consults `render_cache` first.
2. **Normalize a packed run.** If the node is one element of a packed
   record, the whole run is the addressable unit: the node is
   reassigned to the run's *leader* and its extent widened to the whole
   run.
3. **Mark the old nodes dead.** `collect_descendants` gathers the old
   subtree (plus, for a packed run, the absorbed siblings and *their*
   descendants), scrubs them from `folded`, and calls `mark_dead` on
   each. **This happens before anything is allocated** — the holes exist
   by the time the push loop runs.
4. **Repair index holders.** Any of `cursor` / `first_node` /
   `override_target` that named an abandoned node is redirected to the
   surviving node; abandoned `pending_heat_recheck` entries are dropped.
5. **Record a text patch.** Rather than splicing `self.lines` here (an
   O(document) memmove per splice), the new lines are queued as a
   `LinePatch` and merged once at the end of the batch. Patches form a
   *tree*, because a freshly spliced node can itself be re-spliced later
   in the same batch.
6. **Build and append the new nodes.** `build_tree(new_spans)` produces
   a private `local_tree: Vec<TreeNode>`, whose indices are then
   translated by `+base` (`base = tree.len()`) and pushed one at a time
   onto the arena. Byte offsets, line ranges and `packed_record_start`
   are rebased in the same loop.
7. **Resize the parallel arrays** (`heat_states`, `dead`) to match.
8. **Fold the new root into the old slot.** The last local node is the
   node's own new self. Its span and child pointers are copied into the
   *existing* `tree[idx]`, and the pushed copy is marked dead. This is
   how the node keeps its index.
9. **Rewire.** Sibling links past an absorbed packed run, `idx`'s child
   ordinals, and the `doc_next`/`doc_prev` seam back into the rest of
   the document.

Garbage is produced at **exactly two sites**: step 3 (the old subtree
and packed orphans) and step 8 (the pushed copy of the local root).
Nowhere else.

## Part 3 — why memory grows, with numbers

### The measurement

*Measured*, on `googleapis.desc` opened as its own blob, pressing
`Down`, then `t` `Enter` `o` `d` `Esc` repeatedly (apply an override at
the root, then remove it):

| step | RSS | `tree.len()` | reachable |
| --- | --- | --- | --- |
| startup | 2045 MiB | 4 501 014 | 4 501 014 |
| apply the override | 3889 MiB | 9 000 349 | 4 499 336 |
| remove it again | 5256 MiB | 13 499 684 | 4 499 336 |
| apply again | > 6 GiB — OOM killed | — | — |

`lines.len()` stays at ~5.28 M throughout. **The text buffer is not the
problem**; it is patched in place. Nor is this allocator fragmentation:
re-running with `MALLOC_ARENA_MAX=2` and tightened trim/mmap thresholds
reproduced the identical curve to within 20 MiB.

`Vec` doubling amplifies it: at 13 499 684 live entries the arena's
`capacity()` was 18 004 056, i.e. ~5.5 GiB reserved.

### The work itself is correct

The obvious suspicion — that the walk is re-rendering more than it needs
to — was checked and is wrong. Instrumenting one batch gave:

```
visits=7772  splices=7771  nodes=4499335  reachable=4499336
```

The descent marks are doing their job (7 772 visits into a 4.5 M-node
arena). The override's origin is `path:field` at the *root*, so it
targets all 7 771 top-level records, and each one is genuinely retyped;
a retype necessarily re-decodes the whole subtree, and 7 771 subtrees
*is* the whole document. Per-batch work is proportional to the
override's reach, which is what it should be.

**The leak is purely that the superseded copy is never freed.** The fix
direction is reclamation, not pruning.

### Where the 3.9 GB peak actually is

This is the one number everything now hangs on, and getting it wrong
(attributing all of it to the arena) was the central error of an earlier
drafting session. A root retype holds **three** large transients at
once:

| transient | *derived* size | why it is live at the same time as the others |
| --- | --- | --- |
| the arena's superseded half | ~1.37 GB | freed by `mark_dead`, but the slots are not reused until the compaction pass runs *after* the batch |
| `local_tree` | 4.5 M × 264 B = **1.19 GB** | `Vec` does not release incrementally, so `for node in local_tree` holds all of it until the loop *ends* — precisely when the arena is widest |
| the render cache's clone | 4.5 M × 96 B = **432 MB** of spans, plus a second `Vec<String>` of every rendered line | `render_node_as` inserts `value.clone()` on a miss, and `RenderCache::get` clones on a hit — the copy is paid on every path |

Arena-at-peak (2.74 GB) + `local_tree` (1.19 GB) = 3.93 GB against a
measured 3.9–4.0 GiB, which leaves the render cache's 432 MB
**unaccounted for**. That discrepancy is real and unresolved; do not
assume it is explained.

**Correction, measured 2026-07-30 (spec 0211).** The three terms are not
"roughly equal", and the table above overstates two of them. Narrowing
the slot by 88 B moved the peak by **2.29** slot-copies, not three:
4 379 916 kB → 3 493 144 kB, i.e. 4.18 → 3.33 GiB. The reason is that
only the *surviving* arena is 4.5 M nodes. The other two terms scale with
the **replacement** tree, and a raw-message retype of this corpus turns
5 281 124 lines into 1 487 288 — so `local_tree` and the arena's new half
are each ≈2.9 M nodes here, not 4.5 M. So:

- The multiplier is **a property of the workload, bounded above by 3**,
  and reaches 3 only for a size-preserving retype. Quote the workload,
  not the constant.
- The render cache's clone holds `NodeSpan`s, so it moved by **0** — it
  is invariant to the link narrowing and only starts shrinking when rows
  2-8 narrow the span itself.
- Two measurement traps, both of which hid this peak from earlier
  drivers: `:type-as-raw` must be applied on **line 0** (one `Down` first
  retypes a 35-line record and does not move peak RSS at all), and the
  **batch** path does not show it either — there, applied and not-applied
  peak identically and the whole slot delta is the arena at rest.

### A finding about the render cache worth checking first

`RENDER_CACHE_MAX_BYTES` is `1 << 20` — one mebibyte. But
`RenderCache::insert` evicts only `while total_bytes > max_bytes &&
entries.len() > 1`, i.e. it **always keeps the entry just inserted, even
if that single entry is 400 MB**. So a root retype evicts the entire
cache and then parks a ~432 MB spans vector plus a full copy of the
document's text in a structure whose stated budget is 1 MiB, where it
stays until the *next* insert.

This is plain in the code. Whether it explains the +128 MiB settled
drift spec 0203 could not attribute is a hypothesis, not a conclusion —
but it is cheap to test and should be tested before anything harder is
attempted.

Note also that a *confirmed* render is essentially never re-read: the
cache key includes an `is_preview` flag, so a confirmed entry can only
be hit by another confirmed splice of the identical `(range, target)` —
the user reverting and re-applying. The cache exists for the preview
path, which re-hits it on every arrow keystroke, and preview renders are
byte-budgeted and therefore small. **The one caller that pays the big
copy is the one that gets no benefit from it.**

## Part 4 — constraints a redesign must respect

These are not opinions; each one is a fact about the current code that
has already cost somebody a debugging cycle.

1. **A retyped node keeps its arena index.** Cursor, folds and the
   jumplist depend on it. Any scheme that renumbers on retype has to
   remap all five index holders plus both line maps.

2. **The top level is a forest, not a tree.** A document's top-level
   fields are a sibling chain of parentless nodes (7 771 of them on the
   reference corpus). Walking down from `first_node`'s topmost ancestor
   reaches only one of them; you must then walk `prev_sibling` to the
   head and seed *every* sibling. Getting this wrong reports most of the
   document as unreachable.

3. **The splice frees before it allocates.** This is what makes slot
   reuse able to bound `tree.len()` to the live high-water mark rather
   than twice it. An earlier spec asserted the opposite; check the
   ordering (`mark_dead` at `override_apply.rs:2436`, first push at
   `:2555`) rather than trusting prose.

4. **Freed nodes are read after they are freed.** `doc_next_after_
   subtree` and the index-holder repair both read links out of
   just-marked-dead nodes, and the local root's slot is marked dead and
   then dereferenced three more times. This is correct today *only by
   statement ordering*, which nothing enforces. Under slot reuse it
   becomes a live hazard.

5. **`descend.len() == tree.len()` is not a standing invariant.** It
   holds only after a batch. And `mark_fresh_subtree`'s `if tree.len()
   <= base { return }` guard would fire on *every* splice once
   `tree.len()` stops growing — under slot reuse it is also the only
   thing that marks a fresh node, so it must be rewritten first.

6. **Reuse fails silently, not loudly.** Today a stale index names an
   abandoned-but-intact node: the reader sees plausible, wrong content.
   Once slots are reused, the same index names a live *unrelated* node.
   Three separate specs agree this is the real risk. The answer built so
   far is `App::verify_arena()`, which recomputes the reachable set and
   checks four well-formedness properties against it — but it is
   `#[cfg(test)]`, and **no fixture reaches the scale where the failure
   appears**. Making it reachable from the shipping binary (behind an
   environment variable, reporting through `trace.rs` since the TUI owns
   the terminal) is a prerequisite, not a nicety.

7. **Three coordinate frames coexist during a batch**, and confusing two
   of them has already caused silent corruption. `node_lines(idx)` gives
   a node's position in the document *as the batch has made it so far*;
   a queued patch's own `lines` is a *frozen snapshot* taken when the
   patch was created and never touched again; and `self.lines` still
   holds *pre-batch* content for the whole duration. A nested patch's
   offset into its parent must subtract exactly the growth accumulated
   since the parent froze.

8. **`packed_record_start` is a byte offset and must be rebased on
   splice.** Leaving it in local coordinates made downstream parsers
   read a tag and a length out of unrelated bytes, which widened a
   "packed run" to a garbage extent, relinked the tree into a cycle, and
   sent `collect_descendants` into unbounded recursion (a 256 MB
   `RUST_MIN_STACK` still overflowed). Any rewrite of the translation
   loop must carry this over, comment included.

9. **`splice_override` must abandon any compaction pass in flight**
   (`reset_compaction`), and it must do so in the splice, not in
   `render_overrides` — the preview path used to reach one without the
   other. Under slot reuse this is promoted from an optimization to a
   correctness requirement, because there would then be two writers of
   `dead`.

10. **The arena has exactly one mutator.** `splice_override` is the only
    `tree.push` in the crate and the only producer of dead slots. That
    is a large part of why any of this is tractable; a redesign should
    work hard to keep it true.

## Part 5 — the levers, and what each is worth

Ordered by how much of the peak they remove, not by difficulty.

### A. Reuse dead slots instead of appending (spec 0206, drafted)

Hand the push loop the holes `mark_dead` just created. `tree.len()`
becomes the high-water mark of the *live* set — 4 499 336 rather than
8 998 671 — and the peak is prevented rather than reclaimed afterwards.
**Removes ~1.37 GB. Expected peak after it alone: ~2.6 GB.**

Settled design points, so they are not re-litigated:

- `dead` *is* the allocator; there is no second free list. (A list would
  hand out live slots, because `relocate_node` fills a hole without
  consulting one.)
- Slot granularity, not extent granularity. Every slot is an identical
  264 B `TreeNode`, so a hole always fits a request and external
  fragmentation is impossible — which retires the strongest of the three
  earlier objections. The extent story only existed because the splice's
  addressing is affine (`|i| i + base`), which is self-imposed.
- Watermark policy: keep `truncate`, drop `shrink_to_fit`, return
  nothing to the OS for now.
- Because `TreeNode` has no `Default`, the interface is `alloc_slots(n)`
  rather than a per-slot push, and the contract at the tail (reserved
  vs. initialized) is an open question.

### B. Build the spliced subtree in place

Have `build_tree` write nodes directly into their destination slots
instead of into a private `Vec` that is then copied out. **Removes
1.19 GB.** It also deletes the `+base` translation entirely, since
`build_tree` would emit global indices in the first place.

**Depends on A** — you can only build in place once the destination
slots are known. Two constraints already identified: do not fork the
linking algorithm (one implementation generic over the index mapping,
or the two copies will drift), and fold the coordinate translation and
the parent remap into the same pass.

### C. Own the render instead of cloning it

**Removes the 432 MB spans copy and a full second `Vec<String>`.**
Independent of A and B, and the smallest of the three.

The obvious version does not work: making the value an `Arc` alone
fails, because the splice *moves* the rendered lines into `self.lines`,
and `self.lines` must own its `String`s — a shared `Arc` forces
`make_mut`, which copies anyway. The resolution is to split by caller,
because the two callers are not symmetric:

- **Confirmed splice — own it.** Never insert; *take* on a hit. The
  render is built once and moved into `self.lines`. Zero copies. The
  cost is one re-render on a revert-and-re-apply, which is user-paced.
- **Preview overlay — share it.** `PreviewOverlay.lines` is read-only,
  so it can become an `Arc` held jointly with the cache. A refcount
  bump, no copy, and the cache keeps serving the workload it was
  actually built for.

Free alongside: the preview path renders a span vector it then discards
entirely, so `RenderedAs` is really two products and the preview wants
only one of them. And `RenderCache::get` should stop returning by value.

### D. Narrow the node (deferred twice, and it is the biggest lever)

264 B → 72 B, field by field, plus interning the per-node type name away.
Against the measured peak this is worth roughly **2.2 GB** — more than A,
B and C individually — because it multiplies with all three: it shrinks
the dead slots A reclaims, the `local_tree` B removes, and the spans C
stops copying, all at once.

It is also the only lever here that introduces no new lifetime hazard.
A, B and C all touch when a node may be read; D touches only how wide it
is, and the compiler finds every site.

Fully worked out in the [annex](#annex--the-slot-field-by-field) at the
end of this document.

### E. Things that are not levers

- **Trimming the allocator.** Ruled out by measurement.
- **Pruning the walk further.** The walk already visits 7 772 of 4.5 M
  nodes. There is nothing left to prune.
- **Shrinking the text buffer.** `lines.len()` is flat across batches.

## Part 6 — what is still unknown

Stated plainly, because a redesign that assumes these are settled will
be wrong.

1. **Does `descend`'s watermark survive slot reuse in full?** The claim
   that two of three per-node target sources need no re-examination was
   checked against `compute_descend_marks`'s own reasoning, not against
   the override-activation path.
2. **How many extents does a real batch actually produce?** The argument
   for slot granularity rests on address-ordered first fit clustering
   holes well enough in practice. Nobody has measured it.
3. **What is the +128 MiB settled drift?** Six batches went 2594 →
   2722 MiB with `tree.len()` exactly constant, so it is not the arena.
   See the render-cache finding in Part 3 for the first thing to check.
4. **Why is the render cache's 432 MB not visible in the peak?** Either
   the ~305 B/node estimate is generous, the sampling window missed the
   insert, or the entry had been evicted.
5. **Should the batch move off the input thread?** Two drafts exist
   (0204, a "this will take a while" warning; 0205, running the batch
   off-thread with a measured 150 ms trigger). Neither is implemented,
   0205 is strictly better, and the likely resolution is to fold 0204
   into 0205 and close it. This is a *latency* concern, not a memory
   one, but it constrains the same code.

## Reproducing any of this

```
target/release/protolens \
  --descriptor-set <path>/googleapis.desc <path>/googleapis.desc
```

then `Down`, then repeatedly `t`, `Enter`, `o`, `d`, `Esc`. Give each
keystroke ~12 s to settle. Killed by the OOM killer in the third cycle
without spec 0203.

Instrumentation goes through `protolens/src/tui/trace.rs`
(`PROTOLENS_TRACE=<path>`) — the TUI owns the terminal, so nothing may
go to stdout or stderr. A probe at the end of `render_overrides`
printing `tree.len()`, `tree.capacity()`, `lines.len()` and RSS from
`/proc/self/statm` was enough to settle the original diagnosis in one
run; no heap profiler was needed, and none is in the dev shell.

There is a temporary `probe` module in `override_apply.rs` (the
`VISITS` / `SPLICES` / `NODES` counters) committed deliberately for this
investigation. Strip it when the work concludes.

## Where the rest of this is written down

- `docs/specs/0202-an-override-is-refused-rather-than-fatal.md` —
  implemented: the headroom guard.
- `docs/specs/0203-the-override-arena-is-compacted.md` — implemented:
  incremental compaction. Read its "Why this is safe" section before
  touching `compact.rs`.
- `docs/specs/0206-the-arena-reuses-its-dead-slots.md` — drafted and
  adversarially reviewed; lever A above.
- `docs/specs/0207-where-the-override-memory-work-stands.md` — the
  wrap-up this document supersedes for orientation purposes, but which
  still holds the full verified-call-site list.
- [document-tree.md](document-tree.md) — the tree as a navigation
  structure rather than as an allocation problem.
- [rendering.md](rendering.md) — the five rendering stages and the three
  coordinate systems.
- `docs/protolens/rendering-scaling-roadmap.md` S12 and
  `docs/protolens/rendering-worklist.md` W25 — the pre-existing plan for
  the annex below. Both predate this document; see the annex for what
  their figures get wrong.

---

## Annex — the slot, field by field

The rest of this document treats the node as an opaque 264-byte cost.
This annex opens it up, because the per-node constant turns out to be
the single largest term in the peak, and because it is the only lever
that is pure arithmetic — no invariant of the pipeline changes, and
nothing about *when* a node may be read is affected.

### This is not a new idea, but the numbers here are new

A plan already exists in two places:
`rendering-scaling-roadmap.md` S12 ("Shrink the arena") and
`rendering-worklist.md` W25 ("Shrink `TreeNode` from 280 B to ~72 B").
Only one of their six steps has landed — spec 0181's deletion of
`natural_annotation`.

**Both are stale in three ways**, and a redesign that quotes them will
quote wrong numbers:

1. They say `TreeNode` is 280 B and `NodeSpan` is 120 B. That was true
   before spec 0181. *Measured today*: **264 B** and **96 B**. The
   arithmetic reconciles exactly — 280 − 24 (`natural_annotation`)
   + 8 (spec 0192's `sibling_ordinal`, added afterwards, 4 B plus 4 B of
   tail padding) = 264.
2. They estimate 13.9 M nodes for `googleapis.desc`, extrapolated from a
   1.1 MB fixture at 0.566 nodes/byte. The *measured* figure is
   **4 501 014** nodes on a 25.6 MB blob — 0.176 nodes/byte. Node
   density is strongly document-dependent and the small fixture is not
   representative. Every absolute memory figure in S12 is therefore
   roughly 3× too pessimistic.
3. Neither knows about the override batch. They cost the arena at rest.
   The interesting number is the arena *mid-batch*, which is where the
   process actually dies.

A third stale figure lives in the code: `override_apply.rs:1277` says
"measured ~305 B/node against a 250 B struct". The struct is 264 B. The
305 B/node effective figure is still right.

### What the 264 bytes are

Derived from the field list; the total is confirmed by measurement.

This table is the layout **before** spec 0210, which added `lines_total`
and `lines_visible` (4 B each) and took the slot to 272 B, and before
spec 0211, which did row 1 and took it to **184 B**. It is left as it
was because it is what the eleven opportunities below were sized
against, and because 0210 changed what `text_range` *means* rather than
how much it costs: the field is now written at build time and re-derived
from the counters on demand, so row 3 below could become a deletion
rather than a narrowing — worth 16 B, not 8.

| field | type | bytes | what it holds |
| --- | --- | --- | --- |
| — | 7 links `parent`…`doc_prev` | **112** | node indices, none of which has ever exceeded 13.5 M |
| `rendered_as` | `Option<(Option<Option<String>>, String)>` | **48** | which override produced this rendering, and under what field name |
| `type_fqdn` | `Option<String>` | 24 | a fully-qualified type name, drawn from a set of ~59 k |
| `raw_range` | `Range<usize>` | 16 | byte offsets into a blob of 25.6 MB |
| `text_range` | `Range<usize>` | 16 | *line* numbers, into ~5.3 M lines |
| `packed_record_start` | `Option<usize>` | 16 | one more byte offset, `None` for the large majority of nodes |
| `field_number` | `u64` | 8 | a protobuf field number, ≤ 2²⁹ − 1 by the wire format |
| `level` | `usize` | 8 | nesting depth, capped at `MAX_WIRE_DEPTH` = 1000 |
| `wire_type` | `u32` | 4 | one of five values |
| `sibling_ordinal` | `u32` | 4 | position among siblings |
| `is_message` | `bool` | 1 | one bit |
| — | tail padding | 7 | |
| | | **264** | |

Read that table as a whole and the shape of the problem is obvious: not
one field in it needs more than 32 bits, and the two largest entries —
160 of the 264 bytes between them — are the two that are *least* likely
to be occupied at all. `rendered_as` is `None` on every node that has
never been spliced, which on the reference corpus is 4 499 336 of
4 501 014. `type_fqdn` is `None` on every scalar.

There is also an off-struct cost the table does not show. `type_fqdn`
and `rendered_as` are `String`s, so each non-`None` one is a *separate
heap allocation*. That is the gap between the 264 B struct and the
~305 B/node measured against RSS: about **41 B/node of string heap**,
and ~4.5 M individual allocations to malloc, to free, and to fragment
the heap with.

### The eleven opportunities

Grouped by what makes them hard, not by what they save. Sizes in the
"after" column are the packed layout, which happens to need no padding
at all (everything is 4-byte aligned or smaller and sorts cleanly).

**Mechanical — the compiler finds every site, nothing else changes.**

| # | change | before | after | saved |
| --- | --- | --- | --- | --- |
| 1 | 7 links → `type NodeIdx = u32` with a named sentinel — **done, spec 0211** | 112 | 28 | **84** |
| 2 | `raw_range` → `Range<u32>` — **done, spec 0212** | 16 | 8 | 8 |
| 3 | `text_range` → `Range<u32>` — **done, spec 0212** | 16 | 8 | 8 |
| 4 | `packed_record_start` → `u32` sentinel — **done, spec 0212** | 16 | 4 | 12 |
| 5 | `field_number` → `u32` — **done, spec 0212** | 8 | 4 | 4 |
| 6 | `level` → `u16` — **done, spec 0212** | 8 | 2 | 6 |
| 7 | `is_message` + `wire_type` → a `bool` + a `u8` — **done, spec 0212** | 5 (+3 pad) | 2 | 6 |
| 8 | `sibling_ordinal` stays `u32` — **done, spec 0212** | 4 | 4 | 0 |

**Needs a table, and the table needs an owner.**

| # | change | before | after | saved |
| --- | --- | --- | --- | --- |
| 9 | `type_fqdn` → an interned `u32` symbol — **done, spec 0212** | 24 | 4 | **20**, plus ~one heap allocation per message node |
| 10 | `rendered_as` → a pair of interned symbols | 48 | 8 | **40**, plus up to two allocations per spliced node |

**Structural, and optional.**

| # | change | saved |
| --- | --- | --- |
| 11 | split the arena into a hot link column and a cold span column | 0 bytes, but see below |

Rows 1-10 sum to **192 B**: `NodeSpan` 96 → 32, `TreeNode` 264 → **72**,
which is exactly the target W25 already set. Counting spec 0210's two
counters, which arrived after this table, the same rows take today's
272 B to 72 B.

Spec 0211 landed row 1 and spec 0212 landed rows 2-9, so the slot is
**120 B** now — `NodeSpan` 96 → **32** and `TreeNode` 272 → 120 — and
`const _: () = assert!(size_of::<TreeNode>() == 120)` beside `TreeNode`
plus `assert!(size_of::<NodeSpan>() == 32)` beside `NodeSpan` keep both
there. The assertions are equalities rather than bounds, so growth is
caught as well as celebrated, and each later row moves the numbers
deliberately. Only row 10 (`rendered_as`, 48 B) and row 11 remain, which
is what puts the slot at the 72 B target.

Spec 0212 also took a decision the "Suggested order" below had left open:
`level` is `u16` with no `debug_assert` needed (`MAX_WIRE_DEPTH` is 1000,
enforced at decode), `field_number` needs no saturation because a
protobuf field number is ≤ 2²⁹ − 1 by the wire format, and `text_range`
did **not** go away — see trap 7.

### What each is actually worth, on the measured run

Applying 264 → 72 B and ~41 → ~1 B/node of string heap to the figures in
Part 3:

| term | today | after | removed |
| --- | --- | --- | --- |
| live arena at rest (4.5 M nodes) | 1.37 GB | 0.33 GB | **1.04 GB** |
| the arena's superseded half, mid-batch | ~1.37 GB | ~0.37 GB | **1.00 GB** |
| `local_tree`, the throwaway build | 1.19 GB | 0.32 GB | **0.87 GB** |
| the render cache's span copy (4.5 M × `NodeSpan`) | 432 MB | 144 MB | **0.29 GB** |
| `heat_states`, unchanged | 180 MB | 180 MB | 0 |

Against a ~3.0 GB accounted peak, narrowing removes about **2.2 GB**.
That is more than lever A (1.37 GB) and more than B and C. The reason is
that the same constant appears in four different places, so it is the
only change that pays four times.

**Calibrate that 2.2 GB against specs 0211 and 0212 before relying on
it.** Row 1 alone predicted 88 B × 4.5 M in each of four places = 1.55 GB
and delivered **0.89 GB** at the peak (2.29 copies, not 4). Two of the
four rows in the table above are the reason: the superseded half and
`local_tree` size with the *replacement* tree, which on this corpus is
≈2.9 M nodes rather than 4.5 M, and the render cache's span copy does not
respond to a link change at all. The rows-2-8 and rows-9-10 estimates in
this table are computed the same optimistic way, so read them as upper
bounds. The at-rest column is the one that behaved exactly as predicted
(within 0.04%).

Spec 0212 then narrowed the span itself by the same 64 B, and the fourth
row *did* respond: it delivered **0.82 GB** more at the peak, a
multiplier of **3.06** — three arena-shaped copies at 2.29, exactly
reproducing 0211, plus ≈0.77 (≈212 MiB) of span-shaped copies. So the
four-place model is right about there being four places; what it gets
wrong is treating them as interchangeable. Three scale with the *node*
count of whichever tree is being built, and one scales with the *span*
count of whatever the render cache is holding. At rest the same spec came
in at 1.12 rather than 1.00, the extra 12% being 34 MiB of `type_fqdn`
heap that row 9 deleted (≈8 B/node averaged over the arena — see trap 2).

The FQDN interning (row 9) deserves its own line: at ~59 k distinct type
names it collapses ~185 MB of scattered small strings into a table of a
few MB. It is also a *startup latency* fix, not only a memory one —
4.5 M small allocations at open and 4.5 M frees at exit. Part 3 of
`protolens_descriptor_startup` records 0.84 s of exit latency from a
structurally identical cause elsewhere in the program.

### Why this gets better in combination, not worse

- **It multiplies with A and B.** Slot reuse bounds the *number* of
  slots; narrowing fixes the *price* of one. Neither subsumes the other,
  and each makes the other's remaining term smaller.
- **It makes the node `Copy`.** Today `TreeNode` derives only `Debug` —
  no `Clone`, no `Default` — precisely because of the `String`s. Rows 9
  and 10 remove the last of them, at which point the node is plain old
  data. That is a real enabler, not a nicety: `relocate_node` becomes a
  memcpy, and lever A's central difficulty ("there is no blank
  `TreeNode` to push") evaporates, because a `Default` slot becomes
  expressible.
- **It changes the cache story that spec 0206 relied on.** 264 B is 4.1
  cache lines, so a node is never adjacent to its neighbour in any
  useful sense, which is why 0206 concluded contiguity was not worth
  much. At 72 B it is 1.1 lines, and with row 11's hot/cold split the
  link column is 28 B — four nodes to a line. The `sibling_position`
  walk that Part 3 of `protolens_render_hot_path` measured at 74% of a
  frame is exactly a pointer chase over that column.

### The traps

Seven things that will bite, in the order they will bite.

1. **`rendered_as` as a side `HashMap<NodeIdx, _>` conflicts with the
   rest of this work.** S12 and W25 both propose it, and on its own it
   saves 48 B instead of 40. But it adds a *ninth* structure keyed by
   node index — see Part 1's index-holder audit — which compaction has
   to rekey on every relocation and which slot reuse has to clear on
   every free. The interning route (row 10) keeps it in the slot, costs
   8 B more, and adds no index holder. **Prefer interning.** This is the
   one place where this annex disagrees with the existing plan.
2. **The refusal guard must be re-tuned, not merely recompiled.**
   Part 2's `per_node = size_of::<TreeNode>() + 64` picks up the new
   `size_of` automatically, but the `+ 64` exists specifically to cover
   the per-node `Option<String>` that row 9 deletes. Spec 0211 left the
   `+ 64` alone deliberately, because that spec touches neither `String`,
   so `per_node` falling 336 → 248 tracks a real drop.

   **Spec 0212 re-derived it and left it at 64, deliberately.** Row 9
   removes the `type_fqdn` half of the ~41 B/node of `String`
   allocations the allowance names, leaving the `rendered_as` half and
   all ~42 B/node of the `heat_states`/`descend`/`dead` parallel arrays
   — so 64 still under-covers what remains, which is the safe direction
   for a guard, and the allowance goes on tracking something real. What
   changed is the comment, which no longer claims to cover `type_fqdn`.
   Re-tuning the number itself needs a measurement of the *guard*, not
   of the slot, and belongs in whichever spec does that. Row 10 is the
   one where the choice is forced: once `rendered_as` is interned too,
   the first half of the `+ 64` names nothing at all, and left alone the
   guard would silently loosen — a larger document accepted before
   refusal, arrived at by accident. Take it to 0 deliberately there.
3. **Row 9 crosses the crate boundary.** `type_fqdn` lives on
   `NodeSpan`, which is `prototext-core`'s type, and
   `prototext_core_constraints` records scope discipline there. The
   alternative worth weighing: keep `NodeSpan` as the library's wire
   format and convert to a protolens-local packed node inside
   `build_tree`. That decouples the change entirely, and has the side
   benefit that `local_tree` (lever B) can then be built narrow even
   before the library moves.

   **Spec 0212 took the crate-boundary route, not the local packed
   node**, for three reasons. First, a local node would leave `NodeSpan`
   at 96 B, and the flat span list is *itself* one of the four places
   the constant is paid — the render cache stores `NodeSpan`s, so a
   protolens-local narrowing would not move that term at all. Second,
   the conversion would have to be written and maintained twice over,
   once each way, since spans travel back out of protolens into
   `render_node_as`'s splice. Third, the library's own consumers benefit:
   the narrowing is a pure win for anything indexing a render, and the
   scope-discipline concern is about *extending* the rendered grammar,
   not about the width of an index type. The cost is one real semver
   break for external users of `decode_and_render_indexed`, accepted.
4. **`build_tree` is `spans.into_iter().map(..).collect()`.** Source and
   destination element sizes already differ, so the source `Vec` stays
   alive alongside the fully allocated destination. Narrowing widens
   that ratio (96 → 32 versus 264 → 72), it does not close it. If the
   transient still matters, `reserve` + `push` + drop-as-you-go.
5. **The `u32` ceiling.** `NodeIdx = u32` caps the arena at ~4.29 G
   nodes. The worst figure ever observed here is `tree.len()` =
   13 499 684, and `capacity()` = 18 004 056 — a 238× margin. Safe, but
   make it a type alias and a named sentinel constant so the cap is one
   line to revisit rather than a repo-wide `usize` hunt. Spec 0211 did
   exactly that: `decode::NodeIdx` and `decode::NO_NODE`. The sentinel is
   `NodeIdx::MAX` rather than a `NonZeroU32` index-plus-one, because
   `build_tree` is post-order and slot 0 is a real node.
6. **`packed_record_start` uses `0` as a legal value.** Part 4 already
   flags this field's rebasing as a live bug source. A `u32::MAX`
   sentinel is correct; a `NonZeroU32` or an index-plus-one is not,
   because offset 0 is a real offset. Spec 0212 did exactly that:
   `NO_PACKED_RECORD`.
7. **An intern table needs exactly two sentinels, not one.** Spec 0212
   found this the hard way. `NO_FQDN` marks a span with no resolved
   type, and `id_of` — the lookup that turns a name into an id so it can
   be compared against a span's — must answer something *else* for a
   name it has never seen. The idiom that replaces
   `span.type_fqdn.as_deref() == Some(name)` is
   `span.type_fqdn == table.id_of(name)`, and the string form is `false`
   for a typeless span. Were the miss to answer `NO_FQDN` it would
   instead compare *equal* to every typeless span, so on a document
   containing no `google.protobuf.Any` every scalar would report itself
   as an `Any`. A second reserved value (`UNINTERNED`) that no span can
   hold makes the substitution exact with no guard at the call sites —
   and there are about fifty of them, so a guard was not an option.
   Row 10 will want the same shape.

### Suggested order

Different from S12's, because the batch changes the priorities.

1. ~~**Row 1, `NodeIdx`.**~~ **Done, spec 0211.** Taken first rather than
   second: it is the largest single row at 88 B, it lives entirely inside
   protolens's own type so trap 3 does not apply, its call sites are
   disjoint from the ones the scalars touch, and it settles the index
   type that lever A's free list and any future window store both want
   to build on.
2. ~~**Rows 2-8, the scalars.**~~ **Done, spec 0212**, together with row
   9 — the two were taken in one pass because both cross the crate
   boundary and the call-site churn overlaps almost completely. Three of
   the four decided points here changed on contact with the code (see
   trap 3 for the fourth): the cap is `MAX_INDEXED_BUFFER` =
   `u32::MAX / 8` (511 MiB), refused by
   `decode_and_render_indexed` rather than at open time, because the
   renderer already reserved `buf.len() * 8` unconditionally and so had
   an unnamed ceiling that *aborted* instead of refusing; `field_number`
   needs no saturation, since the wire format bounds it at 2²⁹ − 1;
   `level` needs no `debug_assert`, since `MAX_WIRE_DEPTH` = 1000 is
   enforced at decode. And `text_range` did **not** go away — spec
   0210's "no production reader" finding is about the *arena's* stale
   copy, not the flat list the library returns, which has three live
   readers.
3. ~~**Row 9, FQDN interning.**~~ **Done, spec 0212.** The table
   (`FqdnTable`) is owned by the caller and passed in, not created
   per-call: an id has to mean the same type in a freshly spliced span
   as in the arena around it, and a per-call table would make that
   silently false. Row 10 reuses it.
4. **Row 10, `rendered_as`.** Needs row 9's table. Take the interning
   route, not the side table — see trap 1. This is also where
   `STRING_ALLOWANCE` must be re-derived, per trap 2.
5. **Row 11, hot/cold split.** Only if the navigation profile still
   justifies it after the above. It is the one row here that is a
   refactor rather than a retype.

Then re-run Part 3's measurement before touching levers A, B or C — the
three transients they address will all have moved, and the case for each
should be re-argued against the new numbers rather than against these.
- [caches.md](caches.md) — the two byte-bounded MRU caches.
