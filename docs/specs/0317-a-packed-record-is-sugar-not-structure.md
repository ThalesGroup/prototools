<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0317 — a packed record is sugar, not structure

Status: draft
App: prototext-core, protolens
Refs: docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (S7: a
        packed run is one arena slot — the invariant this restores),
        docs/specs/0210-a-node-counts-its-own-lines.md (`lines_total` vs
        `lines_visible`, the split this reuses),
        docs/specs/0222-the-text-lives-in-the-nodes.md (per-slot text,
        which becomes per-slot *generated* text for one node kind),
        docs/specs/0249-a-large-document-answers-the-user-first.md
        (`row_budget`, which this leaves alone and explains why),
        docs/specs/0261-an-export-waits-for-the-lines-it-names.md (export
        is faithful and fold-blind),
        spec 0316, "a preview is the real thing" — abandoned, and living
        only on the branch `spec-0316-preview-splice`. Its removal of the
        preview's byte budget made this crash reachable from one
        keystroke; docs/specs/0318-a-preview-ends-where-a-record-ends.md
        keeps the budget and is what shipped instead. The crash below
        predates both and neither fixes it.

## Background

Open `googleapis.desc` against itself, press `t`, arrow to
`…ReportValue.IntList`, press `o`. protolens allocates 2.3 GB, stops
responding for 56 s, and is killed by the OOM reaper on a smaller
machine. Under a pty: 840 MB at startup, 920 MB after `t`, 4 299 MB
peak.

On the abandoned `spec-0316-preview-splice` branch the same freeze
arrives one keystroke earlier, on the `Down` that *previews* the
candidate — which is how it was found. The preview is byte-bounded here
and stays so (spec 0318), so `o` is the reproduction.

The highlight lands on `google.ads.admanager.v1.ReportValue.IntList`.
Overriding the root to it wraps the whole 25 660 332-byte payload in one
`IntList`, whose field 1 is a repeated varint — so each of the payload's
7 771 top-level field-1 LEN records is re-read as a *packed* run, and all
7 771 sit at one nesting level.

Measured on the dev VM against `/tmp/googleapis.desc`, `row_budget =
Some(48)` in both rows:

| root rendered as | arena slots rendered | rows emitted | rows/slot | wall | RSS |
|---|---|---|---|---|---|
| `FileDescriptorSet` (committed) | 7 772 | 15 585 | 2 | 0.69 s | +44 MB |
| `…ReportValue.IntList` | 7 772 | **25 003 523** | **3 218** | **56.4 s** | **+2.3 GB** |

`row_budget` was set and honored throughout. It could not help, and no
row budget could: `row_budget_spent()` has one call site, `descend()`
(`render_text/mod.rs:255`), so it bounds *nested descents*, and there is
no nesting here.

This is not new, and not about previews. The freeze is in
`splice_override(idx, target, confirm_row_budget())`
(`override_apply.rs:92`), which the commit path has always called. The
preview overlay's 4 096-**byte** budget is the only bound in the system
that caps the *scan*, and it protects the preview alone; spec 0316
removed it in favor of `row_budget` and let one arrow key reach what had
needed a deliberate confirm. Spec 0318 keeps it, and keeps it in bytes
for this reason. Neither spec bounds the commit.

### The actual invariant

The right measure is **rows of text per arena slot**:

| | slots | rows | ratio |
|---|---|---|---|
| 5 M sibling scalars (a 10 MB file of `\x08\x01`) | 5 M | 5 M | 1 |
| googleapis root as `IntList` | 7 771 | 25 003 523 | 3 218 |

A non-packed run's rows *are* its nodes. That cost is proportional to the
arena, and the arena is a cost the design already pays and absorbs —
4.9 M slots, 840 MB, about a second. Expensive, but honest.

A packed record is the only construct where rows and nodes decouple, and
they decouple without bound. One wire record, one LEN field, one arena
slot — spec 0216 S7 already says so — and the 1.3 M rows of the corpus's
widest run are something the *display* invented. The bytes contain a
repeated field; the rows are how we choose to show it.

So a packed record is syntactic sugar over one node, not structure, and
the document should treat it that way.

## Goals

- **G1.** The **text** a render materializes is bounded by what was
  asked for, not by the document. Every node kind but one already
  satisfies this, at O(1) rows per slot; a packed record becomes O(rows
  in the requested window) per slot. Not O(1): a record that is fully
  shown is fully materialized, and must be — the goal is that nothing
  materializes rows nobody asked to see.
