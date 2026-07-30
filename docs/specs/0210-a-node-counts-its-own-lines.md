<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0210 — a node counts its own lines

Status: step 1 implemented, step 2 draft
Implemented in: 2026-07-29 (step 1)
App: protolens
Refs: docs/specs/0135-protolens-packed-run-normalization.md (packed runs
        are re-spanned, not shifted — the one case where a line count
        changes without a splice),
      docs/specs/0167-protolens-render-overrides-deferred-line-splice.md
        (why `lines` is written only at the end of a batch),
      docs/specs/0185-protolens-preview-overlay-anchor.md
        (`visible_rows` is relied on to be sorted ascending),
      docs/specs/0186-protolens-incremental-line-map-repair.md (the
        repair walk this deletes; its N1 accepts O(nodes after the
        splice) as the bound),
      docs/specs/0187-protolens-window-scoped-highlighting.md (S3 — the
        precedent: a document-sized parallel array deleted for exactly
        this reason),
      docs/specs/0188-protolens-line-map-suffix-clear.md (the `fill`
        that replaced `HashMap::retain`),
      docs/specs/0203-the-override-arena-is-compacted.md,
      docs/specs/0206-the-arena-reuses-its-dead-slots.md (independent of
        this spec — S9 says why),
      docs/protolens/design/arena-and-batch.md (the resident-cost table
        and the slot annex, whose row 3 already narrows `text_range`)

## Background

Overriding `/1` on `googleapis.desc` — the *first* positional field of
a 4.5 M-node document — takes a little over a second, and so does
deactivating, reactivating, or deleting that same override. Only `/1`'s
own rendering changes. Everything else in the document keeps its
content and merely moves down or up by a few lines.

The time is not in the splice. It is in `finalize_override_batch`'s
repair walk (`override_apply.rs:1880-1887`):

```rust
let mut after = self.tree[last].doc_next;
while let Some(a) = after {
    if delta != 0 { Self::shift_span(&mut self.tree[a].span, delta); }
    self.map_node_lines(a);
    after = self.tree[a].doc_next;
}
```

This is O(nodes *after* the splice) by design — spec 0186 N1 states
that bound and accepts it. For a splice at `/1`, "after" is the entire
document. Per node it reads and writes the 264-byte slot's `span`,
writes `line_to_node[start]`, usually `footer_line_to_node[end - 1]`,
and chases `doc_next`: roughly three to four cache misses, scattered
across a 1.19 GB arena and two 42 MB maps. 4.5 M × 4 × ~80 ns ≈ 1.4 s,
which is the right order for the second that is observed.

`rebuild_visible_rows_from(from)` then runs with `from ≈ 0`, so its
surviving-prefix `partition_point` keeps nothing and all ~5.28 M
entries of `visible_rows` are rebuilt (`navigation.rs:55-67`).

Every byte of that work maintains one *derived* fact: a node's absolute
line number. Nothing about the node itself changed.

### The precedent

This is the same shape of problem spec 0187 already fixed one level up.
`window_styles` used to be a `Vec<LineStyles>` parallel to `lines`,
costing a whole-document tree-sitter parse per load and per commit — 85%
of a commit measured — to produce data of which only the ~50 rows on
screen were ever read. S3 made it window-sized. The line maps and the
span shift are the same eager-whole-document reflex, not yet walked
back.

### Why "size", not "offset relative to the parent"

Both encodings make a node's stored value independent of the document's
absolute position. They differ in what a change costs, and only one of
them delivers the stability that motivates the change:

| | mutate a subtree by δ lines | resolve a line from scratch |
| --- | --- | --- |
| child stores its start relative to its parent | rewrite **every following sibling** at every level of the root path — 7 770 slots for `/1` on googleapis | O(depth): sum the offsets up the root path |
| child stores its own subtree's line count | rewrite the node and its ancestors' counts — **O(depth)**, tens of slots | O(depth × fanout): scan preceding siblings at each level |

Storing the offset still leaves a node's state dependent on what
precedes it, so moving a subtree up or down a few lines still touches
its siblings. Storing the count does not: a node's value depends only on
its own subtree, and only the ancestor chain's totals change.

The trade is the right way round. Mutation is the frequent, expensive
thing being fixed. From-scratch resolution happens only on a teleport
(search, jumplist, mouse click, `gg`/`G`), where microseconds are
invisible — and ordinary navigation never pays it at all (S3).

### What one `u32` replaces

