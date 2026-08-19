<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0327 — a color needs ink to be read

Status: implemented
Implemented in: 2026-08-19
App: protolens
Refs: docs/specs/0138-protolens-main-pane-inference-heat-cue.md (the cue,
        its column and its twelve brightness levels),
        docs/specs/0322-a-leaf-can-be-wrong-too.md (rejected `■` for the
        fold column, on an argument that does not transfer),
        docs/specs/0190-the-activity-dot-reports-the-highest-live-tier.md
        (the activity dot, which keeps `●`)

## Background

The heat cue is a color, and the color is the whole message: red or
blue for mismatch or tie, and one of twelve brightness levels for how
big the finding is (spec 0138 G5). It is drawn as `●` U+25CF, a filled
circle inscribed in a cell — roughly half the cell's area is inked, and
the rest is background. Twelve levels of one hue read across that much
ink only at the extremes; the middle of the ramp is where two adjacent
levels stop being tellable apart, and the middle of the ramp is most of
the corpus.

Spec 0193's fold marker was enlarged twice for exactly this reason, and
spec 0260's five-color margin is what forced it. The cue in column 0 was
not revisited at the time.

## Goals

- **G1.** `heat_cue::HEAT_GLYPH` becomes `■` U+25A0 BLACK SQUARE — a
  fully inked cell, so the hue and level have the whole cell to be read
  in.

## Non-goals

- **N1.** *The activity dot is not changed.* `render::ACTIVITY_GLYPH` is
  its own constant precisely so the two can diverge (spec 0190 S5), and
  it carries no scale: it is present or absent, in one of a few tier
  colors, and a dot is what a status indicator conventionally is. Its
  doc comment's remark that the two "currently share a character"
  becomes false and is corrected, which is the whole of the change
  there.
- **N2.** *The manage pane's `●`/`○` is not changed.* That pair is a
  radio button and its meaning is the contrast between the two, not a
  color.
- **N3.** *No new column and no width change.* `HEAT_FIELD_WIDTH` stays
  2. This is one character for another in the same cell.

## Specification

- **S1.** `HEAT_GLYPH` is `"■"`, U+25A0 BLACK SQUARE.

- **S2.** Two properties are what select it, and a replacement must have
  both:
  - **No Emoji property.** A terminal that reaches for an emoji font
    draws the glyph in the font's own color, which destroys the only
    thing the cue is. This is the constraint `FOLD_GLYPH_OPEN` and
    `ANOMALY_GLYPH` already document; it rules out `▪` U+25AA, `◼`
    U+25FC, `⬛` U+2B1B and `⏹` U+23F9.
  - **East Asian Ambiguous width**, the same class as the `●` it
    replaces and as `ANOMALY_GLYPH` `◆` U+25C6. Under a CJK locale
    every one of them may draw double-width; changing the class would
    make column 0 behave differently from the fold column beside it.

- **S3.** The ladder back down, should the square prove too heavy beside
  the fold column's `▼`/`◆`, is `●` U+25CF — a one-line revert.

## Alternatives considered

**Spec 0322's rejection of `■` for the anomaly mark.** That spec turned
`■` down for the fold column on three arguments, and it is worth saying
why none of them applies here.

- *"A filled square is the stop mark; a diamond is the caution
  silhouette."* The cue is not a severity mark. It says another type
  scores better, which is a finding and not a warning; there is no
  silhouette it should be borrowing.
- *"A fully inked cell is the heaviest mark available, and this is the
  narrowest-scoped signal in that column."* The heat cue is the widest
  signal on its row — it is about the node's whole byte range — and it
  is alone in its column, so there is no weighting to invert.
- *"Two adjacent hues are hardest to tell apart on a solid block."* This
  is the argument that looks like it transfers and does not. The fold
  column carries **five hues** at one brightness, and hue discrimination
  does fall off on a large uniform patch. The cue carries **two hues at
  twelve brightnesses**, and brightness discrimination goes the other
  way: it improves with area. The two columns want opposite glyphs, and
  that is a finding about the two scales, not an inconsistency.

**Enlarging the circle instead.** There is no larger filled circle at
one cell width without an emoji property; `⬤` U+2B24 is Neutral-width
and drawn as a black circle by some fonts and not at all by others.

**Two columns for the cue.** More area, at the cost of a column of text
on every row for a mark that is absent on most of them. `HEAT_FIELD_WIDTH`
is already 2 and the second column is the separator spec 0325 bought.

## Test plan

1. Existing render tests pin the gutter's contents; they must not need
   any change to *column* arithmetic — that they do not is what proves
   the width class was preserved.
2. `popup_doc`'s `heat_chrome_hit` compares the drawn glyph against
   `HEAT_GLYPH` by value, so the hover tests over column 0 exercise the
   constant rather than a copy of it.

## Measured outcome

Implemented 2026-08-19 as one constant. No test changed at all: every
site that names the glyph — `heat_cue.rs`'s three render assertions,
`popup_doc.rs`'s two hover ones — reads `HEAT_GLYPH` rather than a
literal, and no column arithmetic anywhere moved. Which is test-plan
item 1's proof that the width class was preserved, arriving as the
absence of a diff.

The one thing that needed saying rather than changing is why this is
not spec 0322 reversed, and it is recorded twice on purpose: in
Alternatives above, and at `HEAT_GLYPH`'s own doc comment, where the
next reader tempted to unify the two columns will be standing.
