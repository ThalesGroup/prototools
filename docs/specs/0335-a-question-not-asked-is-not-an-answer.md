<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0335 — a question not asked is not an answer

Status: draft
App: protolens
Refs: docs/specs/0331-a-node-that-fits-can-say-so.md (the ` [vetoed]`
        suffix this splits in two, `HeatDisplay::Settled`, and the rule
        that a settled answer is worth words),
      docs/specs/0138-protolens-main-pane-inference-heat-cue.md (the
        `■` glyph and the rule that it is reserved for a finding),
      docs/specs/0287-the-chrome-beside-a-row-says-what-it-is.md
        (`SuffixShape`, `DocElement::HeatGlyph`, and the rule that
        every drawn mark has a box),
      docs/specs/0135-protolens-override-raw-tag-rewrap.md (why
        `can_override` admits every wire type a value can carry, and
        so why it is the wrong gate here)

## Background

Spec 0331 gave the settled-and-vetoed case words: ` [vetoed]`, in the
brightest red the heat palette has. On a schema-resolved document that
turns out to be almost every row. Opening `googleapis.desc` with the
cues in `All`:

```
  ▼ 1 {  #@ FileDescriptorSet = 1 [2829366]
  │ ▼ file {  #@ repeated FileDescriptorProto = 1 [35]
  │     name: "google/protobuf/duration.proto"  #@ string = 1 [vetoed]
  │     package: "google.protobuf"  #@ string = 2 [vetoed]
  │   ▼ message_type {  #@ repeated DescriptorProto = 4 [13]
  │       name: "Duration"  #@ string = 1 [vetoed]
■ │     ▼ field {  #@ repeated FieldDescriptorProto = 2 [9@5]
  │         name: "seconds"  #@ string = 1 [vetoed]
  │         number: 1  #@ int32 = 3 [vetoed]
  │         label: LABEL_OPTIONAL  #@ Label(1) = 4 [vetoed]
  │         type: TYPE_INT64  #@ Type(3) = 5 [vetoed]
  │         json_name: "seconds"  #@ string = 10 [vetoed]
  │       }
```

Every one of those is a `string`, an `int32` or an enum. Inference
proposes *message* types; it was never going to propose anything for a
varint, and the schema already says the `string` fields are text. The
word does not report a search that failed — it reports a search that was
never meaningful.

That matters twice over. It buries the case where the word *is* the most
interesting thing the heat worker can say — bytes that could be a
message, searched against the whole pool, and matched by nothing — and
it spends the loudest color in the pane on the noise rather than on the
signal.

The two states are already distinguishable in the data. They are
conflated because `heat_cue_at`'s only eligibility gate is
`can_override`, and `can_override` answers a different question.

## Goals

- **G1.** A node inference could never have had a candidate for draws
  nothing, in every mode. It is not a row whose answer was "nothing
  fits"; it is a row with no question.
- **G2.** A node inference did search, and found nothing, is the loudest
  mark in the pane: the `■` glyph in the brightest amber, plus a word.
- **G3.** The word is one a reader of a protobuf document can read
  without knowing how the scoring walk is built.

## Non-goals

- **N1.** *No change to `can_override`.* It gates the override pane,
  which must open on a varint so the reader can retype it to
  `sfixed32` — spec 0135 widened it to every wire type a value can
  carry, on purpose. The predicate this spec adds is narrower and
  answers a different question, and the two must stay separate. Anyone
  tempted to unify them should read this line first.

- **N2.** *Only the settled-with-no-score case is gated.* A node the new
  predicate refuses can still draw a `Mismatch`, a `Tie` or either
  pending. Those are findings about a real candidate list; nothing about
  them was ever misleading. Widening the gate to the whole cue would
  silence, among other things, a declared `string` whose bytes actually
  score as a message — which is a genuine and rare finding.

- **N3.** *No change to what is scored or requested.* Like spec 0331's
  mode, this is a decision about what reaches the screen, taken after
  `heat_cue_resolve` has run. The caches stay warm and the worker's
  queue is unchanged.

- **N4.** *Not a new brightness.* The amber is a sentinel, one color,
  outside any ramp — see S4. Grading it would need a number, and the
  whole point of this state is that there is no number.

## Specification

- **S1.** A new predicate in `heat_cue.rs`,
  `App::inference_applies(idx) -> bool`. It is false when either holds:

  1. the node's wire type is `WT_VARINT`, `WT_I32` or `WT_I64`. Those
     bytes cannot be a nested message under any typing, so the candidate
     list was empty before it was built.
  2. the schema declares a type for the node and that type is neither a
     message nor `bytes`. The schema has already said what these bytes
     are; inference exists for the bytes it has not said.

  `bytes` is the carve-out because a `bytes` field is the schema
  declining to say — which is exactly inference's job — and it is the
  common home of an embedded message. A declared `string` is not: it
  asserts UTF-8 text, and a message that is also valid UTF-8 is a
  coincidence rather than a finding.

  Otherwise true: a `WT_LEN` node with a declared message type, with
  `bytes`, or with no declared type at all.

