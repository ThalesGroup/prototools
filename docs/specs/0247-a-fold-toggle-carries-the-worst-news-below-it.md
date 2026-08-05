<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0247 — a fold toggle carries the worst news below it

Status: implemented
Implemented in: 2026-08-05
App: protolens
Refs: docs/specs/0216-….md (the immutable level-order arena this walks),
        docs/specs/0222-….md (each slot owns its own text; the line
        partition this reads), docs/specs/0210-….md
        (`refresh_line_counts`, the ancestor walk this copies),
        docs/specs/0237-….md (`--field-name`, which is what clears an
        unknown), docs/specs/0187-….md (`annotation_start`)

## Background

A defect in a protobuf blob is currently visible only where it is: a
yellow or red annotation on the offending row. Fold the node that
contains it, or scroll past it, and the document looks clean. On a real
payload the interesting rows are a handful out of thousands, and there
is no way to steer toward them — the user folds a subtree shut and
loses the only evidence that it was worth opening.

The fold toggle is the natural carrier. It sits at the left of every
row that has children, it is the thing the user clicks to decide
whether to look inside, and it is currently drawn unstyled
(`Span::raw(margin)`, `tui/render.rs:779`).

## Goals

- **G1.** Every node carries a status on a four-rung severity ladder,
  derived from what its own rows say and rolled up from its subtree.
- **G2.** The fold toggle is colored by that status, so a folded node
  advertises the worst thing hidden inside it.
- **G3.** The full computation is one linear pass over the arena, and an
  override updates it without recomputing the document.

## Non-goals

- **N1.** No status on a leaf's glyph — leaves have no fold toggle
  (`fold_marker_of` returns `None` when `!has_children`). A leaf's
  status is already the color of its own annotation.
- **N2.** No new rung, and no per-defect detail on the toggle. Four
  colors is what a two-column margin can say.
- **N3.** No filtering, jumping or searching by status. That is a
  separate spec; this one only makes the information exist and be
  visible.
- **N4.** The status does not consult the override table. It reads the
  rendered document, which an override rewrites — see S8.

## Specification

### The ladder

- **S1.** `Status` is `#[repr(u8)]` with `Ok = 0 < Unknown = 1 <
  NonCanonical = 2 < Invalid = 3`, deriving `Ord`, so `worst_of` is
  `Ord::max` on a byte.

  It is deliberately *not* `annotation::Tier`. `Tier::Landmark`
  (`pack_size`) is not a defect and maps to `Ok`; `Unknown` is not a
  `Tier` at all but a rendering convention (S3).

### A node's own status

- **S2.** A node's own rows are exactly the rows `overlay_spans` gives
  its slot: a bracketed node's header line (its footer is derived,
  0222), a flat or packed node all of its rows. Every rendered line
  belongs to exactly one node — the property `decode.rs`'s
  `overlay_spans` already states and relies on — so `own` partitions
  across slots with no double counting.

  `own(n)` is the worst rung over those rows.

