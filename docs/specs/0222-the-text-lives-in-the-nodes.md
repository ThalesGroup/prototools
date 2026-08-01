<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0222 — the text lives in the nodes

Status: implemented
App: protolens
Implemented in: 2026-08-01
Refs:
- `docs/specs/0210-a-node-counts-its-own-lines.md` — this spec **replaces
  its S7, S8 and S9**, which it labels "a direction with estimated
  numbers rather than a settled design". Step 1 and S11 are implemented
  and are the foundation here: a node counts its own lines, positions
  are derived, and `LinePos` already names a line as
  `(node, line_in_node)`.
- `docs/specs/0216-...` (the maximal arena) — the arena is immutable and
  slot-indexed, and a packed run is **one slot drawing many rows**. That
  last point is what §S4 exists for.
- `docs/specs/0167-...` (deferred line patches) — the machinery this
  spec deletes.
- `docs/specs/0187-highlighting-is-a-property-of-the-viewport.md` — the
  highlight window is already scoped to the drawn rows; §S8 keeps its
  input byte-identical.
- `docs/specs/0215-the-cursor-knows-which-line-it-is-on.md` — `row_text_of`
  already takes a caller-supplied owner to skip a `line_pos` descent.
  §S3 makes that the only shape.
- `docs/specs/0223-highlighting-yields-to-pending-input.md` —
  **implemented 2026-08-01, after the table below was measured.** The two
  are independent (N3), but 0223 changes what "a frame" costs, so read
  the note under that table before re-measuring anything for G3.

## Background

`App::lines: Vec<String>` is the whole document pre-rendered to text,
one `String` per line, produced eagerly at load and deleted by nothing.

Measured on `googleapis.desc` (25.6 MB, the reference corpus) at the
commit this spec was written against:

| | |
| --- | --- |
| lines | 5 281 124 |
| text bytes | 238 220 090 (mean 45.1 B/line) |
| `String` headers | 5 281 124 × 24 B = 126.7 MB |
| separate heap allocations | 5 281 124 |
| arena slots | 4 737 284 |
| of which rendered | 2 831 045 |
| …bracketed (header + footer) | 780 110 |
| …flat, one line | 1 456 293 |
| …flat, many lines (packed runs) | 594 642, holding 2 264 611 lines |
| longest flat run on this corpus | 16 lines |

The counts close exactly: `780 110 × 2 + 1 456 293 + 2 264 611 =
5 281 124`. That is spec 0210 S1's invariant — every rendered line
belongs to exactly one node — holding on a real corpus.

Two of those rows correct spec 0210 S8, which was written before the
0216 arena landed:

- It assumed **4.5 M nodes at one or two lines each**. The real figure
  is 2 831 045 rendered nodes at **1.87 lines each** — but the average
  hides the shape. **43% of the document's lines live in the 594 642
  multi-line flat nodes**, and nothing bounds how long one gets: 16 is
  this corpus's longest packed run, not a limit. A design that assumes
  "one or two" is wrong on nearly half the document.
- It assumed the storage was **roughly memory-neutral**. It is not, and
  in the favorable direction — see §S9.

### The last O(document) term in a commit

Inserting a line at position 1 shifts n−1 elements: 126.7 MB of
`String` headers moved (the text bytes stay behind their pointers).
Spec 0167 reduced this from one memmove per patch to one per batch,
which is as far as a flat array can be taken. Spec 0210's S11 measured
what is left after everything else was removed:

| | first top-level record | last top-level record |
| --- | --- | --- |
| whole keystroke | 102 ms | 61 ms |
| …of which the `lines` merge | 87.4 ms | 42.9 ms |

The merge is now **most of the keystroke**, at either end of the
document. Spec 0210 flagged the 2× spread between the two columns as
unexplained on one run each; §"Test plan" item 9 re-measures rather than
inheriting either number.

### The per-frame baseline this spec must not regress