`text_range` is `start..end` over the node's *whole* rendered block,
children included: `map_node_lines` writes `line_to_node[start]` for the
header line and `footer_line_to_node[end - 1]` for the closing brace
(`override_apply.rs:2087-2089`). So `end - start` **is** the subtree's
line count. Keep the count, derive `start` from the ancestor chain, and
`end = start + count`. 5.28 M lines against `u32::MAX` is an 810×
margin.

### The one gap in the accounting: malformed fields

Deriving a position by summing counts only works if the counts add up —
if every rendered line belongs to exactly one node. Today one class of
line does not. `IndexingTextSink::malformed` (`sink.rs:1284`) delegates
without pushing a `NodeSpan`, while the render underneath it
(`sink.rs:757-827` → `render_invalid`, `render_invalid_tag_type`,
`render_truncated_bytes`) writes a line and a `newline()` for all seven
`MalformedKind` variants. An `INVALID_VARINT` or `TRUNCATED_BYTES` line
therefore sits between two children with nothing pointing at it. On the
blobs protolens exists for — no schema, a wrong override, a truncated
capture — that is ordinary, not exotic.

Today this is harmless, because positions are read straight out of the
render's own line counter. Under counters it would silently corrupt
every position after the first malformed field, so S1 closes it.

(`virtual_scalar`, `sink.rs:1243`, has the same shape but protolens
never reaches it: `expand_any` and `expand_message_set` are both off,
`decode.rs:1022-1023`, precisely so that every field gets a real
`NodeSpan`.)

### Where the walk terminates

There is exactly one parentless node, and its first line is line 0.
`render_resolved` wraps the whole blob as field 1 of a virtual
encompassing message (`wrap_blob(1, blob)`, `decode.rs:1005`), so the
decode emits a single level-0 span — the wrapper — and post-order
`build_tree` makes every other node its descendant. That wrapper's
header is `lines[0]`, which `decode.rs:1044` patches by index. No
file-level `#@ prototext: protoc` header is emitted in this
configuration, so there is no leading offset to account for. On
googleapis this root is the node with 7 771 children.

## Goals

- **G1.** A commit costs O(what it splices) + O(the depth of the splice
  point), not O(the document after it).
- **G2.** Overriding the *first* top-level field costs about what
  overriding the *last* one costs.
- **G3.** Fold and unfold likewise: O(depth), not a full rebuild.
- **G4.** No durable state is keyed by an absolute line number, except
  `lines` itself.
- **G5.** Teleports stay imperceptible, and ordinary navigation does no
  resolution work at all.
- **G6.** Resident memory goes down.

## Non-goals

- **N1.** Doing both steps at once. Step 2's specification below was
  *provisional*: written so the direction was on record and so step 1
  was not designed into a corner, and to be revised against step 1's
  measurements before any of it was implemented. That revision is done:
  it added S11, which is now step 2's first change, and corrected S6 and
  S10. S8 itself still stands as written and is unimplemented.
- **N2.** Making decode or render faster. The blob is decoded and
  rendered exactly as today.
- **N3.** Changing children from a linked list to a contiguous slice.
  That is an optimization *behind* S2's interface and should be done
  only if measurement asks for it.
- **N4.** A rope or piece table over the text.
- **N5.** Anything in the arena-growth line of work (specs 0203, 0206).
  Orthogonal, and composes — see S10. Neither step depends on the
  arena's layout; S9 says why.
- **N6.** Changing what is rendered. Not one character of output moves.

## Implementation steps

**Step 1 — S1 to S6.** Positions stop being stored and start being
derived from per-node counters. `lines` is untouched. This is
self-contained and shippable on its own, and takes the motivating case
from ~1 s to the residue in S10.

**Step 2 — S11 first, then S7 to S9.** S11 deletes the span-shift walk
that step 1 left behind in `render_overrides_inner`, which is what the
motivating case actually costs now. Then the rendered text moves out of
the flat `lines` array and into the nodes, which removes the remaining
residue and unblocks lazy rendering.

The sequence is deliberate: **implement step 1, then revise step 2's
specification against what step 1 actually measured, then implement
step 2.** The revision has happened and S11 is its product; S7 to S9
remain a direction with estimated numbers rather than a settled design,
the search figures in S9 especially.

Step 1 is also a prerequisite, not merely a convenient first half: while
`text_range` holds absolute indices *into* `lines`, `lines` is obliged
to be a flat absolutely-indexed array and step 2 is not expressible.

## Specification

### S1. Every line belongs to a node, and each node counts its own *(step 1)*

**Premise.** `IndexingTextSink::malformed` pushes a `NodeSpan` for the
line it renders instead of delegating only: one line, `is_message:
false`, `text_range: start..start + 1`, the `raw_range` and wire type it
already has in hand. Malformed fields become ordinary scalar-shaped
leaves. Not one character of rendered output changes — this adds an
index entry, not a syntax form — and with it every rendered line belongs
to exactly one node, which is what makes the identities below exact
rather than assumed.

