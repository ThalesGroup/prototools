<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0282 — a wire byte says what it is

Status: implemented
Implemented in: 2026-08-13
App: protolens
Refs: docs/specs/0280-a-heat-cue-says-what-its-score-is-made-of.md (the
        dwell, the box, and the one-target rule this widens)
      docs/specs/0225-a-line-shows-the-bytes-it-is-made-of.md (the wire
        row, its regions, and "one classifier, two rows")
      docs/specs/0279-the-wire-row-points-at-the-byte-the-anomaly-is-about.md
        (the accusation keywords this reads aloud)
      docs/specs/0268-the-bytes-are-shown-where-the-reader-asked.md (the
        row-height map that says which terminal row is the wire row)

## Background

The wire row is the one surface in protolens that shows the reader the
actual bytes, and it is the one surface that explains none of them. It
draws `2|08:05[68 65 6c 6c 6f]` — correct, dense, and silent. A reader
who can already decode protobuf in their head learns nothing they did
not know; a reader who cannot gets no way in.

Everything needed to explain it is already computed and then thrown
away:

- `Painter::region` says which of `Tag` / `Type` / `Len` / `Payload`
  every drawn byte belongs to, and is overwritten as the next byte is
  drawn.
- `Painter::accuse` names the annotation keyword for each defect it
  finds (`TAG_OOR`, `tag_ohb`, `INVALID_VARINT`, …) and keeps it only
  under `#[cfg(test)]`.
- `App::parent_field` resolves the node's field descriptor — name,
  kind, cardinality — and is used today only by the override pane.
- `prototext_core::helpers::codecs` has one decoder per proto type, and
  `serialize::common::scalars` one protoc-compatible formatter per
  type.

Spec 0280 built a hover box over a type name. The same box over a wire
byte is the missing half: 0280 answers "how well do these bytes fit
that type", this answers "what do these bytes say".

## Goals

- **G1.** Resting the pointer on a wire row's wire-type digit, field
  number, length prefix or value opens a box explaining that part — and
  so does resting it on either of the row's two marks, the `??` of a
  truncation and the `…×N` of an elision.
- **G2.** The box reports the *same* defect the row is already coloring
  for, named in the same words the `#@` annotation uses — one
  classifier, now three surfaces.
- **G3.** A value is shown as its declared type reads it, and then as
  every other proto type of the same wire type would read it.
- **G4.** Nothing about the render hot path changes: the mapping from
  column to wire part is derived on hover, not kept per frame.
- **G5.** `wire.rs` stays schema-free (0225 S11). It reports structure
  and its own keywords; the schema half of the box is resolved by the
  caller.

## Non-goals

- **N1.** **No box over the separators — `|`, `:`, `[`, `]`.** They are
  punctuation between the parts, they carry no byte, and there is
  nothing to say about them that the parts on either side do not
  already say. The two *marks*, `??` and `…×N`, are a different matter
  and do get a box: see S11 and S12. They stand where bytes should have
  been, which is exactly the place a reader stops and asks what
  happened.
- **N2.** **No editing from the box.** It reads. Changing a type is
  `o`/`:override`, and a surface that both explains and mutates makes
  the 400 ms dwell dangerous.
- **N3.** **No new dwell, no new dismissal, no new key.** Every timing
  and teardown rule is 0280's, unchanged, and the two boxes are one
  mechanism with two contents. In particular there is no `s`-equivalent
  keyboard opening: `s` names a *node*, and these targets are parts of
  a row that only a pointer can distinguish.
- **N4.** **The box does not re-parse the blob to find the parts.** It
  asks the painter what it just drew. A second parser beside
  `wire_spans` would drift from it, and the drift would show as a box
  describing a byte other than the one under the pointer.

## Specification

### The target

