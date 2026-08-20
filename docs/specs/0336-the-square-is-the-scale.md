<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0336 — the square is the scale, the word is the class

Status: implemented
Implemented in: 2026-08-20
App: protolens
Refs: docs/specs/0138-protolens-main-pane-inference-heat-cue.md (the
        two hues, the twelve levels, and the four hand-written ramps
        this replaces),
      docs/specs/0331-a-node-that-fits-can-say-so.md (`heat_agree_style`
        and the ` [{score}]` suffix, both re-homed here),
      docs/specs/0335-a-question-not-asked-is-not-an-answer.md (the
        amber sentinel square, which is the third hue and the one
        exception to the grading),
      docs/specs/0327-a-color-needs-ink-to-be-read.md (why the glyph is
        a filled block, which is what makes a lightness ramp legible in
        it)

## Background

The heat column carries two hues over twelve hand-tabulated brightness
stops, and it assigns them the wrong way round.

Amber, the warm ramp, is the `Mismatch` cue: *a better type exists for
these bytes*. That is the pane's one call to action, and warm reads as a
fault in the data. Blue is the `Tie` cue: optimal but not uniquely so —
genuinely a cold, low-urgency state, and correctly colored. Meanwhile
spec 0331 gave the settled-and-agreeing suffix a green of its own,
`#4CEE5E`, on the argument that green is the absence of a finding. So
green means "fine", amber means "act", and the reader has to learn that
the alarm color is the one pointing at an improvement.

The grading is also spread across the wrong channel. Three of the six
suffixes carry a number the reader can read directly, and two of those
are additionally styled from a graded palette — the mismatch suffix
takes the ramp's top stop, the tie suffix takes `accent_style` — so a
label that must stay legible against document text is drawn from a scale
built to run down to near-background.

Underneath, the ramps are four hand-written twelve-entry `[Color; 12]`
tables. They cannot express a continuous scale, and the two blue ones
were derived by swapping the R and B channels of the warm ones under a
doc comment claiming this "carries the exact same luminance
progression". It does not: relative luminance weights red at 0.2126 and
blue at 0.0722, so `#FFAC06` computes to about 178 and its swap
`#06ACFF` to about 143. The blue ramp has run some 20% dimmer than the
warm one at every stop, and the comment asserting otherwise is why
nobody measured.

## Goals

- **G1.** Two channels with two jobs: the **square is graded** and says
  how much, the **word is flat** and says which of three kinds. A word
  is never drawn from a ramp, so it is always legible.
- **G2.** Green is the call to action, blue is settled, amber is the
  unmatched sentinel. Three hues, three meanings, none doing double
  duty.
- **G3.** A square's color is computed from a fraction in `[0, 1]`, not
  looked up in a table, so the scale is continuous and so a later spec
  can change where the fraction comes from without touching the color.

## Non-goals

- **N1.** *Where the fraction comes from.* This spec takes `t` from the
  existing Fibonacci bucketing, `(level - 1) / 11`, and changes nothing
  about `heat_level`. Replacing that with a calibrated logarithm is spec
  0337; splitting them keeps a color change and a scale change from
  landing in one commit, where a regression in either would be
  attributed to the other.

- **N2.** *Perceptual uniformity across hues.* A green at a given HSL
  lightness reads much brighter than a blue at the same lightness — the
  intrinsic luminances differ by around a factor of ten at full
  saturation. This is not corrected. It is wanted: green is the call to
  action and should carry the larger dynamic, blue is the weaker signal
  and a compressed range suits it. Do not re-propose a per-hue lightness
  curve or a shared luma target to "fix" it; the asymmetry is the
  design, and the reason it is safe is G1 — no word depends on the ramp
  for legibility.

- **N3.** *A hue for negative scores.* All three hues are now spoken
  for, and a negative best score is a mismatch whose best candidate is
  itself poor — closer in meaning to unmatched than to act, but not the
  same. Deciding this without knowing whether the negative tail is
  populated at all would be guessing; the measurement belongs to spec
  0337 and so does the decision. Until then a negative score maps to
  `t = 0`, as it does today.