Two consequences are user-visible and are accepted deliberately:

- **Positional paths shift** in documents that contain malformed fields.
  `sibling_ordinal` counts nodes among siblings (`decode.rs:577`), so a
  malformed field renumbers everything after it, and an already-saved
  override file for such a document points one position off. This is the
  more logical numbering: the path is meant to be positional, and today
  a malformed field is an invisible hole in it.
- **Malformed lines become cursor stops.** Today `line_to_node` is
  `None` there, so `j`/`k` skip over them and they cannot be selected.
  They become navigable and extractable like any other leaf. An
  improvement.

**The counters.** Each node stores:

- `lines_total: u32` — lines its subtree occupies, header and footer
  included.
- `lines_visible: u32` — the same count with folded subtrees collapsed.

with the invariants

```
lines_total(n)   = 1 + Σ lines_total(c)   + footer(n)
lines_visible(n) = if folded(n) { 1 }
                   else { 1 + Σ lines_visible(c) + footer(n) }
```

**`header` and `footer` cost nothing to store, because they are not
free variables.** A node's own header is always exactly one line — every
span-producing sink event writes one line before recursing
(`scalar_field`, `begin_nested`, and now `malformed`) — and its footer
is one line if and only if `lines_total > 1`, zero otherwise. That is
already the rule the code states in two places: `has_children`
(`navigation.rs:24-27`) tests exactly `end - 1 > start`, and folding
hides exactly `start + 1 .. end` (`navigation.rs:84-88`), which is why a
folded node's visible count above is `1` and not `header(n)`.

`TreeNode` stops reading `span.text_range` once `build_tree` has
consumed it. The field itself stays in `prototext_core::NodeSpan` for
step 1 — that struct is the decode interface, and protolens needs the
absolute range at build time to derive the counters in the first place.
Carrying it afterwards costs 16 B × 4.5 M = 72 MB of a field that is
correct at build time and stale forever after, which is accepted for
step 1 and removed in step 2, where `TreeNode` stops embedding
`NodeSpan` wholesale and copies out only the fields it still uses.

Note the counters do **not** shrink the slot relative to the compaction
plan: the annex's row 3 already narrows `text_range` to `(u32, u32)` =
8 B, and two `u32` counters are also 8 B. The memory win in G6 comes
entirely from the side arrays S4 and S5 delete.

### S2. Position is derived, never stored *(step 1)*

```
absolute_start(n) = absolute_start(parent) + 1
                  + Σ lines_total(preceding siblings of n)
```

terminating at the single parentless node, whose start is 0 (see
"Where the walk terminates"). Cost is O(depth × fanout).
On googleapis that is dominated by the root's 7 771 children, and
because siblings are a linked list (`decode.rs:483-485`) it is 7 771
pointer chases ≈ 0.5 ms, not a prefix sum over an array. That is fine
for a teleport and is what N3 leaves open.

`absolute_footer(n) = absolute_start(n) + lines_total(n) − 1`, which is
what `cursor_footer` needs.

### S3. Ordinary navigation is incremental *(step 1)*

The cursor carries its absolute line as a cached value alongside its
node index. `Up`, `Down`, `PageUp`, `PageDown` and scrolling adjust it
by the lines actually crossed — they already walk the nodes they cross.
S2 is invoked **only** on a teleport: search, jumplist pop, mouse click,
`gg`/`G`, and the initial position.

This is what keeps S2's O(depth × fanout) off every hot path. The
viewport is a contiguous line range, so once the top row's node is
known, the frame is a `doc_next` walk — the resolution index is not in
the per-frame path at all.

When a commit splices above the cursor, the cached value is adjusted by
the commit's own delta — O(1), and the delta is already computed
(`pending_shift`).

### S4. `line → node` becomes a descent *(step 1)*

`line_to_node` and `footer_line_to_node` are deleted (85 MB — 2 ×
5.28 M × 8 B, `Option<u32>` having no niche). Resolving a line walks
down from the root, at each level choosing the child whose cumulative
`lines_total` range contains the target, with a header/footer test at
each node. Same cost as S2 and the same call sites.

`map_node_lines` is deleted. The two `fill(None)` passes in
`finalize_override_batch` (`override_apply.rs:1853-1854`) go with it.

### S5. `visible row → node` is the same descent *(step 1)*

`visible_rows: Vec<usize>` (42 MB) and `hidden_mask: Vec<bool>` (5.3 MB)
are deleted. "The Nth visible row" descends on `lines_visible` exactly
as S4 descends on `lines_total`.