The reason to be careful is that the four readers of `lines` are not
equal: one of them runs on every frame. Measured through a pty driver at
50×200 on the reference corpus, `PROTOLENS_TRACE` reading `key … us=`,
`render … us=` and `draw … us=`, 48 drawn rows, 20 `PageDown` then 20
`PageUp` from the top and 15 of each after `G`:

| µs per frame | top of document | after `G` (end) |
| --- | --- | --- |
| `key PageDown`/`PageUp` | 8–14 | 30–49 |
| `window` (descent + walk) | 5–62 | 17–89 |
| `styles` (tree-sitter) | **364–486** | **220–491** |
| `heat` | 49–102 (5–12 on later frames) | 33–69 |
| `ovr` | 11–42 | 6–19 |
| `lines` (row spans, incl. text) | 52–80 | 32–103 |
| whole `terminal.draw` | 1249–2637 | 658–1556 |

Three things follow, and they are the whole answer to "will paging get
slower":

1. **Paging is already O(page), not O(document).** `move_page_down`
   takes `page` O(1) `next_visible` steps and then **one** `carry_caret`
   (spec 0215 S2). The end of the document costs 30–49 µs against 8–14
   at the top — the single `absolute_start` inside `carry_caret` crossing
   the root's 7 771 children. It is 3–4× worse and it is 40 µs.
   *(The doc comment at `navigation.rs:454` still quotes "13–23 ms
   against 47 µs" for the one-fix-up-per-row variant. That predates spec
   0216 S23's contiguous sibling scan; the current cost of one
   `absolute_start` at the end of this corpus is ~40 µs, so 48 of them
   would be ~2 ms, not 13–23. The comment's conclusion stands, its
   numbers do not.)*
2. **Reading the text is not the expensive part of a frame.** Of a
   0.7–2.6 ms draw, everything this spec touches is inside `window` +
   `lines` — 50–190 µs, under 10%. The dominant single term is
   tree-sitter at 220–490 µs, which this spec does not touch.
3. **The end of the document draws no slower than the top.** Only `key`
   and `window` grew; `lines`, `styles` and the whole draw did not.

**Since spec 0223 there are two kinds of frame, and G3 must be measured
against the right one.** A frame drawn with terminal events still queued
skips tree-sitter entirely, and the whole draw falls to ~630 µs with
`styles` at 0, `lines` at ~19 and `window` at 1–2. That is the same work
this spec touches, measured against a much smaller total: on a
monochrome frame `window` + `lines` is ~3% of the draw at the numbers
above but **closer to a third** of it. Point 2's "under 10%" is a
statement about colored frames only.

Two consequences. First, G3's acceptance numbers are the ones in the
table — measure with the keys far enough apart that every frame is
colored, or the comparison is against a different denominator. Second,
if this spec were to make `window` + `lines` slower, the cost would land
hardest exactly on the frames 0223 made cheap, which are the frames
drawn during a scroll.

## Goals

- **G1.** Delete `App::lines`. Each node owns the text of the lines it
  renders itself, its children's excluded.
- **G2.** A commit moves and allocates nothing proportional to the
  document. The `lines` merge and spec 0167's deferred-patch machinery
  both go.
- **G3.** **The per-frame path does not regress.** Concretely: `window`
  + `lines` in the table above must not grow at either end of the
  reference corpus, and paging must stay O(page).
- **G4.** No new O(document) work on any interactive path. Search is
  allowed to get slower (§S6) because it is not one.

## Non-goals

- **N1. No lazy or just-in-time rendering.** The document is rendered
  once at load, exactly as today; this spec only changes where the
  resulting bytes are stored. Storing the text per node and deriving
  positions is what would make a lazy render *possible* later — nothing
  would then force a node's text to exist until a row is drawn — but
  doing it here would put decode work inside `terminal.draw`, which is
  the one place the measurements above say there is no headroom. It is a
  separate spec and it needs its own evidence.
- **N2. Not making search faster.** §S6 accepts a regression and says
  what it is.