- **S1.** `Hover` grows a target enum rather than a second field:

  ```rust
  enum HoverTarget {
      /// Spec 0280: the type name in a `#@` annotation.
      Type(usize),
      /// This spec: one part of one wire row.
      Wire(WireHit),
  }
  struct Hover { target: HoverTarget, anchor: (u16, u16) }
  ```

  `handle_hover`'s "still on the same thing, do not restart the dwell"
  test compares targets, so sliding along a payload run keeps the dwell
  running exactly as sliding along a type name does.

- **S2.** Which target a point names is decided by *which terminal row
  of the pair* it is in, not by what is drawn there.
  `RowHeights::row_at` already returns `(content_row, part)` and
  `main_pane_line_idx` discards the part. Add `main_pane_line_part`
  returning both, and let `main_pane_line_idx` delegate to it. Part 0
  is the document row and keeps 0280's target; part 1 is the wire row
  and takes this spec's.

  This does not weaken 0225 S8 — a *click* anywhere in the pair still
  selects the line. A click chooses a line; a hover asks about a thing,
  and the two rows hold different things.

### What the painter records

- **S3.** `Painter` gains an optional recorder, `parts:
  Option<Vec<PartSpan>>`, filled only when one is asked for:

  ```rust
  struct PartSpan {
      /// Columns within the row's hex, before the margin is prepended.
      cols: Range<usize>,
      part: WirePart,
      /// Absolute offsets in the blob — `rec` is a slice, so the
      /// painter's own indices are rebased on the way in.
      bytes: Range<usize>,
  }

  enum WirePart {
      /// A region of the record, one-for-one with what the painter
      /// bands: `Tag`, `Type`, `Len`, `Payload`, `Unclaimed`.
      Region(Region),
      /// The `??` a `cut()` draws.
      Truncated,
      /// The `…×N` a `finish()` draws, carrying its own N.
      Elided { hidden: usize },
  }
  ```

  `Region` is wrapped rather than extended. Its five variants are
  exactly the parts of a record, and the painter uses it to decide
  *banding*; the two marks are not parts of a record — they stand for
  bytes that were never drawn — so giving them `Region` variants would
  put two non-regions into a banding enum and oblige every `match` on
  it to answer for them. `Elided` carries `hidden` because `finish()`
  has already counted it, and re-deriving it in the popup would be a
  second count that can disagree.

  Consecutive spans of the same part coalesce, so a payload run is one
  entry and a multi-byte tag's continuation bytes join the field number
  they continue.

- **S4.** An accusation is recorded with the part it is about.
  `Painter::flags` is promoted out of `#[cfg(test)]` and becomes
  `Vec<(Region, &'static str)>`; `accuse` takes the part as its first
  argument.

  The part must be *stated*, not read from `painter.region`: a
  `TYPE_MISMATCH` is found while the painter is drawing the tag but is
  about the type digit, which `tag_head` paints separately. Stating it
  at the call site is where spec 0279 already puts the flaw-to-byte
  pairing, and for the same reason — the site is what knows.

  This is deliberately *not* threaded through `byte`/`bytes` as a
  per-byte mark. The box needs "what is wrong with this part", which is
  a row-level fact filtered by part; a per-byte mark would be a larger
  change to every drawing call to answer a question nobody asked.

### The hit test

