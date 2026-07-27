<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0194 — the cursor is a caret

Status: draft
App: protolens
Refs: docs/specs/0113-protolens-tui-refinements.md (D24, horizontal pan),
      docs/specs/0126-protolens-focus-independent-keys.md (G2, the
        shifted sibling keys),
      docs/specs/0138-protolens-main-pane-inference-heat-cue.md
        (N1, the reserved glyph column; G9, the suffix),
      docs/specs/0142-protolens-cursor-on-closing-brace.md
        (the `cursor_footer` half-coordinate),
      docs/specs/0147-protolens-status-message-command-line-split.md (G4),
      docs/specs/0185-the-preview-is-an-overlay.md (S2, G3),
      docs/specs/0193-the-fold-marker-lives-in-a-gutter.md (S1, S2, N1)

## Background

### 1. The cursor names a node, and is drawn as a whole row

`self.cursor` is an arena index. Its *line* is derived, not stored:
`cursor_line()` (`navigation.rs:160-167`) returns the node's header line,
or its footer line when the boolean `cursor_footer` is set — the
half-coordinate spec 0142 added so that a message's closing `}` is a
reachable cursor stop.

It is drawn by turning the whole row inside out
(`render.rs:855-859`):

```rust
if self.scroll_offset + row == cursor_draw_row || selected {
    for span in &mut spans {
        span.style = span.style.add_modifier(Modifier::REVERSED);
    }
}
```

Note the `|| selected`. The cursor and a mouse selection are the *same*
mechanism, distinguished only by which rows they cover. That is why a
caret cannot simply be a narrower version of this: the row highlight has
a second job, and it keeps it.

### 2. A row highlight cannot point at a character

Spec 0193 S2 wanted to identify the cursor node's `{` and its matching
`}`. With no way to mark a character, it had to invent a color —
`theme::brace_match_style`, bright red plus bold — and its own non-goal
N1 recorded the intended replacement: under a block caret the pair stops
being a *color* and becomes a *shape*, with the matching brace drawn as a
dimmer copy of the caret.

What spec 0193 does leave behind is the mechanism. `cut_segments`
(`render.rs:93-111`) removes a byte range from a row's syntax segments so
it can be re-emitted as a styled insertion at the same position.
Restyling one byte range of a row is exactly what a one-character caret
needs, and it is already built and tested.

### 3. The terminal's own cursor is already spoken for

Only the global command/search/rename row calls
`frame.set_cursor_position` (`render.rs:1021-1024`), which is spec 0147
G4's rule: the one hardware cursor belongs to the row the user is typing
into. A main-pane caret therefore has to be *drawn*, not delegated.

