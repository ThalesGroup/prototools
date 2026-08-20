<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0260 — a fold nobody has read says so

Status: implemented
Implemented in: 2026-08-08
App: protolens
Refs: docs/specs/0249-a-large-document-answers-the-user-first.md (S12,
        which put `Status::Unbaked` on the ladder and gave it the
        landmark violet), docs/specs/0255-the-document-finishes-itself-while-nobody-waits.md
        (the bake, and `auto_folded` — the set this spec reads),
        docs/specs/0247-a-fold-toggle-carries-the-worst-news-below-it.md
        (the fold toggle the violet already lands on),
        docs/specs/0193-the-fold-marker-lives-in-a-gutter.md (the
        `" ... }"` collapse summary this spec colors),
        docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md (where
        `#FFAEF8` was derived)

## Background

Two complaints, one color.

**The violet is nearly white.** `Status::Unbaked` borrows
`Tier::Landmark` (`theme.rs:517`), and on the dark palette that is
`#FFAEF8` — full value, but **saturation 0.318**. Every other color the
fold margin can wear is between 0.667 and 0.749:

| margin color | hex | saturation | luma |
|---|---|---|---|
| `Unbaked` / `Tier::Landmark` | `#FFAEF8` | **0.318** | 196.6 |
| `Status::Unknown` | `#4FC1FF` | 0.690 | 173.2 |
| `Tier::NonCanonical` | `#FFB440` | 0.749 | 187.6 |
| `Tier::Invalid` | `#FF5555` | 0.667 | 121.1 |

The margin wears these *unleveled* — prominence is the column's whole
job — so the landmark is the one entry in the set that arrives as a pale
tint rather than as a hue. Saturation, not luma, is the axis that
separates a color from white, and this is the only one under half the
others'. It shows in both places `Unbaked` is drawn: the bake's activity
dot and the fold toggle of a stop.

**A stop's `{ ... }` says nothing.** A node the bake has not reached yet
draws as one row — its header, with spec 0193's `" ... }"` summary
spliced in — styled exactly like a region the reader folded by hand.
The two are not the same fact. A hand-folded region is complete and
collapsed; a stop is a region *nobody has looked inside*, whose contents
are not merely hidden but unknown. Today the only thing that says so is
the toggle in the margin, one column away from where the reader is
looking.

## Goals

- **G1.** The `Unbaked` violet is as distinguishable from the default
  foreground as the other three status colors are from theirs.
- **G2.** A stop's collapse summary reads as provisional at the point in
  the row where the missing content would be.

## Non-goals

- **N1. A new palette entry for `Unbaked`.** It borrows
  `Tier::Landmark` deliberately (spec 0249 S12): a landmark and a stop
  say the same thing in the same register — provisional, not wrong — and
  a second copy of the color is how the two would drift. Deepening the
  shared constant keeps that and fixes both readings of it.
- **N2. The light palette.** Its `tier_landmark` is `#AF00DB`,
  saturation 1.0, against a white page. Nothing to fix.
- **N3. The sixteen-color fallback.** `Tier::Landmark` is
  `LightMagenta` there, which is as distinct from white as sixteen
  colors allow.
- **N4. Coloring every fold.** A region the user folded is complete; the
  cue must mean "unread", not "collapsed". See "Alternatives
  considered".
- **N5. A second signal — a distinct glyph, a count, a different
  ellipsis.** The toggle and the summary are two readings of one fact
  and one color is what ties them together. A shape as well as a color
  would have to be explained; a color that is already on the row above
  it does not.

## Specification