- **N3. Not touching the highlight pass.** §S8 hands `window_styles_for`
  byte-identical input. Tree-sitter is the largest per-frame term and
  therefore the largest available win, but it is orthogonal to where the
  text is stored and belongs in its own spec — which it now has, 0223.
  That spec skips the pass under load; it does not change what the pass
  is given, so §S8's obligation is unaffected.
- **N4. No global text arena.** Attractive on memory and on search, but
  it needs reclamation machinery this design gets from Rust ownership
  for free. See "Alternatives considered".
- **N5. `TreeNode` does not grow.** It is 44 B, pinned by
  `const _: () = assert!(size_of::<TreeNode>() == 44)` at
  `decode.rs:598`. The text goes in a side table indexed by slot — see
  §S1 and the alternative that would have cost 20 B a slot.

## Specification

### S1. What a node stores

A new `App` field, one entry per **arena slot**, parallel to
`heat_states`:

```rust
/// The text of the lines `slot` draws itself, its children's
/// excluded, newline-separated and with no trailing newline.
/// `None` for a slot this interpretation does not render
/// (`lines_total == 0`) and for a bracketed node's footer, which
/// is derived — see S2.
node_text: Vec<Option<Box<str>>>,
```

`Option<Box<str>>` is 16 B (null-pointer optimization), and living in
its own vector rather than in `TreeNode` keeps the 44 B slot and its
4-byte alignment intact (N5).

What each kind of node holds:

- **Bracketed** (`span.is_message`): its **header line only** — `file {`
  with whatever annotation the render appended. Its footer is derived
  (§S2). Its body belongs to its children.
- **Flat, one line**: that line — `name: "foo"  #@ …`.
- **Flat, many lines** (a packed run, one slot per §0216 S22): all N
  lines, joined by `\n`, in one allocation. They are consecutive in the
  render output, so this is one contiguous byte range and not N of them.

The addressing is already in place: `LinePos { node, line_in_node }`
names exactly one line, and `line_in_node` already means "which of this
node's own lines" rather than a screen offset (`lines.rs:31-50`). §S1
gives that coordinate something to index.

### S2. The footer is derived, never stored

A closing line is exactly `indent + "}"` and nothing else —
`write_close_brace` (`prototext-core/.../helpers/output.rs:117-122`)
writes an indent, one byte, and a newline. No annotation, no suffix.

So a bracketed node's footer is reconstructed as *its own header line's
leading whitespace, plus `}`*. Deriving it from the header rather than
from `span.level` is deliberate: the header's indent is the render's own
output and is correct by construction, whereas `level` is the wire-walk
depth and the two need not agree under a synthetic wrapper. The
invariant is already asserted in production-shaped test code —
`assert_line_counts_are_exact` checks `indent_of(close) ==
indent_of(start)` (`override_apply.rs:922-928`).

This removes **780 110 lines and 5.85 MB** from storage, and with them
780 110 allocations.

### S3. The per-frame path reads what the walk already produced

This is the section G3 rests on, and the point is that the per-frame
path gets **shorter**, not longer.

Today, drawing one row does this:

1. `build_window` descends once and walks, producing
   `(absolute_line, LinePos)` pairs — it *has* the owning node and
   `line_in_node` for every row.
2. It **discards the `LinePos`**, pushing `DisplayRow::Committed(line)`,
   and stashes the pairs in `window_nodes` (`set_window_nodes`).
3. `row_spans` → `display_row_source` → `node_at_header_line(line)` →
   `line_pos(line)` → `cached_line_pos` — a binary search over those 48
   entries **to recover the node it just threw away**.
4. `display_row_text` indexes `self.lines[line]`: a random access into a
   5 281 124-element vector — one cache miss for the `String` header,
   then a second, dependent one for the text.

Under this spec:

1. `DisplayRow::Committed` carries the pair the walk already has:
   `Committed { line: usize, pos: LinePos }`. The absolute `line` stays
   because `heat_cue_for`, the drag-selection range and mouse hit-testing
   are all keyed on it.
2. `set_window_nodes`, `cached_line_pos` and the `window_nodes_version`
   guard are deleted. Step 3 above disappears; the owner is in hand.