- **G2.** A packed record stays **honest about its size and lazy about
  its text**: `lines_total` remains the true element count, so line
  accounting, scrolling, row lookup and the scrollbar are exact and
  unchanged.

  Honesty is not free, and G1 does not extend to it: the *scan* stays
  O(payload bytes) and record-wide — S2. What this spec removes is the
  N `String`s and N `PackedElem`s, not the pass over the bytes.
- **G3.** Export and any other faithful consumer still see every element.
  Truncation is a display policy and must not be reachable from a path
  whose output is bytes.

## Non-goals

- **N1.** Touching `row_budget`, or bounding `render_packed`'s element
  loop with it. `row_budget` bounds *rows*, and rows are not what runs
  away here — S2's scan is O(payload bytes) whatever the budget says.
  An earlier draft added a second budget check inside `render_packed`
  and an *unbounded* expansion to make the bake terminate; the second of
  those is what made it dishonest and it stays rejected.

  What that draft got right, and this spec's own first version then
  wrongly discarded, is that **something is stopped and does need
  resuming**. Not the row loop: the scan (S2b). The reason the earlier
  attempt could not resume was that a stopped run re-rendered from
  element 0 and made no progress; S1's element range removes that
  obstacle, so resumption is now the cheap part.
- **N2.** Bounding the *preview* specifically — by node size, by byte
  budget, or by declining to preview large nodes. Every such fix leaves
  the commit path hanging and moves the freeze one keypress later, and a
  size threshold silently disables previews on exactly the node a reader
  of a large document is standing on.
- **N3.** Reviving spec 0316's byte budget and `preview_truncate.rs`.
  Same objection as N2, plus it reinstates the `...` marker, which is not
  in the prototext grammar and whose tree-sitter error recovery swallows
  the following siblings' captures.
- **N4.** Budgeting the sibling loop, or giving a parent a "N more
  fields" summary row. That would break spec 0249's guarantee that every
  frontier node owns a row, which `overlay_spans` asserts on
  (`decode.rs:1091`). It is also unnecessary: per the table above, a
  non-packed run's rows are already proportional to its slots.
- **N5.** Making the arena a rope. Random access *inside* a packed record
  is needed (S3) and is easy; a balanced index over the arena's sibling
  lists is a separate and much larger question, and this crash is not
  evidence for it.
- **N6.** Making the encoder accept a truncated display. It must never
  see one — S6.

## Specification

- **S1.** `render_packed` takes an element range and renders only that
  range. One function serves all three callers: the initial render, the
  display asking for a scroll window, and export asking for `[0..N)`.
  **Faithfulness is a parameter, not an exemption.**

- **S2.** `render_packed` reports the record's true element count `N`
  without *materializing* the elements outside the requested range —
  but it still **reads** them. The scan is record-wide and cannot be
  windowed, for two reasons that are properties of the format, not of
  the code:

  - **Validity is a record-level verdict that changes the whole shape
    of the output.** `decode_packed_elems` returns `Result`, and on
    `Err` `render_packed` emits one `INVALID_PACKED_RECORDS` line
    instead of N element lines (`packed.rs:307-321`). No row of any
    range can be written before that verdict is known.
  - **The count itself appears in the text.** `pack_size: N` is a
    record-level annotation modifier (`packed.rs:261`), so N is an
    input to a row, not merely to the line accounting.

  What that costs, per kind, is not uniform, and the spec commits to
  the cheap answer wherever there is one:

  | kind | validity | count | per-element anomalies |
  |---|---|---|---|
  | `fixed32/64`, `sfixed32/64` | O(1): `len % width` | O(1): `len / width` | none |
  | `float`, `double` | O(1): `len % width` | O(1): `len / width` | `nan_bits`, windowable |
  | `int64`, `uint64`, `sint64` | O(bytes) | O(bytes) | `ohb`, windowable |
  | `int32`, `sint32`, `uint32`, `bool`, `enum` | O(bytes) | O(bytes) | `ohb`, `neg`, `ENUM_UNKNOWN`, windowable |

  So the fixed-width half of the problem is genuinely O(1) in both
  columns, and only the varint kinds need the pass.

  **The O(1) invalidity test is real but partial.** A varint packed
  record whose last byte carries the continuation bit is truncated and
  therefore invalid, and that is one byte to check — truncation can only
  ever fall at the end of the payload. It does not settle validity,
  because two other families of `Err` sit anywhere in the run:
  *overflow* (a varint past 64 bits, `varint.rs:186-196`) for every
  varint kind, and *range violation* — `bool > 1`, `uint32`/`sint32`
  ≥ 2³², and the forbidden middle band for `int32`/`enum`
  (`packed.rs:154-197`) — for the narrow ones. The test is worth having
  as an early exit on the common truncated tail; it is not a substitute
  for the scan.

  What the scan must therefore be is **allocation-free**: `parse_varint`
  and a counter, no `PackedElem`, no `String`. Today's
  `decode_packed_elems` builds the whole `Vec<PackedElem>` — each
  element owning a formatted `String` — before the first row is written
  (`packed.rs:307`). That is the 2.3 GB, and it is what goes away; the
  bytes were being walked either way.

  **Owed by implementation, and the number that decides whether this is
  enough:** the wall time of an allocation-free scan of the 25 MB
  corpus interpreted as one `IntList`, against the 56.4 s of the
  Background. If a bare scan is not comfortably interactive, S2 is not
  sufficient on its own and the count has to be cached per slot at
  arena-build time instead of recomputed per render.

