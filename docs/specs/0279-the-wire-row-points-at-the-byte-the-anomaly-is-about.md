<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0279 — the wire row points at the byte the anomaly is about

Status: implemented
Implemented in: 2026-08-12
App: protolens
Refs: docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md (the
        row itself, its four borrowed hues and its `??`),
        docs/specs/0232-an-alarm-points-at-the-bytes-it-is-about.md
        (the one channel by which a document-row accusation reaches
        the wire row — this spec widens it),
        docs/specs/0226-a-fixture-shows-every-anomaly.md (the blob
        every case below is read off),
        docs/specs/0271-a-script-walks-the-reader-through-the-blob.md
        (the walkthrough, and the viewport rule S6 left out)

## Background

Spec 0232 gave the wire row a way to be *told* something it could not
work out on its own: `WirePalette::flaw`, read off the document row's
annotation, aimed at the bytes inside a LEN payload. It shipped with
exactly two cases — `INVALID_STRING` and `INVALID_PACKED_RECORDS` — and
nothing has been added since.

Walking `grpconf/anomalies.pb` end to end shows what that leaves out.
Of the thirty annotations the fixture carries, four name a *byte* and
the row says nothing about it:

| the row above says | the wire row draws | the byte in question |
|---|---|---|
| `double_value: nan  #@ double = 6; nan_bits: 0x7ff8000000000001` | `1|31[01 00 00 00 00 00 f8 7f]` | the leading `01` |
| `label: 99  #@ Label(99) = 4; ENUM_UNKNOWN` | `0|20[63]` | `63` |
| `path: -1  #@ … ; neg` | four plain `ff`s and an `0f` | all five |
| `1: 7  #@ varint; TYPE_MISMATCH` | `0|08[07]` | the wire-type digit |

In each case the reader is told *that* something is unusual and then
handed a row of undifferentiated hex to find it in — which is the exact
complaint spec 0232 was written to answer, still standing four cases
later.

A second, separate defect: two annotations for the same failure — a
varint whose last byte still carries a continuation bit — are drawn two
different ways.

```
3: "\200\200"        #@ INVALID_VARINT           →  0|18[80 80 ??
1: "\001\002\200"    #@ INVALID_PACKED_RECORDS   →  2|08:03[01 02 80]
```

The first reddens both bytes and marks the absence; the second reddens
the open run and marks nothing. A reader who has just learnt what `??`
means does not see it where it applies.

## Goals

- **G1.** Every annotation that names a byte puts the band on that byte
  and on no other.
- **G2.** One failure, one rendering: a varint left open by its last
  byte's continuation bit is drawn the same way wherever it is found —
  that byte accused, `??` in the slot that byte promised.
- **G3.** A script step's node is placed where its subtree can be read,
  not wherever the previous step's scroll left it.

## Non-goals

- **N1.** No schema in this module. Spec 0225 S11's "one classifier, two
  rows" stands: the wire row is still told *what* is wrong by the
  document row's keyword and works out only *where*. Comparing eight
  bytes against `f64::NAN.to_bits()` is arithmetic, not schema — which
  field is a `double` is precisely the part that arrives as a keyword.

- **N2. Withdrawn 2026-08-12 — see S6.** It read: *no accusation for an
  undeclared field. `200: 42  #@ varint` is not an anomaly and
  prototext-core emits no keyword for it; there is nothing to be told.
  The tag's field-number half already borrows the document row's own hue
  for the numeric key (spec 0225 S11), so the bytes are distinguished
  without inventing a severity for them.*

  Walking section 3.b of the fixture against section 6 showed the second
  sentence to be the false one. Both rows carry a numeric key, so both
  halves of the tag borrow the same hue, and the reader is left to tell
  "the schema does not declare this" from "these bytes are broken" by a
  difference the wire row does not draw. The first sentence stands and
  is what S6 is built on: an undeclared field is *not* an anomaly, so it
  gets a band that is not a severity.

- **N3.** No second flaw per row. `WirePalette::flaw` stays a single
  `Option`: prototext-core emits at most one of these keywords per line,
  and a two-flaw row would need a precedence rule that nothing would
  exercise.

## Specification

- **S1.** `PayloadFlaw` becomes `SchemaFlaw` — what it always was, a
  fact the document row's classifier had and this row could not — and
  gains four variants: `EnumUnknown`, `NanBits`, `Neg`, `TypeMismatch`.

