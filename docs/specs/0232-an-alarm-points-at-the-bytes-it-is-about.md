<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0232 — an alarm points at the bytes it is about

Status: implemented
Implemented in: 2026-08-03
App: protolens
Refs: docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md
        (the wire row's regions, its two-channel color model, and "one
        classifier, two rows", which this spec extends rather than
        revises); docs/specs/0231-the-document-rows-loudness-means-one-thing.md
        (the document row's one brightness, which this finishes by
        collapsing the value colors)

## Background

Five reports, all from use, all one complaint: a color says something,
and what it says is not where the reader has to look.

- A document row spells a string, a number, a bool and an enum value
  name in four different colors. None of the four is news — the
  annotation already gives the type, and the wire row spells the bytes
  out underneath — so the four hues compete with the field name for
  nothing.
- `5: "\001\002\003"  #@ INVALID_FIXED64` drew `1|28[01 02 03!` with
  only the `!` in red. The three bytes that *are* there are the ones a
  reader inspects; they were plain.
- The same row put the `Invalid` red on the wire-type digit. The wire
  type is `1`, which is a perfectly legal fixed64; the accusation is
  about the payload. The row was accusing the one part of itself that
  was fine, because `wire_palette` borrows the hue of the first
  annotation token and on an invalid row that token *is* the keyword.
- `10: "\377\376"  #@ INVALID_STRING` and
  `1: "\001\002\200"  #@ INVALID_PACKED_RECORDS` drew their payloads
  entirely plain. In both cases some specific bytes are the reason, and
  the hex row is the only place they could be pointed at.
- A packed run of forty elements drew forty copies of the same heat
  glyph down the gutter. It is one node with one score; the column read
  as forty findings.

## Goals

- **G1.** A value on a document row is one color, whatever kind of
  value it is.
- **G2.** Wherever an anomaly is about particular bytes, those bytes
  wear the tier's band on the wire row.
- **G3.** No part of a wire row wears a tier the accusation is not
  about.
- **G4.** A heat cue appears once per node.

## Non-goals

- **N1.** No new classifier. This module stays schema-free (spec 0225
  S11): it never decides *that* a payload is wrong, only *where*. Both
  new cases are told to it by the document row.
- **N2.** Nothing for `string_escape`. An escape is a span inside a
  value, not a value, and telling the two apart is what its color is
  for.
- **N3.** The wire row's four borrowed hues stay four. Collapsing the
  document's value colors makes `Region::Payload` one color across
  value kinds as a consequence; the tag/type/length/payload split is
  untouched.

## Specification

- **S1.** `RgbPalette::string_literal` becomes `value`, and
  `StringLiteral`, `Number`, `Boolean` and `Constant` all take it.
  `RgbPalette::number` is deleted. Both ANSI-16 tables collapse the same
  four onto green.

- **S2.** `RgbPalette::boolean` becomes `accent`, reachable only through
  a new `theme::accent_style(theme)`, and the two borrowers that were
  never document text — the manage pane's origin-path header and the
  heat cue's tie suffix `[3@85]` — call it instead of
  `style_for(SyntaxRole::Boolean, …)`. Neither has a `highlights.scm`
  capture behind it, so neither belongs in `SyntaxRole`; this is the
  arrangement `manage_entry_style` already uses.

- **S3.** A payload that ran out wears the `Invalid` band over the bytes
  that *did* arrive, not only over the `!` that closes it. That is
  `INVALID_FIXED64`, `INVALID_FIXED32` and `TRUNCATED_BYTES` — the same
  defect in three wire types, and drawing one of them differently would
  make the band mean "some truncations".

- **S4.** `wire_palette` refuses to borrow a *tier* color for the wire
  type. The type slot is read from the first token after `#@`, which on
  an invalid row is the accusation rather than a type name; the fallback
  is the field name's own hue, which is what a row with the annotations
  hidden already uses. The tags that really are malformed —
  `INVALID_TAG_TYPE`, `TAG_OOR` — are colored by this row's own
  `accuse` calls, which name the bits they mean, and are unaffected.