The consequence worth stating separately: **fold and unfold become
O(depth)**. Today the fold path deliberately passes `from = 0` to
`rebuild_visible_rows_from` (spec 0186 N4), so every fold toggle rebuilds
all 5.28 M entries. That is a second user-visible stall, not currently
filed, that this deletes for free.

Spec 0185's preview overlay relies on `visible_rows` being sorted
ascending. A descent yields rows in ascending order by construction, so
the property survives, but the anchor code must be re-expressed against
the new accessor rather than the vector.

### S6. What a commit does instead *(step 1)*

After `splice_override` and `materialize_line_patches`:

1. Recompute `lines_total` / `lines_visible` for the freshly spliced
   subtree (bounded by the splice).
2. Propagate both deltas up the ancestor chain — O(depth).

That is all. The repair walk at `override_apply.rs:1880-1887` and the
`rebuild_visible_rows_from` call are both deleted.

`pending_shift` survives as the quantity S3 uses to fix up the cursor,
but nothing else consumes it.

**Both of those last two sentences turned out to be wrong**, and S11
is what follows from it. "That is all" describes the *finalizer*
correctly, but the same walk also exists inside
`render_overrides_inner`, which step 1 did not touch: the carried-down
`inherited`/`child_owed` correction, its pruned-sibling arm, and the
ancestor `text_range.end` growth at `override_apply.rs:1826-1828`. And
`pending_shift` has a second consumer that predates S3 —
`splice_override`'s patch-target arithmetic (`pending_shift_before`,
`children_base_shift`), which is unrelated to the cursor and stays.

**Packed runs.** Spec 0135's normalization re-spans a packed run's
elements rather than shifting them, which is precisely the case spec
0188's comment cites as making positional preservation unsound. Under
counters it is not a special case at all — normalization changes the
elements' `lines_total`, and the propagation in step 2 carries it up
like any other change. This is a simplification, but it must be tested
rather than assumed.

### S7. What `lines` is, and why step 1 leaves it alone *(step 2)*

`lines: Vec<String>` is the entire document pre-rendered to text, one
`String` per line, produced eagerly at load. On the reference corpus
that is 5.28 M strings: 127 MB of `String` headers plus roughly 230 MB
of text bytes and 5.28 M separate heap allocations — call it ~360 MB,
about 18% of the 2045 MiB startup RSS.

Four things read it: the ~50 visible rows per frame, `/` search,
extract/clipboard, and the tree-sitter highlight window. Only the first
is per-frame, and it needs 50 of 5.28 M lines — 0.001%.

It is also the last O(document) term in a commit. Inserting a line at
position 1 shifts n−1 elements: **127 MB moved** (the `String` headers
only — the text bytes stay behind their pointers and do not move).
Spec 0167 already reduced this from one such memmove *per patch* to one
*per batch*, which is as far as a flat array can be taken.

### S8. The text moves into the nodes *(step 2)*

Each node owns the lines it renders **itself**, excluding its children's:
a scalar node's single `foo: 42`, a message node's `file {` header and
its `}` footer. `lines` is deleted.

Crucially that is **one or two lines per node, not many** — which is
what makes the storage cheap rather than expensive. A `Box<str>` per
node holding its own line(s), newline-separated, is 16 B of header:

| | headers | allocations | text |
| --- | --- | --- | --- |
| today, `Vec<String>` | 5.28 M × 24 B = 127 MB | 5.28 M | ~230 MB |
| per-node `Box<str>` | 4.5 M × 16 B = 72 MB | 4.5 M | ~230 MB |

So step 2 is roughly **memory-neutral to slightly better**, which is not
the reason to do it. The reason is that inserting or removing lines
becomes O(1): there is no global array left to shift, and S7's 127 MB
memmove disappears entirely along with the deferred-patch machinery
(spec 0167) that exists to amortize it.

It also unblocks a genuinely lazy render — with the text per-node and
positions derived, nothing forces the text to exist at all until a row
is drawn — but that is a further step again and is not specified here.

**The three other readers of `lines` all become the same walk.**

- **Extract**, in prototext format, walks the target subtree in document
  order and concatenates each node's own lines. O(subtree), which is
  already what producing that text costs.
- **Clipboard** walks the *visible* nodes of the selection — the same
  walk driven by `lines_visible` rather than `lines_total` — and
  concatenates.
- **The tree-sitter highlight window** concatenates the ~50 rows it is
  already scoped to (spec 0187 S3).

Neither of the two things that might pollute the highlight window
actually does.

