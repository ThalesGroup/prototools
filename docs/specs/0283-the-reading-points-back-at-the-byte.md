<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0283 — the reading points back at the byte

Status: implemented
Implemented in: 2026-08-13
App: protolens
Refs: docs/specs/0282-a-wire-byte-says-what-it-is.md (the hover box this
        adds a mark to, its recorder, and the G4 this inherits)
      docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md (the
        wire row and its regions)
      docs/specs/0235-a-search-answers-while-it-is-still-being-typed.md
        (S14's two match styles, one of which this borrows)
      docs/specs/0192-a-frame-costs-the-same-wherever-the-cursor-is.md
        (what bold and bold+underline already mean, per N3)

## Background

Spec 0282's box answers *what is this byte*, and for a LEN payload it
answers with the whole payload:

```
string   "hello world"  ← declared
bytes    "hello world"
```

The pointer is on one byte. The box shows eleven. A reader who hovers
`6f` has to count hex pairs and then count characters to find out which
`o` they are on — and the two counts do not agree the moment the payload
holds an escape, which is exactly when the question is worth asking:

```
2|08:06[41 ff fe 42 0a 43]
bytes    "A\377\376B\nC"
```

Six bytes, twelve characters, no alignment between them. The box has the
answer and does not point at it.

## Goals

- **G1.** Hovering a byte of a LEN payload marks the glyphs *that byte*
  produced in the box's `string` and `bytes` readings.
- **G2.** No new visual vocabulary: the mark is one protolens already
  uses for "the thing you asked about is here".
- **G3.** No mark rather than a mark that is wrong or partial.
- **G4.** Nothing per frame, inheriting 0282 G4 — the pointer moving is
  what pays.

## Non-goals

- **N1.** No mark on the numeric readings. A varint's bytes are 7-bit
  groups; they do not align to the decimal digits of `int32`, to the
  zig-zag of `sint64`, or to anything else the reader could act on. The
  same holds for `double` and `float`: the fifth byte of a `fixed64`
  contributes to the exponent *and* to the mantissa, and no substring of
  `1.4142135623730951` is it.
- **N2.** No mark on the tag, wire type or length boxes. Those parts say
  one fact about a whole varint and have no glyph-per-byte reading to
  point into.
- **N3.** No underline, no bold, no reverse video, no fourth color.
  Spec 0192 spends bold and bold+underline on override emphasis and
  reverse video is the caret's; a fourth spelling of "look here" would
  mean the reader has to learn which is which.
- **N4.** Not a selection. Hovering a byte does not let the reader copy
  it, and this adds no binding.
- **N5.** Wide characters are not fixed here. `render_popup` measures its
  box in `chars().count()`, so a payload holding CJK text already draws
  narrower than it needs; the mark inherits that and does not correct it.
  It is one bug, in `render_popup`, and it is not this one.

## Specification

- **S1.** The recorder of 0282 S3 gains a second, *uncoalesced* list: for
  every byte the painter draws, the columns it drew it in and the
  absolute offset it came from. `Painter::byte` and `Painter::tag_head`
  both file one — `tag_head` files a single entry for the byte its two
  halves share. Coalescing is what makes a payload run one *part*, and
  is right for the part; this is the resolution the part throws away.
  Like the part list it lives behind the same `Option<WireRecord>`, so a
  drawn row still allocates nothing.

- **S2.** `WireHit` gains `byte: Option<usize>` — the offset of the byte
  whose recorded columns contain the pointer. `None` for the `??` and
  `…×N` marks, which stand where bytes are not.

- **S3.** A box line becomes text *plus* an optional mark: a character
  range within that line. The mark is attached to the line and not held
  in a table keyed by line index, because `fit` (0282 S10) drops lines
  and moves the flaws past an ellipsis — an index-keyed table would have
  to be remapped at every one of those steps, and would be wrong the
  first time one was added.

- **S4.** Only `Region::Payload` of a `WT_LEN` record marks anything, and
  only on the two lines that spell the payload out. The bytes are the
  ones 0282 S9 already reads — the *drawn* ones — so a byte that can be
  hovered is always a byte the readings contain.

- **S5.** The `bytes` reading's mark is exact. `escape_bytes` maps one
  input byte to one escape group with no dependence on its neighbors
  (the octal form is always three digits), so the escape of a prefix is
  the prefix of the escape: the mark starts at
  `1 + escape_bytes(&payload[..i]).len()` — the `1` is the opening quote
  — and is `escape_bytes(&payload[i..=i]).len()` wide. The output is
  ASCII throughout, so counting bytes and counting characters agree.

- **S6.** The `string` reading marks the whole *character* the byte
  belongs to. `escape_string` maps one character to one escape group, so
  the same prefix argument holds one level up; a hovered continuation
  byte then marks a character wider than itself. That is the honest
  answer and not an approximation: those bytes spell that character, and
  no shorter piece of the string is the byte's contribution. A `string`
  reading of `not valid UTF-8` marks nothing — there are no glyphs to
  mark.

- **S7.** The mark is drawn in `theme::search_current_style`, the style a
  search gives the match it landed on. Not `search_match_style`: the
  muted variant means "another occurrence, context rather than the
  answer", and the hovered byte is precisely the answer to the question
  the pointer asked.

- **S8.** A mark that would not be fully drawn is dropped, not clamped. A
  box line is cut when it exceeds the pane width, and a half-drawn mark
  claims the byte is shorter than it is — which is the one thing this
  feature exists to get right. `fit` dropping a line takes its mark with
  it, since S3 attached the two.

- **S9.** `render_popup` splits a marked line into up to three spans.
  This is the only place the mark reaches a `Style`; everything above is
  ranges.

- **S10.** The help text's hover paragraph gains a clause saying that the
  byte's own glyphs light up in the value.

- **S11.** Sliding along a payload is one gesture, not one gesture per
  byte. Two hits that differ only in `byte` do not restart the dwell —
  spec 0282 S1's rule, which S2 would otherwise have broken, since the
  hit now resolves finer than the thing it names. And once the box is
  open, moving to another byte of the same part re-marks it *at once*,
  at the same anchor: the dwell exists to decide whether the reader
  wants a box at all, and one is already in front of them. That event is
  the one motion owed a frame, and only when the byte actually changed —
  the two columns of one hex pair are the same byte and cost nothing.

## Alternatives considered

### Underline or reverse video

Rejected under N3. Both are already spoken for and the reader would have
to learn a third dialect for a fact the search already has a color for.

### Clamping the mark at the cut

The cheaper half of S8: draw as much of the mark as fits. Rejected
because it produces a confident wrong answer — a four-character
`\377` marked as `\37` says the byte spells three characters.

### A side table of `(line, range)` on the box

Rejected under S3. `fit` reorders and drops; every one of its four
branches would have to maintain the table, and the failure mode of
forgetting one is a mark on the wrong line, which is worse than none.

### Marking the numeric readings by digit

Rejected under N1: for no numeric reading in the family is there a
substring that the hovered byte produced.

### Keeping a per-byte column map for every drawn row

This is what 0282 G4 rejected for the part list, for the same reason: a
`Vec` per wire row per frame, for a feature idle almost always.

### The other direction — hovering a glyph in the box, lighting the hex

A different feature: the pointer would be over the box, which is drawn
over the pane it would have to highlight, and the box is dismissed by
the next mouse event. Not ruled out forever, but nothing here builds
toward it.

## Test plan

1. `a_hovered_payload_byte_marks_its_escape` — hovering `ff` in
   `41 ff fe 42` marks exactly the four characters `\377` of the `bytes`
   line, and the box's other lines carry no mark.
2. `the_mark_moves_with_the_byte` — hovering each of a payload's bytes in
   turn yields ranges that are adjacent, ordered, and cover the reading
   between the quotes exactly once.
3. `a_multi_byte_character_is_marked_whole` — a two-byte UTF-8 character
   marks the same single character from either of its bytes, on the
   `string` line (S6).
4. `an_invalid_string_reading_marks_nothing` — the `not valid UTF-8` line
   has no mark, while the `bytes` line beside it does.
5. `a_varint_reading_carries_no_mark` — N1, over an `int32` payload.
6. `a_tag_box_carries_no_mark` — N2.
7. `a_mark_past_the_box_edge_is_dropped` — S8, with a pane narrow enough
   to cut the reading.
8. `a_dropped_line_takes_its_mark_with_it` — S3, with `avail` small
   enough that `fit` drops the `bytes` line.
9. `the_mark_is_the_searchs_current_style` — a rendered frame, reading
   the popup's cells back and checking the background against
   `theme::search_current_style` (S7, S9).
10. `sliding_along_a_payload_keeps_one_box` — S11: the dwell is not
    restarted, the open box does not move, it re-marks on the spot, and
    leaving the part is still a new question.

## Measured outcome

Implemented 2026-08-13, in 25 `popup_wire` tests (10 of them new); 994
protolens tests pass.

Deviations from the text above:

- **S11 was written after the rest of the spec was implemented.** The
  first build made every byte its own hover target, so the dwell
  restarted on each one and the box flickered its way along a payload —
  which is exactly the reading gesture the feature is for. `WireHit`
  gained `same_part`, an exhaustive destructuring that names `byte` as
  the one field it ignores, and `handle_hover` grew a third case between
  "the same target" and "a different one".

- Test plan item 10 was `a_wire_hover_still_costs_no_frame`, which 0282's
  `a_wire_hover_costs_no_frame` already asserts unchanged. Its slot went
  to S11's test, which asserts the harder half: which motions *do* owe a
  frame.

- S5's mark is computed on the payload, and S9's suppression compares it
  against the box's inner width — so `wire_value_lines` shifts every mark
  by its type-name prefix (`format!("{name:<8} ")`) on the way out. The
  spec's arithmetic is the payload's; the box's is nine characters to the
  right of it.