3. `display_row_text` becomes
   `node_text[pos.node]` indexed by `pos.line_in_node` (§S4), or the
   derived footer (§S2). The arena slot was read microseconds earlier by
   the walk, so it is hot; only the text itself can miss.

Net per row: **one binary search removed, one dependent cache miss
removed, no walk added.** The window is still one descent plus `height`
O(1) steps, unchanged.

Paging is untouched. `move_page_down` never reads text at all — it steps
`next_visible` `page` times and calls `carry_caret` once — and
`carry_caret` reads exactly one row, which is one `node_text` lookup
instead of one `lines` index.

### S4. A packed run is scanned once per frame, not once per row

Finding line `k` inside a flat node's `Box<str>` means finding the k-th
newline: O(k). With runs of up to 16 lines on the reference corpus that
is nothing, but nothing bounds a packed run's length, and 48 rows all
landing inside one long run would be O(page × k) — quadratic in the
run's length, and exactly the kind of per-frame regression G3 forbids.

So the per-frame path does not index; it **carries a cursor**.
`build_window` already walks the rows in order, so alongside each row it
maintains the byte offset of that row's text within its owning node's
`Box<str>`: entering a node sets the offset to 0 (or scans to the entry
line, once), and staying in the same node advances it past the next
`\n`. The offset rides in `DisplayRow::Committed` next to the `LinePos`.

Cost: **one scan per frame, bounded by the drawn rows' own bytes**, plus
at most one scan to the entry line when a frame opens in the middle of a
run. Off the per-frame path — `carry_caret`, extract, a search hit —
plain O(k) indexing is fine and is what those callers get.

Rendering a long packed run is already O(k) per row *today* in a
different place: `assert_line_counts_are_exact` aside, spec 0219 and the
`packed_record_extent` path re-parse the record's payload to recover
element k, because the arena deliberately stores no per-element ranges
(`decode.rs:526-531`). This spec neither adds to nor removes that.

### S5. What a commit does

`splice_override` re-renders a subtree and today queues a `LinePatch`
onto `pending_line_patches`; `finalize_override_batch` then calls
`materialize_line_patches`, which does `mem::take(&mut self.lines)` and
rebuilds the whole vector through `merge_replacements`.

Under this spec a splice writes each re-rendered node's own line(s) into
`node_text[slot]` as it rewrites that slot's overlay, and that is the
whole of it. There is no global array to shift, so:

- `line_patch.rs` (`LinePatch`, `LinePatchTarget`, `merge_replacements`,
  `materialize_line_patches`) is deleted, and with it spec 0167's reason
  to exist — the deferred batching existed only to amortize the memmove.
- `pending_line_patches`, `pending_patch_min_line` and
  `pending_shift` go with it.
- `refresh_line_counts` is unchanged and remains the O(depth) carry-up.

Reclamation is Rust's: overwriting `node_text[slot]` drops the old
`Box<str>`. A retype that re-renders the whole document replaces every
entry and frees every old one, which is the property N4's alternative
would have had to build by hand.

### S6. Search walks the document, and gets slower

`jump_to_match` (`tui/override_select.rs:759`) scans `self.lines`
linearly today (~36 ms per spec 0210) — **and its doc comment says it
does so deliberately, naming the exact cost this spec reintroduces**:

> Scans `lines` rather than the `doc_next`/`doc_prev` chain (spec 0210
> S1). A node's opening line is not a stored number, so walking the
> chain would cost a root-to-node descent per candidate; scanning the
> text resolves an owner only for the lines that actually match.

So §S6 is not "search happens to get slower", it is "this spec reverses
a decision that was made against it". The reversal is answerable: a walk
*carries* the node it is standing on, so it needs no descent per
candidate — the descent the comment worries about was the cost of going
the other way, from a line number back to an owner. What is left is one
`absolute_start` per *hit*, which is the first bullet below.

With no `lines` vector, the walk is over nodes in document order —
`doc_next` (`tui/structure.rs:163`), which already exists and already
ignores folds — running `str::find` on each node's own text.

