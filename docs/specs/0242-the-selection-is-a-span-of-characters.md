<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0242 — the selection is a span of characters

Status: implemented
Implemented in: 2026-08-04 (amended 2026-08-05)
App: protolens
Refs: docs/specs/0129-….md (the drag selection and its `REVERSED`
        highlight), docs/specs/0131-….md (`Ctrl-c` is the only copy),
        docs/specs/0194-….md (the caret track, its two zones and
        `caret_draw_index`), docs/specs/0199-….md (motion continues into
        the tree at a *voluntary* end), docs/specs/0126-….md
        (`Shift-Down`/`Shift-Up` alias `J`/`K`)

## Background

protolens can select text, but only whole lines: `select_anchor` and
`select_end` are line indices (`tui/mod.rs:886-890`), the highlight
reverses whole rows (`render.rs:1296-1308`), and `selected_text` joins
whole `row_text`s with `\n` (`mouse.rs:247-269`). There is no keyboard
route into it at all — the only way to select is to drag with the mouse.

So the two things a reader most often wants to lift out of a document —
one field's value, one `{ … }` block's body without its surrounding
lines — cannot be selected at all, and nothing can be selected without a
mouse.

Meanwhile the four `Shift` chords the keyboard would naturally use are
spent on tree operations that already have unmodified spellings:
`Shift-Left`/`H` and `Shift-Right`/`L` fold and unfold the sibling level
(reachable as `zC`/`zO`), and `Shift-Down`/`J` and `Shift-Up`/`K` move by
sibling.

## Goals

- **G1.** A selection is a span of characters — a start and an end each
  identified by a line *and* a column — not a set of lines.
- **G2.** `Shift-Left`, `Shift-Down`, `Shift-Up`, `Shift-Right` and their
  aliases `H`, `J`, `K`, `L` extend the selection.
- **G3.** A mouse drag produces exactly the selection the equivalent
  `Shift`-arrow presses would. One model, one code path, no second
  notion of what is selected.
- **G4.** A selection motion moves the caret over what is *currently* on
  screen and changes nothing else: it neither folds nor unfolds. What
  `Ctrl-c` copies is everything between the two ends **in the document**,
  folded content included — the fold decides what is drawn, never what is
  copied.
- **G5.** Every binding displaced by G2 keeps a spelling.

## Non-goals