- **S2b. The scan runs in the background, and until it finishes the
  record does not know what it is.** S2 establishes that the scan is
  O(payload bytes) and record-wide. On the corpus's widest run that is
  not a cost to pay on the keystroke that reveals it, so it is not paid
  there: the first render scans a screenful, renders those rows, and
  leaves the rest to the bake, exactly as spec 0255 does for a message
  subtree.

  The state this needs is **"don't know"**, and it is stronger than "an
  unknown element count". Until the scan reaches the last byte, the
  record's *shape* is provisional: one bad varint anywhere turns N
  element rows into a single `INVALID_PACKED_RECORDS` line (S2). So a
  partially scanned record is not a correct document with a missing
  tail — it is a document that may still be revised, and the reader is
  owed that.

  - **It wears spec 0260's unbaked violet**, and it *does* belong in
    `auto_folded`: that set means "the renderer never went in here" and
    carries a bake obligation, which is precisely true here. This
    reverses an earlier note in this spec that a packed record must
    never join it — that note was reasoning from a record whose scan had
    already completed, and it remains correct for that case. The rule is
    the obligation, not the node kind: **a packed record is in the
    unbaked set exactly while its scan is incomplete, and leaves when
    the scan reaches the end.** A record that never leaves is a bake
    that never drains, so this is the invariant to test.
  - **`lines_total` stays exact at every instant.** It is the number of
    elements *scanned so far*, not an estimate, so spec 0210's
    accounting and `assert_line_counts_are_exact` hold continuously; the
    bake grows it the way it grows a message subtree's. Nothing may
    guess N from `payload_len / average_width`.
  - **`pack_size: N` cannot be written while the count is provisional**,
    and no faithful consumer may see a record in this state. Spec 0261
    already has the mechanism — `:export` bakes first or refuses — and
    this is the same rule for the same reason, so S6's faithful list
    inherits it rather than growing a second one.
  - **The O(1) truncated-tail test earns its place here.** It is the one
    disqualification available before any scanning, so the common
    corruption — a record cut by a truncated file — is reported
    immediately instead of after a background pass over 20 MB.

  **Owed by implementation:** the bake already has a row budget
  (`BAKE_ROW_BUDGET`); whether the scan should be metered in *bytes*
  instead is an open question, because a run of one-byte varints and a
  run of ten-byte varints give the same rows for ten times the work.

- **S2a.** The element count is a property of *the interpretation*, not
  of the bytes: the same payload read as packed `fixed32`, `fixed64` or
  varint yields three different counts, and two of the three may be
  invalid. Nothing may cache `N` against a byte range; it caches against
  a slot under a given `rendered_as`, and an override invalidates it —
  which is already exactly the lifetime of everything else in the
  overlay.

- **S3.** Random access into a run. Element *i* of a fixed-width run
  starts at `i × width`: O(1). A varint run carries a **sparse offset
  index**, one entry per 1 024 elements, built once when the record is
  first rendered; locating element *i* is one lookup plus at most 1 023
  varint skips. This is the whole of the "rope" a packed record needs.

  The index is built by the S2 scan, which already visits every element
  boundary, so it costs one `u32` per 1 024 elements and no second pass.

