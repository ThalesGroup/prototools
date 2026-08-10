<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0193 — the fold marker lives in a gutter

Status: implemented
Implemented in: 2026-07-27
App: protolens
Refs: docs/specs/0113-protolens-tui-refinements.md (D33, fold markers),
      docs/specs/0116-tree-sitter-textproto-highlight-captures.md (§7, §9),
      docs/specs/0133-protolens-dynamic-annotations-toggle.md (G4),
      docs/specs/0138-protolens-main-pane-inference-heat-cue.md
        (N1, the reserved glyph column),
      docs/specs/0147-protolens-status-message-command-line-split.md (G2, G4),
      docs/specs/0185-the-preview-is-an-overlay.md (S2, G3),
      docs/specs/0187-highlighting-is-a-property-of-the-viewport.md (S3),
      docs/specs/0192-a-frame-costs-the-same-wherever-the-cursor-is.md (S2)

## Background

Four interactive-feedback items about the main pane's chrome. They are
unrelated in mechanism but share a theme: the pane's fixed columns and its
status line should report position without moving the thing being read.

### 1. The fold marker displaces the text it marks

`row_content` (`render.rs:237-265`) and its styled twin `row_spans`
(`render.rs:288-325`) both *insert* the fold marker at the end of the
line's own indentation:

```rust
let indent_len = content.len() - content.trim_start().len();
let mut insertions = vec![(indent_len, marker.to_string())];
```

