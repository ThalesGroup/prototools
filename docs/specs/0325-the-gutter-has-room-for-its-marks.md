<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0325 — the gutter has room for its marks

Status: implemented
Implemented in: 2026-08-18
App: protolens
Refs: docs/specs/0193-… (the fold marker and `FOLD_FIELD_WIDTH`),
        docs/specs/0138-… N1 (the heat cue's reserved leading column),
        docs/specs/0260-… (the five-color fold margin, which is why the
        marker's color must be the terminal's to choose),
        docs/specs/0322-… (`ANOMALY_GLYPH` `◆`, already an East Asian
        Ambiguous glyph in this column), docs/specs/0190-… S5 (the
        activity dot's reserved column on the global row),
        docs/specs/0194-… (`caret_draw_index`, the caret track's two
        zones), docs/specs/0284-… (the gutter as a click zone of its
        own)

## Background

Three marks share the left edge of a protolens window, and until now
they were packed one column each:

```
● ▾ options {
```

column 0 the heat cue (spec 0138 N1), column 1 the fold marker (spec
0193), column 2 the separating blank the fold field's second column
provides, then the row's own text.

Two things are wrong with that on a real terminal.

**The fold marker was drawn too small to read.** It is the one mark
whose *state* has to be legible at a glance — open against closed, and
since spec 0260 in one of five colors — and `▾`/`▸` (U+25BE/U+25B8,
`SMALL`) are the smallest triangles Unicode offers. Enlarging to
U+23F7/U+23F5 (`MEDIUM`) was judged "almost OK" on a real terminal.

**Enlarging it made the two marks collide.** `●▼` with no gap between
them reads as one compound mark; the eye resolves the pair before it
resolves either glyph. The heat cue had no separating blank of its own
because spec 0138 N1 spent exactly one column, on the assumption that
the fold field's *leading* column would do — which it does not, since
the fold field's blank is on its trailing side.

The same collision exists on the global command/message row, where the
activity dot (spec 0190 S5) sits in column 0 and a command being typed
begins with `:` in column 1: `●:override`.

## Goals

- **G1.** The fold marker is drawn at the largest size available, and
  open and closed are the same glyph rotated a quarter turn.
- **G2.** Every mark in a leading gutter is separated from what follows
  it by a blank that belongs to the gutter, not to the text.
- **G3.** Both gutters stay reserved unconditionally. A field that
  appeared and vanished with the window's contents would move the text
  origin as the reader scrolls, which is worse than spending the
  columns.

## Non-goals

- **N1.** Making either width configurable. There is nowhere a
  preference would be remembered, and every hit test, caret mapping and
  pan clamp in the pane derives from the two constants — which is
  exactly why they are constants and not literals.

- **N2.** Merging the heat and fold fields into one gutter constant.
  They are set by different specs for different reasons and must be
  free to differ; the arithmetic that adds them is already written as a
  sum of the two.

- **N3.** A glyph the terminal cannot get wrong. There is not one. The
  risks are stated at the constants and the fallback ladder with them;
  this spec picks a point on that ladder, it does not eliminate it.

## Specification

- **S1.** `FOLD_GLYPH_OPEN` / `FOLD_GLYPH_CLOSED` become `▼` U+25BC
  `BLACK DOWN-POINTING TRIANGLE` and `▶` U+25B6 `BLACK RIGHT-POINTING
  TRIANGLE` — the largest pair available, and a genuine pair: same
  block, same weight, same optical size, one rotated from the other.

  Two risks are accepted knowingly, both recorded at the constants in
  `render.rs` along with the two-step ladder back down (U+23F7/U+23F5,
  then U+25BE/U+25B8), because that is where someone changing them will
  look:

  - U+25B6 carries the Emoji property (`Emoji_Presentation: No`), so a
    terminal that reaches for an emoji font anyway draws it colored and
    double-width, destroying spec 0260's color. U+25BC carries no emoji
    property, so the failure shows as an *asymmetry* between the two
    markers — which is what makes it diagnosable on sight.
  - Both are East Asian Ambiguous, where U+23F7/U+23F5 were Neutral, so
    a CJK locale shifts a foldable row's text one column. Spec 0322
    already admitted an Ambiguous glyph to this column (`◆` U+25C6) and
    the heat cue's `●` U+25CF has always been one; this is the same
    bet, not a new one.

- **S2.** `render::HEAT_FIELD_WIDTH: usize = 2` — the cue's glyph, then
  the blank. It replaces the bare `1` at every site that mapped a text
  column to a drawn cell or back: `tint_columns`, `caret_draw_index`,
  the selection and search restyles, the wire row's indent, the caret
  cell in `menu.rs`, `pan_to_caret`'s usable width,
  `max_pan_offset`, and the four hit tests in `mouse.rs`, `popup.rs`
  and `wire.rs`.

  The hit tests change shape as well as constant: `rel_col >= 1 && …
  rel_col - 1` becomes `rel_col.checked_sub(HEAT_FIELD_WIDTH)`, and
  `set_caret_from_click`'s `if x == 0 … else if x - 1 < panned` becomes
  a `match` on `x.checked_sub(HEAT_FIELD_WIDTH)`. A width of 2 makes
  the old subtract-one guards silently wrong rather than
  merely off by one, so the guard and the offset are now the same
  expression.

- **S3.** `render::ACTIVITY_FIELD_WIDTH: u16 = 2` — the dot, then the
  blank that keeps it off the leading `:` of a command being typed.
  Spec 0190 S5's layout already derives `cmd_area`, the pan clamp and
  the terminal cursor position from the second `Constraint`, so
  widening the first is the whole change.

  Its own constant rather than `HEAT_FIELD_WIDTH`: the global row and
  the document pane are unrelated surfaces, and a shared name would
  invite a future change to one to move the other. `u16` because
  `Constraint::Length` takes one, where the document pane's arithmetic
  is all `usize`.

## Alternatives considered

**`►` U+25BA `BLACK RIGHT-POINTING POINTER` for the closed marker.**
Tried first, because unlike U+25B6 it carries no emoji property at all,
which would have retired half of S1's first risk. Rejected on sight: it
is a *different shape* from `▼` — narrower, with a flatter base — so
the pair no longer reads as one glyph rotated, and the eye registers
the mismatch before it registers the direction. Consistency of the pair
outranks the emoji risk, which is conditional on the terminal.

**Keeping one column and putting the blank in the fold field.** The
fold field is already two columns, marker plus trailing blank; taking
its blank for the cue's separation would put the marker flush against
the identifier instead (`▼options` is one token to the eye). The
collision would move, not go away.

**Drawing the separating blank only when a cue is present.** It is the
same mistake spec 0138 N1 names for the cue itself: the text origin
would move as the reader scrolls past a row that has a cue and one that
does not.

## Test plan

The suite already pins this densely, which is the point — nothing new
was needed, and roughly a dozen existing tests had to move:

1. The `mouse` and `menu` suites, which assert click coordinates
   against caret columns, are the direct check on S2's hit tests. A
   fold-marker click and a caret click disagreeing by one column is the
   failure mode S2's rewrite exists to make impossible.
2. The `render`, `popup`, `popup_doc`, `popup_wire` and `selection`
   suites compare drawn rows and hover targets, so they pin the gutter
   width and the glyphs together.
3. `key_dispatch` and `navigation` pin `max_pan_offset` and
   `pan_to_caret` against the widened gutter.
4. No test may hard-code an RGB value — the sandbox has no `COLORTERM`
   — so the glyph change is asserted on the character, and the color it
   carries stays spec 0260's business.

## Measured outcome

Judged on a real terminal rather than measured: `▼`/`▶` were reported
legible, which is what the two earlier pairs were not. Cost is one
column of text per document row and one per command row.