- **S3a.** **`is_first` means element 0, never "first in the window".**
  `render_packed`'s record-level modifiers — `pack_size`, `tag_ohb`,
  `TAG_OOR`, `len_ohb` — hang off the record's *first element*
  (`packed.rs:260-271`), and today `is_first` is the loop counter
  (`packed.rs:351`). Under S1 that counter is the window's, so a window
  starting at element *k* > 0 would stamp `pack_size:` on a middle
  element, and the encoder would rebuild a different record. The
  parameter is the absolute element index.

  The consequence is deliberate and must be stated rather than
  discovered: a display window that does not include element 0 shows no
  record-level annotation. That is correct — those modifiers describe
  the record's framing, which is not on screen — and it is invisible to
  the encoder, because every faithful consumer asks for `[0..N)` (S6).

- **S4.** protolens computes a packed slot's `lines_total` from S2's
  count rather than from the span stream. Today `IndexingTextSink`
  pushes one `NodeSpan` per element (`sink.rs:1575`) and `overlay_spans`
  merges them into the one slot, accumulating `lines_total` as it goes
  (`decode.rs:1103-1119`); under S1 there are no longer spans for the
  elements that were not rendered. The record gets one span, and
  per-element byte coordinates are derived on demand through S3 — the
  arena's `raw_start`/`raw_end` are already the authority for a packed
  slot, and `overlay_spans` already overwrites the span's own
  `raw_range` with them (`decode.rs:1141`).

  **That one span must still carry `packed_record_start`.** It is
  tempting to reuse the shape of the empty and undecodable cases
  (`sink.rs:1595-1602`), but those set `NO_PACKED_RECORD`, and that
  field is what four readers use to *recognize* a packed run:
  `extract::message_payload_range` (which otherwise strips the
  tag+length as if the payload were a message), `wire.rs:408`,
  `override_select.rs:26`, and `override_apply.rs:1300`'s
  `in_packed_run`. Spec 0219 fixes its meaning as "still rendered as a
  run", which is exactly what this span is. It becomes the record's
  marker rather than an element's back-pointer; nothing else changes.

  What *does* fall out is `same_packed_record` (`decode.rs:508-514`) and
  the `is_rendered()` merge arm at `decode.rs:1103`: with one span per
  record there are no sibling element spans left to coalesce. Both are
  spec 0115 machinery whose whole purpose was to undo the one-span-per-
  element decision this item reverses. Removing them is part of the
  change, not a follow-on.