- **N4.** *The two pending suffixes.* ` [?]` and ` [?/best]` keep the
  comment style. They say "still working", which is neither of the three
  classes, and coloring them into the vocabulary would make a document
  flash a verdict during a sweep and then settle out of it.

- **N5.** *A fourth column, a different glyph, or a wider gutter.* The
  `■` in the reserved column, exactly as spec 0327 left it.

## Specification

- **S1.** `HeatHue` gains `Green` and `Amber` and keeps `Blue`. `Red` is
  removed rather than renamed: nothing in the new vocabulary is red, and
  a variant that survived a rename would keep its call sites pointing at
  the wrong meaning.

- **S2.** The four `[Color; 12]` tables in `heat_rgb` are deleted and
  replaced by one function, `heat_color(t: f32, hue: HeatHue, theme) ->
  Color`, computing HSL and converting to RGB. Fixed hue, fixed
  saturation, lightness linear in `t`:

  | | hue | dark `L(0) → L(1)` | light `L(0) → L(1)` |
  |---|---|---|---|
  | green | 127° | 0.18 → 0.62 | 0.90 → 0.32 |
  | blue | 210° | 0.18 → 0.62 | 0.90 → 0.32 |
  | amber | 40° | 0.18 → 0.62 | 0.90 → 0.32 |

  with `S = 0.83` throughout. The light theme's range runs downward
  because a light theme's ramp goes pale-to-deep, which is the same
  statement — further from the background means more.

  **Where those numbers come from.** Hue 127° and `S = 0.83` are the
  hue and saturation of `#4CEE5E`, spec 0331's green, decomposed; its
  lightness, 0.62, sets `L(1)`. The ramp's top stop is therefore exactly
  the green already in the tree, and the other two hues inherit its
  saturation and lightness rather than being picked separately. The top
  stops that falls out — `#4CEE5E`, `#4D9EEE`, `#EEB94D` — are mutually
  distinguishable at a glance, which is the property the ceiling below
  full lightness exists to preserve. The evidence lives in a doc comment
  beside the constants, not here.

  `L(1)` is deliberately not near 1.0: at full lightness every hue is
  white and the green/blue/amber distinction — which is the *class*, not
  the magnitude — would be lost exactly on the loudest squares.

- **S3.** `heat_style` takes `t: f32` in place of `level: u8`. Its
  ANSI-16 fallback keeps today's three-band shape, thresholded on `t`
  rather than on the level: below 0.25 no square is drawn at all, below
  0.60 the base color, otherwise the light one. `Green`/`LightGreen` and
  `Blue`/`LightBlue` as expected; `Amber` falls back to
  `Red`/`LightRed`, because ANSI yellow reads as a warning of its own
  and there is no amber slot.

  The floor is explicit and load-bearing: a square drawn at a lightness
  indistinguishable from the background is worse than no square, because
  the reader learns the column is quiet when it is saturated. On
  truecolor the floor is `L(0)`, which is above the background but only
  just.

- **S4.** `heat_chrome`'s table becomes:

  | display | square | word | word style |
  |---|---|---|---|
  | `Cue(Mismatch)` | green, graded | ` [c/b]` | flat green |
  | `Cue(Tie)` | blue, graded | ` [n@s]` | flat blue |
  | `Settled { Some(n) }` | none | ` [n]` | flat blue |
  | `Settled { None }` | amber, flat | ` [unmatched]` | flat amber |
  | `PendingCurrent` | none | ` [?/b]` | comment (N4) |
  | `Unknown` | none | ` [?]` | comment (N4) |

  `Settled { Some(n) }` is blue because it *is* a tie of one: the
  current type is the top scorer and nothing shares the top. The tie
  suffix and the agree suffix say the same thing about the same node
  with different cardinality, and giving them different hues asserted a
  difference in kind that is not there. It draws no square, as spec 0331
  N6 has it — the square is for a finding, and being right is not one.