That is **2 050 935 `str::find` calls, not 5 281 124**: the 594 642
multi-line flat nodes hold their 2 264 611 lines in one string each, and
the 780 110 footers are `}` and cannot match a pattern that has already
been matched against the header's indent. Spec 0210 estimated 300–400 ms
against a 4.5 M-node walk; scaled to this shape, expect **150–250 ms**.
**Accept it and measure it** (test-plan item 10). A full-document search
is a deliberate, infrequent action, and it is not per-frame.

Two things the walk must get right, both of which the flat vector was
hiding:

- The result is a `(LinePos, byte-in-line)` pair, but the caller needs an
  absolute line number for `cursor_column` and for scrolling. That is one
  `absolute_start` on the matched node plus a newline count within it —
  paid once per hit, not once per candidate.
- **Backward search needs a `doc_prev` that does not exist.** Only
  `doc_next` is written; level order makes a *forward* pre-order step
  derivable, and nobody has needed the reverse. So `?` cannot simply
  mirror `/` — it would degrade to "scan forward from the top and keep
  the last hit", which is quadratic over a wrapped search. Writing
  `doc_prev` is therefore part of this spec, not an assumption of it:
  the mirror of `doc_next`'s three cases — previous sibling's deepest
  last descendant, else the parent.

  Note what this bullet used to say and why it was wrong, because the
  same stale claim may be repeated elsewhere: backward search is *not*
  already quadratic through an eager `unwrap_or(self.last_node())`.
  **Spec 0195 fixed that on 2026-07-27**, and today neither direction can
  reach the hazard at all, precisely because `jump_to_match` walks no
  chain. Reintroducing a chain walk reintroduces the exposure, so
  `doc_prev` must be lazy about its endpoint the way spec 0195 S1
  required.

### S7. Extract and clipboard concatenate

Both read `&self.lines` today (`command_line.rs:647`, `:722`; then
`extract::extract_bytes` slices `lines[start..end]` and dedents).

- **Extract**, in prototext format, walks the target subtree in document
  order and concatenates each node's own lines, synthesizing footers per
  §S2. O(subtree), which is what producing that text costs anyway.
- **Clipboard** walks the *visible* nodes of the selection — the same
  walk driven by `lines_visible` instead of `lines_total`.

Both already receive a `&[String]`-shaped input; they become a small
`Vec<String>` built by the walk, so `extract.rs` itself is unchanged.

### S8. The highlight window is byte-identical

`window_text` (`render.rs:439`) already clones the ~50 drawn rows into a
fresh `Vec<String>` per frame, and `refresh_window_styles` already
discards it. It keeps doing exactly that; only `display_row_text`'s
implementation changes underneath it, so `window_styles_for`'s input is
byte-for-byte what it is today.

Neither of the two things that could pollute that input does, and both
are worth restating because both are real hazards elsewhere:

- **Heat cues never enter the text.** They are row chrome, assembled at
  draw time as `Span::styled(HEAT_GLYPH, …)` (`render.rs:661`,
  `heat_chrome`), so no concatenation of node text can pick them up.