- **S5.** `WirePalette` gains `flaw: Option<PayloadFlaw>`, the one thing
  it carries that is not a color: `Utf8` for `INVALID_STRING`,
  `PackedElements` for `INVALID_PACKED_RECORDS`, read out of the
  annotation text by keyword. A whole, present LEN payload is then drawn
  through `draw_payload`, which bands:

  - for `Utf8`, every ill-formed sequence `str::from_utf8` reports — all
    of them, not the first, since a reader hunting the bad byte wants
    each one;
  - for `PackedElements`, the run from the first varint left open by its
    continuation bit to the end of the payload.

  Both are localizations of a verdict already reached, which is what
  keeps N1 true: neither could have been *decided* here, since whether
  these bytes are a `string` or a `bytes`, and whether this record's
  elements are varints or eight bytes wide, are schema questions.

  A packed payload whose varints all close is a fixed-width record whose
  length is not a multiple of its element size — and the size is exactly
  what this row cannot know, four and eight being equally consistent
  with the bytes. The whole payload is named, which is the most that is
  true.

  `draw_payload` scans no further than the row will draw
  (`WIRE_ROW_MAX_BYTES`, the row's limit): a payload can be megabytes,
  this runs per frame, and bytes the elision swallows cannot be pointed
  at.

- **S6.** `heat_cue_at` returns nothing for any line with
  `line_in_node > 0`, replacing its `is_footer` check. One rule where
  there were two cases: a bracketed node's closing brace and a packed
  record's second and later elements are both lines of a node that has
  already said its cue.

## Alternatives considered

### Keep four value colors and dim them together

Spec 0231 brought every document hue to one brightness, which leaves
hue as the only axis. Four hues spent on a distinction the annotation
and the wire row both already make is four hues not available to the
things that do need telling apart — and the field name, which is the
row's most-scanned token, is one of them.

### Let `SyntaxRole::Boolean` keep the accent and give values three roles

It is what shipped before this. The tie suffix and the origin-path
header would have gone brick along with every real value, and a tie
count is not a value.

### Have the wire row detect invalid UTF-8 itself

It cannot: a `bytes` field holding `\377\376` is perfectly well formed,
and the row has no schema to tell it which it is looking at. Guessing
would put a red band on correct data, which is worse than the plain
payload it replaces.

### Guess the packed element width from the payload length

Tried on paper and dropped. For a length of five, width 4 leaves a
one-byte remainder and width 8 leaves five, and nothing in the bytes
chooses between them — so the rule would name the wrong bytes about
half the time it fired. Naming the whole payload is less precise and is
never wrong.

### Suppress the packed run's repeated cues in the renderer

The repetition is not a rendering artifact: every element line asks
`heat_cue_at` about the same node and gets the same true answer. The
place to say "once per node" is where the node is known, which is the
one line-position test S6 makes.

## Test plan

1. `a_payload_that_ran_out_wears_the_band_it_earned` — the three
   truncation cases of S3, each as a caret mask against the drawn row.
2. `an_accusation_about_a_payload_is_pointed_at_its_bytes` — S5's two
   flaws, including that the bytes *around* the bad ones stay plain, and
   that the row reports the keyword it was told.
3. `the_payload_takes_the_values_hue` — rewritten for S1: a string row
   and a numeric one now paint their payloads alike, and both still
   differ from the tag.
4. `a_packed_run_scores_one_cue_over_the_whole_record` — extended for
   S6: the run's first line shows the cue and its later lines show none.
5. The existing `colorize.rs` role tests — unchanged. Collapsing two
   palette entries moves colors, never which role a token gets.

## Measured outcome

`cargo test -p protolens` passes 621, plus 25 in `tests/batch_export.rs`.

No cost added to the ordinary row: `draw_payload` returns to the single
`bytes` call it replaced whenever the document row made no accusation,
which is every row that is not one of the two invalid kinds.

Not achieved, and named in S5: a fixed-width packed record that fails on
its length alone is banded across its whole payload rather than on its
remainder. Localizing it needs the element width, which is schema and
which this module deliberately does not have.