- **S5.** `App::wire_part_at(column, row) -> Option<WireHit>` re-runs
  the row with a recorder attached and looks up the column:

  ```rust
  struct WireHit { pos: LinePos, part: WirePart, bytes: Range<usize>,
                   flaws: Vec<&'static str> }
  ```

  `flaws` is `Painter::flags` filtered to the hit's part. A `Truncated`
  hit inherits the flaws of the region the `cut()` interrupted, which
  is where S4 files them — the keyword accuses a byte, and `??` is that
  same accusation drawn where the byte is missing.

  The column arithmetic is `set_caret_from_click`'s, because
  `render_document` builds a wire row the same way it builds a document
  row: `pan_spans(…, self.pan_offset)` and then one raw space for the
  heat gutter. So display column `x` maps to row char `x - 1 +
  pan_offset`, and the row's own hex starts `FOLD_FIELD_WIDTH + indent
  + WIRE_CONNECTOR.chars().count()` characters in, with `indent` the
  leading spaces of `display_row_text` — the very expression the render
  loop passes to `wire_row`.

  One row per motion event, capped at `WIRE_ROW_MAX_BYTES` = 64 bytes.
  The `PackedCursor` memo is rebuilt from scratch for that one row,
  which costs a walk of the enclosing packed run; that is the same walk
  the render loop already does for the row, and it happens at most once
  per pointer movement.

### What each box says

- **S6.** The wire-type digit:

  ```
  wire type 2 — LEN
  reads as: test.Inner
  ```

  The symbolic name is the protobuf spelling (`VARINT`, `I64`, `LEN`,
  `SGROUP`, `EGROUP`, `I32`); types 6 and 7 are `invalid — no such wire
  type`, which is `INVALID_TAG_TYPE` seen from the other side. The
  second line is `current_type_key(node)` — the *effective* type, so an
  override is reflected — and is omitted when there is none.

- **S7.** The field number:

  ```
  field 5 — "name"
  ```

  The number is the decoded one; the name comes from
  `parent_field(node)`, which reads the parent's schema and answers
  `None` for an undeclared field, an unresolved parent, or the root. No
  name is printed rather than a placeholder, and the flaw line below
  says `no schema declares this field` when that is why.

- **S8.** The length prefix:

  ```
  length 5 bytes
  ```

  and, when the record is a packed run, `length 5 bytes — packed run`,
  because `pack_size` is the one mark on this part that is a landmark
  rather than a defect (0225's 2026-08-06 amendment).

- **S9.** The value — the declared reading first, then every other
  proto type the wire type admits, one per line:

  ```
  int32          150          ← declared
  int64          150
  uint32         150
  uint64         150
  sint32         75
  sint64         75
  bool           true
  ```

  The families are fixed by the wire type: VARINT gives the seven
  above, I64 gives `fixed64`/`sfixed64`/`double`, I32 gives
  `fixed32`/`sfixed32`/`float`, LEN gives `string` (or `not valid
  UTF-8`) and `bytes`. Every reading goes through
  `prototext_core::helpers::codecs` and `serialize::common::scalars`,
  so a value in the box and the same value in the document are the same
  digits — the alternative is a second formatter that disagrees with
  the document about `-0`, about NaN, or about float precision.

  The declared line is marked and listed first; when the declared type
  is an enum, its line carries the name the schema gives the value. A
  LEN payload longer than the box shows is elided the way the row
  elides it.

- **S10.** **The box fits the terminal, and the flaws outrank the
  alternatives.** When the pane cannot show every line, the box is
  built by reserving, in order: the declared reading, then the flaw
  lines of S13, then as many alternative readings as the remaining
  height allows. A dropped alternative leaves a single `…` line where
  the list stops, immediately *before* the flaws:

  ```
  int32          150          ← declared
  int64          150
  …
  val_ohb — this varint is padded, not minimal
  ```

  A flaw is why the reader stopped on this byte; a sixth spelling of
  the same number is not. So the `…` says "there are more readings"
  and the lines after it say the thing worth saying. If the height is
  so small that even the declared line and the flaws do not fit, the
  flaws are cut from the bottom and the `…` stands for them too — the
  declared reading is the last line to go, because it is the answer to
  the question that was asked.

  This is the only place the box's *content* depends on the terminal
  size. The chrome's own clamping (0280) still applies on top of it,
  and must not be the only clamp: a box clipped by the chrome would
  drop the flaws first, which is exactly backwards.

- **S11.** The truncation mark, `??`:

  ```
  ?? — bytes the message does not contain
  this record needs 5 more bytes; the blob ends here
  ```

  The second line is the arithmetic the `cut()` site already did: what
  the framing claimed minus what the blob holds. When the cut is inside
  an unterminated varint the sentence names that instead — `this varint
  has no final byte` — because there is no declared length to subtract
  from. The part's flaw line (S13) follows, and it is the whole point
  of the box: `??` is drawn at two sites for one accusation (0279), and
  this is where the reader is told which accusation.

- **S12.** The elision mark, `…×N`:

  ```
  …×212 — 212 bytes not shown
  a wire row draws at most 64 bytes; :export --binary writes them all
  ```

  `N` is `Elided { hidden }`, not recounted (S3). It names no flaw,
  because eliding is not one — the row ran out of columns, the message
  did not run out of bytes (0225 S11), and this box exists to say that
  in words rather than leave the reader to infer it from a gray glyph.

  The second line is the only *instruction* any of these boxes gives,
  and it must name something that actually works. **`w`/`W` do not:**
  `WIRE_ROW_MAX_BYTES` is enforced per *row*, in `Painter::byte`,
  `bytes`, `tag_head` and `punct`, so opening a subtree's wire spans
  (0268) yields more rows that are each capped the same way. The 212
  bytes are not behind another keystroke; they are only behind
  `:export --binary` (0261).

- **S13.** Every box ends with the part's flaws, one line each, in the
  words the `#@` annotation uses and then in a clause of plain English:

  ```
  tag_ohb — the tag varint is padded, not minimal
  ```

  The keyword is printed because it is what the row above shows and
  what `docs/prototext/annotation-format.md` documents; the clause is
  printed because a keyword is not an explanation. A part with no flaw
  gets no line — an empty "no problems found" is a form to read.