- **S3.** Per row, in this order:

  | test | rung |
  |---|---|
  | annotation holds a token in `annotation::INVALID` | `Invalid` |
  | annotation holds a token in `annotation::NON_CANONICAL` | `NonCanonical` |
  | **the row's key is numeric rather than a field name** | `Unknown` |
  | otherwise | `Ok` |

  The unknown test is the first non-space character of the row being an
  ASCII digit. A proto field name cannot begin with one, and neither
  can either synthetic name spec 0237 derives (`f<number>`,
  `p<position>`).

  This is exact, not a heuristic: the renderer's numeric-key rule is
  driven by the same `is_known` boolean that suppresses the field
  declaration — `use_numeric_key = unknown || is_wire`
  (`helpers/scalar.rs`), `wob_prefix_n(…, !is_known, …)` and the bare
  number for an unknown group (`sink.rs`'s `begin_nested`). An unknown
  submessage header is literally `N { #@ message`.

  The overlaps resolve by rank, which is why the order above is the
  order it is: an invalid row and a wire-type mismatch both render
  numeric keys, and both carry a keyword (`INVALID_*`,
  `TYPE_MISMATCH`) that outranks `Unknown`.

  **Deliberate call:** a *known* field rendered as raw wire
  (`WireBytes`/`WireFixed*`) also gets a numeric key with no anomaly
  keyword, so it reads `Unknown`. That is the intent — the row is being
  shown un-typed — but it is a decision, not an accident.

  **One exception, at the root.** A root's key is always numeric: the
  wrapper is field 1 of a *virtual* encompassing message (0216 S1), so
  it renders `1 {` whatever the blob holds. Left alone the unknown test
  would tint the topmost toggle of every document ever opened, which is
  the one place the signal has to mean something. A root has no
  enclosing message, so no schema could have declared it and the rung
  has nothing to say: `own` drops `Unknown` to `Ok` when the node has
  no parent. The annotation rungs are untouched, and `rolled` is not —
  a defect below a root still reaches it.

- **S4.** Reading the keyword lists needs the annotation region, from
  `prototext_core::serialize::encode_text::annotation_start` — the same
  format-driven split the `a` key already uses (0187), so a `#@` inside
  a string value does not fool it.

  Prefilter, **per token rather than per annotation**: the commonest
  token by far is the field declaration, and `push_field_decl` is the
  only thing that writes `" = "` into an annotation — every modifier
  spells itself `key: value`. So one substring search per token is what
  keeps the ordinary row off `tier_of`'s comparisons.

  The tempting whole-annotation version is unsound and was tried
  first: `AnnWriter` joins tokens with `"; "`, so a clean annotation
  does hold one token and no `;` — but the converse fails.
  `render_invalid` emits a single-token `#@ INVALID_VARINT`, which a
  `!ann.contains(';')` prefilter would skip, and the row would then
  read `Unknown` off its numeric key instead of `Invalid`.

- **S5.** The status is independent of the `a` key. protolens always
  renders with `annotations: true` (0133) and strips at display time,
  so `node_text` always carries the annotations; and the S3 unknown
  test reads the document half of the row, which `a` never touches.

### Storage and the roll-up

- **S6.** Two parallel `Vec<Status>` indexed by arena slot, siblings of
  `heat_states`:

  - `own[i]` — from the node's own rows (S2).
  - `rolled[i]` — `own[i].max(max over children of rolled)`.

  One byte each: **2 B/slot**, about 9.5 MB against googleapis'
  4 737 284 slots, where `heat_states` alone is 12 B/slot.

  Two arrays rather than one array of pairs, because the children scan
  then reads a contiguous `&rolled[a..b]` — that slice *is* the hot
  loop.

- **S7.** The full computation is one **reverse linear pass, O(n)**.
  The arena is in level order: a node's children are the contiguous
  block `first_child(i)..first_child(i+1)`, and a parent's index is
  always below any child's. So

  ```rust
  for i in (0..n).rev() {
      let kids = self.child_slots(i);   // 0..0 when unrendered
      rolled[i] = own[i].max(
          rolled[kids].iter().copied().max().unwrap_or(Status::Ok),
      );
  }
  ```

  No recursion, no stack, no queue; each slot is touched once as a
  parent and once as a child, with forward-sequential reads inside
  every block. Unrendered slots roll up as `Ok`.

  `own[i]` is computed in the same pass, from `node_text[i]`, rather
  than filled inside `overlay_spans`: the pass has to run anyway, the
  text is already in hand, and keeping it here leaves the roll-up in
  one readable place instead of splitting it across two crates.

  Folding changes nothing. The roll-up is fold-independent by
  construction, which is exactly what lets a folded toggle speak for
  what it hides.

### Incremental update

- **S8.** An override updates the status in **O(k + d·m)**, where k is
  the spliced subtree's slot count, d the depth and m the largest
  sibling block on the path. It copies `refresh_line_counts`' shape:

  1. Recompute `own` and `rolled` over the spliced subtree, **O(k)** —
     a subset of work the re-render already pays.

     This one is **recursive post-order, not S7's reverse scan
     restricted to a range**. Level order groups slots by *depth*, so a
     subtree is a union of one range per level rather than a single
     range, and any range covering it would sweep in unrelated
     siblings. The recursion is bounded by the arena's own depth cap,
     and the deepest real document measured is 13.
  2. Walk to the root recomputing `rolled[p] = own[p].max(max over its
     children)`, stopping at the first ancestor whose value is
     unchanged. **O(d·m)**.

  `max` is not invertible, which is why step 2 is not O(d): an
  *increase* needs one compare per level, but a *decrease* — the case
  an override is for — must re-max the siblings. With depth 13 measured
  on googleapis and a sibling block being a contiguous `u8` slice, this
  is a few short byte scans, once per override commit, never per frame.

  `finalize_override_batch` is where the batch-level version belongs if
  one batch splices several subtrees.

- **S9.** Nothing in S1-S8 reads the override table. An override that
  supplies `--field-name` makes `splice_override` render a symbolic key
  (`field_name_for` honors the entry's name), so the node leaves
  `Unknown` on its own. An override that supplies only `--as` leaves
  the key numeric and the node blue, while its subtree goes `Ok`
  because the children now resolve against the asserted type — which
  reads correctly as "these bytes are a `Foo`, but nothing says what
  field 12 of the parent is."

  On the ordinary path this is moot: `o` always pre-fills
  `--field-name` with a value that is never empty (`display_name_for`,
  0237), so confirming the pre-fill clears the blue. The two only
  diverge for a hand-typed `:override … --as Foo` with no name, where
  the numeric key is the truthful answer.

### Rendering

- **S10.** The fold margin becomes a styled span. `Ok` keeps today's
  default; `NonCanonical` and `Invalid` take the same colors as the
  annotations themselves, via `theme.rs`'s existing `tier_color`;
  `Unknown` takes a new light blue, needed in three palettes (dark RGB,
  light RGB, ANSI-16) and leveled to the same luma as its neighbours,
  like every other palette entry.

  A new `pub fn status_color(status, theme) -> Color` in `theme.rs`
  delegates to the private `tier_color` for the two shared rungs, so
  there is one home for each color.

  *Amended 2026-08-05.* The margin is now split so that only the
  **glyph** is styled, the surrounding spaces staying raw. Coloring the
  whole margin was harmless — ink on a space is invisible — but spec
  0192's override cue also lands on this marker, and an underline on a
  space is not invisible: it would draw a rule across the indentation,
  saying how deep the node is rather than anything about the node. The
  split is paid only on a margin that has something to say.

- **S11.** The toggle shows `rolled[i]` whether the node is folded or
  not. Unfolded it is redundant with what is on screen only when the
  whole node fits the viewport — which is exactly when it costs
  nothing, and the rest of the time it is the only thing saying there
  is something red below the fold or beyond the pan.

- **S12.** With no descriptor loaded every field is unknown and the
  whole tree goes blue. That is kept, not suppressed: it is honest
  signal, and after a partial resolution the surviving blue is what
  points at the subtree still lacking a descriptor.

- **S13.** Per frame this is one array index per visible row — no
  lookup, no walk, no allocation.

## Alternatives considered

### Testing the annotation for a field declaration instead of the key

`push_field_decl` is the only writer of `" = "` into an annotation
(every modifier uses `key: value`), so `!ann.contains(" = ")` is
*exactly equivalent* in coverage to S3's numeric-key test — both are
driven by the same `is_known`. Rejected anyway on three counts: the key
is what the user is looking at, so the cue agrees with the screen; it
survives the `a` key with no special case; and it is one character
against a substring search over the annotation.

### A status field on `NodeSpan`

Free at runtime — the renderer already knows `is_known` and the
anomaly, so nothing would need re-deriving from text. Rejected because
`NodeSpan` is pinned at exactly 32 bytes by an equality assert
(`sink.rs`, spec 0212), 4.74 M live instances make the growth real, and
`NodeSpan` lives in prototext-core, which has no business carrying a
protolens display concern.

### Per-node child counts, `[u32; 4]`

Counting how many children sit on each rung makes a *decrease*
decrementable, so S8's ancestor walk becomes O(d) with no sibling scan.
Rejected on memory: 16 B/slot is about 76 MB on googleapis, against a
history where peak RSS was deliberately cut 4.18 → 1.66 GiB (0216) —
too much to buy down an operation that is already well under a
millisecond and runs once per commit.

### "An explicit override means known by convention"

Would have `Unknown` clear for any non-automatic override regardless of
`--field-name`. Rejected: it makes a text-derived table depend on the
override table, adding a second input to invalidate on, and it claims
the schema knows a field when the user merely *asserted* a type — the
one case where the reminder is worth most. S9 gets the useful half of
it for free anyway.

## Test plan

The classifier's cases sit next to it in `node_status.rs`; the roll-up's
are in `tui/tests/node_status.rs`; the drawn-cell one is in
`tui/tests/render.rs`, where the frame harness lives.

1. `a_defect_tints_the_fold_marker_of_every_node_above_it` — a fixture
   with one undeclared field; assert the drawn marker cells wearing the
   status color are exactly the nodes on the path to it, **and** that
   the same document without the field tints nothing. Only the pair is
   informative: a one-sided test would pass on a renderer that tinted
   everything.
2. `the_worst_child_wins` — a clean sibling and a defective one under
   one parent; assert the parent's `own` stays `Ok` while its `rolled`
   takes the worse child.
3. `a_numeric_key_is_an_unknown_field` — a numeric key reads `Unknown`,
   both synthetic names 0237 derives read `Ok`.
4. `a_landmark_is_not_a_defect` — a `pack_size` row stays `Ok`.
5. `an_anomaly_keyword_outranks_a_numeric_key` — `TYPE_MISMATCH` on a
   numerically-keyed row reads `Invalid`; a single-token
   `#@ INVALID_VARINT` does too, which is the case the discarded
   whole-annotation prefilter got wrong (S4).
6. `a_hash_at_inside_a_string_value_is_not_an_annotation` — the case a
   naive `find("#@")` would misclassify.
7. `hiding_annotations_does_not_change_any_status` — toggle `a`,
   recompute, assert the array is unchanged.
8. `folding_does_not_change_any_status` — same, for a fold.
9. `assert_status_is_exact` — hung off `finalize_override_batch` behind
   `verify_repair`, exactly like spec 0186's G3 check, so that *every*
   splice in the suite is a case rather than one dedicated test. This
   is what covers the S8 decrease path, which the ancestor walk's early
   stop is most likely to get wrong.
10. `naming_a_field_clears_the_unknown` — `:override … --field-name`
    takes the node and its parent back to `Ok` (S9).

## Measured outcome

Implemented as specified, with the three corrections folded into S3,
S4 and S8 above — each of which was a real defect the implementation
found, not a wording change:

- the root's synthetic `1 {` key would have tinted the top toggle of
  every document;
- the whole-annotation `;` prefilter would have read a single-token
  `#@ INVALID_VARINT` row as `Unknown`;
- a subtree is not a contiguous slot range in level order, so step 1 of
  the incremental update had to be recursive.

Cost is as designed: 2 B/slot, one reverse linear pass at load, one
array index per visible row.