- **S5.** A "flat" word style is the hue's ramp at `t = 1`. One
  function, `heat_label_style(hue, theme)`, replacing
  `heat_suffix_style`, `heat_agree_style`, and the tie suffix's borrowed
  `accent_style`. Three call sites collapse to one, and the word and the
  brightest square of its own class match by construction rather than by
  a second constant.

  `heat_agree_style` is deleted outright, not left as an alias. Its
  green is not lost — it is now `L(1)` of the whole scheme (S2).

- **S6.** The `DARK_BLUE`/`LIGHT_BLUE` doc comment's luminance claim
  goes with the tables it describes. It is recorded here only so that
  the next person to reach for an R/B swap knows it was tried and was
  wrong by 20%.

- **S7.** The hover boxes follow the colors. `DocElement::HeatGlyph`'s
  "brighter means a bigger difference" / "brighter means a higher score"
  stay true; spec 0335's third kind says its color is flat and not on
  the scale.

## Alternatives considered

**Keeping the tabulated ramps and adding a green one.** A fifth and
sixth hand-written twelve-entry table, still incapable of a continuous
scale, and still leaving the next spec to replace all six. The tables
are also where the luminance error hid: a generator's constants are six
numbers a reader can check, where a table is 144 bytes nobody audits.

**OkLCh instead of HSL.** The correct space — same three knobs, but a
perceptually uniform lightness, so one curve would mean one perceived
brightness across hues. Rejected for two reasons. It is solving N2,
which we do not want solved. And sRGB's gamut narrows near the top, so
chroma cannot be held constant across the range and the implementation
needs gamut clipping; HSL always lands in gamut by construction, which
for a six-constant color scheme is worth more than the uniformity.

**Green for both the mismatch square and the agreeing suffix**, on the
reading that green means "these bytes have a good type" and the square's
presence is the call to action. Drafted and dropped: it makes the
loudest color in the pane the one on the most common row, and it splits
"is there something to do here" across two channels the reader has to
combine. Blue for the agreeing suffix says it in one.

**Grading the word by score as well as the square.** Two channels
carrying one number, and the word is the channel that cannot afford it:
its job is to be read, and the bottom of any ramp is by definition close
to unreadable. The number is already *in* the word.

## Test plan

1. `the_three_hues_are_distinguishable` — `heat_color(1.0, ..)` for the
   three hues yields the three documented top stops, in both themes.
   Pins S2's constants where a change to them is visible.
2. `a_ramp_never_reaches_white` — across `t` in `[0, 1]` and all three
   hues, no output has all three channels equal; the hue survives to the
   top.
3. `a_mismatch_is_green_and_a_tie_is_blue` — on the drawn frame, so the
   mapping is asserted where it lands and not at the palette.
4. `an_agreeing_node_wears_the_tie_blue` — S4: the ` [n]` suffix and a
   ` [n@s]` suffix on the same fixture draw in the same style, and it is
   not the retired green.
5. `a_word_is_never_dim` — every one of the six suffixes, at every `t`,
   draws in a style whose color is a ramp top or the comment color.
   This is G1 stated as a test, and it is what makes N2 safe.
6. `the_ansi_floor_draws_no_square` — S3: below the threshold the glyph
   cell is blank, not a glyph in a color the terminal cannot show.
   Exercised with the color depth passed in, since the nix sandbox has
   no `COLORTERM`.
7. `heat_level_is_untouched` — N1: the Fibonacci bucketing and its
   boundaries are unchanged by this spec, so spec 0337 starts from a
   known place.

## Measured outcome

Four hand-written `[Color; 12]` tables (48 `Color` entries) replaced by one
`hsl_to_rgb` converter and one `heat_color(t, hue, theme)` function. Three
call sites that each imported a different style function (`heat_suffix_style`,
`heat_agree_style`, and a borrowed `accent_style`) collapsed to one
`heat_label_style(hue, theme)`. The test suite — 1 173 protolens tests, 25
theme tests — passes unchanged. The top stops produced by the formula are
`#4EEF60` (green), `#4E9EEF` (blue), `#EFB94E` (amber); these differ from the
spec's anchors `#4CEE5E` / `#4D9EEE` / `#EEB94D` by two units per channel —
within one rounding step, not a visible difference.