### Where it lives

- **S14.** `wire.rs` owns `WirePart`, `PartSpan`, the recorder,
  `WireHit` and `wire_part_at` — structure and keywords, no schema
  (G5).

  The popup modules take the `tui/` tree's prefix-family shape
  (`override_*`, `search_*`, `heat_*`):

  - `score_popup.rs` is renamed `popup.rs` (`git mv`, so the 0280
    history follows it). It keeps the dwell, the anchor, the
    dismissal, the chrome, S10's fitting, the body enum, and — since
    it is small — 0280's own score body.
  - `popup_wire.rs` is new and owns this spec's text: it is the module
    that asks `parent_field` and `current_type_key`, knows what `LEN`
    is called, and decodes a value.

  The rename is worth its churn: with two bodies, `score_popup.rs`
  names one of the two things it holds, and the next reader looking
  for the wire box would not look in it.

- **S15.** The help's "Heat cues" hover line gains a sibling under the
  `w` binding, since that is where a reader looking at a wire row will
  look.

## Alternatives considered

**Record the parts every frame, in `WireRow`.** Rejected on G4: the
map is wanted on hover and only on hover, at most once per pointer
movement, and a `Vec` per wire row per frame is a per-frame allocation
bought for a feature that is idle almost always.

**Derive the part by re-parsing the bytes in the popup module.** This
is the shape that looks cheapest — the popup knows the byte range, so
it could parse the tag itself. Rejected on N4: `wire_spans` has seven
`cut()` sites, an out-of-range arm, an overlong-varint arm and a packed
walk, and a second parser would have to agree with all of them or
describe a different byte than the one the pointer is on. The painter
is the parser; ask it.

**Map the column by measuring the drawn spans instead of recording.**
The spans are `Vec<Span>` and their widths are known, so a hit test
could walk them — but a span carries no part and no byte offset, so
this recovers *where* without recovering *what*, and the what is the
question.

**A per-byte `Mark { band, keyword }` threaded through the painter.**
Considered and rejected as over-built for the question asked. It would
give byte-precise attribution — "this byte is the padding" — where the
box asks only "what is wrong with this part". It touches every drawing
call; S4's `(Region, keyword)` list touches `accuse` and its callers.

**Reuse the `#@` annotation text as the source of flaws.** The
annotation carries the same keywords, and `schema_flaw` already parses
them. Rejected because the annotation names the flaws of the *line*,
with no way to say which part of the tag a `tag_ohb` is about — and
because two of the keywords the wire row raises (`INVALID_TAG_TYPE`,
`END_MISMATCH`) are found by this module and appear in no annotation.

**Let the box scroll, or page, when it does not fit.** Rejected: a
hover box that has to be driven is no longer a hover box — the pointer
is already the thing holding it open, and any key that scrolls it is a
key that 0280 dismisses it with (N3). S10's ranking answers the same
need without a second mode.

**Cap by simple truncation, flaws and all.** This is what the chrome
would do on its own, and it is backwards: the alternative readings are
the padding and the flaw is the news, so a naive bottom-cut drops the
one line the reader needed. Hence S10 builds the body against the
height rather than building it and letting it be clipped.

**Open the box on click instead of on dwell.** A click already means
"put the caret here" in the pair (0225 S8), and overloading it would
make byte inspection and line selection fight.

## Test plan

1. `a_hover_over_the_type_digit_names_the_wire_type` — the digit's
   column yields `Region::Type` and the box's first line names `LEN`;
   the column one to the right (the `|`) names nothing (N1).
2. `a_hover_over_the_field_number_names_the_field` — with a schema
   loaded, the box carries the declared name; without one, it carries
   the number and the `no schema declares this field` line (S7).
3. `a_multi_byte_tag_is_one_field_number_target` — a two-byte tag's
   continuation byte is the same recorded part as the first byte's
   number half, so the dwell does not restart across it (S3).
4. `a_hover_over_the_length_names_its_value` — and says `packed run`
   on a packed record's head row (S8).
5. `a_varint_value_lists_every_varint_type` — the seven readings, the
   declared one first and marked, each equal to what
   `codecs`+`scalars` produce (S9).
