<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0231 — the document row's loudness means one thing

Status: implemented
Implemented in: 2026-08-02
App: protolens
Refs: docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md
        (S11's `leveled`, which this borrows and generalizes; S12's
        annotation vocabulary and its drift test)

## Background

A protolens document row is almost entirely colored tokens: field name,
value, and — with annotations on — a type, a field number, and any
number of anomaly keywords. The palette is VSCode's, where brightness is
part of how each scope is told apart. Across a whole screen that reads
as noise: ten hues at ten brightnesses (measured on `DARK_RGB`:
`comment` 138, `link` 173, `number` 198, `attribute` 209, `type` 229)
give the eye a second axis to track that carries no information, since
the hue already said which scope it is.

The cost is not the noise itself but what it crowds out. An anomaly
keyword has nothing left to be loud *with* — it is one bright token
among many.

Two further reports, both from use:

- `type`'s pale yellow and `tier_non_canonical`'s gold read as the same
  color at a glance, which is the one confusion S11's leveling work was
  supposed to have ended.
- `INVALID_STRING` and `INVALID_PACKED_RECORDS` were drawn as plain
  comment text. Both are keywords prototext-core emits; neither was in
  `annotation::INVALID` or in `highlights.scm`.

## Goals

- **G1.** Every meaning-bearing token on a document row is the same
  brightness, so hue is the only thing distinguishing them.
- **G2.** Loudness on a document row means exactly one thing: an
  anomaly.
- **G3.** The type color and the non-canonical tier's color are
  distinguishable at a glance.
- **G4.** Every anomaly keyword the renderer can emit is colored by its
  tier.

## Non-goals

- **N1.** Nothing for the wire rows. They have their own two-channel
  model (spec 0225 S11) and their own target luminance; this is the
  document row's.
- **N2.** Heat cues are outside it. They are a gradient in their own
  reserved column, and the whole point of a gradient is that its
  brightness varies.
- **N3.** No leveling in the ANSI-16 palettes. There is nothing between
  the sixteen named entries to blend toward, and that palette was chosen
  for legibility as a set rather than for its brightnesses matching.

## Specification

- **S1.** `RgbPalette` gains `doc_luma: f32`, the one brightness every
  document hue is brought to — 170 on `DARK_RGB`, 85 on `LIGHT_RGB`.
  The evidence for both values is next to the field.

- **S2.** `style_for_rgb` routes every arm through
  `doc_leveled(color, p.doc_luma)`. The two *anomaly* tiers —
  `AnnotationNonCanonical` and `AnnotationInvalid` — are the only
  exceptions, which is what makes G2 true: after this they are the only
  text on a document row brighter (dark) or deeper (light) than
  everything around them, and a reader needs to know nothing about the
  palette to see it.

  `AnnotationLandmark` — `pack_size` and `[packed=true]` — is leveled
  with the ordinary roles. Both say where a packed record begins, which
  is structure, and a landmark that shouted would spend the one signal
  S2 exists to reserve.

- **S3.** `doc_leveled` moves a color *both* ways: toward white to lift
  one that is under the target, toward black to deepen one that is over
  it. `leveled` (the wire row's) only ever moves toward the page, which
  is right for a row that must recede and wrong here — the palette's
  spread straddles the target in both themes, and a one-way rule would
  leave half of it unleveled.

  Both delegate to `blended(r, g, b, target, background)`. Luminance is
  affine in the blend factor, so the amount needed is a division rather
  than a search and the result lands on the target exactly.

- **S4.** For G3, both `Dark`'s `type` and `Dark`'s
  `tier_non_canonical` move, because leveling leaves only hue and
  saturation to tell them apart:

  - `type` → `#F2EA8C`, hue 55°, saturation 0.42 — paler and less red.
  - `tier_non_canonical` → `#FFB440`, hue 36°, saturation 0.75 — redder
    and brighter, putting the three tiers in severity order from 36° to
    `tier_invalid`'s 0°.

  36° is as red as a full-value hue can be while staying brighter than
  `doc_luma`: at 0.75 saturation it lands on luma 187 against the
  document's 170. Any redder and an anomaly would be *dimmer* than the
  ordinary text it has to stand out from.

- **S5.** `INVALID_STRING` and `INVALID_PACKED_RECORDS` join
  `annotation::INVALID` and `highlights.scm`'s `#any-of?` list. Both are
  emitted by `render_invalid` and named by `encode_text/fields.rs`, so
  both round-trip and neither was ever an invented token.

## Alternatives considered

### Dim every hue by a fixed fraction

Cheaper, and wrong for the same reason it was wrong on the wire row: a
fixed fraction preserves the palette's brightness *spread*, so the
second axis the eye has to track is still there. G1 is a statement about
the spread, not about the average.

### Move only one of the two yellows

Tried first, at 48°/0.55 for `type`, and reported as still too close.
Hue alone is not enough separation once both colors are at the same
luminance; saturation is the more legible of the two remaining axes and
both had to move.

### Leave the landmark unleveled with the anomaly tiers

It is the shipped behavior this spec changes. `[packed=true]` and
`pack_size` were as loud as an `INVALID_*` keyword, which made "loud"
mean "structural or wrong" — two things, which is one too many for a
signal that has to be read without thinking.

### Key severity on capitalization instead of a keyword list

`docs/prototext/annotation-format.md` does spell severity in case, and a
regex would have caught both missing keywords for free. Rejected under
spec 0225 S12 and still rejected: `pack_size` is lower case and
structural, `ENUM_UNKNOWN` is ALL CAPS and informational. Two
counterexamples in a vocabulary this small is a rule that does not hold.

## Test plan

1. `a_tier_looks_the_same_named_as_it_does_captured` — extended to run
   both palettes and to put the `Landmark`/RGB pair through
   `doc_leveled` before comparing, rather than dropping the case. A
   landmark that drifted to a different hue is still caught.
2. `every_keyword_is_colored_by_its_tier` — spec 0225 S12's drift test,
   which now covers the two added keywords by construction: they are in
   `annotation::vocabulary()`, so `highlights.scm` cannot fall behind
   again quietly.
3. The existing `colorize.rs` role tests — unchanged. Leveling moves a
   color, never which role a token gets.

## Measured outcome

`cargo test -p protolens` passes 619, plus 25 in `tests/batch_export.rs`.
No measurement of cost: `style_for_rgb` feeds `theme::styles`' `OnceLock`
table, so each leveled color is computed once per process.

Not achieved, and deliberately: the ANSI-16 palettes keep their spread
(N3), so on a 16-color terminal G1 and G2 do not hold. The tier colors
there are already the two warm slots, which is the most the palette can
say.