- **The `...` truncation marker is preview-only.** It is genuine text, it
  is a syntax error, and tree-sitter's error recovery from it swallows
  *following* siblings (spec 0187's finding) — but it lives in the
  override-select pane's overlay, never in the committed document.
  `window_text`'s existing blank-line substitution for it is on the
  `DisplayRow::Overlay` arm and is untouched.

### S9. What this costs and saves

| | today | after |
| --- | --- | --- |
| per-line/per-slot headers | 5 281 124 × 24 B = **126.7 MB** | 4 737 284 × 16 B = **75.8 MB** |
| text bytes | **238.2 MB** | **232.4 MB** (footers derived) |
| heap allocations | **5 281 124** | **2 050 935** |

Roughly **−90 MB** once per-allocation rounding is counted on both sides
(5.28 M allocations of a 45 B mean against 2.05 M of a 113 B mean), and
3.2 M fewer allocations. Spec 0210 S8's "roughly memory-neutral" was
wrong in the favorable direction, chiefly because it did not know the
footers were free.

The 75.8 MB is paid per **slot**, not per rendered node, so 1 906 239
vacant slots carry a `None` costing 16 B each — 30.5 MB of the total.
That is the same trade `heat_states` makes and for the same reason: slot
indexing is what lets an overlay be rewritten without touching the arena.

Memory is not the reason to do this. G2 is.

### S10. What is deleted

`App::lines`; `Decoded::lines`; `line_patch.rs` in full; `pending_line_patches`,
`pending_patch_min_line`, `pending_shift`; `set_window_nodes`,
`cached_line_pos`, `window_nodes`, `window_nodes_version`. Spec 0210's
probe counters in `override_apply.rs`, which that spec marks TEMPORARY
and instructs be stripped only once S8 has re-measured the same phases —
test-plan item 9 is that re-measurement, so they go with this spec and
not before.

## Alternatives considered

### `Box<str>` inside `TreeNode`

What spec 0210 S8 proposed. `TreeNode` is 44 B with 4-byte alignment
(`NodeSpan` is 32 B of `u32`/`u16`/`u8`); a `Box<str>` forces 8-byte
alignment, so the slot becomes 48 + 16 = 64 B. That is **+20 B × 4 737 284
= +94.7 MB**, which turns a 90 MB saving into a 5 MB loss, and it breaks
the `size_of::<TreeNode>() == 44` assertion that exists precisely to
catch this. A side vector costs the same 16 B without the padding and
keeps the text out of the cache lines the structural walk touches.

### One global text arena, nodes storing `(start, len)`

The original render is one contiguous 232 MB buffer; a node stores two
`u32`s, 8 B a slot instead of 16 — about 38 MB cheaper again — and search
becomes a single `memmem` over contiguous bytes, *faster* than today
rather than slower. It is genuinely the better end state.

Ruled out for reclamation. A splice's new text has to go somewhere, and
appending to one buffer never frees the superseded bytes: the spec 0202
reproduction (three root retypes) re-renders the whole document each
time and would leak 232 MB a cycle — the exact OOM class spec 0216 just
closed. Making it safe means chunking the buffer, adding a chunk id to
every node (back to 12 B a slot), and reference-counting chunks so a
chunk is dropped when its last node stops pointing at it. That is real
machinery in exchange for ~38 MB, and `Box<str>` gets the same guarantee
from `Drop` for nothing.

### `Box<[Box<str>]>` per node — one entry per line

O(1) indexing into a packed run with no cursor and no scan, which is
what §S4 works around. But it costs a second allocation and a second
indirection on **every** multi-line node, and 780 110 bracketed nodes
would pay it to hold a header and a derived footer. §S4's cursor is
strictly cheaper on the path that matters and no more complex, because
`build_window` was already walking in order.

### Keep `lines` and make it a rope / gap buffer

Turns the O(document) memmove into O(log n) without touching anything
else, so it looks like the small change. It is not: every reader indexes
`lines` by absolute line number, and after spec 0210 the absolute line
number is a *derived* quantity that a node does not store. The rope
would keep alive the one coordinate system the rest of the design has
been removing, and the per-frame path would still do §S3's binary search
back to a node it already knew. It optimizes the wrong axis.

### Leave `lines` alone

Defensible on memory — 126.7 MB is 6% of startup RSS — but not on G2.
The `lines` merge is 42.9–87.4 ms of a 61–102 ms keystroke after spec
0210's S11; it is not a residue, it is the cost.

## Test plan

1. `a_node_owns_the_lines_it_renders` — for every rendered node in a
   fixture, `node_text` holds exactly its own line(s): one for a scalar,
   the header for a message, N for a packed run. Drives the
   header/flat/packed three-way split of §S1 directly.
2. `a_footer_is_the_headers_indent_and_a_brace` — the derived footer of
   every bracketed node in a fixture equals what the renderer emitted,
   character for character. This is §S2's whole risk.
3. `the_document_reassembles_byte_for_byte` — walking the document in
   order and concatenating (footers synthesized) reproduces the text
   `decode` produced. The one test that would catch a systematic
   off-by-one in §S1's ownership split, and it must run on a real corpus
   too — extend the existing `#[ignore]`d
   `the_arena_covers_a_real_corpus` harness rather than adding a second
   one.
4. `a_display_row_carries_its_own_owner` — `build_window`'s rows resolve
   to the same `(node, line_in_node)` the deleted `line_pos` binary
   search would have returned, over a fixture with folds, an empty
   message and a packed run. Guards §S3's central claim.
5. `a_frame_inside_a_long_packed_run_scans_once` — a synthetic node with
   a 100 000-line packed run, viewport opened at row 90 000: assert the
   number of bytes scanned is bounded by the drawn rows plus one entry
   scan, not by 48 × 90 000. §S4's quadratic is the failure this spec is
   most likely to ship.
6. `a_commit_touches_only_the_spliced_subtree` — after a splice, assert
   that no `node_text` entry outside the re-rendered subtree was written.
   The direct statement of G2, and it replaces spec 0210's
   `spans shifted == 0` assertion in kind.
7. `search_finds_the_same_hits_in_the_same_order` — the walk-based
   search returns the identical sequence of `(line, column)` pairs as
   the current `lines` scan, forward and backward, on a fixture with a
   packed run and a folded subtree. Ported from the existing search
   tests (`tui/tests/search.rs`), not written fresh, so a behavior
   change shows up as a diff. **Backward is the half that can actually
   break**, since it runs on a `doc_prev` §S6 has to write; give it the
   wrap-around case and a match in the very first node explicitly.
   Spec 0195's regression tests come along unchanged — if `doc_prev`
   reintroduces an eager endpoint, they are what says so.
8. `extract_and_clipboard_are_unchanged` — the existing extract and
   selection tests must pass without modification. If one needs
   changing, §S7 is wrong.
9. **The commit re-measurement.** The same pty run spec 0210 S11 used —
   `PROTOLENS_TRACE`, `key Enter us=` — on the first and the last
   top-level record of `googleapis.desc`, **three runs each**, reporting
   the whole keystroke and the walk. It must (a) show the 42.9–87.4 ms
   merge gone, and (b) either explain or dismiss the 2× first-vs-last
   spread spec 0210 left unexplained on one run each. Report both
   columns even if the spread persists.
10. **The search re-measurement.** Time a full-document `/` miss on
    `googleapis.desc` before and after. §S6 predicts 36 ms → 150–250 ms.
    If it exceeds 400 ms, stop and reconsider — the contiguous-buffer
    alternative exists for exactly this case.
11. **The per-frame re-measurement — the acceptance test for G3.**
    Re-run the Background table's pty script exactly (50×200, 48 rows,
    20 `PageDown` + 20 `PageUp` from the top; `G` then 15 + 15), and
    report `key`, `window`, `lines` and the whole `draw`. `window` +
    `lines` must not exceed today's range at either end. Report the
    medians, not the extremes — the ranges above are single runs and
    include the heat worker's interference. Keep the script's inter-key
    gap at its original 1.5 s: at that spacing spec 0223 never engages,
    so every frame is colored and the numbers are comparable with the
    table. A run with the keys close together measures a different frame
    (see the note under the Background table) and must be reported
    separately if at all.

