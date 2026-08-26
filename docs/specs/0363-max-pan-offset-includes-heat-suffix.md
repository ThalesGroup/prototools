<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0363 — `max_pan_offset` includes the cursor row's heat suffix

Status: implemented
Implemented in: 2026-08-26
App: protolens
Refs: docs/specs/0194-the-cursor-is-a-caret.md (the caret track and its
        suffix zone; `caret_suffix_len`),
      docs/specs/0304-the-caret-brings-the-view-with-it.md (`pan_to_caret`
        and `max_pan_offset`),
      docs/specs/0284-the-heat-cue-is-a-control.md (the heat suffix as a
        navigable zone),
      docs/specs/0343-shadow-mark.md (`shadowed_scalar` suffix, which
        widens a raw line)

## Background

On a node whose line contains a `; shadowed_scalar` annotation followed by
a heat cue suffix, pressing `$` moves the caret to the last column of the
heat suffix — `caret_bounds().1 = row_text.chars().count() - 1 +
caret_suffix_len` — and calls `pan_to_caret`. The caret lands just past
what the viewport shows, but `pan_to_caret` cannot pan far enough: it
clamps to `max_pan_offset()`, which is based only on `max_visible_line_len()`
and does not include the heat suffix.

The same limit blocks manual panning (Ctrl-Right): `pan_horizontal` also
clamps to `max_pan_offset()`, so the rightmost reachable pan position stops
at the end of the main line text — short of the heat cue.

`max_visible_line_len` calls `row_content` for each visible row.
`row_content` → `row_text_of` → `line_display_text` → `display_pieces`:
this chain includes `; shadowed_scalar` (Phase 3), so shadowed lines are
measured correctly. The heat suffix, however, is chrome rendered after the
line text in the draw loop; no call to `row_content` reaches it.

The heat suffix is only navigable on the caret's own row — the suffix zone
is part of `caret_bounds` only for that row. Widening `max_pan_offset` by
the cursor row's suffix length is therefore both necessary and sufficient.

## Goals

- **G1.** `$` on the cursor row brings the heat cue into view when the
  line is wide enough to require panning.
- **G2.** Ctrl-Right can pan all the way to the heat cue on the cursor
  row.
- **G3.** Rows that have no heat suffix are unaffected.

## Non-goals

- **N1.** Do not account for heat suffixes on non-cursor rows.  The heat
  suffix is not part of `caret_bounds` for any row other than the cursor's,
  so there is nothing to navigate to there.  Including it in the pan bound
  would allow panning past the actual content of non-cursor lines.

## Specification

**S1.** `max_pan_offset` (`navigation.rs`) currently computes:

```rust
let width = (self.main_area.width as usize).saturating_sub(render::HEAT_FIELD_WIDTH);
self.max_visible_line_len().saturating_sub(width)
```

Replace with:

```rust
let width = (self.main_area.width as usize).saturating_sub(render::HEAT_FIELD_WIDTH);
let content_max = self.max_visible_line_len();
// The cursor row's heat suffix extends its effective width beyond what
// max_visible_line_len measures (row_content excludes the suffix).
let cursor_row_effective = self.row_content(self.cursor_row()).chars().count()
    + self.caret_suffix_len;
content_max.max(cursor_row_effective).saturating_sub(width)
```

`row_content` is `&self` and `cursor_row` is also `&self`, so no borrow
conflict with the `&mut self` receiver — the `max_visible_line_len` call
that precedes this is the only `&mut self` step.

**S2.** No change to `pan_to_caret`, `caret_to_line_end`, or
`caret_suffix_len`.  The caret track already reaches the heat suffix; only
the pan ceiling was too low.

## Alternatives considered

### Widen `max_visible_line_len` to include heat suffixes for all rows

Would require calling `heat_chrome` (which needs `&mut self` for lazy
computation) for every visible row inside `max_pan_offset`, which is itself
called from `pan_horizontal` on every Ctrl-Right keypress. Cost is not
justified: the heat suffix on a non-cursor row is unreachable anyway (N1).

### Add a separate `max_pan_offset_with_suffix` function

No benefit over S1. Same result, more surface area.

## Test plan

1. `dollar_on_shadowed_line_with_heat_cue_brings_cue_into_view` — render a
   node that is `shadowed` and has a heat suffix; press `$`; assert that
   `pan_offset` is large enough for `cursor_column` to be inside
   `[pan_offset, pan_offset + usable)`.
2. `pan_right_reaches_heat_suffix_on_cursor_row` — on the same setup,
   Ctrl-Right repeatedly; assert `pan_offset` reaches the value that would
   show the last column of the heat suffix.
3. `max_pan_offset_unaffected_when_no_heat_suffix` — with
   `caret_suffix_len = 0`, assert `max_pan_offset` equals its pre-spec
   value (widest line minus usable width).

## Measured outcome

Three new tests pass (`dollar_on_line_with_heat_suffix_brings_cue_into_view`,
`pan_right_reaches_heat_suffix_on_cursor_row`,
`max_pan_offset_unaffected_when_no_heat_suffix`). One pre-existing test
(`a_budgeted_preview_clamps_a_pan_made_for_the_untruncated_row`) had an
assertion that `max_visible_line_len() > pan_offset`, which assumed
`max_pan_offset` was always bounded by `max_visible_line_len`. That
assumption broke: when the cursor row's content + suffix drives the bound,
`max_pan_offset` legitimately exceeds `max_visible_line_len`. The assertion
was replaced with `max_pan_offset() >= pan_offset`, which is what the test
actually needs to verify (the clamp worked). All 1285 tests pass.

