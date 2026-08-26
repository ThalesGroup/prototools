<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0361 — Root node display label

Status: implemented
Implemented in: 2026-08-26
App: protolens
Refs: docs/specs/0222-arena.md (node_text layout),
      docs/specs/0187-syntax-highlighting.md (window_styles byte offsets)

## Background

The root arena node (slot `first_node`, index 0) is the virtual wrapper
protolens writes to make every blob a single-rooted prototext message.
Its raw `node_text` is `"1 {"`, where `1` is the field number protolens
chose for the one field of that synthetic wrapper.  That field number is
meaningless to the user — it is an implementation detail, not something
the file contains.

The user navigates and speaks about nodes using path notation, where
`/` denotes the root.  Displaying `"1 {"` on the root row is therefore
inconsistent with the rest of the UI.

## Goals

- **G1.** Render the root node's opening row as `"/ {"` instead of
  `"1 {"` in the main pane — both in the text path
  (`row_text_of` / `line_display_text`) and in the span path
  (`row_spans`).
- **G2.** Keep the search haystack consistent with what is displayed:
  searching for `/` must find the root row.
- **G3.** Keep `row_content` and `row_spans` byte-identical (the
  invariant spec 0185 G3 requires and `row_content_and_row_spans_agree_
  byte_for_byte` enforces).
- **G4.** Do not feed the transformed text to the tree-sitter
  highlighter: `"/ {"` is not valid prototext (a slash is not a legal
  field-name character), so it must never reach `window_styles_for`.

## Non-goals

- **N1.** A single-source-of-truth cursor for all display transforms
  (fold summary, shadowed_scalar, root label).  That is a larger
  architectural change that can follow independently.
- **N2.** Changing the raw `node_text` stored in the arena — the
  highlighter and any other consumer that reads raw text must see `"1 {"`.

## Specification

- **S1.** The substitution is `"1"` → `"/"`.  Both are exactly one
  byte in UTF-8, so no byte offsets shift and no hint adjustment is
  needed in `row_spans`.

- **S2.** `line_display_text` applies the substitution when
  `owner == Some(self.first_node)` and the raw line starts with `"1 "`:
  replace the leading `"1"` with `"/"`.  This is the single point of
  change for the text path (used by `row_text_of`, `sweep_test`, and
  clipboard).

- **S3.** `row_spans` applies the same substitution to its local
  `full_content` string before calling `segment_line`.  Because the
  replacement is 1-for-1, `full_hints` (byte ranges into the raw text)
  remain valid with no adjustment.

- **S4.** `window_text` (and therefore `window_styles_for`) continues
  to use the raw `node_text` — `display_row_text` is called before the
  display transforms, which is exactly the existing behaviour.  The
  `"1"` the highlighter parses is the prototext field number; the `/`
  the user sees is produced downstream.

- **S5.** `DocCursor` (multi-line search path) does not need to change:
  it already walks raw `node_text`.  The root node's chunk is `"1 {"…`,
  and a search for `"/ {"` will miss it.  A future spec may extend the
  cursor to apply display transforms; until then, multi-line patterns
  that cross the root row are a known limitation.

- **S6.** `sweep_test` (single-line search path) uses `line_display_text`,
  which applies S2.  Single-line searches for `"/"` therefore find the
  root row correctly.

## Alternatives considered

### "root" as the label

`"root {"` is syntactically valid prototext (a message-typed field
named `root`), so it could safely reach the highlighter.  However, as
a four-byte replacement for the one-byte `"1"` it shifts all
`window_styles` hint byte offsets by +3, requiring a per-hint mapping
pass in `row_spans` and a corresponding reverse shift when resolving a
search match's column to a cursor column.  Both passes were implemented
and produced three test failures that were hard to diagnose.  The
1-for-1 substitution avoids all of this, and `"/"` is also more
consistent with the path notation the rest of the UI uses for the root
node.

### A cursor-based single source of truth

The user requested this as a follow-on (N1 above).  Extending
`DocCursor` to yield transformed chunks for both rendering and search
is architecturally cleaner but requires reworking `spans_with_insertions`
to accept a cursor iterator rather than a `&str` with byte-range hints.
That work is independent of the label choice and is deferred.

## Test plan

1. `the_fold_marker_does_not_displace_the_row_it_marks` — checks that
   the root row's content contains `"/ {"` and that the fold glyph sits
   immediately left of it.
2. `row_content_and_row_spans_agree_byte_for_byte` — enforces G3 for
   every row in the fixture, including the root row.
3. `search_finds_the_same_hits_in_the_same_order` — confirms that the
   search haystack matches the displayed text.
4. `a_document_opens_closed` and the other folding tests — confirm that
   `closed_rows()` / `one_level_open()` output `"/ {"` where they
   previously asserted `"1 {"`.