(Aside: `suspend`'s comment at `mod.rs:2013-2015` still claims protolens
"never" calls `set_cursor_position`. That has been false since spec 0147.
The `show_cursor()` call it guards is still correct — the last `draw()`
before a suspend may well have had no command line open — but the
comment's reasoning needs fixing whether or not this spec proceeds.)

### 4. A column is a coordinate the rest of the UI does not have

Every cue protolens shows about a node is currently row-granular, so the
only way to ask about a *part* of a row is to put the answer somewhere
permanent. The heat cue is the clearest case: the glyph in column 0 and
the ` [current/best]` suffix (`render.rs:811-848`) are all the room there
is, so the score is compressed into two numbers. With a caret there is a
place to ask from — a caret resting on the cue can spell the score out in
full in the command/message row, which costs no columns at all when the
caret is elsewhere.

### 5. A row is drawn from three sources, in three coordinate systems

This is the fact the whole specification has to respect, and it is not
obvious from looking at a screenshot:

| zone | source | panned? |
|------|--------|---------|
| heat glyph, column 0 | `spans.insert(0, ...)` (`render.rs:836`) | no |
| fold margin + text | `row_content` = `fold_margin` + `row_text[body_start..]` | yes |
| heat suffix | `spans.push(suffix)` (`render.rs:835`) | no |

`row_text` (`render.rs:322-337`) is the row's own text *including* its
leading indentation; `fold_margin_of` (`render.rs:348-355`) reports the
byte offset `indent_len` at which the margin's coverage ends, and the
margin is exactly `indent_len + FOLD_FIELD_WIDTH` columns wide. So a
text character at column `c` is drawn at screen column
`1 + FOLD_FIELD_WIDTH + c - pan_offset`, and the two chrome zones do not
move with the pan at all.

## Goals

- **G1.** The cursor is drawn as a vim-style **block caret** over exactly
  one character of one row, not as a full-row highlight.
- **G2.** When the caret is on a brace, its match is drawn as a *dimmer*
  caret. Spec 0193's red brace pair goes away.
- **G3.** The caret moves horizontally by one character, and its column
  survives vertical movement: a remembered desired column, clamped to
  each row without being forgotten, with one sticky special case (S5).
- **G4.** The full-row highlight survives as the **selection** cue only,
  so the two stop sharing a modifier and can be told apart on screen.
- **G5.** The caret's column is a first-class piece of state that other
  parts of the UI may consult — the motivating case being a detailed heat
  score in the command/message row when the caret rests on the heat cue.
- **G6.** Horizontal panning (spec 0113 D24) and the fold gutter (spec
  0193 S1) never desynchronize the caret from the character it names.
- **G7.** The caret stays **findable**. One inverted cell on a full
  screen must not be something the user has to hunt for after a jump.

## Non-goals

- **N1.** Character-granular *selection*. `select_anchor`/`select_end`
  stay line indices (`mod.rs:862-865`), and the clipboard keeps copying
  whole rows.
- **N2.** Editing. This is a reading caret; nothing about it implies an
  insert mode.
- **N3.** Using the terminal's hardware cursor — see Background 3.
- **N4.** A vim operator-pending language (`dw`, `ciw`, `f<char>`). This
  spec adds a position and two motions, not a grammar.
- **N5.** Wrapping the caret to the previous/next row at a row's ends.
  vim's default `whichwrap` does not let `h`/`l` wrap, and neither does
  this.
- **N6.** A *semantic* sticky column — one that keeps the caret on the
  same kind of token (field name, value, cue) across vertical movement.
  It is the more interesting idea and is recorded in A7, but it is a
  different feature and needs its own spec.
- **N7.** A caret in the override or manage panes. Those are lists of
  whole entries; the full-row highlight is the right cue there and stays.

## Specification

### S1. The cursor gains a column

A new field beside `cursor_footer`:

```rust
cursor_column: usize,
```

indexing **characters** — `char`s, not graphemes, each assumed to occupy
exactly one terminal cell; see A5 — of the row's **caret track**, which
is `row_text` followed by the row's heat suffix
when it has one. Not `row_content`, and not a screen column: the gutter's
width and the pan offset are display concerns that `render` already
resolves, and threading them into the cursor state would make the caret
move whenever the user pans (A4).

The track therefore has two zones, and the mapping to a screen column
differs between them (Background 5):

| zone | columns | screen column |
|------|---------|---------------|
| text | `indent_len .. row_text.chars().count()` | `1 + FOLD_FIELD_WIDTH + column - pan_offset` |
| suffix | the heat suffix's own characters | fixed at the row's right end, never panned |

`set_cursor` (`navigation.rs:151-155`) is the sole mutation path for
`self.cursor` and already resets `cursor_footer`; it resets the column to
the new row's first reachable one (S3), so a node-level jump lands at the
start of the row's text. Vertical movement instead keeps a separate
*desired* column and clamps, per S5 — the two rules differ and both are
needed.

### S2. The caret is drawn by restyling one character

Using spec 0193's mechanism: cut the caret character's byte range out of
the row's segments and re-emit it as a styled insertion carrying
`theme::caret_style`, which is `Modifier::REVERSED` over whatever the
character's syntax color already was. In the suffix zone the suffix is a
single span already, so it is split rather than cut.

The row-wide loop at `render.rs:855-859` splits in three:

| cue          | today                | under this spec                |
|--------------|----------------------|--------------------------------|
| caret        | whole row `REVERSED` | one character `REVERSED`       |
| caret's row  | (same thing)         | `theme::cursor_row_style` (G7) |
| selection    | whole row `REVERSED` | unchanged                      |

`theme::cursor_row_style` is a dim background applied to the whole drawn
row, chrome included — vim's `cursorline`. It is what keeps the caret
findable (G7), and it is visibly weaker than the selection's full
reverse, which is what keeps G4's two cues distinguishable. A blinking
caret would answer G7 too, and is rejected in A11.

A row whose track is empty — a blank row — draws the caret on a synthetic
trailing space, as vim does at end-of-line.

### S3. Where the caret may rest

The rule, stated once so that the two asymmetric limits stop looking
arbitrary:

> **The caret may rest on anything that carries information. It may not
> rest on a control.**

Leftward, the caret stops at the row's **first non-blank character** —
column `indent_len`. Everything left of it is the fold margin, and the
fold marker is a control: `mouse.rs`'s hit test toggles a fold when it is
clicked, and a caret resting there would invite an "activate" key this
spec does not want (N4). This limit is exactly vim's `^`, which is a
rule users already know.

Rightward, the caret runs to the last character of the **heat suffix**
when the row has one, and otherwise to the last character of the text.
The suffix is information — it is a rendered fact about the node, and
letting the caret reach it is what makes S8 possible.

The heat *glyph* in column 0 stays unreachable: it is left of the text
origin, and the suffix already gives the caret somewhere to stand to ask
about the cue.

### S4. The matching brace becomes a dimmer caret

`theme::brace_match_style` (spec 0193 S2) is replaced by
`theme::caret_match_style`: the caret's own treatment at lower contrast,
with no hue of its own. The pair then reads as one shape at two
intensities rather than as a color the theme has to justify, and it stops
competing with the heat cue's red for meaning.

The rule for *which* brace is matched is inherited unchanged from spec
0193's `cursor_brace` (`render.rs:384`), including its bracketed-node
test.

`%` jumps between the two. Because both braces belong to the cursor node
— the `{` on its header line, the `}` on its footer — `%` is exactly
"flip `cursor_footer`, and put the column on the brace". It is a no-op
with a message on an unbracketed node.

### S5. Vertical movement keeps an absolute desired column

A remembered `desired_column`, set by every *horizontal* movement and
left alone by every vertical one, clamped into each new row's reachable
range (S3) without being forgotten. This is vim's rule, unchanged.

The one addition: **if the caret is sitting exactly on the first
non-blank character, vertical movement keeps it on the first non-blank
character.** Since that is also the leftmost reachable column (S3), the
case is easy to detect and cannot be reached by accident. It is vim's
`'startofline'` behavior, and it is what makes `j`/`k` usable for
scanning down a message whose indentation changes on nearly every row.

Explicitly *not* the rule: a column measured as an offset from the
indentation. See A6.

### S6. Key bindings

`h`, `l`, `Left` and `Right` are all four bound to tree navigation today
(`key_dispatch.rs:489-534`). They are also vim's character motions, and
handing the caret the arrows while the letters kept folding subtrees
would be backwards from every user's muscle memory (A7). So the letters
move to the caret, and tree navigation moves up one shift level — which
protolens has already half-built, since `J`/`K` are sibling moves and
`Shift-Down`/`Shift-Up` alias them (spec 0126 G2).

The organizing rule: **unshifted moves in the text, shifted moves in the
tree, `z` folds.**

| key | meaning | was |
|-----|---------|-----|
| `h` / `Left` | caret one character left (stops at first non-blank) | fold, then parent |
| `l` / `Right` | caret one character right (stops at end of cue) | unfold, then first child |
| `0` / `^` | caret to first non-blank | — |
| `$` | caret to last reachable column | — |
| `%` | jump to matching brace (S4) | — |
| `H` / `Shift-Left` | move to parent | `H`: fold all siblings |
| `L` / `Shift-Right` | move to first child | `Shift-Right`: unfold all siblings |
| `J` / `Shift-Down` | next sibling | unchanged |
| `K` / `Shift-Up` | previous sibling | unchanged |
| `Backspace` | move to parent (alias) | — |
| `Space` | toggle fold | unchanged |
| `za` / `zc` / `zo` | toggle / close / open this node's fold | bare `z` toggled |
| `zA` / `zC` / `zO` | the same for all siblings | `H` / `Shift-Left` / `Shift-Right` |
| `Ctrl-Left` / `Ctrl-Right` | pan | unchanged |

Two costs are worth naming rather than hiding. `h`/`l` change meaning for
every existing user; that is the price of the caret and there is no
version of this that avoids it. And bare `z` stops being a toggle,
because `z` becomes vim's fold prefix — `Space` is kept precisely so the
common case still costs one key, and the `gg` chord
(`key_dispatch.rs`) is the precedent for the prefix machinery.

Note that `h`/`l` lose their *fold* duty entirely, not just their
parent/child duty: today `h` on an open foldable node folds it and only
moves to the parent on a second press (the nvim-tree pattern). Under this
spec `H` moves to the parent immediately, and folding is `zc`/`Space`.

### S7. A click sets the column

`mouse.rs:368-383` resolves a click to `(node, cursor_footer)` and
ignores the x coordinate. It now inverts S1's mapping: a click in the
text zone gives `column = x - 1 - FOLD_FIELD_WIDTH + pan_offset`, a click
in the suffix zone gives the suffix column directly, and a click left of
the first non-blank clamps to it (S3) rather than being rejected.

The fold marker's own hit test runs first and is unchanged, so clicking
the marker still toggles the fold and does not move the caret.

A click also sets `desired_column`, exactly as a horizontal key would.

### S8. A search hit puts the caret on the match

`jump_to_match` (`override_select.rs:873-898`) currently reports *which
node* matched and discards *where*. It gains the match's byte offset
within the node's header line, and `jump_to_match`'s caller converts it
to a character column and sets the caret there, clamped into the
reachable range.

This is what vim does, and it is what makes the caret worth having for
search alone rather than only for S9.

Sequencing note: this touches the same function as the pending fix for
the quadratic backward walk (the eager `unwrap_or(self.last_node())` at
`override_select.rs:882`). The two changes collide and should be
sequenced deliberately, not merged blind.

### S9. What the column is for

Not required for G1-G7, but the reason a column is worth having: with the
caret resting on the heat suffix, the command/message row can describe
the inference in full — the competing types and their scores — instead of
the two numbers that fit in the suffix. Nothing about S1-S4 presumes
this; it is the first consumer, and it is what distinguishes this work
from cosmetics.

### S10. The jumplist records the whole cursor position

`record_jump` pushes a bare node index today (`key_dispatch.rs:181-184`,
`back_stack: Vec<usize>`), so `Ctrl-o` returns to the right node but not
to the character the user was reading — and, already today, not to the
right *half* of a bracketed node either, since `cursor_footer` is not
recorded and a jump back from a node's `}` lands on its `{`.

Both stacks become stacks of a whole cursor position — node,
`cursor_footer`, `cursor_column` — and a restore reinstates all three.
This is vim's behavior, and it fixes the `cursor_footer` loss as a side
effect rather than leaving a known-wrong half-coordinate in place.

A stored column can go stale: the row's text changes when a fold toggles
(spec 0193's `{ ... }` summary) or when an override resplices the
subtree. The restore therefore clamps into the row's reachable range
(S3), which is the same clamp S11 already applies on a fold toggle. It
is not stored as a desired column: a jump back restores a position, and
`desired_column` follows it as it would after any other horizontal
move.

### S11. Incidental corrections

- A fold toggle changes the row's text (spec 0193's `{ ... }` collapse
  summary is inserted into `row_text`). The column is clamped into the
  new track, and `desired_column` is not disturbed.
- `mod.rs:2013-2015`'s comment claiming protolens never calls
  `set_cursor_position` is corrected while the surrounding code is being
  read (Background 3). The `show_cursor()` it guards stays.

## Alternatives considered

### A1. A VS Code-style insertion caret

A thin bar between two characters. Rejected on the recorded preference:
protolens's cursor designates a *thing* — a node, and now a character —
rather than a gap where typing would land, and there is no typing. A block
caret says "this one"; a bar says "here, between".

### A2. Keep the full-row highlight and add an underline for the character

Rejected: the row highlight is the selection's cue (G4), and a row that is
both the cursor's and inside a selection would then be indistinguishable
from a row that is only selected. G7's dim `cursor_row_style` is the
weaker treatment that does not have this problem.

### A3. Drive the hardware cursor and let the terminal draw it

Free, and gets the user's own cursor shape and blink. Rejected — see
Background 3: there is exactly one, spec 0147 G4 gives it to the command
row, and it also vanishes when the terminal loses focus, which would make
the reading position disappear.

### A4. Store the column as a screen column

Simpler to draw. Rejected: the caret would then slide across characters
when the user pans (spec 0113 D24) or when spec 0193's fold gutter
changes width under it, and every consumer of G5 would have to undo the
display transform to find out what the caret is actually on.

### A5. Index graphemes, and measure cell width

A Rust `char` is one Unicode scalar, and the specification's simplifying
assumption is that one of them is one terminal cell. Two ways that is
untrue:

- What the eye calls one character can be two `char`s — `e` plus a
  combining acute — so stepping by `char` takes two presses to cross one
  glyph, and rests in between on half a letter.
- One `char` can occupy two cells, as CJK does, so the arithmetic that
  converts a column to a screen position counts it once where the
  terminal counts it twice.

Deferred, not rejected, because the blast radius is small and bounded.
Field names in prototext are ASCII, so neither case can arise outside a
string *value*. And S2 draws the caret by cutting a byte range out of the
row's segments and re-emitting it as its own span, so **the caret is
drawn in the right place regardless of width** — ratatui measures the
spans. The one-cell assumption enters only the *arithmetic*: S1's
column-to-screen mapping, and its inverse in S7. A row containing a
wide character therefore misplaces a mouse click by one column per wide
character to the click's left, and nothing else. That is a defect worth
having later rather than a `unicode-segmentation` plus `unicode-width`
dependency now.

The trap that is *not* deferred, and must be got right from the start, is
bytes versus chars. `é` is two bytes, one `char`, one cell — so a column
must never be used to slice a `str` directly. `marker_column`
(`render.rs:76-83`) computes its `indent_len` in bytes and is correct
only because indentation is ASCII; the caret has no such excuse and
converts through `char_indices` every time.

The test plan is phrased in terms of "one visible unit" rather than "one
`char`" so that the stepping and measuring functions can be swapped later
without rewriting the tests.

### A6. An indentation-relative vertical column

Considered and rejected: vertical movement would preserve the caret's
offset from the row's first non-blank rather than its absolute column.

Rejected on two grounds. It is what no editor does — vim, emacs and VS
Code all keep an absolute desired column, and there is no option name to
explain the behavior with. And it would be actively unpleasant *here*:
prototext is a tree, so nearly every row enters or leaves a message and
the indentation changes under the caret on most `j` presses. The caret
would slide sideways almost every keystroke. In source code, where indent
is stable for runs of lines, the rule would be nearly invisible; in this
document it would be constant jitter.

S5's sticky first non-blank captures the useful part of the intuition —
scanning down a message from the start of each row — at one specific
column where it cannot jitter.

### A7. A *semantic* sticky column

The caret remembers what *kind* of thing it is on — field name, `:`,
value, heat cue — and vertical movement keeps it on the same kind. `j`
with the caret on a value would then walk down the values of a repeated
field, ignoring how the names shift width.

This is the version that is genuinely better than a text editor can
manage, because protolens knows each row's structure and vim does not.
It is also a different feature: it needs the syntax segments exposed to
the cursor layer, and it needs an answer for rows that have no
corresponding kind. Recorded as N6 and left for its own spec.

### A8. Arrows to the caret, letters to the tree

The cheap version of S6: `Left`/`Right` become caret motion and `h`/`l`
keep fold/parent/child, so nothing else has to move. Rejected: it is
backwards from vim, where `h`/`l` *are* the character motions and the
arrows are their synonyms. A user pressing `l` to move right would
unfold a subtree. It costs nothing today and is wrong permanently.

### A9. Other homes for parent/child

- **`[[` / `]]`** — vim's previous/next section, and the `g` chord shows
  the machinery exists. Rejected on ergonomics: a two-key bracket chord
  is heavy on a Mac keyboard.
- **`-` / `Enter`** — netrw and oil.nvim use `-` for "go up". Rejected:
  `Enter` is already `open_smart_override_or_manage()`, which is a good
  binding to keep, and `-` alone is asymmetric.
- **`<` / `>`** — vim's dedent/indent operators, and in prototext
  indentation *is* depth, so the mnemonic is real rather than
  arbitrary. The strongest runner-up; `H`/`L` win only because they
  complete a family protolens has already started with `J`/`K`.

### A10. Keep bare `z` as the fold toggle

Would avoid S6's one genuinely gratuitous change. Rejected because
fold-all-siblings then has nowhere to go: `H`, `Shift-Left` and
`Shift-Right` are all being reclaimed for tree motion, and inventing a
one-off key for it is worse than adopting vim's `z` prefix wholesale.

### A11. Blink the caret instead of cueing its row

The other way to answer G7, and the one the eye is most sensitive to:
motion beats contrast, it costs no columns, it needs no new theme entry,
and it cannot be confused with the selection. It is also what terminals
do for their own cursor, so it needs no explaining.

Rejected, on one structural objection and two smaller ones.

**It converts an event-driven program into a timed one.** protolens's
main loop consumes `AppEvent`s (`event.rs:24-50`); the reader thread's
200 ms `event::poll` is a shutdown-latency bound, not a frame clock, and
every real keypress wakes it immediately. A blink means a periodic tick
and a redraw at roughly 2 Hz *forever*, including while the user reads a
static screen — which is exactly the redraw discipline specs 0190-0192
spent their effort establishing. Paying that permanently to solve a
findability problem that a static background solves for free is the wrong
trade.

**The caret is absent half the time.** A paused screen, a screenshot, a
screen recording or a terminal that has just been scrolled back may show
no caret at all. For a *reading* tool whose whole point is "where am I in
this document", a position cue that periodically isn't there is a strange
choice.

**Blink is a setting users hold opinions about.** Many people disable
cursor blink system-wide, for attention or vestibular reasons, and an
application-drawn blink cannot read that preference — the terminal's
setting applies to the hardware cursor protolens is not using (N3).

A bounded variant is worth keeping in mind if the static cue proves
insufficient in practice: **flash the caret for ~1 s after a jump**, then
rest solid. That is a timer with an end, not a frame clock, and it puts
the motion exactly where the findability problem is (after `n`, `t`,
`Ctrl-o`) rather than everywhere. It is not specified here because the
row cue should be tried first.

## Test plan

Throughout, "one visible unit" rather than "one `char`" — see A5.

1. The caret covers exactly one visible unit, and the rest of its row is
   drawn in its ordinary syntax colors.
2. A selected row is still fully reversed; the caret's row carries
   `cursor_row_style`; a row that is both is distinguishable from one
   that is only selected.
3. `h`/`l` move by one unit and stop at both ends of the reachable range
   without wrapping (N5).
4. The caret cannot be moved left of the row's first non-blank character
   at any `--indent` width, including 0 and 1 where spec 0193 puts the
   marker in the reserved field instead of the indentation.
5. The caret can be moved onto every character of the heat suffix, and
   not onto the heat glyph in column 0.
6. Vertical movement across a short row and back restores the original
   column — vim's desired-column rule, which a naive clamp loses.
7. A caret on the first non-blank stays on the first non-blank across
   vertical movement between rows of different indentation (S5), and a
   caret one column to its right does not.
8. The caret stays on the same character across a horizontal pan and
   across a fold-gutter width change (G6). The heat suffix does not move
   under the pan, and the caret does not move within it.
9. A node-level jump (override, `t`) puts the caret on the row's first
   non-blank.
10. A search hit puts the caret on the first character of the match
    (S8), including when the match is in the row's indentation, where it
    clamps to the first non-blank.
11. The matching brace carries `caret_match_style` and the caret's own
    brace carries `caret_style`; no span anywhere carries a red brace
    style — spec 0193 S2's test rewritten rather than deleted.
12. `%` moves between a node's `{` and `}`, flipping `cursor_footer`,
    and reports a message on an unbracketed node.
13. A click sets the column; a click on the fold marker toggles the fold
    and leaves the caret alone; a click in the gutter clamps to the first
    non-blank (S7).
14. On a blank row and past a short row's end, the caret is drawn on a
    synthetic trailing space rather than vanishing.
15. `h`/`l`/`H`/`L`/`z`-prefix all dispatch to what S6's table says, and
    no reclaimed key retains its old behavior.
16. On a row whose string value contains a multi-byte character, one
    press crosses it, and no column-to-byte conversion panics on a
    character boundary (A5's non-deferred half).
17. `Ctrl-o` after a jump restores the node, `cursor_footer` *and* the
    column; a `Ctrl-o` to a row that has shrunk since (a fold toggled
    under it) clamps rather than pointing past the row's end (S10).

## Open questions

**Q1. When is the Unicode dependency taken?** A5 defers graphemes and
double-width cells, and A5's own analysis says the only visible symptom
until then is a mouse click misplaced by one column per wide character
to its left. The trigger for revisiting is therefore a real payload that
reads badly — a CJK string value, or one with combining marks — and not
a schedule.

## Measured outcome

(To be filled in on implementation.)