## Measured outcome

Implemented 2026-08-01. All eight code items of the test plan are in
tree and green; the three measurements follow.

### Item 9 — the commit

`:type-as-raw` on a top-level record of `googleapis.desc` rendered
typed, three runs each, `PROTOLENS_TRACE` reading `key Enter us=` and
the `finalize_us` phase where the `lines` merge used to live:

| | first top-level record | last top-level record |
| --- | --- | --- |
| whole keystroke, before | 102 ms | 61 ms |
| whole keystroke, after | 658, 1058, 793 µs | 1809, 1990, 1182 µs |
| `finalize_us`, before (the merge) | 87.4 ms | 42.9 ms |
| `finalize_us`, after | 19, 24, 25 µs | 68, 96, 30 µs |

(a) The merge is gone: a phase that was 42.9–87.4 ms is now tens of
microseconds. (b) The 2× first-vs-last spread spec 0210 left unexplained
**inverted**, which explains it. It used to be *first*-slower because a
memmove near the front of a 5 281 124-element `Vec` moves more elements
than one near the back. With no vector to shift, last is now the slower
column by about a millisecond, and that residue is `inner_us` plus the
positional path at the end of the document — not storage.

### Item 10 — search, and a correction to §S6's baseline

`/` with a pattern that matches nothing, over the whole document. The
"before" column is the pre-implementation binary built from HEAD in a
throwaway worktree, not an inherited figure:

| | before | after |
| --- | --- | --- |
| lowercase pattern (smartcase folds) | 1.80, 1.87, 1.80, 1.80 s | 1.69, 1.63, 2.02, 2.05, 1.99, 1.64 s |
| case-sensitive pattern (`memchr`) | 158, 164, 258, 275 ms | 269, 279, 438, 499 ms |

**§S6's "~36 ms today" was never a measured number, and the 400 ms stop
rule was calibrated against it.** A full-document miss already cost
~1.8 s before this spec. The reason is `SearchPattern`'s
case-insensitive path (`tui/mod.rs:217`): under smartcase an all-lowercase
pattern folds `char::to_lowercase` at *every byte position* across
238 MB, and that dominates everything else by an order of magnitude.

Against that, what this spec actually did to search: the node walk costs
~1.7× on the `memchr` path — about 110–225 ms in absolute terms, at the
top of §S6's own 150–250 ms prediction — and is lost in the noise on the
path a user hits. The contiguous-buffer alternative would have bought
back only the 1.7×; the fold is the cost, and it is a separate spec.

### Item 11 — the per-frame path, G3's acceptance test

The Background table's pty script re-run exactly: 50×200, 48 drawn rows,
1.5 s inter-key gap, 20 `PageDown` + 20 `PageUp` from the top, then `G`
and 15 of each. Medians in µs, with min–max:

| µs per frame | top of document | after `G` (end) |
| --- | --- | --- |
| `key` | **10** (8–20) | **24** (4–78) |
| `window` | **5** (3–20) | **57** (34–117) |
| `styles` | **276** (228–651) | **431** (360–791) |
| `heat` | **7** (4–142) | **10** (5–143) |
| `ovr` | **11** (4–26) | **14** (9–30) |
| `lines` | **75** (63–149) | **93** (81–135) |
| whole `draw` | **821** (690–1500) | **1359** (954–2083) |

G3 asked that `window` + `lines` not exceed the Background table's range
at either end. It is **80 µs at the top** against a range of 57–142, and
**150 µs at the end** against 49–192. Inside at both ends, so G3 passes.
Paging stayed O(page): `key` is 10 µs at the top and 24 at the end,
the same shape as before.

### Deviation from §S6: no `doc_prev`

§S6's second bullet made writing `doc_prev` part of this spec.
`jump_to_match` instead walks the **line-level** `next_line`/`prev_line`
pair, and `doc_prev` was not written.

The reason is that the cursor can rest on a **footer**. A node-level
pre-order `doc_next` from a node whose footer the cursor is on would
descend back into that node's own children, re-visiting every line the
cursor has already passed. The line-level pair takes the footer as a
position in its own right, which is the coordinate `jump_to_match`
actually holds. Spec 0195's lazy-endpoint requirement is met the same
way it is today — neither direction materializes `last_node()` eagerly.

One consequence to record: search does **not** use §S4's byte cursor. It
resolves each line independently, so a match inside a long packed run
still pays `line_offset`'s O(k) rescan. That is off the per-frame path
(G4 permits it) and is invisible next to the fold cost measured above,
but it is a real difference from the per-frame reader.