- **S5. Held — do not implement with the rest.** A long packed record is
  **folded by default**, not capped: `lines_visible < lines_total`, and
  unfolding is a display action that under S1 costs only a larger
  window.

  Two mechanisms were conflated in drafting this, and separating them is
  what puts the item on hold:

  - **Windowed materialization** (S1, S9): `lines_total` is N, every one
    of the N rows is reachable by scrolling, and only the ones on screen
    are ever built. Nothing is hidden.
  - **Folding** (this item): `lines_visible < lines_total`, so the rows
    are genuinely absent from the document and scrolling cannot reach
    them.

  **The Background's crash is fixed by the first alone.** Folding is a
  convenience — so that a reader is not made to scroll 1.3 M rows to
  reach the next field — and it is what costs the entire new node state
  below. It is therefore separable, and separating it keeps the change
  that fixes the crash small.

  Open, and not to be guessed: whether the shown part is a **leading
  prefix** or a **head and a tail**. The second is the better reading
  for a packed run, where the last element is as informative as the
  first, and it is not what the rest of this item assumes.

  A folded *message* is bracketed and collapses to exactly
  one row — its header — and the code says so in three places that a
  partially-visible flat node walks straight into:

  - `refresh_line_counts` (`lines.rs:162-167`) hard-codes
    `(lines_total, lines_total)` for a flat node, over the comment "it
    cannot be folded, so the walk ends here". A folded packed record
    would be silently unfolded by the next refresh of its own counts.
  - `assert_line_counts_are_exact` asserts `visible == total` for a flat
    node — "flat node {n} cannot be folded, so its two counts must
    agree" (`override_apply.rs:890-894`) — and separately asserts that
    the node's held text has exactly `lines_total` lines
    (`override_apply.rs:919-924`), which S9 breaks too.
  - There is no fold glyph: `fold_marker_of` returns `None` unless
    `has_children(idx)`, which is `is_bracketed` (`render.rs:594-604`).
    A folded record with no marker is a document with rows missing and
    nothing on screen saying so.

  So this spec introduces a genuinely new node state — **flat,
  partially shown** — and owes it the same three things a bracketed fold
  has: a `lines_visible` derivation (`min(lines_total, window)`, in
  `refresh_line_counts`'s flat branch, which stops being a no-op), an
  assertion that admits it (`visible <= total` for a flat node, with
  equality unless the node is a folded packed record), and a marker.
  The marker is the open design question of this spec: the existing
  glyph column is keyed on `has_children`, and a record's collapsed
  summary cannot be `{ ... }` because a record has no braces.

  A *user* fold is **not** an `auto_folded` stop, and the distinction is
  now the one S2b draws. `auto_folded` means "the renderer never went in
  here" and carries a bake obligation; a record whose scan has completed
  has no such obligation, so folding it must not put it in that set or
  the bake will never drain. A record whose scan is still running is in
  that set for exactly the right reason, and leaves it when the scan
  ends — whether or not the reader has also folded it.

  **The threshold is not yet fixed.** It wants a histogram of packed-run
  lengths across the corpus first: the intent is that ordinary runs —
  coordinates, small arrays, bitmaps — are shown whole and only
  pathological ones fold, so the constant belongs just above the bulk of
  the distribution, not at a round number chosen for looking like one.
  Record the histogram next to the constant.

- **S6.** Every consumer of a node's text is classified **display** or
  **faithful**, and the classification is explicit at the call site.
  Faithful consumers pass `[0..N)`; display consumers pass the window.

  - Faithful: export (spec 0261, already fold-blind — `subtree_lines`
    sizes to `lines_total` and `push_subtree_lines` never consults
    `is_folded`), the editor hand-off, and anything whose output is
    bytes.
  - Display: the draw path, the highlighter, hover, `node_status`, the
    wire row.
  - **Clipboard is faithful.** It shares `subtree_lines` with export
    today (`lines.rs:674`) and copying a node should yield the node; a
    truncated copy is silent data loss, and a reader who wants what is
    on screen has the terminal's own selection.

  The rule this enforces: a truncated render must never reach the
  encoder. `K` rows each annotated `pack_size: N` would re-encode as "N
  claimed, K present". A folded message already lives under exactly this
  rule — header and footer, no body — so nothing new is being asserted,
  but it now has to be stated per call site rather than assumed.

- **S7.** **Search scans totals, not visibles.** A sweep that stopped at
  `lines_visible` would make "no match" a lie for every element past the
  fold, which is a silent correctness regression across nine specs
  (0235, 0246, 0272–0278, 0281). The sweep therefore generates rows
  through S1 as it goes — streaming, retaining nothing. Spec 0235's
  resumable `SearchSweep` is what makes that affordable.

  An earlier draft said S3's random access is what keeps this from being
  quadratic. It is not, and the distinction matters for where the bug
  would come back. Spec 0272's O(K²) was `line_offset` finding row *j*
  by counting newlines from byte 0 of the node's joined text — O(*j*),
  so O(K²) over a whole run (`lines.rs:571-584`). The fix was not an
  index but spec 0222 S4's **byte cursor**: `line_text_at` exists so a
  walker carries its own offset forward and each step costs one row.

  **This spec does not fix that, and makes the discipline harder to
  keep.** A cursor into a stored string is a byte offset; over generated
  rows it has to be an element index *plus* a byte offset into the
  payload — which means **the generator itself must be resumable**, and
  that is a requirement on S1's signature, not a convention the sweep
  can adopt on its own. A generator that can only be entered at the
  start of a range reinstates the quadratic the moment anything walks a
  run.

  S3's sparse index does not fix it either; it caps it. A row asked for
  cold costs at most 1 023 varint skips rather than *j*, so a lapse
  degrades to O(K·1024) instead of O(K²). A ceiling on the damage, not
  a substitute for the cursor.

- **S8.** Export streams. `subtree_lines` opens with
  `Vec::with_capacity(tree[idx].lines_total)` (`decode.rs:1001`), so
  exporting a 25 M-row document builds a 25 M-entry `Vec<String>` before
  a byte is written. G1 fixes the display and leaves that untouched. It
  is the same generator and belongs in the same change.

- **S9. Where the generated rows live.** Spec 0222 gave every slot its
  own text and `line_text_at` returns `Cow::Borrowed` into it
  (`lines.rs:562-568`); `line_text` takes `&self`, so nothing can
  generate a row on demand and hand out a borrow. Generating per row
  would also be quadratic again — each row costs a walk to its element.

  So `node_text[slot]` keeps holding a real string, and what changes is
  that for a packed record it holds **the current window's rows, not the
  record's**. Two consequences the implementation must not discover
  later:

  - The slot needs a **base**: which element the held text starts at.
    `LinePos::line_in_node` is an index into the node's rows, and
    `offset_of_line(text, line_in_node)` (`lines.rs:576-584`) would
    otherwise index the window by an absolute row number. This is a new
    field, and `TreeNode` is pinned by
    `const _: () = assert!(size_of::<TreeNode>() == 44)`
    (`decode.rs:659`), an equality kept deliberately so that growth is
    caught. Either the base is packed into existing space or the
    assertion moves — and moving it is a decision, not a fixup.
  - Something must **re-materialize the window when it moves**. That is
    a mutation on a scroll, on a path (`render`) that today only reads.
    The natural home is wherever the frame's window is already computed,
    before `window_text` runs.

- **S10. The window render needs a field descriptor the overlay does not
  store.** `render_packed` takes a `&FieldOrExt` (`packed.rs:295`).
  `TreeNode` carries a `NodeSpan` and a `ProvenanceId`
  (`decode.rs:583-646`) — the override and the field *name*, interned —
  not the resolved descriptor the render had. Every existing re-render
  goes through `splice_override`, which reconstructs the type from the
  target and re-decodes the field's bytes.

  This is the item with the least analysis behind it and it should not
  be implemented on a guess. Two shapes, to be chosen on measurement,
  not on taste:

  1. **A window is a splice.** Scrolling inside a long record re-splices
     that one slot with a different range. Maximum reuse, no new
     resolution path; but a splice rewrites the overlay and the line
     counts, which is a lot of machinery to run on an arrow key, and
     `refresh_line_counts` would have to see the counts *not* change.
  2. **A narrow re-render entry point** that takes the slot, the byte
     range, the resolved field and an element range, and returns rows.
     Cheaper per scroll; owes an answer to where the resolved field
     comes from, which is the whole difficulty.

  **Owed by implementation:** which of the two, and the measured cost of
  one scroll step inside the corpus's 1.3 M-element run.

## Alternatives considered

**Budget the packed element loop, report the record undescended, resume
it through the bake.** Written out in full as the first draft of this
spec, and rejected on review. It bounds the *first* render only: a
stopped run has nothing below it, so a bounded expansion re-renders from
element 0 and makes no progress, and spec 0255 attempts each node once —
so the expansion has to be unbounded, which is a 3 s freeze on the
corpus's widest run relabeled as acceptable. Resuming from element *k*
instead would need `splice_override` to append to a slot rather than
replace it. S1's range parameter subsumes both: there is nothing to
resume when any range can be asked for directly.

**Bound the preview and leave the renderer alone**, in three variants —
node-size guard, byte budget, refuse-to-preview. Rejected under N2/N3.
The measurement that killed them is that the committed path produces the
identical 56 s and 2.3 GB, so each merely relocates the freeze.

**Treat the non-packed breadth as the same problem.** Argued during
design on the grounds that a 10 MB file of `\x08\x01` yields 5 M rows,
"a factor of two, not a difference in kind". Wrong: those 5 M rows are
5 M arena slots, one row each, and the arena already pays for them.
The ratio table in the Background is the refutation and the reason this
spec is about one node kind rather than about the renderer at large.

## Test plan

1. `a_packed_record_reports_its_length_without_rendering_it` — S2, for
   both the fixed-width and varint kinds; asserts no `PackedElem` outside
   the requested range is built.
2. `an_invalid_packed_record_is_invalid_from_any_window` — S2's first
   bullet, over all three families: a truncated tail, an overflowing
   varint in the middle of the run, and an out-of-range `bool`. Asking
   for a window that excludes the offending element must still yield
   `INVALID_PACKED_RECORDS`, byte-identical to today.
3. `the_same_payload_counts_differently_under_three_kinds` — S2a, on one
   byte string read as packed `fixed32`, `fixed64` and `int64`.
3a. `a_partially_scanned_record_leaves_the_unbaked_set` — S2b, the
   invariant that decides whether the bake drains: a long run renders a
   screenful, is in `auto_folded` and violet, and after the bake is in
   neither. Plus its negative — a run whose scan completed on the first
   render never entered the set.
3b. `a_record_scanned_past_its_bad_varint_becomes_invalid` — S2b's
   shape-is-provisional case: a run whose first screenful is clean and
   whose 10 000th element overflows renders as element rows, then as one
   `INVALID_PACKED_RECORDS` line once the bake reaches it, with
   `lines_total` exact at both instants.
3c. `a_truncated_tail_is_reported_without_scanning` — S2b's last bullet:
   a record whose final byte carries the continuation bit is invalid on
   the first render, having read one byte.
4. `a_packed_record_renders_an_arbitrary_element_range` — S1/S3, against
   a run long enough to cross a sparse-index entry, compared element by
   element with a full render.
5. `a_window_past_element_zero_carries_no_record_annotation` — S3a; and
   its converse, that `[0..N)` is byte-identical to today's render for
   every fixture in the corpus, which is the regression net for the
   whole change.
6. `the_sparse_index_locates_the_same_element_as_a_linear_scan` — S3, on
   a varint run with mixed-width encodings, including the non-minimal
   varints spec 0288 cares about.
7. `a_packed_slot_line_count_is_its_element_count` — S4/G2:
   `lines_total` equals `N` although only the window was materialized,
   and `assert_line_counts_are_exact` passes over the document.
8. `a_packed_slot_span_is_still_marked_packed` — S4's second paragraph:
   one span, `packed_record_start` set, and
   `extract::message_payload_range` returns the range unstripped.
9. `a_short_packed_run_renders_whole_and_is_not_folded` — S5, guarding
   against turning every packed field into a fold; output byte-identical
   to today's.
10. `a_folded_packed_run_survives_refresh_line_counts` — S5's first
    bullet, the silent-unfold case: refresh the record's own counts and
    assert `lines_visible` did not jump back to `lines_total`.
11. `a_long_packed_run_is_folded_but_not_auto_folded` — S5's last
    paragraph; the bake drains to empty with the record still folded.
12. `scrolling_inside_a_long_packed_run_keeps_its_rows_correct` — S9:
    scroll into the middle of a run and assert each drawn row equals the
    corresponding row of a full render. This is the test that catches a
    missing window base.
13. `exporting_a_folded_packed_run_emits_every_element` — S6/G3, the one
    that matters: export bytes are identical whether the record is folded
    or not.
14. `copying_a_folded_packed_run_copies_every_element` — S6's clipboard
    call.
15. `searching_finds_a_match_past_the_fold` — S7, the silent-lie case.
15a. `sweeping_a_long_run_is_linear_in_its_length` — S7's real risk:
    time a sweep over runs of K and 4K elements and assert the ratio is
    near 4, not near 16. A quadratic reintroduced through a
    non-resumable generator is invisible to every correctness test here.
16. `repro_preview_the_root_of_googleapis` (`tests/profiling.rs`,
    `#[ignore]`, needs the 25 MB corpus) — the Background reproduction:
    the root previewed as `…ReportValue.IntList` settles in well under a
    second with RSS within tens of MB of its pre-`Down` value, against
    56.4 s and +2.3 GB today.
17. Live, under the pty driver: `t` then `Down` on the root of
    `googleapis.desc` leaves RSS within a few tens of MB of its post-`t`
    value.

## Measured outcome

Filled in at implementation. Owed, in the order that decides whether the
design holds:

- **S2** — the wall time of an allocation-free scan of the corpus read
  as one `IntList`. This no longer decides whether the design holds, now
  that S2b puts the scan in the bake; it decides how long the violet is
  on screen.
- **S2b** — whether the scan's unit of work is rows or payload bytes,
  and whether the bake drains on the corpus.
- **S10** — which re-render path a scroll takes, and what one scroll
  step costs inside the 1.3 M-element run.
- **S7** — the K vs. 4K sweep ratio.
- **S5**, if it is taken up at all — prefix or head-and-tail, the
  packed-run length histogram behind the threshold, and what the marker
  for a partially-shown flat node looks like.