6. `a_fixed64_value_reads_as_a_double` — the I64 family, including a
   NaN payload whose `nan_bits` flaw line appears (S9, S13).
7. `a_len_payload_reads_as_a_string_and_as_bytes` — and reports `not
   valid UTF-8` rather than lossy text when it is not (S9).
8. `a_flawed_part_names_its_keyword` — an overlong tag's field-number
   box carries `tag_ohb`, and its *type* box does not (S4: the flaw is
   filed under the part it is about).
9. `a_short_terminal_keeps_the_flaws_and_drops_the_readings` — the
   same flawed varint in a tall pane and in a four-row pane: the tall
   box has seven readings and the flaw, the short one has the declared
   reading, `…`, and the *same* flaw (S10). A second case with no
   flaw at all shows the `…` as the last line.
10. `a_hover_over_the_truncation_mark_says_what_is_missing` — a record
    whose length runs past the blob: the `??` column yields
    `WirePart::Truncated`, the box gives the byte shortfall, and it
    carries the same keyword the row's coloring does (S11).
11. `a_hover_over_the_elision_mark_counts_the_hidden_bytes` — a record
    longer than `WIRE_ROW_MAX_BYTES`: the `…×N` column yields
    `Elided { hidden }` with `hidden` equal to the drawn N, the box
    carries no flaw line, and it names `:export --binary` and not `W`
    (S12). Both marks' *separators* on either side still yield nothing
    (N1).
12. `the_document_row_of_a_pair_still_hovers_its_type` — part 0 of a
    two-row pair opens 0280's box, part 1 opens this one (S2).
13. `a_wire_hover_costs_no_frame` — 0280 G5's assertion, extended: a
    move onto a wire part leaves `event_changed_nothing` true.
14. `a_panned_wire_row_hits_the_same_byte` — with `pan_offset` set, the
    column that lands on a given byte shifts by exactly the pan (S5).

## Measured outcome

Implemented as specified, with four deviations from the spec text.

**The recorder stayed a parameter (S3, S4).** S4 asked for `Painter::flags`
to be promoted unconditionally out of `#[cfg(test)]`. It was not: both
the part list and the keyword list live in one `Option<WireRecord>`,
filled only by `wire_spans_recorded` and left `None` by `wire_spans`. The
drawing path therefore still allocates nothing per row, which is G4's
whole point, and `WireRow::flags()` is `#[cfg(test)]` — it exists for
0225 S11's cross-check, and the box reads `record.flags` directly,
filtered by the region the keyword was filed under. The two wire-row test
helpers (`draw_of`, `told_of`) had to move to the recording entry point;
three tests that called `wire_spans` by hand moved with them.

**`WirePart::Truncated` carries its context.** The spec wrote it as a bare
variant. It is `Truncated { region, missing }`: the `??` mark belongs to a
region (a length that ran off the end is not the same story as an open
varint), and the box needs the byte count to say *how much* is not there
without re-deriving it.

**`pack_size` is not a flaw line.** `accuse` files it under `Region::Len`,
so by S13's letter it would have been read aloud as a defect. It is
instead consumed where it means something — the length box appends
`— packed run` — and filtered out of the flaw list. This also removed the
need for a separate "is this a packed record" query, keeping the fact
derived from the painter, which is N4's spirit.

**S5's margin is measured on the line, not on the row.** The first
implementation read the indent off `row_content`, which is the rendered
row — fold field included. `margin` draws `FOLD_FIELD_WIDTH + indent`, so
that subtracted the fold field twice and every hover on a row without a
fold marker landed two columns to the left. The indent is now taken from
`display_row_text`, which is where `render` and `wire_palette` take it.
`every_drawn_hex_column_names_the_byte_under_it` is the guard: the other
tests model `render`'s arithmetic and so agreed with the hit test while
both were wrong, where that one renders a frame and reads the terminal.

**`Region::Unclaimed` gets a box.** S6-S9 enumerated four regions and left
the fifth silent. Bytes no framing accounts for are exactly the ones a
reader is most likely to point at, so they get a one-line box saying so.

Cost, as G4 requires: one extra painted row per pointer *movement* onto a
new part, none per frame. The four gates are clean with and without
`COLORTERM`; 984 protolens tests pass, 15 of them new — one per item of
the test plan, plus the frame-reading guard above — alongside 0280's ten,
which moved with the rename of `score_popup` to `popup` and still pass
unchanged.