- **S1. `DARK_RGB.tier_landmark` becomes `#D24DFF`.** Hue 285°,
  saturation 0.70, value 1.0 — the saturation of its three neighbors in
  the margin, at a hue twenty degrees off the pink the current constant
  reads as. Its unleveled luma is 118, beside `Tier::Invalid`'s 121, so
  it is no dimmer in the margin than the color the palette already
  trusts there.

  This also moves the document's `#@` landmark annotation and the wire
  row's `pack_size` prefix, which take the same constant through
  `doc_leveled`. Leveled to `doc_luma`, `#FFAEF8` → `#DC96D6` becomes
  `#D24DFF` → `#E390FF`: saturation 0.318 → 0.435, at the same
  brightness as before. That is the intended consequence, not a side
  effect — the landmark is quiet on a document row for a reason (spec
  0116's leveling), but quiet is not the same as gray.

- **S2. An auto-folded node draws its `{ ... }` in the `Unbaked`
  color.** In `row_spans`, the collapse summary already goes through
  `spans_with_insertions` as a `(position, text, Option<Style>)` triple
  whose style is `None` today. When the node is in `auto_folded`, that
  becomes `Some(Style::default().fg(status_color(Status::Unbaked, …)))`,
  and the opening brace is folded into the insertion — `cut_segments`
  removes it from the segments and the inserted text becomes
  `"{ ... }"`, which is the mechanism `spans_with_insertions` already
  documents for bytes an insertion stands in for.

  The whole brace pair is colored rather than the ellipsis alone: a lone
  violet `... }` beside a gray `{` reads as two separate things, when
  the point is that the *region* is unread.

- **S3. The predicate is `auto_folded` membership, not `is_folded`.**
  A node can be in both sets — the user can fold a stop — and it is
  still unread, so membership in `auto_folded` is the test either way
  (spec 0255's rule that `auto_folded` is the truth and the bake queue
  only a hint).

- **S4. `row_content` is unchanged.** The summary's *text* is the same
  six bytes it was; only its style differs. The invariant that
  `row_content` and `row_spans` agree byte for byte therefore holds
  untouched, and so does everything that reads a row as text —
  selection, copy, search, export.

## Alternatives considered

### Color the `{ ... }` of every fold

Rejected under N4. The statement would become "there is content here you
are not seeing", which is true of any fold and which the `...` already
says. Worse, it would decouple the violet from a status: the same color
would mean "unread" in the margin and "collapsed" three characters
later.

### Color a hand-folded region that *contains* a stop

The tempting middle ground, and unnecessary: the fold toggle of such a
node already reads violet, because `Status::Unbaked` rolls up from the
descendants (spec 0247). The strict rule leaves no gap, only a
difference in which column carries the news.

### Give `Unbaked` its own palette entry

Rejected under N1.

### Raise the landmark's luma instead of its saturation

The obvious reading of "too close to white", and the wrong axis. At
saturation 0.318 the color is already at value 1.0; there is nowhere
brighter to go, and brighter is the direction *toward* white anyway.

## Test plan

1. `an_unbaked_fold_is_violet` — a bounded document's stop row carries a
   `{ ... }` span in `status_color(Status::Unbaked, …)`, and the same
   row after `expand_auto_fold` does not.
2. `a_hand_folded_region_is_not_violet` — S3/N4: fold a fully rendered
   node and assert its summary keeps the default styling. Fails if the
   predicate is `is_folded`.
3. `a_folded_stop_the_user_also_folded_stays_violet` — S3's both-sets
   case.
4. `row_content_and_row_spans_agree` (existing) — S4, over a bounded
   document.
5. `a_tier_looks_the_same_named_as_it_does_captured` (existing) and the
   theme suite — S1 changes a constant, not a mapping.
6. `every_status_color_is_a_hue_and_not_a_tint` — a saturation floor on
   the four margin colors, so the next person choosing one is told the
   constraint rather than left to rediscover it.

## Measured outcome

`#FFAEF8` → `#D24DFF`, saturation 0.318 → 0.700 at the same value. The
`#@` landmark and the wire row's `pack_size` prefix, which take the
constant through `doc_leveled`, go `#DC96D6` → `#E390FF` — saturation
0.318 → 0.435 at unchanged brightness, as S1 predicted.

The `{ ... }` cue landed as one styled insertion. The mechanism was
already there: `spans_with_insertions` has taken an `Option<Style>` per
insertion since spec 0193 and it had never been anything but `None`, and
`cut_segments`' own doc comment already described a second caller — "a
brace that is re-emitted as a styled insertion at the very same
position" — that did not exist until now.

Three mutations, each killed:

| mutation | killed by |
|---|---|
| `unread_fold_style` tests `is_folded` | `a_hand_folded_region_is_not_violet` |
| the opening brace keeps its grammar color | `an_unbaked_fold_is_violet`, `a_folded_stop_the_user_also_folded_stays_violet` |
| the old `#FFAEF8` | `every_status_color_is_a_hue_and_not_a_tint` |

Nothing else moved: 812 protolens tests and the full workspace green,
`row_content` untouched, and the existing `row_content`/`row_spans`
byte-agreement test needed no change.

## Amended 2026-08-20 — the violet is a neutral gray

S1's whole argument was about finding `Unbaked` a *hue* with enough room
on the wheel, and the answer was that there was no good room: blue,
amber and red take 50° each, green reads as "done", and violet was what
was left. The reframe is that `Unbaked` was never on that scale to
begin with. It is not a judgement about the bytes — it says the render
has not caught up, and it goes away by itself — so it takes no hue at
all: `#8C8C8C` on both pages, luma 140, quieter than the default
foreground an `Ok` node's mark wears and louder than the page. A hue
that has run out rather than a hue of its own.

Everything else in this spec stands, S2's whole-brace-pair insertion
included; only the constant changed. The consequences for the record:

- The three tests above are `an_unbaked_fold_is_gray`,
  `a_hand_folded_region_is_not_gray` and
  `a_folded_stop_the_user_also_folded_stays_gray`.
- Test plan 6 is `every_severity_color_is_a_hue_and_not_a_tint` and no
  longer governs `Unbaked`. What holds `Unbaked` instead is
  `unbaked_is_a_neutral_and_not_a_hue`, which asserts the opposite —
  `r == g == b` — plus a luma window it must sit in on *both* pages.
- The ANSI-16 fallback gained a constraint the truecolor entry does not
  have, and it was found by a failing test rather than by eye: the cue
  is drawn on a folded node's braces, so a fallback equal to
  `PunctuationBracket`'s color does not weaken it, it deletes it. Pinned
  by `the_unbaked_fallback_is_not_the_brace_color`. The two pages take
  different named colors — `DarkGray` on dark, `Gray` on light — because
  "quieter than the default foreground" points opposite ways on them.