- **S2.** `schema_flaw(text)` (was `payload_flaw`) scans *every* token of
  the annotation, not only the first. `INVALID_STRING` and
  `INVALID_PACKED_RECORDS` open an annotation; `ENUM_UNKNOWN`,
  `nan_bits`, `neg` and `TYPE_MISMATCH` all follow a type token, so the
  first-token reading finds none of them. The first keyword that matches
  wins (N3).

- **S3.** Where each flaw lands, and on what:

  - `Utf8`, `PackedElements` — a LEN payload, unchanged from spec 0232.
  - `EnumUnknown` — the value varint of a `WT_VARINT` record, or of a
    packed element. Every byte of it: the number is the accusation and
    the number is all of them.
  - `NanBits` — a fixed-width payload, on the bytes that differ from
    `f64::NAN` (8 wide) or `f32::NAN` (4 wide). That is the whole of
    "what a re-encode would change", stated in the one place it is
    visible.
  - `Neg` — the value varint of a packed element, every byte. `neg` is
    the packed-run spelling of `truncated_neg`: a negative int32 written
    in five bytes where the specification asks for ten, so the element
    *is* the anomaly and there is no sub-run to point at inside it.
  - `TypeMismatch` — the wire-type digit of the tag, and nothing else.
    The field number is right, the payload is what the tag says it is;
    the three bits that contradict the schema are the whole of it.

  Overhang keeps its own band where the two overlap: an accused varint
  that is *also* padded draws its padding in `val_ohb`'s tier, because
  that is a second fact about a specific run of bytes and the value's
  band is a fact about all of them.

- **S4.** A varint whose last byte carries an unwarranted continuation
  bit accuses **that byte alone** and then draws `??`. This replaces two
  different renderings:

  - `draw_varint`'s truncation arm banded every byte from the varint's
    start; it now bands the last one.
  - `packed_flaw` returned the open run and drew no `??`; it now returns
    the open run's last byte and asks for one.

  The bytes before it decoded; saying they did not is false, and the
  `??` is what says the fault is an absence.

- **S5 (amends spec 0271 S6).** After a step has applied its folds, its
  node and its wire span, it scrolls so that the view opens on the
  **outermost thing enclosing the node that still fits in the pane**. A
  step *declares* a view (0271 N1), and `clamp_scroll_to_cursor` — which
  only ever moves far enough to bring a row on screen — declares
  nothing: it lands the node on the pane's **last** row whenever the
  previous step was above it, putting the subtree the step is about
  entirely off screen.

  A fixed margin from the top was the first draft and is wrong: it
  places the same subtree differently depending on what happens to sit
  above it, and two steps aimed at the twin halves of one anomaly have
  to land on the same view. The fitting-ancestor climb gives them one.

  **Amended 2026-08-12: the fitting ancestor's previous sibling comes
  too, if the two still fit together.** A caption need not be an
  ancestor of what it captions — spec 0226's amendment of the same date
  makes it the opposite, a top-level `name` line *beside* the wrapper,
  so that folding the document leaves the headings readable. The climb
  alone then opens the view on the wrapper's own first row and leaves
  the row naming the section one row above it, off screen. Reaching
  back over one sibling is structural in the same way the climb is, so
  the twin-halves property survives: a sibling is the same sibling for
  both steps.

- **S6 (added 2026-08-12, reversing N2).** When no schema declares the
  field, the whole field-number half of its tag is banded — including
  the padding-free bytes of an overlong tag, which keep their own
  `tag_ohb` band the way a padded value's do. `TAG_OOR` outranks it: a
  field number out of range is a defect, not a gap in the schema.

  The band is **not a tier**. It is the fold margin's own
  `Status::Unknown` blue, so the toggle summarizing a subtree and the
  bytes inside it are one fact read twice — the argument spec 0225 S11
  already makes for the four borrowed hues. This is why banding stops
  being `Option<Tier>` and becomes `Option<Band>`, with
  `Band::Tier(Tier)` and `Band::Unknown`; `From<Tier> for Band` keeps
  every existing call site and test helper unchanged.

  The detection does not go through a keyword, because there is none to
  go through: prototext-core emits a bare wire type and a numeric key,
  and `tier_of` would have nothing to answer. `schema_flaw` falls back
  to `node_status::row_status(text) == Status::Unknown` — the same
  classifier the fold margin reads. That predicate is exactly "numeric
  key and nothing accused": `Status`'s ladder ranks every anomaly above
  `Unknown` and combines the two readings with `max`, so a *declared*
  field that happens to render numerically (`1: 7  #@ varint;
  TYPE_MISMATCH`, all of section 6) is not caught by it. Scanning
  `schema_flaw`'s own word list for `"varint"` would have been, since
  `find_map` runs in document order and the type token comes first.

