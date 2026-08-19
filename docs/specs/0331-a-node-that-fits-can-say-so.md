<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0331 — a node that fits can say so

Status: implemented
Implemented in: 2026-08-19
App: protolens
Refs: docs/specs/0138-protolens-inference-heat-cue.md (the cue, its
        glyph, its two hues and the `i` this replaces),
      docs/specs/0154-protolens-progressive-heat-cue.md (`HeatDisplay`,
        the four shapes a row can draw, and where `None` comes from),
      docs/specs/0284-the-heat-cue-is-a-control.md (double-click on a
        cue opens the pane — which the new cue inherits for free),
      docs/specs/0287-the-chrome-beside-a-row-says-what-it-is.md
        (`SuffixShape`, and the rule that every drawn mark has a box)

## Background

The heat cue answers "could these bytes be something else?", and it
answers it **only when the answer is yes**. A node whose current type is
the unique best fit draws nothing at all — same as a node nobody has
scored yet, same as a node whose every candidate was vetoed, same as a
node that is not scoreable at all. Four different states, one blank.

For a reader working down a document that is mostly right, the blank is
the common case and it is the ambiguous one. "No cue here" cannot be
read as "this one checks out" while it also means "not looked at yet".
The number exists — `heat_display`'s unique-optimum arm has `current`
and `best` in hand and equal — and it is thrown away.

`i` today is a two-state toggle, cues and no cues, and two states is
exactly one short of being able to say this.

## Goals

- **G1.** `i` rotates forward through three states — no cues, findings
  only, every scored node — and `I` rotates backward. A session starts
  in the first.
- **G2.** In the third state every scoreable node says something: a
  node whose current type is the unique best fit draws ` [{score}]`, and
  a node no candidate at all fits draws ` [vetoed]`. Both are
  double-click targets like every other cue.

## Non-goals