- **S2.** `heat_cue_at` returns `HeatDisplay::None` for
  `Settled { score: None }` when `inference_applies` is false. The test
  sits beside spec 0331's mode match, which is where the "what of a
  resolved answer reaches the screen" decision already lives, and it is
  evaluated only on that one arm (N2).

  In `heat_cue_at` and not in `heat_display`, because `heat_display` is
  a pure function of one `HeatState` and this predicate needs the node.

- **S3.** The surviving case is renamed ` [unmatched]`. Not ` [vetoed]`:
  "veto" is a term from the scoring walk's internals and names a
  mechanism the reader has no view of. Not ` [no fit]` either, which is
  a hair from `SuffixShape::NoFit`'s existing ` [-/47]` — "the current
  type does not fit" and "nothing fits" are the two statements this pane
  most needs to keep apart. `SuffixShape::Vetoed` follows the word to
  `SuffixShape::Unmatched`.

- **S4.** `heat_chrome` draws `HEAT_GLYPH` for `Settled { score: None }`
  in `theme::heat_suffix_style` — the brightest stop of the warm ramp,
  which is the color the suffix already wears — and the suffix in the
  same style. This is the one square whose color is a **sentinel and not
  a ramp position**: it sits at the top of the range because it must be
  seen, not because it is large. Its doc comment must say so, or the
  next reader will infer a maximum score from it.

  This is a deliberate exception to spec 0138 G9 / 0331 N6, which
  reserve the glyph for a finding. Once the noise is gone (S1), an
  unmatched range *is* a finding, and the one that most deserves the
  column.

- **S5.** `DocElement::HeatGlyph { tie: bool }` becomes a three-way
  enum-carrying variant, since there are now three things the square can
  mean. Its hover box gains the third: that nothing known fits these
  bytes, and that the color is flat rather than graded — a reader who
  has learned "brighter means bigger" must be told this one is not on
  that scale.

- **S6.** `SuffixShape::Unmatched`'s box keeps spec 0331's prose. The
  new gate means the reader only ever sees it on a range where it is
  true and interesting, which is what it was written for.

## Alternatives considered

**Reusing `can_override`, narrowed in place.** One predicate is fewer
than two, and it would silently change what the override pane opens on.
The pane must offer `sfixed32` for a varint; the cue must not claim a
message search happened there. Different questions, and the fact that
they have looked alike for two hundred specs is what produced this bug.

**Suppressing the whole cue on a refused node, not just the settled
arm.** Simpler to state and it throws away the case that motivates the
predicate's carve-outs: a declared `bytes` field, or a `string` field
whose bytes really do parse as a message, is precisely where a mismatch
cue earns its keep. See N2.

**Leaving the word blank instead of renaming it.** Spec 0331 already
considered and rejected a blank here, for the reason that survives
unchanged: blank cannot be told from "not reached yet". The complaint
was never that the state is unworthy of words, only that it was being
claimed for rows that are not in it.

**Keeping the glyph out of it, per 0331 N6.** N6's argument was that on
a mostly-correct document an unmatched range is common, so the glyph
would ink most of the column. S1 is what makes that false: after it, an
unmatched range is rare, and rare is the condition N6 was really
protecting.

## Test plan

1. `a_declared_scalar_asks_no_question` — a `string` node and an
   `int32` node with `best_score: None` seeded draw nothing in `All`,
   where before this spec they drew ` [vetoed]`.
2. `an_unmatched_message_says_so_loudly` — a `WT_LEN` node with no
   declared type and `best_score: None` seeded draws the `■` glyph and
   ` [unmatched]`, both in the warm suffix style, asserted on the drawn
   frame.
3. `a_bytes_field_is_still_asked` — the S1 carve-out: a declared
   `bytes` node with `best_score: None` draws the mark, and the
   neighbouring `string` node does not. One test, because the pair is
   the rule.
4. `a_refused_node_still_reports_a_mismatch` — N2: a `string` node with
   a real `best` and no `current` entry still draws its mismatch cue and
   its suffix.
5. `the_gate_asks_for_nothing_new` — N3: the requests a frame pushes
   are unchanged by the predicate, established the way spec 0331's
   `the_third_state_asks_for_nothing_new` is.
6. `the_unmatched_square_has_a_box` — spec 0287 S6 on the third
   `HeatGlyph` kind: hovering it yields its own prose, not the tie's and
   not the mismatch's.
7. `can_override_is_unchanged` — N1, pinned directly: the four wire
   types `can_override` admits are still admitted, so the override pane
   opens where it did.

## Measured outcome

Filled in at implementation.
