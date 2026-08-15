<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0287 — the chrome beside a row says what it is

Status: implemented
Implemented in: 2026-08-13
App: protolens
Refs: docs/specs/0285-a-document-token-says-what-it-is.md (the box, the
        dwell, and `DocElement`; this adds the chrome S4 left out),
      docs/specs/0138-protolens-heat-cue.md (N1: column 0 is a reserved
        gutter; G9: the trailing suffix),
      docs/specs/0154-protolens-heat-cue-progressive-display.md (G6:
        `HeatDisplay`'s four shapes, which are what the box reads),
      docs/specs/0193-the-fold-marker-lives-in-a-gutter.md (S1: the
        margin, `marker_column`, `FOLD_FIELD_WIDTH`),
      docs/specs/0247-a-node-says-what-is-wrong-inside-it.md (S10: the
        marker's color is the subtree's worst status),
      docs/specs/0260-an-unbaked-region-is-not-a-folded-one.md (S2: the
        violet `{ ... }`),
      docs/specs/0280-a-heat-cue-says-what-its-score-is-made-of.md (the
        score box, which the suffix's explanation points at),
      docs/specs/0284-the-heat-cue-is-a-control.md (S1/S2:
        `heat_cue_at_point`, and the double-click the box names)

## Background

Spec 0285 gave every *token* of a document row an explanation on hover.
It stopped at the row's text. Everything drawn around that text is still
unexplained, and it is the part a new reader meets first, because it is
the part that is not protobuf:

- the `●` in column 0, and its color;
- the ` [3/47]`, ` [2@85]`, ` [?/47]` or ` [?]` at the end of the row;
- the `⏷` / `⏵` in the fold margin;
- the `{ ... }` a folded node collapses to.

Four glyphs and two bracketed forms, none of which appears in any
protobuf document the reader has seen elsewhere. A reader who does not
already know what they mean has nowhere to find out short of `?`, and
`?` describes keys, not marks on the screen.

## Goals

- **G1.** Hovering any of the four says what it is, in the same box, on
  the same dwell, as spec 0285's tokens.
- **G2.** The box describes **what is drawn**, not what could be: the
  glyph's own color and the suffix's own shape decide the words, so the
  box and the mark can never disagree.
- **G3.** Where the mark is also a control (spec 0284's cue, spec 0193's
  marker), the box says so. That is the one thing a reader cannot
  discover by looking.

## Non-goals

- **N1.** The wire row's chrome. Spec 0282 already explains every part
  of a wire row, and the `\x ` prefix is its own target there.
- **N2.** The side panes and the command row's activity dot. The dot is
  a different mark with a different meaning (spec 0190) and its own
  place to be explained; nothing here forbids a later spec doing it, and
  the machinery below is not the one it would use — the panes have no
  hover at all yet.
- **N3.** A box on the *reserved blank* in column 0. Nothing is drawn
  there, so there is nothing to point at; a box that appears over blank
  space is a box that appears by accident.
- **N4.** Changing any of the four marks. This spec adds words, not
  glyphs.
- **N5. A verbosity level, which is this spec's successor.** Hover
  boxes fall into two classes, and every one of the four here is in
  the second:
  - **information** — a value only the box has: what an `#@`
    annotation means, what a run of bytes decodes to, what a score is
    made of, what the suffix's numbers are;
  - **orientation** — what a mark *is*: the `●`, the `⏷`, the
    `{ ... }`, and (were they targets) the statusline's `[start..end)`
    and `(preview)`.

  A reader learns the second class once and then never needs it again,
  which is exactly the population a verbosity level serves — plausibly
  three states (nothing / marks explained / everything) rather than a
  flag, defaulting to the middle, and remembered nowhere, being a
  display attribute like spec 0268 N3's run.

  It is named here rather than left implicit because the successor's
  cost lands on *this* spec's shape: the class is a property of the
  box, so S4's arms must be written knowing which one they are, or
  every one of them is revisited to add the tier. Nothing here
  implements the level; S4 is written so that adding it is one field
  and one filter.

## Specification

- **S1.** Four new members of `DocElement`, all chrome rather than
  grammar: `HeatGlyph`, `HeatSuffix`, `FoldMarker`, `FoldSummary`. They
  join spec 0285's nine so that there is one hit type, one dwell, one
  "still on the same thing" comparison and one box builder. A parallel
  `ChromeHit` would duplicate `DocHit`'s `line`/`at` identity — which is
  the whole of what makes two hovers the same hover — for no gain.

- **S2.** They are produced in `doc_element_at_point`, not in
  `doc_elements`. `doc_elements` lexes the row's grammar and must keep
  doing only that: it is also 0280's type-span query, and chrome is not
  in the string it parses in the same sense — the fold margin is
  prepended by `row_content` and the `{ ... }` is spliced by
  `row_text_of`. `doc_element_at_point` has the node, so it can ask
  whether the node is folded rather than pattern-matching `"{ ... }"`
  out of text that might legitimately contain it inside a string.

- **S3.** Where each one is hit-tested, and why none of them collides
  with a token:

  - **`HeatGlyph`** — pane column 0 exactly. Spec 0285's mapping does
    `checked_sub(1)` on that column and gives up, so this arm sits in
    front of it and takes a column no token can reach. A target only
    when `heat_chrome` returned the glyph rather than the reserved
    blank (N3).
  - **`HeatSuffix`** — `heat_cue_at_point`, unchanged, which already
    measures the drawn suffix and adds no `pan_offset` because the
    suffix does not pan. Past the row's last character, where
    `doc_element_at_point`'s `nth(index)` already yields `None`.
  - **`FoldMarker`** — the two-column fold field, located by
    `marker_column` + `FOLD_FIELD_WIDTH`, which is the same locator
    `handle_click` uses. Sharing it is the point: the hover target and
    the click target are the same rectangle by construction, so a
    reader cannot be told "click this" over a column that would not
    have toggled. Inside `row_content`, so `pan_offset` is added back —
    again as `handle_click` does.
  - **`FoldSummary`** — the `{ ... }` of a folded node, which
    `row_text_of` has already spliced into the content the mapping
    indexes. The span runs from the `{` to the `}`, so pointing at the
    brace and pointing at the ellipsis give one answer.

  A row's tokens are unaffected: `push_body` already skips the fold
  field, and a folded node's `{ ... }` lies past its key.

- **S4.** What each box says. The token line leads, as it does for every
  0285 box, so the reader sees the mark they pointed at.

  All four are *orientation* boxes in N5's sense — they say what a mark
  is, not what a value is — so the classification N5's successor needs
  is a constant over this spec rather than a decision per arm. No box
  here carries a number the reader does not already have on screen:
  where one is wanted, the box points at where it is — the `[…]` at
  the end of the row, spec 0280's score box — rather than fetching it.
  That is what keeps the class uniform, and it is a rule about these
  boxes, not a limitation of them.

  `HeatGlyph`, worded from the drawn `HeatCueKind`:

  | kind | lines |
  |---|---|
  | `Mismatch` | another type scores higher on these bytes / brighter means a bigger difference |
  | `Tie` | another type scores exactly as well as this one / brighter means a higher score |

  Both end with a pointer at the numbers: *the `[…]` at the end of the
  row has them*.

  `HeatSuffix`, worded from the drawn `HeatDisplay`:

  | display | lines |
  |---|---|
  | `Mismatch { current: Some, best }` | left: what this node's type scores here / right: the best score any candidate reached |
  | `Mismatch { current: None, .. }` | the `-` is *this node's type does not fit these bytes at all* |
  | `Tie { tie_count, score }` | `n` types score `s` here — the best, but not the only best |
  | `PendingCurrent` | the best is known; this node's own score is still being computed |
  | `Unknown` | still scoring these bytes |

  Every one of them also carries G3's line: *double-click to choose a
  type for this node* (spec 0284 S2).

  `FoldMarker`: which way it is, and that it is a control — *this node
  is folded / unfolded*, *click to unfold it / fold it*. When
  `fold_marker_color` gave the glyph a status color, one more line: *the
  color is the worst thing found anywhere inside*.

  `FoldSummary`: *this node is folded: its fields are not shown*, and
  the same *click the marker in the left margin* — unless spec 0260's
  unbaked style is on it, in which case it is not a fold the reader
  made and the box says so instead: *nobody has looked inside this
  region yet*.

- **S5.** The dwell is `EXPLAIN_DWELL`, like every other `Doc` target:
  these are explanations for a reader who does not know, not readouts
  for one who does, and 0285 S7's argument for the longer wait applies
  unchanged.

- **S6.** Order in `handle_hover`: the wire row still wins on part > 0,
  then the type name (0280), then this chrome, then 0285's tokens. The
  chrome tests are cheap and disjoint from the tokens; putting them
  before the token lex means the common case — pointing at text — pays
  two integer range checks.

## Alternatives considered

### A `HoverTarget::Chrome` variant of its own

Rejected: `HoverTarget`'s job is to keep "same thing, do not restart the
dwell" to one comparison. A fifth variant with its own hit type,
identity and box builder is three duplications to buy a taxonomy nobody
reads. `DocElement` is already a list of *things drawn on a document
row*; the chrome is drawn on a document row.

### Detecting the fold summary by searching the row for `"{ ... }"`

Rejected: a string value may contain those characters, and
`doc_elements` cannot tell a value from a splice. The node's own
`is_folded` is authoritative and already in hand at the hit test.

### Explaining the reserved blank in column 0

Rejected under N3. The blank exists so that indentation does not shift
(spec 0138 N1); explaining it would mean a box over every unremarkable
row in the pane, which is exactly what a dwell is meant to prevent.

### Re-deriving the cue for the box

Rejected under G2. `heat_cue_at` / `heat_chrome` are what drew the mark;
asking them again is the same call, and asking anything else is how a
box comes to describe a cue the reader is not looking at.

## Test plan

1. `the_heat_glyph_explains_its_color` — a `Mismatch` and a `Tie` row,
   asserting the two different second lines from column 0.
2. `the_heat_suffix_explains_its_numbers` — all five `HeatDisplay`
   shapes, each hit through `heat_cue_at_point`'s own geometry.
3. `the_heat_suffix_names_the_double_click` — G3, on all five shapes
   the suffix takes, and its absence from the glyph's box: spec 0284
   S2's double-click is measured over the *suffix*, so the glyph is not
   the control and must not claim to be.
4. `the_fold_marker_explains_which_way_it_is` — folded and unfolded,
   asserting the box flips with the glyph.
5. `the_fold_marker_hover_and_click_share_a_rectangle` — every column of
   the fold field is both a hover target and a toggle, and no column
   outside it is either. The rule S3 buys by sharing the locator.
6. `a_folded_node_explains_its_summary` — `FoldSummary` from the `{`
   and from the `}`, one answer.
7. `an_unbaked_summary_says_nobody_has_looked` — spec 0260's arm.
8. `a_hidden_cue_is_not_a_target` — with `i` pressed, column 0 and the
   suffix columns name nothing.
9. `chrome_does_not_steal_a_token` — a row whose key starts immediately
   after the fold field still answers `Key` at its first character.

## Measured outcome

Nine tests, all nine of the test plan. No new hover machinery: the four
arms live inside `doc_element_at_point`, so `handle_hover`, `dwell` and
the "same thing" comparison are untouched — S1's whole argument, and
S5 and S6 needed no code at all.

Two things S3's shared locator paid for immediately:

- `handle_click`'s thirty-line fold-field block became a call to the
  new `App::in_fold_field`, which is now the one answer to "is this
  the fold field" that both the box and the toggle read.
- `heat_cue_at` already returns `HeatDisplay::None` under
  `heat_cues_hidden`, so `i` takes both cue targets away with the
  marks without a guard being written for it.

One thing the tests found that the spec did not say: a cue's two
progressive shapes are the *absence* of a cache entry, not an entry
holding `None`. `HeatDisplay::Unknown` needs no `by_range` entry at
all (an entry whose `best_score` is `None` means every candidate was
vetoed, which is `HeatDisplay::None` and draws nothing), and a
`heat_worker` must be present or `heat_cue_resolve` reads an unsettled
cache as "nothing will ever resolve this" and draws nothing either.
