<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0362 — display_pieces: single source of truth for display transforms

Status: draft
App: protolens
Refs: docs/specs/0274-cursor-search.md (DocCursor / regex_cursor::Cursor),
      docs/specs/0343-shadow-mark.md (shadowed_scalar),
      docs/specs/0194-fold-summary.md ({ ... } insertion),
      docs/specs/0260-unread-fold-style.md (bake-unread brace color),
      docs/specs/0361-root-node-display-label.md (/ label)

## Background

Three display transforms are applied to raw `node_text` before a row
reaches the screen:

1. **Root label** (spec 0361): `"1"` → `"/"` at byte 0 of slot
   `first_node` — a 1-for-1 byte substitution, so no hint offsets
   shift.
2. **Fold summary** (spec 0194): `"{ ... }"` replaces the content from
   the last `{` onward on the header line of a user-folded node.
3. **Shadowed-scalar suffix** (spec 0343 B10): `"; shadowed_scalar"`
   appended at the end of the annotation clause for a shadowed slot.

These transforms are currently implemented in two places:

| Site | Transforms applied |
|------|--------------------|
| `line_display_text` | root label, fold summary, shadowed-scalar |
| `row_spans` | root label, fold summary (via `insertions`), shadowed-scalar (via `insertions`) |

Every time a transform is added or changed, both sites must be updated
in sync.  They have already drifted (spec 0361's "root" attempt) within
one session, causing hard-to-diagnose test failures.

`DocCursor` (`search_cursor.rs`, spec 0274) is a third site that handles
the shadowed-scalar suffix for multi-line regex search.  It is kept
separate — see Non-goals.

## Goals

- **G1.** One definition of every display transform, used by both the
  text path (`line_display_text`) and the span path (`row_spans`).
- **G2.** No performance regression.  The raw line text arrives as a
  borrowed `&str` slice of `node_text`; the shared function must not
  force a full string copy for rows where no transform fires.
- **G3.** `window_styles_for` continues to receive raw `node_text`
  (valid prototext); no transform reaches the tree-sitter highlighter.

## Non-goals

- **N1.** Unifying `DocCursor` with this mechanism.  `DocCursor` is
  bidirectional, abort-aware, and operates on whole `node_text` entries
  spanning many lines.  Single-line rendering concerns do not belong
  there.  The shadowed-scalar suffix is the one transform `DocCursor`
  needs for search correctness; it already handles it with
  `reload_mark`, which can be rewritten on top of `display_pieces` once
  this spec is implemented.
- **N2.** Extending `DocCursor` to emit fold-summary or root-label
  chunks for cross-line searches.  That is a separate spec.
- **N3.** Changing `node_text` storage.

## Specification

### DisplayPiece

- **S1.** Introduce `DisplayPiece<'a>`, a unit of display output for
  one contiguous piece of a rendered row:

  ```rust
  pub(super) enum DisplayPiece<'a> {
      /// A contiguous slice of the raw line text, with its byte range
      /// in that raw text and the syntax role the highlighter assigned
      /// to it (None for unstyled gaps between hints).
      Raw {
          text: &'a str,
          range: Range<usize>,
          role: Option<SyntaxRole>,
      },
      /// A literal string with no backing in the raw text.
      /// `style` is `None` for pieces whose color `row_spans` derives
      /// from `role`; `Some` for pieces whose color is known at
      /// construction time (e.g. the fold summary's bake-unread color).
      Lit {
          text: &'static str,
          role: Option<SyntaxRole>,
          style: Option<Style>,
      },
  }
  ```

  `Raw` pieces are zero-allocation: they borrow from the caller's
  `&'a str`.  `Lit` pieces are `&'static str` constants — also
  zero-allocation.  The only allocation is the single `String` built
  by `line_display_text` when it concatenates the pieces.

### `display_pieces` function

- **S2.** Add a free function (not a method, to avoid borrow conflicts
  when `row_spans` holds `&self.window_styles`):

  ```rust
  fn display_pieces<'a>(
      raw: &'a str,
      owner: Option<usize>,
      hints: &'a LineStyles,
      first_node: usize,
      fold_closed: bool,
      fold_style: Option<Style>,   // spec 0260: bake-unread color, or None
      shadowed: bool,
      annotations: bool,
  ) -> impl Iterator<Item = DisplayPiece<'a>>
  ```

  `fold_style` is the value `unread_fold_style(owner)` returns in
  `row_spans` — `Some(style)` when the node has not been read by the
  bake, `None` otherwise.  Passing it here keeps the fold-summary piece
  fully styled without any post-loop fixup in the caller.

- **S3.** `display_pieces` is a forward-only iterator that yields
  pieces in screen order.  It is a small state machine with four
  phases, each executing at most once:

  **Phase 0 — Root prefix** (fires when `owner == Some(first_node)`
  and `raw.starts_with("1 ")`):
  - Look up the role of byte range `0..1` in `hints`.
  - Yield `Lit { text: "/", role: <that role>, style: None }`.
  - Advance the internal raw cursor to byte 1.

  **Phase 1 — Raw body** (always fires; emits raw bytes from the
  current cursor to `end`):
  - `end` is `brace_pos` if `fold_closed` and a `{` exists in `raw`;
    otherwise it is `annotation_start(raw)` when `!annotations`, else
    `raw.len()`.
  - Walk `hints` in order.  For each byte range `[cursor .. end)`,
    yield `Raw` pieces split at hint boundaries: one piece per
    hint-covered sub-range (carrying that hint's role) and one `Raw`
    piece per gap between hints (carrying `role: None`).
  - `brace_pos` is the byte offset of the last `{` in `raw`
    (pre-computed once, not searched per piece).

  **Phase 2 — Fold summary** (fires when `fold_closed` and a `{`
  exists in `raw`):
  - Yield `Lit { text: "{ ... }", role: None, style: fold_style }`.
  - No further raw bytes are emitted — everything from `brace_pos`
    onward is suppressed.

  **Phase 3 — Shadowed-scalar suffix** (fires when `shadowed &&
  annotations && !fold_closed`):
  - Yield `Lit { text: ";", role: Some(SyntaxRole::Comment),
    style: None }`.
  - Yield `Lit { text: " shadowed_scalar", role: None, style: None }`.
    The NonCanonical amber color is applied by `row_spans` using the
    same `kw_style` lookup as today — it is a `Status`-derived color,
    not a `SyntaxRole`.

- **S4.** Packed-run inner lines (`line_in_node > 0`): the caller
  passes `fold_closed = false`, `shadowed = false`.  Phase 0 never
  fires (inner lines do not start with `"1 "`), Phases 2 and 3 are
  suppressed, so the iterator is a plain hint-splitting walk of the
  raw line — identical to the current behavior.

### `line_display_text` rewritten on top of `display_pieces`

- **S5.** Replace the body of `line_display_text` with a call to
  `display_pieces` followed by concatenation of each piece's text:

  ```rust
  display_pieces(
      raw, owner, &NO_STYLES, self.first_node,
      self.fold_marker_of(owner) == Some(FOLD_GLYPH_CLOSED),
      None,   // fold_style irrelevant: we only need text, not color
      self.annotations && owner.is_some_and(|i| self.is_shadowed(i)),
      self.annotations,
  )
  .map(|p| match p {
      DisplayPiece::Raw { text, .. } => text,
      DisplayPiece::Lit { text, .. } => text,
  })
  .collect()
  ```

  `hints` is `&NO_STYLES` because `line_display_text` needs text
  only.  The `Raw` pieces borrow from `raw` at the correct byte
  ranges, so concatenation assembles the right string without roles.

### `row_spans` rewritten on top of `display_pieces`

- **S6.** Replace the `segment_line` + `insertions` + `cut_segments`
  + `spans_with_insertions` machinery in `row_spans` with a call to
  `display_pieces` (passing `self.window_styles[window_index]` as
  `hints` and `self.unread_fold_style(node)` as `fold_style`).

  For each piece yielded:

  - `Raw { text, role, .. }`: call `self.make_span(text.to_string(),
    role, weight(role))`.
  - `Lit { text, role, style: None }`: call `self.make_span(
    text.to_string(), role, weight(role))`, except for the
    `" shadowed_scalar"` piece which uses the pre-computed `kw_style`
    (one `Status`-derived lookup before the loop, same as today).
  - `Lit { text, style: Some(s), .. }`: `Span::styled(text, s)` —
    the fold-summary piece with the bake-unread color.

  `weight(role)` is determined once per piece:
  - First `Attribute` piece seen: `emphasis` (plus `SYNTHETIC` when
    `node == Some(self.first_node)` and `self.wrapper_offset > 0`).
  - Any `Type` piece: `emphasis`.
  - All others: `Modifier::empty()`.

  The `insertions` `Vec`, `cut_segments`, and `spans_with_insertions`
  are removed entirely.  The overlay-ellipsis insertion is kept as a
  post-loop append — it is a preview-overlay concern, not a
  `node_text` transform.

## Alternatives considered

### A `DisplayChunk` struct with an owned `String` field

The first draft allocated one `String` per chunk.  That is worse than
the current `row_spans` code, which slices an already-owned `String`.
The `Raw` / `Lit` split keeps the allocation profile: zero copies for
raw bytes (borrowed slices), zero copies for insertions (`&'static
str`), one `collect::<String>()` only in `line_display_text`.

### Passing `unread_fold_style` as a post-loop fixup

The previous draft of this spec put `role: None` on the fold-summary
`Lit` and told `row_spans` to detect it and override the style after
the fact.  Passing `fold_style` in avoids that special-case detection
in the caller: the piece carries its final color, and the loop body
needs no knowledge of which `Lit` is the fold summary.

### Shared `(byte_position, insertion)` list

A function returning the list of `(pos, text, style)` triples that
both sites apply shares the *decision* of what to insert but not the
*application*: each site still implements annotation-hiding truncation
and hint-splitting independently.  `display_pieces` puts all of that
in one place.

### Extending `DocCursor` to cover rendering

`DocCursor` is bidirectional and chunk-granular (one whole `node_text`
entry per chunk, spanning many rows).  Rendering is forward-only and
line-granular.  Merging the two would import the abort/epoch/segment
machinery into rendering for no benefit.

## Test plan

1. `row_content_and_row_spans_agree_byte_for_byte` — G1: every row
   including folded and shadowed rows.
2. `the_fold_marker_does_not_displace_the_row_it_marks` — `"/ {"` is
   at the right column.
3. `the_wrapper_roots_key_says_protolens_wrote_it` — `'/'` carries
   `SYNTHETIC`.
4. `search_finds_the_same_hits_in_the_same_order` — haystack matches
   rendered text for folded and shadowed rows.
5. All folding tests — `"/ {"` and `"/ { ... }"` strings.
6. A new unit test `display_pieces_cover_every_transform` that calls
   `display_pieces` directly and asserts the exact piece sequence for
   each transform (root, fold-read, fold-unread, shadowed) in
   isolation, plus the no-op case (plain inner line).