The line's indentation is deliberately kept intact — `render_line_content`'s
doc comment says so in as many words ("kept intact — not shortened by one
column to make room"). The consequence is that a foldable line's first
token sits one column to the right of a non-foldable sibling's:

```
  fielda: 1
  ▾fieldb {
    ...
  }
```

`fieldb` is one column right of `fielda`, and the `}` of the closing row is
one column left of the `▾`. Nothing in the pane lines up.

### 2. The braces of the node under the cursor are not identifiable

When the cursor is on a message or group, its `{` and the matching `}` may
be dozens of rows apart with a whole subtree between them, and nothing
connects them visually. Every editor of this shape (vim's `matchparen`,
VS Code's bracket-pair highlight) tints the brace under the cursor and its
match.

This is also a precursor to replacing the main pane's full-row
`Modifier::REVERSED` cursor (`render.rs:698-702`) with a real character
cursor — a change *not* attempted here, but the brace styling is the piece
of it that stands alone.

### 3. A too-long left status hides its own informative end

`statusline_text` (`mod.rs:354-376`) keeps the **head** of the left part
and appends a trailing `<`:

```rust
let mut line: String = left_chars.into_iter().take(budget - 1).collect();
line.push('<');
```

The left part is `<blob path> <node path>: <type> [tag]`, optionally with a
` - preview (main pane locked)` suffix. The blob path is the least
informative piece and the first to be shown; the node path, the type and
the lock notice are the parts a user actually reads, and they are the parts
that get cut.

### 4. `L<n>/<m>` freezes while the view scrolls

Every pane's status line right-flushes `L<cursor>/<total>`
(`render.rs:765`, `render.rs:945`, `manage_pane.rs:851`). In the main pane
`z`/`x` and the mouse wheel pan the viewport **without** moving the cursor,
so the indicator sits still while the whole screen moves — which reads as a
bug.

The reported instinct was to switch the indicator to the first visible
line. That would be wrong for this application: protolens is
editor-shaped, not pager-shaped — there is a cursor, navigation is
cursor-driven, and every operation (`t`, `x`, extract, override) acts on
the node under it. vim's `ruler`, VS Code's status bar and emacs all report
the cursor; only pagers (`less`) report the viewport. But vim solves
exactly this complaint by *pairing* the cursor ruler with a viewport
indicator — `Top` / `Bot` / `All` / `45%` — rather than replacing it.

## Goals

- **G1.** A foldable line's text sits in exactly the same column as a
  non-foldable line's text at the same depth. The fold marker never
  displaces content.
- **G2.** The fold marker sits immediately left of the token it marks, in
  columns that belong to the marker, not to the line.
- **G3.** When the cursor is on a message or group, its opening `{` and its
  matching `}` are drawn in a distinct high-visibility color — including
  when the node is folded, where the closing brace is the synthetic one in
  `{ ... }`.
- **G4.** When the left half of a status line does not fit, its **tail**
  survives, marked with a leading `<`.
- **G5.** Every pane's status line reports both the cursor line *and*
  whether the viewport is at the top, the bottom, showing everything, or
  some percentage in between.
- **G6.** The mouse's fold-marker hit test keeps agreeing with what is
  drawn, for every `--indent` value.

## Non-goals

- **N1.** Replacing the full-row `REVERSED` cursor with a character
  cursor. G3 is a precursor, not the change itself.

  Recorded for that later spec, since it changes what G3 should
  eventually become: the agreed direction is a **vim-style block caret**,
  not a VS Code insertion caret — protolens's cursor designates a *node*
  rather than a gap between characters, and the terminal's own blinking
  cursor is already spoken for by the command/search/rename row (spec
  0147 G4). Under a block caret the brace pair stops being a *color* and
  becomes a *shape*: the matching brace gets a dimmer copy of the caret,
  and S2's red goes away entirely. S2's real contribution to that work is
  therefore mechanical, not visual — it is the ability to restyle one
  byte range of a row's content, which is exactly what a one-grapheme
  caret needs. That work also brings in `Left`/`Right` and a column
  coordinate alongside the line, which in turn unlocks things a row
  highlight cannot express — e.g. a caret resting on a heat cue can spell
  the full score out in the command/message row.
- **N2.** Highlighting brace pairs anywhere other than the cursor's own
  node — no general `matchparen`, no bracket-pair rainbow.
- **N3.** A configurable fold-gutter width, or a vim-style
  `foldcolumn`-like display of nesting depth. The gutter is exactly two
  columns and shows at most one marker.
- **N4.** Changing what the left half of a status line contains, or its
  field order. G4 changes only which end survives truncation.
- **N5.** Making the viewport indicator configurable or hideable.

## Specification

### S1. The fold marker occupies two reserved columns, and never inserts

> **Amended 2026-08-10 — the glyphs are `⏷`/`⏵`, not `▾`/`▸`.** This
> section and the diagrams throughout this spec were written against
> U+25BE and U+25B8, whose Unicode names say `SMALL TRIANGLE` and which
> are. Spec 0260 later made the marker carry a five-color status, and a
> glyph with that little ink is where two of those colors stop being
> distinguishable. The replacements are U+23F7 and U+23F5, `MEDIUM`, and
> are still one column: both are East Asian Neutral, as the small pair
> was. The obvious `▼`/`▶` was rejected — both are East Asian Ambiguous
> and go double-width under a CJK locale, and `▶` (U+25B6) is in
> addition the ▶️ play button in `emoji-data.txt`, which an emoji font
> draws in a color of its own and so takes away the one thing this
> marker exists to carry. Everything below about *widths and columns* is
> unchanged and still governs; only the two characters differ.

Every main-pane row gains a **two-column fold field** prepended to its
content, in both `row_content` and `row_spans`. The field is:

- `"▾ "` or `"▸ "` when the row's node is foldable **and** the line's own
  indentation is narrower than two columns (root-level rows, and every row
  when `--indent` is 0 or 1);
- `"  "` otherwise.

When the row's node is foldable and its indentation is at least two
columns, the marker instead **overwrites** the last two columns of that
indentation — bytes `indent_len - 2 .. indent_len` of the content become
`"▾ "` — and the two-column field stays blank.

Both branches produce the same total width for every row at a given depth,
which is G1. Both put the marker in the two columns immediately left of the
first non-blank token, which is G2.

Stated as one rule: *the marker is drawn in the two display columns
immediately left of the line's first non-blank character; when those
columns would fall left of the pane's text origin, it is drawn in the
reserved field instead, which is exactly two columns wide so the fallback
always fits.*

#### The marker's absolute column does not move

With the default `--indent 2` this is a pure realignment of the *text*, not
of the marker. Today the marker is at pane column `1 + indent_len` (the
leading `1` is spec 0138 N1's always-reserved heat-cue column). Under S1:

- `indent_len < 2`: the marker is at field offset 0, i.e. pane column 1.
  For `indent_len == 0` that is the same column as today.
- `indent_len >= 2`: the field is two blank columns, content starts at pane
  column 3, and the marker is at content offset `indent_len - 2`, i.e. pane
  column `indent_len + 1` — the same column as today.

What moves is the text: every row shifts right by two columns relative to a
non-foldable row today, and a foldable row's token shifts right by one.
That is the price of the gutter, and it is charged to every row equally.

#### `marker_column` becomes a shared helper

`marker_column` (`mod.rs:1896-1902`) currently returns `indent_len` and
`mouse.rs:358` adds the heat column back. That coincidentally still holds
for `--indent >= 2`, and breaks for `--indent 1`, where the marker falls
into the reserved field but `marker_column` still reports column 1.

`marker_column` is therefore rewritten to return the marker's offset **from
the start of the rendered row** (heat column excluded, as today), computed
by the same rule S1 renders with:

```rust
fn marker_column(line: &str) -> u16 {
    let indent_len = line.len() - line.trim_start().len();
    if indent_len < FOLD_FIELD_WIDTH { 0 } else { indent_len as u16 }
}
```

There must be exactly one expression of this rule; `row_content`,
`row_spans` and `marker_column` all use it.

#### The clipboard takes the text, not the row

`row_content` splits in two: `row_text` applies the *content* transforms
(spec 0133 G4's annotation hiding, and a folded node's `{ ... }` collapse
summary), and `row_content` is `row_text` with the margin in front.
`selected_text` (`mouse.rs`) switches to `row_text`, because the margin
is gutter furniture and a `▾` or two leading blanks pasted into a
`.textproto` would not parse. That also removes the pre-existing bug
where a copied foldable row carried its `▾` along with it.

### S2. The cursor node's braces are drawn in the brace-match color

`row_spans` gains a brace-highlight pass. For a committed row whose line is
one of the cursor node's *own* lines, one brace is styled:

- on the node's header line, the last `{` in the content;
- on the node's footer line, the last `}` in the content;
- when the node is folded, both the content's own `{` **and** the `}` of
  the synthetic `" ... }"` insertion, since the folded header line carries
  the whole pair.

A row with no brace at the expected position (a scalar node, a node whose
annotation truncation removed it) is left alone. Overlay rows are excluded,
exactly as they already are for the fold marker and the override hint.

The two lines are taken from the *cursor node's* own
`span.text_range` — `start` is its header, `end - 1` its footer — rather
than by asking each row which node it belongs to. `display_row_source`
resolves a row to `node_at_header_line` only, so a footer row reports no
node at all and could never be recognized that way. The same range also
supplies the bracketed-node test, `end - 1 > start`: it is what
`line_to_node`'s construction uses to decide a node has a footer line,
and without it `content.rfind('{')` on an unbracketed scalar would
redden a brace inside a string literal.

#### Threading the style through `spans_with_insertions`

`spans_with_insertions` (`render.rs:333-361`) currently emits every
insertion as `Span::raw` — unstyled by construction, documented as
"fold-marker/collapse-summary text is never part of the highlighted source,
so it never carries a role". An insertion becomes
`(byte position, text, Option<Style>)`, so the synthetic `}` can be red
while the `" ... "` beside it stays plain. The folded case therefore
pushes **two** insertions at the same position — `" ... "` unstyled and
`"}"` styled. `sort_by_key` is stable, so they keep that order.

The header line's own `{` is a different problem: it is real source text
already carrying a real `SyntaxRole`, so it has to be *re*styled rather
than inserted. The mechanism is a single helper, `cut_segments(&mut
segments, range)`, which removes a byte range from the segment list,
splitting the segments straddling either end. A cut range is simply never
emitted, so a cut brace can be re-emitted as a styled insertion at the
same position — with no extra parameter on `spans_with_insertions`, and
no ordering rule beyond the one the insertions already follow.

The same helper is what makes S1 work at all, and this is the reason the
two sections share an implementation. The fold margin cannot be prepended
to `content` before `segment_line` runs: `window_styles`' hint ranges are
absolute byte offsets into the row's *raw* line (spec 0187 S3 builds them
from `display_row_source`), so shifting the text would misalign every
hint on the row. Instead the margin subsumes the line's own indentation —
`cut_segments(&mut segments, 0..indent_len)` — and is emitted as one
`Span::raw` in front. `row_content` follows the identical rule via
`fold_margin_of`, which returns `(margin, body_start)` for both callers.

#### Why this is not a new `SyntaxRole`

`SyntaxRole` (`colorize.rs:45-59`) has exactly 13 variants whose
discriminant order must match `RECOGNIZED_NAMES: [&str; 13]`
(`colorize.rs:69-83`), the capture-name list handed to tree-sitter's
`HighlightConfiguration::configure`. The rule is strictly one variant per
`queries/highlights.scm` capture name, and a cursor-relative highlight has
no corresponding capture — it is a property of the cursor, not of the
syntax.

`theme::manage_entry_style` (`theme.rs:364-377`) is the existing precedent
and says so in its doc comment. S2 adds a sibling:

```rust
pub fn brace_match_style(theme: ThemeKind) -> Style
```

returning bright red plus `BOLD` in both themes — deliberately
theme-independent, like `focus_style`, since the point is maximum
salience rather than palette harmony. On the RGB path this is a named
constant in each of `dark_rgb`/`light_rgb`; on the ANSI-16 fallback,
`Color::LightRed` (dark) and `Color::Red` (light), mirroring
`PunctuationBracketExtension`'s existing per-theme pair.

The cursor row is separately `REVERSED` for its whole width, so on the
header row the brace reads as a red *block* and on the footer row as red
text. That asymmetry is correct: it is how vim distinguishes the brace
under the cursor from its match.

### S3. A truncated left status keeps its tail

`statusline_text`'s overflow branch is inverted: keep the last
`budget - 1` characters of `left` and prepend `<`.

```
before:  /long/path/to/blob.pb .messages[3].fie<L12/900
after:   <ges[3].field: .foo.Bar [7] (preview)  L12/900
```

Nothing else about the function changes: the right part is still clipped to
`width` first, still right-flushed, and a zero budget still yields the right
part alone.

#### The lock notice is dropped, and the preview notice is shortened

Keeping the tail only helps if the tail is worth keeping, and today it is
the *longest* and *least* informative field: ` - preview (main pane
locked)` is 30 columns, wide enough to consume a half-width main pane's
whole budget on its own. So the suffix (`render.rs:753-760`) becomes:

| state                     | today                          | S3                |
|---------------------------|--------------------------------|-------------------|
| pane open, overlay shown  | ` - preview (main pane locked)` | ` (preview)`      |
| pane open, no overlay     | ` - main pane locked`           | *(nothing)*       |
| pane closed               | *(nothing)*                     | *(nothing)*       |

The lock half is redundant with `OVERRIDE_FOCUS_LOCK_MESSAGE`
(`mod.rs:104`), which already fires in the global command/message row on
both ways of trying to leave — `Tab` and a main-pane click — and says
strictly more than the status suffix did, including how to get out
(`Esc` closes, `Alt`-arrows pan). A lock is a thing you discover by
pushing against it; a status line that announces it permanently spends
columns on a fact the user learns for free the moment it matters.

This reverses spec 0185 S7/Q1, which tied the notice to the pane being
open rather than to an overlay existing, so that a candidate which failed
to render still said something. Under S3 the suffix means exactly one
thing — *what you are looking at is hypothetical* — and its absence
correctly means the main pane is showing committed content, which is
what a failed candidate leaves on screen anyway.

### S4. Every pane status line gains a viewport indicator

A shared helper in `tui/mod.rs`, next to `statusline_text`:

```rust
fn viewport_label(first_visible: usize, height: usize, total: usize) -> String
```

following vim's own rule:

- `total <= height` → `"All"`
- `first_visible == 0` → `"Top"`
- `first_visible + height >= total` → `"Bot"`
- otherwise → `"{}%"` of `first_visible * 100 / (total - height)`

appended to the existing ruler with two spaces, so the right part reads
`L12/900  45%`. It is three to four columns and is never dropped — it is
the cheapest part of the ruler and the one that answers "where am I".

Per pane:

| pane     | `first_visible`         | `height`                  | `total`                       |
|----------|-------------------------|---------------------------|-------------------------------|
| main     | `self.scroll_offset`    | `window.len()`            | `self.composed_row_count()`   |
| override | `self.override_scroll`  | `self.override_list_height` | `self.override_candidates.len()` |
| manage   | the pane's `start`      | its list height           | its `total_rows`              |

The main pane's viewport figures are display-row counts, matching
`scroll_offset`'s own units; its `L<n>/<m>` stays in `self.lines`
coordinates, unchanged. The two are different questions and the mismatch
is pre-existing.

## Alternatives considered

### A1. Shorten the indentation by one column instead of inserting

Keeps the pane one column narrower and needs no reserved field. Rejected:
it only works when the line has an indent to give up, so root-level
foldable rows — the most common kind at the top of a blob — would still
displace, and `--indent 0` would break entirely. It also glues the marker
to the token (`▾options`), which reads as one word.

### A2. Put the marker in a fixed gutter at depth-independent column 0

This is literally vim's `foldcolumn`, and it is the simplest possible
implementation. Rejected on the reported preference: with a deep tree the
marker ends up arbitrarily far from the row it marks, and the association
has to be inferred from the row rather than seen. `jless`'s `▶`/`▼` sit at
the item's own indent for the same reason.

### A3. Style the braces via a 14th `SyntaxRole`

Rejected — see S2. It would require adding a matching entry to
`RECOGNIZED_NAMES` and therefore a capture name to
`queries/highlights.scm` for something tree-sitter cannot capture, since
the highlight depends on where the cursor is.

### A4. Highlight the whole cursor node's header and footer rows instead

Cheaper — no per-byte work at all. Rejected: the footer row would light up
as a full bar far from the cursor, which reads as a second cursor. The
point is to identify a *character*.

### A5. Ellipsize the middle of an over-long left status

`/long/…/blob.pb .messages[3].field` keeps both ends. Rejected as more
machinery than the complaint needs, and the head is not worth the columns
it would cost — the reported problem is precisely that the head is what
survives today.

### A6. Replace `L<n>/<m>` with the first visible line

The reported instinct. Rejected — see Background 4. It would make the
indicator disagree with every operation the user can perform, all of which
act on the cursor.

## Test plan

1. **A foldable row's text starts in the same column as a non-foldable
   row's at the same depth.** Over a fixture with both, assert
   `row_content` puts the first non-blank character at the same index for
   both. This is G1 stated directly, and it fails today.
2. **The marker sits exactly two columns left of the token.** For a
   fixture with foldable nodes at depth 0 and depth 2, assert the marker's
   index in `row_content`'s output is `first_non_blank - 2` for each.
3. **`marker_column` agrees with `row_content` for `--indent` in
   {0, 1, 2, 4}.** A table test that renders a foldable line and asserts
   `marker_column(line) + 1` is the pane column the `▾` actually occupies
   in `row_spans`' output. This is G6, and it is the assertion that
   catches the `--indent 1` case A1 would have broken.
4. **`row_content` and `row_spans` produce the same text.** Concatenating
   `row_spans`' span contents equals `row_content` for every row of a
   fixture with folded, unfolded and non-foldable rows. Spec 0185 G3 needs
   these two to stay identical; S1 edits both.
5. **The cursor node's braces are styled, and only those.** With the cursor
   on a message node, assert the `{` span on its header line and the `}`
   span on its footer line carry `brace_match_style`, and that no span on
   any other row does — including the rows of a *sibling* message, which is
   what N2 excludes.
6. **A folded node's synthetic `}` is styled and its `" ... "` is not.**
   The insertion-splitting half of S2, which nothing else covers.
7. **Moving the cursor off the node clears both braces.** Guards against
   the highlight being computed once and cached against a stale cursor.
8. **`statusline_text` keeps the tail.** Assert the truncated result ends
   with `left`'s last characters and begins with `<`. The existing tests
   assert the opposite and are rewritten, not added to.
9. **The preview suffix is `(preview)`, and there is no lock suffix.**
   The three existing assertions at `tests/override_select.rs:1818-1841`
   pin today's exact strings (`"preview (main pane locked)"`, and
   `"main pane locked"` without `"preview"`); they are rewritten to pin
   `"(preview)"` present-with-overlay and **absent** without one. The
   `!contains("locked")` assertions on either side stay as they are —
   they are already correct and become stronger.
10. **`viewport_label` returns `All`/`Top`/`Bot`/`NN%` at the boundaries.**
   A table test including `height == total`, `height > total`,
   `first_visible == 0` with more below, and the last full screen — the
   last of which must be `Bot`, not `99%`.
11. **Panning without moving the cursor changes the viewport label and
    not `L<n>/<m>`.** This is G5's whole point and the reported complaint,
    asserted end to end over a rendered `TestBackend` frame.

## Open questions

**Q1 (resolved). Does S3's tail-keeping let the mode suffix crowd out the
node path?** It did, and the fix was to make the suffix small rather than
to move it: ` - preview (main pane locked)` becomes ` (preview)`, and the
no-overlay lock notice goes away entirely — see S3. Right-flushing the
notice into the *right* part alongside the ruler was the alternative
considered and was not needed once the suffix was 9 columns instead of 30.

**Q2. Should the two-column fold field be spent on rows in a subtree with
no foldable node at all?** It is reserved unconditionally, as spec 0138
N1's heat column is. A width-conscious alternative reserves it only when
the visible window contains a foldable row — but then the text origin moves
as the user scrolls, which is worse than losing two columns.

## Measured outcome

Eight tests added and two rewritten; the suite goes from 426 to 439
passing. The rewritten pair are the ones that pinned the behavior S3
reverses — `main_statusline_truncates_the_left_side_with_a_marker_when_
narrow` now asserts the result *begins* with `<` and keeps
`[message][2..4)`, and `the_main_statusline_announces_the_focus_lock`
now asserts `(preview)` appears with an overlay and is absent without
one, with `locked` absent throughout.

Three further tests needed their expectations shifted by exactly
`FOLD_FIELD_WIDTH`, which is the whole of S1's cost stated in numbers:
two annotation-display tests move their expected rows two columns right,
and `pan_right_reaches_the_true_end_of_the_longest_visible_line`'s
maximum pan offset becomes `line.len() + FOLD_FIELD_WIDTH -
usable_width`. The fold field is *inside* the panned row, unlike spec
0138 N1's heat-cue column which is prepended after panning — a
distinction that only this test makes visible.

`render_line_content` is deleted. It existed only because `selected_text`
needed a fold-marker-free row; `row_text` now serves that need from the
same code path everything else uses, so the second line-rendering path
spec 0185 G3 warns about is gone rather than merely unused.