- **Heat cues never enter the text.** They are assembled per row at draw
  time as `Span::styled(HEAT_GLYPH, …)` row chrome (`render.rs:661`,
  `heat_chrome`), so a concatenation of node text cannot pick them up.
- **The `...` truncation marker is preview-only.** It appears in the
  override-select pane's live preview content and never in the committed
  document, so the document-text path this section describes never sees
  it. It matters — it is genuine text, it is a syntax error, and
  tree-sitter's error recovery from it swallows *following* siblings
  (spec 0187's finding) — but only on the preview path, which step 2
  does not change.

So the highlight window's input under step 2 is the same clean prototext
it is today, obtained by concatenation instead of slicing.

### S9. Search does the dumb thing *(step 2)*

Search today scans `lines` sequentially. With the text in the nodes it
must walk the document instead, following `doc_next`: ~4.5 M
cache-missing pointer chases through the arena, estimated **300-400 ms**
against today's ~36 ms. **Accept the ~10×.**

This is a deliberate choice to do the simple thing first and measure,
rather than to design against an estimate. A full-document search is a
deliberate, infrequent user action, not a per-frame cost, and 300 ms is
not a hang.

**The clever version, and why it is not being taken.** A linear scan of
the arena is sequential rather than pointer-chasing — 1.19 GB streamed
(324 MB after slot compaction) instead of 4.5 M random accesses,
plausibly restoring today's numbers. Two reasons not to:

- Arena order is post-order, not document order, so `/` — "the next
  match *after the cursor*" — needs the matches re-ordered afterwards.
  An earlier version of this argument claimed that was cheap because
  "matches are few". **That is wrong**: a single-character pattern, or a
  wildcard, matches a large fraction of 5.28 M lines, and the ordering
  step then becomes the new O(document) cost. The trick optimizes
  exactly the searches that were already fast.
- It would require the arena to *stay* post-ordered, which spec 0206's
  slot reuse does not preserve.

**Nothing else in this spec depends on the arena's layout**, and that is
worth stating explicitly because an earlier draft claimed a conflict
with spec 0206 that does not exist. Every traversal in both steps is
*link*-based, never index-based: `absolute_start` walks `parent` and
`next_sibling`, the S4/S5 descents walk `first_child`/`next_sibling`,
S6 propagates up `parent`, and the search above follows `doc_next`. A
node's position in the arena is never read as information. So 0206 may
land before or after either step without interacting with it.

(Spec 0206 does have an ordering problem, but with *existing* code, not
with this spec: `collect_descend_targets(start, end, …)` scans an index
*range* on the assumption that nodes added since the last batch form a
contiguous suffix above a watermark — `compute_descend_marks`' `scanned`
and `mark_fresh_subtree`'s `base`. Slot reuse breaks that. It belongs in
0206's own audit.)

### S10. What still costs O(document), and how much

**After step 1:** `materialize_line_patches` still merges into `lines`
at the patch position — for `/1` that is near line 0, so 127 MB moved,
estimated 15-30 ms. The honest target for the motivating case is
therefore **a few tens of milliseconds, not zero**: roughly a 30-50×
improvement on the measured second, with the residue concentrated
entirely in `lines`.

**That last clause was wrong**, and the measurement says so: there are
*two* residues after step 1, not one. The `lines` merge is 82-100 ms —
worse than estimated, and it moves the same 127 MB wherever the patch
lands, so the patch position this paragraph reasoned from does not
enter into it. The other residue is four times larger and is the
asymmetric one: the span-shift walk S11 deletes.

**After step 2:** nothing. S8 removes the `lines` merge and S11 the
span-shift walk.

Composition with the arena work: the walk step 1 deletes is miss-bound
over a 264 B slot, so specs 0203/0206 and the slot compaction shrink the
same term. They do not conflict on this point and neither subsumes the
other — compaction makes the walk ~3.7× cheaper, step 1 removes it. The
one place they do collide is S9's second bullet.

**Estimated memory delta:** step 1 gives −85 MB (line maps) −42 MB
(`visible_rows`) −5.3 MB (`hidden_mask`) ≈ **−132 MB** of the 2045 MiB
startup RSS, with the slot unchanged against the compaction target.
Step 2 adds a further ~−55 MB of `String` headers, and ~0.8 M fewer
allocations.

### S11. The span-shift machinery is deleted *(step 2, and first)*

This section is written last but is to be done first: it is small, it is
independent of S8, and on its own it is what makes test-plan item 5
pass. S8 can then be judged on the `lines` merge alone, which is what it
was always about.

**What is deleted.** `NodeSpan.text_range` stops being maintained after
build time, and everything whose only purpose is to maintain it goes
with it:

- `shift_span` and its three call sites — the prologue's correction to
  `idx` and to its packed-run siblings (`override_apply.rs:1652-1661`),
  and the pruned-sibling subtree walk (`1770-1818`), which is the whole
  4.5 M-span cost.
- The `inherited` parameter of `render_overrides_inner` and the
  `child_owed` accumulator threaded through its child loop.
- The ancestor footer growth at `1826-1828`.
- The absolute translation of a freshly spliced subtree's ranges at
  `2525-2526`. `decode::build_tree` derives `lines_total` from the
  *local* range before this runs (`decode.rs:555-560`), so the
  translation feeds nothing else. `raw_range`'s translation on the
  adjacent line is unrelated and stays — it is read for real, by
  extract, by `packed_record_extent` and by the heat cue.

`pending_shift` is **not** deleted: `splice_override` still needs it to
place a patch (`pending_shift_before`, `children_base_shift`). It is
the *per-node* propagation that goes, not the batch's running offset.
S8 removes the patch machinery and with it the last consumer.

**Why this is safe: no production code reads the maintained value.**
Audited exhaustively over `.text_range` in `protolens/src`. Every read
is one of three kinds, and none of them needs the field to have been
kept up to date:

1. **Build-time, and exact there.** `decode.rs:560` turns the local
   range into `lines_total`. `insert_truncation_marker`
   (`override_apply.rs:309-317`) adjusts the freshly rendered local
   spans before `build_tree` sees them. Both operate on a render's own
   output, in that render's own coordinates.
2. **Re-derived from the counters immediately before use.**
   `render_node_as:2769` overwrites `old_span.text_range` with
   `node_lines(idx)`, which is the only value `splice_override` and the
   preview overlay ever see — so the delta at `2354`, the patch target
   at `2452-2479`, and spec 0185's overlay anchor
   (`override_select.rs:822-825`) are all counter-derived already.
   `packed_record_extent:3017-3021` likewise sums `lines_total` instead
   of reading the field. `extract` is handed a range by
   `command_line.rs:581`, again from `node_lines`.
3. **Write-only.** What `shift_span` and the two translations maintain
   is read by nothing at all after step 1.

**The one reader that does depend on it is a test**, and retiring it is
correct rather than a loss:
`every_derived_position_equals_the_range_the_renderer_recorded`
(test-plan item 3) asserts `node_lines(idx) == span.text_range` for
every live node of every real-decode fixture — and those fixtures have
been through `App::new`'s startup splices, so the equality holds only
because `shift_span` maintained it. The test's stated purpose is to
compare the new derivation against the number the old implementation
stored; once the old number is gone there is nothing left to compare,
and the invariant that replaces it — the counters against a full
recount, plus the root's total against `lines.len()` — is test-plan
item 1, which is already the stronger check.

**Ordering against S8.** S8 deletes `NodeSpan` from `TreeNode`
altogether, so it subsumes this section's field deletion; what it does
not subsume is the walk, which would otherwise still be there
maintaining a field nobody reads. Doing S11 first also means item 5 is
green before the much larger change starts, so a regression in S8 has
something to be measured against.

## What step 1 measured

These are the numbers step 2's specification is to be revised against
(N1). They were taken with step 1 in, on `googleapis.desc`
(4 501 014 nodes, 5 281 124 lines), through the pty driver of
`docs/protolens/design/arena-and-batch.md`: open the document, put the
cursor on one top-level record, apply `:type-as-raw` to it, and read
back the whole keystroke's duration from `PROTOLENS_TRACE`'s
`key Enter us=`. Temporary probe counters inside the batch split that
duration into its phases.

| | first top-level record | last top-level record |
|---|---|---|
| whole keystroke | **500-541 ms** | **108 ms** |
| the walk (`render_overrides_inner`) | 401-412 ms | 18.5 ms |
| …of which the splice itself | 9.2-9.4 ms (42 nodes) | 12.0 ms (1336 nodes) |
| the finalizer | 88-100 ms | 87 ms |
| …of which the `lines` rebuild | 82-100 ms | 84 ms |
| spans shifted by pruned siblings | **4 500 963** | — |

Answering the four questions in order:

- **The residual `lines` memmove is 82-100 ms**, 3-6× S10's estimated
  15-30 ms. It is also the *symmetric* term — first and last record
  cost the same — which is what identifies it: the rebuild allocates a
  fresh `Vec` and moves every one of the 5.28 M 24-byte `String`
  headers regardless of where the patch lands, so the patch position
  S10 reasoned from does not matter. Step 2's justification stands and
  is stronger than estimated.
- **`absolute_start` does not appear in the profile at all**, and
  neither do the S4/S5 descents. Against S2's estimated 0.5 ms this is
  below the noise of a 100 ms measurement. N3's contiguous-children
  change is not forced.
- Full-document search was not measured; it is step 2's baseline, not
  step 1's, and nothing in step 1 changed it.
- **No call site needs `absolute_start` per row** (open question 4).
  Rendering a frame spends ~54 µs total on line work for ~50 rows, so
  S3's argument holds and neither step changes shape. This also
  disposes of test-plan item 8's cached cursor line: there is nothing
  for it to save.

### Test-plan item 5 fails: the walk kept a copy of the deleted pass

Item 5 is the spec's headline claim, and at 4.6× apart the two records
are not "within a small factor". The asymmetry is entirely in the walk,
and `shifted = 4 500 963` names the culprit exactly:
`render_overrides_inner`'s pruned-sibling arm
(`override_apply.rs:1770-1818`) walks each pruned sibling's whole
subtree by `doc_next`, calling `shift_span` on every node. Summed over
the top-level records after the splice, that is 4.5 M spans for an
override on the first record and none for one on the last.

This is a *second* copy of the very walk step 1 deleted — the arm's own
comment says so, justifying its cost as "the same walk, and the same
contiguity assumption, as `finalize_override_batch`'s pass 2 … which
that pass already costs and which pass 1's re-mapping already costs
unconditionally". Both of those passes are now gone, so the argument
that made the arm free has been removed from under it, and the arm is
now the whole of the O(document) cost this spec set out to remove. The
Background's sentence applies to it verbatim: every byte of that work
maintains one derived fact, a node's absolute line number, and nothing
about the node itself changed.

The correction it carries (`inherited`, `child_owed`, `shift_span`)
exists only to keep `NodeSpan.text_range` accurate — the field S1 says
`TreeNode` stops reading once `build_tree` has consumed it, and which
step 1 knowingly left in place and stale. If that is true of *every*
reader, then the arm, the carried-down correction and `shift_span`
itself are all pure waste and can simply be deleted, and item 5 passes
without any new mechanism.

**The audit was done and it is true of every reader.** S11 records it
and specifies the deletion. So step 2's first task is not the `lines`
memmove it was scoped around; it is removing a walk that maintains a
field nobody reads.

## Test plan

Items 1-11 are step 1; items 12-14 are S11. The rest of step 2's test
plan is deliberately not written yet; it is to be produced when S8 is
revised.

The existing `verify_repair` hook (`assert_repair_matches_full_rebuild`)
is the right oracle and should be kept and extended rather than replaced:
it already asserts that an incremental repair matches a full rebuild, and
"the counters agree with a full recount" is the same assertion.

1. **Counters agree with a recount, and the lines add up.** After every
   commit in the suite, a bottom-up recount of `lines_total` and
   `lines_visible` matches what is stored, for every node — and the
   root's `lines_total` equals `lines.len()`, which is what actually
   catches an unowned line (S1's premise, open question 1).
2. **Malformed fields are nodes.** A blob with an `INVALID_VARINT`
   between two well-formed fields yields a node for it, one line, with
   the right `raw_range`; the following sibling's `sibling_ordinal`
   accounts for it; and the rendered text is byte-identical to before
   the change.
3. **Position agrees with the old maps.** For a document loaded without
   any override, `absolute_start(n)` equals the `text_range.start` the
   current implementation produces, for every node. This is the
   equivalence test that makes the change reviewable. **Retired by
   S11**, which stops maintaining the number it compares against; item 1
   is what remains, and is stronger.
4. **Descent is the inverse of position.** For every line `l`,
   `line_to_node_descent(l)` is the node whose `absolute_start` is `l`,
   or `None`.
5. **First field costs like last field** (G2). Commit-time for an
   override on `/1` and on the last top-level record are within a small
   factor. This is the spec's headline claim and needs a real corpus, so
   it belongs with the manual pty driver in
   `docs/protolens/design/arena-and-batch.md`, not in the unit suite.
   **Measured, and it fails after step 1** — 4.6× apart, because a
   second copy of the deleted walk survives in
   `render_overrides_inner`'s pruned-sibling arm. See "What step 1
   measured"; it is step 2 that has to make this pass.
6. **Fold is O(depth)** (G3). Toggling a fold does not touch a number of
   nodes proportional to the document.
7. **Packed runs.** A normalization that re-spans a packed run without
   splicing propagates correctly (S6).
8. **The cursor survives a commit above it** — cached absolute line
   adjusted by `pending_shift`, verified against a from-scratch
   resolution. **The cache was not built**, so there is nothing to test:
   `absolute_start` does not appear in the measured profile at all
   (~54 µs of line work per frame, for ~50 rows), so a cache would save
   nothing. The cursor is held as a node index and stays correct across
   a commit without one.
9. **Teleports.** Search, jumplist pop and mouse click land on the same
   line as today, on a document with folds active.
10. **Overlay anchor.** Spec 0185's preview overlay still anchors
    correctly with `visible_rows` gone.
11. **Nothing rendered changes** (N6). The full rendered output for the
    fixture corpus is byte-identical before and after.
12. **Nothing rendered changes, again** (N6, for S11). The same
    byte-identity check across the deletion. This is the one that
    matters: S11's whole claim is that it removes work with no
    observable effect, so the way it fails is a document that renders or
    navigates differently, not a compile error.
13. **First field costs like last field, for real** (G2). Item 5
    re-measured through the same pty driver, with the same two targets.
    The asymmetry must be gone; the residual should be the symmetric
    `lines` merge alone, which is S8's to remove.
14. **The counters still agree with a recount after a pruned splice.**
    Item 1's oracle exercised specifically on a batch that takes S11's
    deleted arm today — a splice with pruned siblings after it, on a
    fixture with a wide top-level sibling group. The arm is the code
    being removed, so the assertion has to run on the path that used to
    reach it, not merely on the suite at large.

## Open questions

1. **Are malformed fields the only unowned lines?** S1's premise is that
   once `malformed` emits a span, every rendered line belongs to exactly
   one node. That was established by reading the sink, and test item 1
   is what turns it into a checked property rather than a belief. The
   one known exception, `virtual_scalar`, is unreachable in protolens's
   configuration — but by option, not by construction, so a future
   change to `expand_any` would reopen it.
2. **Does `lines_visible` belong in the node at all?** It makes fold
   state part of the arena's derived data, so a splice must recompute it
   for fresh nodes. The alternative — a separate parallel array — is what
   this spec is deleting elsewhere, so probably not; but the invariant is
   new and is the most likely source of a subtle bug.
3. **What replaces `pending_shift`'s other consumers?** S6 claims only
   the cursor needs it. That claim was not exhaustively audited.
   **Answered, and it was wrong**: `splice_override` needs it to place a
   patch, which has nothing to do with the cursor, and the cursor cache
   S6 pointed at was never built. See S11.
4. **Is `absolute_start` needed anywhere per-frame after all?** S3 argues
   not. If a call site is found that needs it per row, the linked-list
   sibling scan (N3) becomes a real cost and the contiguous-children
   change is no longer optional. **Answered: no.** It does not appear in
   the measured profile at all. N3 stays optional.
5. **Ordering against specs 0203/0206.** They touch the same struct and
   the same repair path. Doing this first makes their arena smaller to
   reason about; doing them first makes this spec's walk cheaper to
   measure against. No strong argument either way yet, and per S9 there
   is no correctness interaction to force one.
6. **Does concatenating beat indexing for the ~50 drawn rows?** *(step
   2)* S8 assumes the per-frame path walks nodes and concatenates rather
   than indexing an array. That is ~50 items either way, so it should be
   noise — but it is the one reader that runs every frame, and it is the
   place a regression would hide.

## Measured outcome

**Step 1 only.** Step 2 is unimplemented and its specification is to be
revised against the numbers above before it is written.

Step 1 landed as specified: the counters are in the slot, the four side
structures (`line_to_node`, `footer_line_to_node`, `visible_rows`,
`hidden_mask`) are gone, position is derived, and
`finalize_override_batch` is reduced to `materialize_line_patches`.
`size_of::<TreeNode>()` went 264 → 272 B, which raised spec 0202's
`per_node` constant from 328 to 336. Test-plan items 1-4 and 6-11 pass;
item 5 does not, for the reason recorded in "What step 1 measured".

Two implementation notes:

- `insert_truncation_marker` matched its closing brace with
  `trim_end() == "}"`, which never fired because the line is indented.
  It is now `trim()`. The bug was latent before this spec — the marker
  simply landed after the brace instead of inside it — and only surfaced
  because item 11's byte-identity check compares the two placements.
  This is item 11's one accepted difference: the `...` marker moved
  inside the closing brace of a truncated live preview, which is where
  S4 of spec 0174 always said it belonged.
- Malformed fields now carry a `NodeSpan` (S1's premise), so they are
  nodes and therefore cursor stops. Accepted: it is the same treatment
  every other rendered line gets, and positional paths shift
  accordingly.

The probe counters that produced the table are in the module
`override_apply.rs` already marks TEMPORARY; they should be stripped
with it, but not before step 2 has re-measured the same phases.
