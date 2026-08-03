<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0234 — a home caret speaks for the brace on its row

Status: implemented
Implemented in: 2026-08-03
App: protolens
Refs: docs/specs/0233-the-caret-is-drawn-the-same-wherever-it-rests.md
        (S5, "on a brace" means the caret's cell is the brace's cell —
        this widens it by one case),
      docs/specs/0199-the-arrow-keys-fold-before-they-leave-the-node.md
        (S1, `CaretAnchor` and the voluntary/involuntary distinction
        this reuses),
      docs/specs/0208-attention-follows-the-cursor.md (S1, `Ctrl-a` and
        `Ctrl-e` as one-key routes to the two anchors)

## Background

The pair lights only while the caret's cell *is* a brace. That is a
correct rule and a nearly useless one here, because it is not where the
caret spends its time: `set_cursor` puts the caret on the row's first
non-blank, `^`/`0`/`Ctrl-a` put it there, and `j`/`k` keep it there
under vim's `'startofline'` rule (spec 0194 S5). So a user walking the
tree — the pane's whole purpose — sits on `items {` all day and is never
shown where `items` ends, and has to press `$` or `%` to ask the row a
question it is already in a position to answer.

## Goals

- **G1.** A caret voluntarily at a row's Home anchor lights the closing
  brace of the message that row opens, exactly as if it were on the
  opening brace.
- **G2.** A caret merely *pushed* to that column by a clamp does not.

## Non-goals

- **N1.** The same for the End anchor. `$` lands on the last character
  of the heat suffix or of a trailing `#@` annotation as often as on
  the `{`, so unlike Home it names no particular token and the rule
  would be a coincidence rather than a reading.
- **N2.** Lighting the opening brace as well. "As if the caret were on
  it" means the caret is what marks it, and the caret is a cell away on
  a row already carrying `cursor_row_style`.
- **N3.** Any change to `%`, which moves the caret onto a brace and so
  reaches the ordinary rule.

## Specification

- **S1.** `render`'s partner resolution gains a third case: the caret's
  anchor is `CaretAnchor::Home`, its line is the pair's *opening* line,
  and its column is the row's first reachable one. The partner is then
  the closing brace. The two existing cases — the caret's cell is one of
  the two braces — are tested first and are untouched, so a row whose
  first non-blank *is* the `{` takes the ordinary path and lights its
  partner the ordinary way.

- **S2.** The column is compared rather than inferred from the anchor.
  `anchor == Home` does imply "on the first non-blank" today, across
  `reset_caret_column`, `settle_anchor`, `carry_caret` and
  `clamp_caret_column` — but that is an invariant of the *navigation*
  layer, and the drawing code should not be the place that breaks when
  someone adds a fifth writer.

- **S3.** A folded node needs no special case. Its `{ ... }` collapse
  summary puts both braces on the caret's own row, so the Home caret
  lights the synthetic `}` three characters to its right, which is
  where the node does in fact end on screen.

## Alternatives considered

### Light the pair from anywhere on the row

Simpler to state, and wrong: from a row inside a message body it would
light the *enclosing* block's braces, which is a different feature
("show me the block I am in") with a different answer for nested
messages, where several blocks enclose the caret and nothing picks one.
Restricting it to the Home anchor keeps it a matching rule, because the
anchor is a position the user chose.

### Fire on `caret_anchor == Home` alone

One comparison cheaper. See S2: it makes the renderer depend on an
invariant maintained four files away, for nothing.

### Extend it to the End anchor too

See N1. The Home anchor is the row's first non-blank, which is a token;
the End anchor is wherever the row happens to stop, which after a heat
suffix or an annotation is not the brace.

## Test plan

1. `a_home_caret_lights_the_brace_its_row_opens` — a voluntary Home on a
   bracketed node's header tints the closing brace and leaves the caret
   drawn as it always is; the same column with the anchor forced to
   `Free`, as a vertical move's clamp would leave it, tints nothing.
2. `a_brace_pairs_with_its_match_only_when_the_caret_is_on_it` — its
   "off a brace" state moves off the Home column, since landing on a
   bracketed header at Home is now a pairing state rather than an idle
   one.

## Measured outcome

`cargo test -p protolens` passes 622 — one more than before, the new
test — plus 25 in `tests/batch_export.rs`.

Nine lines in `render`, and the whole cost is one extra `caret_bounds()`
per frame, taken only when the anchor is `Home`. It builds the cursor
row's text, which `clamp_caret_column` already does immediately above
it on every frame.

`Ctrl-a`/`Ctrl-e` needed no work: spec 0208 S1 already bound them to
`caret_to_line_start`/`caret_to_line_end`, which are `^`/`$` and which
declare the anchor — so they were already the one-key route into this
spec's state.