## Alternatives considered

**Have the wire row find the NaN itself.** It could: a fixed64 whose
top twelve bits are all set is a NaN whatever the schema says. Rejected
as N1 — the row would then have to guess that eight bytes are a float
rather than an `sfixed64`, and would flag every large negative integer
whose bit pattern happens to collide. The keyword is already there.

**Highlight the sign-extension bytes of a `neg` element only.** Ten
bytes of which the last five are "the extension" reads well until the
value is `-2`, where the boundary between value and extension is not at
a byte at all. Rejected: the variable-length encoding has no such seam,
and drawing one would be a claim about the bytes that is not true.

**Give `truncated_neg` a band too.** Spec 0226's §2a writes `-1` in five
bytes where ten are specified — but all five decode, nothing is missing,
and a band would be an accusation against bytes that are simply the
whole of the field. The comparison with its canonical twin is what makes
the point, and the script now shows both rows.

**Band an undeclared field in the non-canonical orange.** The first
draft of S6, and it says the wrong thing twice: an undeclared field is
not less canonical than a declared one — the bytes are well formed and
mean exactly what the tag says — and the orange is already spoken for
by section 1's padding, three sections above. Blue is the color the
fold margin already gives the same fact.

**Keep `clamp_scroll_to_cursor` and give it a margin.** It is the
*reader's* rule — a caret pushed off the bottom by `j` should move the
view by one row, not jump — and a margin there would move the document
under every ordinary keypress. The script's rule is a different rule
because a step is a different gesture.

## Test plan

1. `an_accusation_about_a_payload_is_pointed_at_its_bytes` — extended
   with the four new flaws (S1/S3).
2. `an_open_varint_accuses_its_last_byte` — S4, at both sites: a
   `WT_VARINT` value and a packed payload.
3. `a_flaw_is_read_from_any_token_of_the_annotation` — S2.
4. `a_step_leaves_room_below_its_node` — S5.
5. `a_step_keeps_the_row_above_its_subtree` — S5's 2026-08-12
   amendment: with room for both, the fitting ancestor's previous
   sibling opens the pane and the subtree still ends inside it.
6. `an_undeclared_fields_number_wears_the_fold_margins_blue` — S6: the
   band is on the field-number half and is neither `NonCanonical` nor
   `Invalid`, the payload and wire-type digit keep their borrowed hues,
   and `flags` is empty because no keyword says any of it.
7. `a_row_with_a_numeric_key_and_no_keyword_is_undeclared` — S6's
   predicate, against the two rows that must *not* trip it: a declared
   field rendered numerically by `TYPE_MISMATCH`, and a section-6
   malformed row.

## Measured outcome

Walking `grpconf/anomalies.script` end to end, the four rows from the
Background table now band the byte the row above names:

| the row above says | the wire row draws | banded |
|---|---|---|
| `nan_bits: 0x7ff8000000000001` | `1\|30[01 00 00 00 00 00 f8 7f]` | `01` |
| `Label(99) = 4; ENUM_UNKNOWN` | `0\|20[63]` | `63` |
| `neg` | `80 80 00 ff ff ff ff 0f]` | the last five |
| `varint; TYPE_MISMATCH` | `0\|08[07]` | the `0` |

And the two spellings of one failure are one spelling:

```
3: "\200\200"        #@ INVALID_VARINT           →  0|18[80 80 ??
1: "\001\002\200"    #@ INVALID_PACKED_RECORDS   →  2|08:03[01 02 80 ??]
```

with the band on the last byte and the `??` in both.

The fixture and the script were reworked alongside: every anomaly is
lettered `1.a.`, `1.b.`, …, section 7's group twin is written in the same
order as every other twin (anomaly above, canonical below) and at the
same field number, and the script is 30 steps — an opening step, then
one per lettered anomaly, each with a wire directive, so the two halves
of a twin are two steps rather than one range spanning both.

S5's effect is what the last item of the reader's report asked for: over
those steps the step's node is never on the pane's last row, and the
subtree it is about is on screen without a keypress.