- **N1.** *No new scoring, and no new requests.* `heat_cues_hidden` is
  read *after* `heat_cue_resolve` has run (spec 0138's "keep the caches
  warm while hidden"), so the number the third state draws is one the
  first two already computed and discarded. The mode changes what is
  formatted, never what is asked for. This is also why the mode can flip
  on a keystroke with no repaint latency.
- **N2.** *A vetoed range is not a silent one.* `HeatDisplay::None` has
  two causes: the unique optimum, and "every candidate for this range
  was vetoed" — which includes the current type, so there is no score to
  print. It is still a settled answer and the third state says it in
  words, ` [vetoed]`, rather than leaving the reader to tell it apart
  from a row that has not been reached.
- **N3.** *A node that is not scoreable at all is not a node without a
  cue.* `heat_cue_at` refuses a non-header line and a `!can_override`
  node before any resolution happens; those rows are not blank because
  the answer was "fine", they are blank because there is no question.
  They stay blank in all three states.
- **N4.** *The two pending shapes are untouched.* ` [?]` and ` [?/best]`
  already draw in states two and three alike; they say "not known yet",
  which is true in every state that shows anything.
- **N5.** *The mode is session state.* Not persisted, not a flag, not
  remembered per document. It is a way of looking, and the reader
  re-chooses it as cheaply as pressing one key.
- **N6.** *The glyph stays out of it.* The margin `■` is graded by
  `heat_level` and reserved for a finding (spec 0138 G9). An agreeing
  node draws the suffix and leaves column 0 blank — the third state adds
  a number, not a second column of marks.
- **N7.** *The demo is not re-pinned here.* Several `grpconf` beats show
  heat cues and reach them by opening a document; with `Off` as the
  opening state each needs an explicit `i` first. The beat scripts carry
  a `STALE` header and are re-pinned as a unit against the synopsis, not
  one spec at a time.

## Specification

- **S1.** `App::heat_cues_hidden: bool` becomes
  `App::heat_cues: HeatCueMode`, a three-variant enum in `heat_cue.rs`:

  | variant | draws |
  |---|---|
  | `Off` | nothing |
  | `Findings` | today's cues: mismatch, tie, and the two pendings |
  | `All` | those, plus ` [{score}]` and ` [vetoed]` |

  `Off` is `Default` and what `App::new` sets. **This changes the
  opening view**: today a session starts with findings shown and `i`
  hides them. With three states the first press has to mean something,
  and the reader who wants the cues asks for them — which is also the
  reading that makes the rotation's direction obvious, each press
  showing more than the last until it wraps.

- **S2.** `i` is `next()`, `I` is `prev()`, over the cycle
  `Off → Findings → All → Off`. Both are `KeyCode::Char` arms in the
  main-pane tier and must carry `modifiers.is_empty()`'s equivalent
  gate — `Ctrl-i` is the jumplist's "forward" and is already claimed in
  the Ctrl/Alt tier above.

  Forward is the direction that shows more, up to the wrap. A reader who
  overshoots presses the other one; that is what `I` is for, and it is
  why this is a rotation rather than two independent toggles.

- **S3.** `heat_display` gains a fifth `HeatDisplay` variant for the two
  arms that today fall to `None`:

  ```rust
  /// Settled, with no finding to report — spec 0331, drawn only in
  /// `All`. `Some(score)`: the current type is the unique best fit,
  /// and `score` is the number both halves agree on. `None`: every
  /// candidate for this range was vetoed, current included, so there
  /// is no number and the cue says so in words.
  Settled { score: Option<i64> },
  ```

  One variant and not two, because the suppression rule is about the
  class — *settled, nothing wrong* — and a class with one member per
  arm would state that rule twice. `HeatDisplay::None` then means only
  what is genuinely nothing: suppressed by the mode, a non-header line,
  a node that cannot be overridden.

  The suppression stays where spec 0138 put it, at the end of
  `heat_cue_at`, and becomes a match on the mode rather than an `if`:
  `Off` suppresses everything, `Findings` suppresses `Settled` alone,
  and `All` suppresses nothing.

  In `heat_cue.rs` and not in `render.rs` because `HeatDisplay` is what
  the hover box, the caret track and the hit test all read; a shape
  invented at draw time would be invisible to the three of them.

- **S4.** `heat_chrome` draws both with a blank glyph (N6) and a suffix:

  - `Settled { score: Some(n) }` → ` [{n}]` in
    `theme::heat_agree_style`, a green chosen per theme with an ANSI-16
    fallback, in the same shape as its two neighbors
    `heat_suffix_style` and `accent_style`. Green because the existing
    two hues are graded scales that mean *degree of finding* and this
    is the absence of one. Its own constants rather than a
    `RgbPalette` member: the only green the document palette has is
    `comment`, which the two pending cues already wear, and the heat
    column has been its own small palette since 0138.
  - `Settled { score: None }` → ` [vetoed]` in `heat_suffix_style`, the
    mismatch red. Not the green, which would claim an agreement that
    does not exist, and not the pendings' dim comment, which would file
    a verdict under "still working". Nothing in the schema fits these
    bytes, and red is what this pane already uses for that.

- **S5.** **The double-click needs no code.** `heat_cue_at_point`
  measures whatever `heat_chrome` returns as a suffix (spec 0284 S1),
  and `caret_bounds` measures the same thing (spec 0194 S1). A new
  `HeatDisplay` shape that formats a suffix is a control and a caret
  zone by construction, which is the whole reason S3 puts the variant in
  `HeatDisplay` rather than the string in the renderer.

- **S6.** `SuffixShape` gains two variants, `Agree` and `Vetoed`, for
  the hover boxes — spec 0287's rule is that every drawn mark says what
  it is, and `SuffixShape::of` returning `None` on a drawn suffix would
  break it. Two here where S3 has one, because a box is prose and the
  two say opposite things: one that this node's type is the best fit for
  these bytes, the other that no known type fits them. Both end with the
  double-click, and neither carries a number the row does not already
  show (0287 N5).

- **S7.** The two places that name the state in words follow it:
  `pane_menu_items`' row, which by 0284's convention names the state it
  moves *to* and now has three to name, and `HELP_TEXT`'s "Heat cues"
  section.

## Alternatives considered

**Keeping `i` a toggle and putting the third state on its own key.**
Two independent controls over one display, with a reachable state where
both say something different. The three are one setting and one setting
is one control.

**Showing ` [{score}/{score}]` for symmetry with the mismatch shape.**
The doubled number is what the reader has to *notice* is doubled in
order to read it as agreement, and it is two more columns on the most
common row in the document. One number says it.

**Drawing the margin glyph for an agreeing node, in green.** See N6. The
glyph's twelve brightness levels are a scale of concern; a green one
would be a thirteenth reading of a column that is currently answerable
at a glance, and on a mostly-correct document it would ink most of the
column.

**Making the third state the default.** It is the more informative view
and it is the wrong default: the cue's value is that it is rare enough
to be worth looking at. A column of numbers down every row is a column
the eye stops reading.

**Keeping `Findings` as the default, as today.** It is the smaller
change and it makes the first `i` press mean "show me less", which is
the wrong first step through a rotation whose other two states both show
more. Starting at `Off` costs the reader who wants cues one keystroke
and buys a control whose direction is legible without the manual.

**Leaving a vetoed range blank in `All` too.** Drafted that way and
corrected on review. Blank is what the third state exists to abolish: it
is exactly the ambiguity between "checked, nothing to say" and "not
reached yet", and a range where nothing fits is the case a reader most
wants told rather than inferred.

## Test plan

1. `i_rotates_forward_and_shift_i_rotates_back` — from `Off`, three `i`
   presses return to `Off` having visited each state once; three `I`
   presses do the same in the other order. Read off `app.heat_cues`,
   driven through `handle_key`, and starting where `App::new` leaves
   it, so the default is pinned by the same test.
2. `an_agreeing_node_says_its_score_in_the_third_state` — a node seeded
   at the unique optimum draws no suffix in `Findings`, draws
   ` [{score}]` in `All`, and the score is the seeded one. Asserted on
   the drawn frame, so the green is asserted where it lands.
3. `a_vetoed_range_says_so_in_the_third_state` — the companion, with
   `best_score: None` seeded: blank in `Findings`, ` [vetoed]` in `All`,
   in the mismatch red and not the green. Both halves of `Settled` in
   one place, since the variant's point is that they are one class.
4. `a_node_with_no_question_stays_blank` — N3: a non-header line and a
   `!can_override` node draw nothing in `All` either.
5. `the_third_state_asks_for_nothing_new` — N1: the requests pushed by
   a frame in `All` are the ones pushed by a frame in `Findings`. This
   is what says the mode is a formatting decision.
6. `an_agreeing_cue_is_a_double_click_target` — spec 0284's gesture on
   the new suffix opens the override pane, and `heat_cue_at_point`
   reports the columns the frame drew it at.
7. `the_new_cues_have_boxes` — spec 0287 S6: hovering either yields a
   `HeatSuffix` hit carrying its own shape, not nothing and not each
   other's.
8. The existing `i_toggles_heat_cues_hidden` is rewritten, not deleted:
   its claim — that the cue disappears and comes back without the
   caches being discarded — is still true, and is now a rotation rather
   than a toggle.

## Measured outcome

Implemented as specified. All eight test-plan items are in the suite;
the protolens suite stands at 1150 passing, and the four gates
(`cargo fmt --all --check`, `cargo clippy --no-default-features
--workspace -- -D warnings`, `cargo test --release --no-default-features
--workspace`, `reuse lint`) are clean.

Three things the implementation found:

- **S4's green is not `style_for`'s.** The draft claimed the color was
  the one a literal value already wears in both themes. It is not:
  `RgbPalette::value` is `0xCE9178` (copper) on dark and `0xA31515`
  (dark red) on light, and the palette's only green is `comment` —
  already spoken for by the two pending cues. `heat_agree_style` was
  given its own constants instead, in the shape of `heat_suffix_style`,
  which bypasses `RgbPalette` for the same reason. S4 above is
  corrected; the rest of its argument stands. Luma check against the
  neighbouring hues: 173 vs `heat_rgb::DARK[11]`'s 178 on dark, 43 vs
  `heat_rgb::LIGHT[11]`'s 43 on light.

- **`heat_display` now never returns `None`.** Both of its former
  `None` arms were exactly the two `Settled` cases, so the change is a
  rename of a verdict that was already being computed — which is the
  concrete form of N1's claim that the third state asks for nothing
  new. `None` survives only as the mode's suppression and as
  `heat_cue_at`'s two pre-resolution refusals. Replacing the final
  `_ =>` catch-all with `Some(current)` also made that match
  exhaustive.

- **No fixture holds a `!can_override` node.** Spec 0135 G3 widened the
  gate to every wire type a value can carry, so test 4 makes the shape
  by hand (`is_message = false`, `wire_type = WT_START_GROUP`) rather
  than searching for one. Worth knowing before writing the next test
  that wants a refused node.

Twelve existing tests needed a line: the default is `Off`, and any
fixture that expects to see a cue must now say `heat_cues = Findings`.
Three of those are the shared `cue_app` helpers in `tests/mouse.rs`,
`tests/popup.rs` and `tests/popup_doc.rs`.