- **N1.** No block/rectangular selection (vim's `Ctrl-v`). The wire row
  (spec 0225) would have no meaningful answer for a column span, and
  nothing in the document is tabular.
- **N2.** No word-wise extension (`Shift-Alt-Left`/`Shift-Alt-Right`).
  Those two chords are deliberately left unbound so they remain
  available for it; see S9.
- **N3.** No auto-scroll when a drag or a `Shift-Down` runs past the
  pane's edge, beyond the scrolling the cursor move already does. Spec
  0129 left this out and it is still out of scope.
- **N4.** The selection does not survive an override commit. The nodes
  it names may not exist afterwards, and tracking a span across a
  re-render is not worth it — the commit drops it.

  A fold is a different matter and *is* tracked: S1's node-relative
  anchor survives one, and the user is free to fold or unfold between two
  selection motions even though no selection motion does it for them.

## Specification

### The model

- **S1.** `select_anchor` becomes `Option<CursorPos>` — node,
  line-in-node and caret-track column — naming the selection's **fixed**
  end. `select_end` is **deleted**.

  `CursorPos` rather than a new type because the anchor *is* a remembered
  caret position, and `cursor_pos()` already builds exactly those three
  numbers for the jumplist.

  Not an absolute line number. A fold or unfold anywhere above the anchor
  shifts every absolute line below it, the anchor's included, and the
  user is free to fold between two selection motions. A node-relative
  line is stable across that, which is exactly why the cursor is stored
  as `cursor_line_in_node` and `cursor_line()` is derived rather than
  stored.

- **S2.** The selection's **moving** end is stored in `select_caret:
  Option<CursorPos>` — the same three fields as the anchor. It is set by
  every gesture that extends the selection (`extend_selection`,
  `drag_caret_to`, `select_sweep_hit`) and cleared by `clear_selection`.

  **Amended by spec 0358:** `select_caret` is independent of
  `cursor_column`. Plain caret motions (`j`, `l`, arrow keys…) update
  `cursor_column` but leave `select_caret` alone, so they do not silently
  grow or shrink the span. Only selection-extension gestures write to
  `select_caret`.

  The selected span runs between `select_anchor` and `select_caret` in
  `(line, column)` lexicographic order and **includes both endpoint
  cells** — the caret here is vim's block, resting *on* a character
  rather than between two of them (`caret_bounds` stops it at `len - 1`),
  so a span that excluded its cell would describe something the user
  cannot see, and the document's very last character would be unselectable
  for want of a column past it. `selection_span` therefore returns the end
  column one past `select_caret.column`, which is what makes the rest of
  the code half-open and ordinary.

  Anchor equal to caret is therefore a **one-character** selection, not
  an empty one — the caret is resting on the anchor's own cell and that
  cell is in the span. `Shift-Right` `Shift-Left` is how the keyboard
  asks for a single character; dragging off a character and back is how
  the mouse does. This is vim's visual mode without needing vim's `v`:
  the first `Shift`-motion both enters the mode and moves, so it selects
  two, and one more motion back gives one.

  "Nothing selected" is a second bit, `select_engaged`, and not the
  anchor being equal to the caret. A bare click arms the anchor — the
  following `Drag` needs somewhere to start from — without engaging, so a
  click selects nothing at all. A `Shift`-motion, a drag or a
  double-click engages; `clear_selection` disengages along with dropping
  both the anchor and `select_caret`.

  The column is a *caret-track* column (spec 0194 S1) — the same
  coordinate `cursor_column` lives in, so column `n` is the `n`th
  character of `row_text` and a column at or past `text_chars` is the
  end of the row's text.

- **S3.** ~~Any main-pane key that is not one of the four selection keys
  of S4 and not `Ctrl-c` clears `select_anchor` before it runs.~~
  **Deleted by spec 0358 S1.** The selection now persists across plain
  caret motions; it is cleared only by `Esc`, a bare click, or
  `script_reset`. `Esc` keeps its explicit clear (spec 0129 §G3).

### The keys

- **S4.** In the main pane:

  | chord | alias | effect |
  |---|---|---|
  | `Shift-Left` | `H` | anchor if unanchored, then one character left |
  | `Shift-Right` | `L` | anchor if unanchored, then one character right |
  | `Shift-Up` | `K` | anchor if unanchored, then one row up |
  | `Shift-Down` | `J` | anchor if unanchored, then one row down |

  "anchor if unanchored" means: when `select_anchor` is `None`, set it
  to the caret's current position first. So the first press starts a
  selection from where the caret already was, and every press after it
  extends.

- **S5.** The vertical pair moves the caret with `move_up`/`move_down`
  — the very functions `j`/`k` call, so a selection extends over exactly
  the rows an unmodified motion would have visited.

  The horizontal pair moves one character within the row and, at the
  row's end, on to the neighboring row's near end: right off the last
  character lands on the first character of the next row, left off the
  first lands on the last character of the previous one.

  It does **not** reuse `caret_left`/`caret_right`, which continue into
  the tree at a voluntary end (spec 0199 S5/S6) and fold or unfold on the
  way. It steps with `step_up`/`step_down` instead, over the rows that
  are visible right now.

- **S6.** **A selection motion changes nothing but the caret.** It
  neither folds a node nor opens one: a fold would hide rows the user has
  already selected, and an unfold would reveal rows they had not. A
  folded node is one visible row like any other, and the caret steps over
  it onto the next visible row.

  The `Ctrl-` spellings of S8 and the `z` chords remain the way to fold;
  a selection is not a fold gesture. The user is free to use them
  mid-selection — S1's anchor survives it.

- **S7.** After every selection motion the pane keeps the caret on
  screen: `clamp_scroll_to_cursor` vertically, and a new `pan_to_caret`
  horizontally that moves `pan_offset` by the **minimum** needed to
  bring `cursor_column` inside the visible columns.

  Minimum, not centered. `center_columns` (`search.rs`) centers because
  a search jump lands somewhere the reader has not been; extending a
  selection by one character should move the view by one column, not by
  half a pane.

- **S8.** Displaced by S4, in the main pane:

  | was | now |
  |---|---|
  | `Shift-Left` / `H` — fold all siblings | `Ctrl-Left` / `Ctrl-h` |
  | `Shift-Right` / `L` — unfold all siblings | `Ctrl-Right` / `Ctrl-l` |
  | `Shift-Down` / `J` — next sibling | `Ctrl-Down` / `Ctrl-j` |
  | `Shift-Up` / `K` — previous sibling | `Ctrl-Up` / `Ctrl-k` |

  `Ctrl-h`, `Ctrl-j`, `Ctrl-k` and `Ctrl-l` are all free today — the
  main pane's whole `Ctrl` letter vocabulary is `n p b f a e o i c`
  (`key_dispatch.rs:506-554`).

- **S9.** Displaced in turn by S8, since `Ctrl`-arrows were the pan keys:

  | was | now |
  |---|---|
  | `Ctrl-Up` / `Ctrl-Down` — pan vertically | `Alt-Up` / `Alt-Down` |
  | `Ctrl-Left` / `Ctrl-Right` — pan horizontally | `Shift-Alt-Up` / `Shift-Alt-Down` |

  `Alt-Up`/`Alt-Down` are unbound in the main pane today (only
  `Alt-Left`/`Alt-Right` are taken, by word motion), so vertical pan
  lands on the obvious key.

  Horizontal pan cannot have `Alt-Left`/`Alt-Right` for that reason, and
  it does not get `Shift-Alt-Left`/`Shift-Alt-Right` either — N2 keeps
  those for word-wise selection. `Shift-Alt-Up` pans left and
  `Shift-Alt-Down` pans right, on the same "the vertical pair, shifted"
  reading.

  **Arm order.** `KeyModifiers::contains(SHIFT)` is true for `Shift-Alt`
  too, so the `Shift-Alt` arms must be matched *before* the S4 `Shift`
  arms, which must in turn precede the plain ones — the same
  "modifier-guard first" convention the file already runs on.

### The mouse

- **S10.** `handle_click` already moves the caret to exactly where the
  click landed (`set_caret_from_click`, spec 0194 S7). That is the whole
  of what G3 needs:

  - **`Down`** on the main pane: after the existing caret placement, set
    `select_anchor` to the caret and leave the selection **disengaged**
    (S2). A click on a character selects nothing.
  - **`Drag`**: move the caret to the pointer through that same
    `set_caret_from_click` path, including moving the cursor to the
    pointer's row, and leave `select_anchor` alone — but **engage**. The
    selection is anchor→caret, which is the identical expression to
    S4's, and a drag that comes back to the character it started on
    stays engaged and selects that one character.
  - **`Up`**: if nothing engaged — no drag happened — drop the anchor.
    A double-click selects the whole row it landed on: it is the one
    mouse gesture that names a *line* rather than a character, and both
    of its clicks land on the same cell, so it has to say so rather than
    rely on where the pointer was.

  A double-click selects, and does nothing else. It used to double as
  the `t`/`o` smart proxy `Enter` is (spec 0139), which is one gesture
  wearing two names — and of the two, opening a side pane is much the
  more disruptive to get by accident while trying to select a line.
  `Enter` keeps the proxy.

  A drag now moves the cursor row, which it did not before. That is
  required by G3, not incidental: `Shift-Down` moves the cursor, so a
  downward drag must too, or the two produce different state.

  A click on the *fold field* is exempt from all of the above: it toggles
  the fold and is kept out of the double-click pairing entirely, clearing
  `last_click` so no pair forms across the two zones either. The marker
  is a control, and a control must act on every click that reaches it —
  four fast clicks on it are four toggles, never three plus a gesture.

- **S10b** (2026-08-05). `Shift`-click extends the selection to the
  clicked character: with nothing engaged the fixed end is wherever the
  caret already is, and with a selection engaged the anchor holds still,
  so the one gesture extends or contracts depending on which side of the
  anchor it lands. It is `Drag`'s state change without the button having
  stayed down, so it reuses `drag_caret_to`; it toggles no fold and forms
  no double-click pair.

  `Ctrl`-click is an alias, and in practice the one that arrives:
  holding `Shift` is the terminal convention for "ignore the
  application's mouse reporting and do your own selection" (xterm, VTE,
  Kitty), so a `Shift`-click is usually eaten before the app sees it —
  the same reason the manage pane's radio marker already offers a
  double-click alternative to its `Shift`-click.

### Drawing and copying

- **S11.** The highlight becomes a range restyle rather than a whole-row
  one. Per drawn row, the selected columns are

  - the row strictly between the two ends: `0..text_chars`;
  - the row holding one end: from/to that end's column;
  - one row holding both ends: between the two columns;

  mapped to span indices the way `caret_draw_index` maps one (spec 0194
  S1) and applied with the existing `restyle_range`, which the search
  highlight already uses.

  A **background tint** (`theme::selection_style`), not the `REVERSED`
  the whole-row highlight used (amended 2026-08-05). Inversion is the
  caret's idiom and only the caret's, on spec 0233 S3's rule: the caret
  *is* the selection's moving end, so the two cues always meet on one
  cell, and two reversals cancel — the character at the end of the
  selection came out looking plain, which is the most confusing thing it
  could look like. With the selection a background, `apply_caret` is a
  plain `patch` again: a tint under an inversion still shows.

  The ANSI-16 fallback is magenta rather than the obvious blue. On 16
  colors the four background cues have no shading to separate them and
  any of the other three can land inside a selection: the cursor's row is
  already `DarkGray`/`Gray`, a matched brace `Blue`/`Cyan`, a search hit
  `Yellow`.

  Only the row's *characters*: the fold margin and the heat suffix are
  gutter furniture and are in no selection. The mapping saturates rather
  than failing when the start is left of the pan, since a selection's
  first row is routinely scrolled past horizontally and its visible tail
  still has to be drawn.

  The highlight is the one part of this that is fold-aware, and only
  because it can be: it draws the rows that are on screen, and by S12 the
  ones that are not are still copied.

- **S12.** `selected_text` returns the characters between the two ends:
  the first row from its column to its end, whole rows in between, the
  last row up to its column, joined with `\n`.

  **The walk is fold-blind**, and deliberately so (G4). An absolute line
  is a line of the *whole* document — `absolute_start` sums `lines_total`,
  not `lines_visible` — so stepping from one endpoint to the other with
  `next_line` visits the rows a fold is hiding as well as the ones on
  screen. A selection dragged across a collapsed message copies the
  message.

  Which is why the text comes from `row_text_of(row, None)` rather than
  `row_text`: the latter splices spec 0193's `" ... }"` collapse summary
  into a folded node's header, and here the hidden body follows on the
  next lines, so the summary would both duplicate it and put a `...` in
  the clipboard.

- **S13.** `Ctrl-c`'s message reports `N character(s)` when the
  selection lies within one line and `N line(s)` when it spans more.
  "1 line(s) copied" is a lie about a half-line selection, and the copy
  message is the only feedback that the copy happened at all.

  `Ctrl-c` with no *span* still copies the cursor's whole line (spec
  0131 §G1) — the fallback is unchanged and still reports lines. The
  test is the span and not the anchor: a bare click arms an anchor
  without engaging (S10), and a click followed by `Ctrl-c` copies the
  clicked line, which is what `Ctrl-c` does when no click happened
  either.

## Alternatives considered

**Keep `select_end` as a second field, kept in sync with the caret.**
Two fields that must agree is exactly the failure G3 is about: the
mouse path and the key path would each have to remember to update both,
and the first one that forgot would produce a selection that draws
differently from what it copies. Deriving the moving end from the caret
makes the disagreement unrepresentable.

**Leave the mouse line-granular and give only the keyboard columns.**
Rejected by the user's requirement, and rightly: a reader who drags over
half a value and gets the whole line has been told the selection is
line-based, and then `Shift-Right` tells them it is not.

**Give the horizontal selection keys `caret_left`/`caret_right`.**
Cheaper by a function, but those fold and unfold at a voluntary end
(spec 0199 S5/S6), and S6 is that a selection motion changes nothing but
the caret.

**Have a selection motion unfold whatever it crosses, so that everything
selected is visible.** Drafted, and reversed by the user: it makes the
selection keys a fold gesture in disguise, and a user extending a
selection downward past a collapsed message would find the message
opened under them — the pane reflowing while they are pointing at it.
The copy invariant it was meant to buy is bought instead by S12's
fold-blind walk, at no cost to the screen.

**Copy what the screen shows, `{ ... }` marker included.** The other
half of the same reversal. It is the simpler rule to state, but the
`...` is a *drawing*, not text the document contains, and pasting it
into a `.textproto` produces something that does not parse. What the
user asked for — and what a fold means — is that the fold decides what
is drawn and never what is copied.

**Let anchor-equal-to-caret be the empty selection, and accept that no
selection is one character wide.** Shipped first, and reversed by the
user: it makes "select this one character" — the single most common
thing a reader wants out of a document viewer — unspellable, and it
makes the first `Shift`-motion read as if it had not moved the caret.
The fix is one bit (`select_engaged`), and it is a bit worth having on
its own: it is also what distinguishes an armed click from a selection,
which the old model could not express either.

**Keep the anchor as an absolute line number.** It is what the
line-based selection stored, and it reads more simply. But the user can
fold between two selection motions, and a fold above the anchor moves
it — the selection would silently slide by however many lines the fold
swallowed. Storing node, line-in-node and column costs one derivation
per use (`absolute_start`, which needs no cache) and cannot be wrong.

**Put horizontal pan on `Shift-Alt-Left`/`Shift-Alt-Right`.** The
natural chord, and available — but it is the natural chord for word-wise
selection too, and that is the extension this design most obviously
invites (N2). Pan is the binding with the weaker claim.

## Test plan

1. `the_first_shift_motion_anchors_and_the_rest_only_move_the_caret` —
   S1/S2/S4, including that both endpoint cells are in the span.
2. `a_plain_motion_drops_the_selection_and_ctrl_c_does_not` — S3,
   driven through the key path.
3. `a_horizontal_selection_wraps_onto_the_neighboring_row` — S5's wrap
   at either end, and the `\n` it puts in the copy.
4. `a_selection_motion_neither_folds_nor_unfolds` — S6, over both
   horizontal ends of a folded row.
5. `the_copy_includes_what_a_fold_is_hiding` — S12: the same characters
   come out of `selected_text` whether or not a node between the ends is
   folded, and no collapse summary leaks in.
6. `a_selected_row_and_the_caret_row_stay_distinguishable` — S11, over
   the rendered frame: the selected characters carry the selection tint,
   the gutter beside them does not, nothing but the caret cell is
   inverted, and that cell keeps the tint underneath.
7. `drag_select_spans_multiple_main_pane_rows`,
   `drag_select_upward_still_copies_top_to_bottom`,
   `plain_click_with_no_drag_deselects`,
   `fresh_click_replaces_selection_esc_clears_it`,
   `double_click_selects_the_clicked_line_for_copy` — S10, all four
   mouse arms, re-asserted against `selection_span`. The last of these
   also pins that a double-click opens no side pane.
7b. `shift_right_then_shift_left_leaves_one_character_selected`,
   `a_drag_out_and_back_selects_the_single_character_it_started_on` —
   S2's engaged bit, from the keyboard and from the mouse;
   `clipboard_unavailable_shows_fallback_message_without_panicking`
   carries its other half, that a bare click selects nothing.
7c. `shift_click_extends_the_selection_from_the_caret_then_from_the_anchor`
   — S10b, both states and both modifier spellings;
   `every_click_on_the_fold_marker_toggles_however_many_arrive` — S10's
   exempt fold field, over a run of six clicks.
8. `the_reassigned_keys_dispatch_where_the_table_says`,
   `ctrl_left_and_ctrl_right_fold_the_whole_sibling_level`,
   `ctrl_down_up_alias_sibling_skip_move` — S8, all eight spellings.
9. `shift_alt_arrows_still_pan_without_touching_the_caret` — S9,
   including that a `Shift-Alt` arrow does not reach the selection arms.
10. `the_shifted_arrows_and_the_capital_letters_are_one_binding` — S4's
    two spellings, over all four keys.
11. `only_the_bound_ctrl_and_alt_chords_do_anything_in_the_main_pane` —
    the pane's `Ctrl` vocabulary is now `n p b f a e o i c h j k l` and
    nothing else.

## Measured outcome

681 unit tests + 25 integration tests green; `clippy --all-targets`
clean.

Four design reversals arrived from the user and are recorded above
rather than dropped: a selection motion was to unfold what it crossed
(now S6: it changes nothing but the caret), the copy was to be what the
screen shows (now S12: it is fold-blind), and anchor-equal-to-caret was
the empty selection (now S2: it is one character, and `select_engaged`
carries "nothing selected"), and the highlight was to keep the whole-row
`REVERSED` (now S11: a background tint, because the caret is inside the
selection and the two reversals cancelled). The first three are in
"Alternatives considered".

The second one cost a single call — `absolute_start` already sums
`lines_total`, so the walk in `selected_text` was fold-blind before it
was asked to be; only `row_text` had to become `row_text_of(row, None)`
so that no `" ... }"` summary was spliced in over the body that follows.
