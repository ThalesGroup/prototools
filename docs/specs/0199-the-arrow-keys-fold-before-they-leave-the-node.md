<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0199 — the arrow keys fold before they leave the node

Status: implemented
Implemented in: 2026-07-28
App: protolens
Refs: docs/specs/0113-protolens-tui-refinements.md (D24, horizontal pan),
      docs/specs/0114-protolens-command-line.md (the command line's
        `Alt-b`/`Alt-f` word motion, reused here),
      docs/specs/0126-protolens-sibling-navigation.md (G2, the shifted
        family),
      docs/specs/0133-protolens-annotation-toggle.md (G3, the `a` key),
      docs/specs/0142-protolens-footer-line-cursor.md (G6.2, folding
        under the cursor),
      docs/specs/0185-protolens-override-preview-overlay.md (S5, the
        override pane's `Alt`-arrow escape hatch),
      docs/specs/0194-the-cursor-is-a-caret.md (S1, S5, S6, S10 —
        amended here)

## Background

Spec 0194 S6 reassigned `h`/`l`/`Left`/`Right` from tree navigation to
caret motion, under the organizing rule *unshifted moves in the text,
shifted moves in the tree, `z` folds*. It took their fold duty away in
as many words:

> Note that `h`/`l` lose their *fold* duty entirely, not just their
> parent/child duty: today `h` on an open foldable node folds it and only
> moves to the parent on a second press (the nvim-tree pattern). Under
> this spec `H` moves to the parent immediately, and folding is
> `zc`/`Space`.

In use that has a dead spot in it, and the dead spot is at the pane's
most common resting position.

`set_cursor` calls `reset_caret_column` (`navigation.rs:156-161`), so
*every* node-level move — `j`, `k`, `H`, `L`, `J`, `K`, a search hit, a
click, a jumplist return — leaves the caret on the row's first non-blank
character. That is also `caret_bounds`' leftmost reachable column
(spec 0194 S3). So in the state the pane is in after almost every
keystroke, `h` and `Left` do nothing at all: `caret_left` clamps to a
column it is already on.

A key that is inert in the common case is worse than a key with two
meanings. The fix is the pre-caret behavior, which is what every tree UI
of this shape does — nvim-tree, `jless`, the ARIA tree-view pattern: at
the left edge of a row, leftward means *close this node*, and only then
*go to my parent*.

### Why a column test is not enough

The obvious form of that fix — "if the caret is at the leftmost
reachable column, fold" — is wrong, and it is wrong for a reason that
only shows up in use.

The caret arrives at the leftmost column two very different ways. The
user can put it there, with `^` or by walking left. Or a *vertical* move
can push it there: `carry_caret` (`navigation.rs:220-227`) clamps the
desired column into the new row's range, and the new row may be shorter
or more deeply indented than the one before it, in which case the caret
lands on an end it never asked for. Under a bare column test, a `j` onto
a short row followed by an `h` folds a node the user was only passing
over. The keystroke that was inert becomes a keystroke that is worse
than inert.

So the caret needs to remember not just *where* it is but *whether it
chose to be there*. That is the anchor, S1. Note that the existing
`desired_column` cannot answer the question: `carry_caret`'s sticky
branch deliberately pins `cursor_column` to each row's first non-blank
while leaving `desired_column` at its pre-move value, so the two
disagree in exactly the case where the position *is* voluntary.

### The right edge is a tree edge too

Once the anchor exists, the same reasoning applies at the other end of
the row, and it pays for itself there.

A message's opening brace is the **last character of its header row**.
So "walk right until you run out of row, then keep going" is,
geometrically, *go through the brace into the first child* — the exact
mirror of "walk left until you run out of row, then keep going" being
*close this node and go up to the parent*. The tree is laid out on
screen in the direction the arrow keys already point; the keys just had
to be told to keep going.

### `Shift` on an arrow is not `Shift` on a letter

Spec 0194 S6 treated `H` and `Shift-Left` as one binding, on the
reasoning that `h`/`Left` are aliases so their shifted forms are too.
That reasoning is backwards for the letter. `Shift-h` *is* `H` — one
gesture producing one character — whereas `Shift-Left` is the Left key
with a modifier the terminal reports separately. Making `Shift-Left`
mean something the plain `Left` does not is asking the user to notice
whether a finger is resting on a shift key while pressing an arrow, and
that is not a distinction anyone tracks.

So the arrows shed the distinction: `Left` and `Shift-Left` are the same
key. But `H` is a character in its own right and keeps a meaning of its
own — and since `Shift-Left` and `H` are the same gesture, that meaning
is what `Shift-Left` does too.

### Two smaller defects in the same table

`key_dispatch.rs:619` binds `KeyCode::Char('a')` with no modifier guard,
so `Ctrl-a` toggles the annotation display (spec 0133 G3). The
neighboring `i` arm at `:628` guards with `modifiers.is_empty()`
precisely to keep `Ctrl-i` free for the jumplist; `a` was never given the
same treatment.

And `Alt-Left`/`Alt-Right` do nothing in the main pane, while the command
line has had word motion on them since spec 0114 (`command_line.rs:108-119`,
`prev_word_boundary`/`next_word_boundary`). A row of prototext is
whitespace-delimited in exactly the way that motion assumes.

## Goals

- **G1.** The caret records whether its position at an end of the row
  was *chosen* or *arrived at by clamping*, and every key that acts
  differently at an end reads that record rather than the column alone.
- **G2.** A press that finds the caret at an end involuntarily adopts
  the position instead of acting on it. The second press acts.
- **G3.** Voluntarily at the row's first column, `h`/`Left` folds an
  expanded node; once it is folded, it moves to the parent.
- **G4.** Voluntarily at the row's first column, `l`/`Right` unfolds a
  folded node. Voluntarily at the row's last column, it moves to the
  first child. Anywhere else, both keys are the caret motion of spec
  0194 S6.
- **G5.** `Left` and `Shift-Left` are one key; `Right` and `Shift-Right`
  are one key. `H` and `L` — hence `Shift-Left` and `Shift-Right` — fold
  and unfold the whole sibling level, and do nothing else.
- **G6.** `Ctrl`-arrows keep panning. `Alt`-arrows move by word, with
  the command line's definition of a word.
- **G7.** Both anchors survive a vertical move, so `^` then `j` walks
  down the first non-blank of each row and `$` then `j` walks down the
  last column of each row.
- **G8.** A mouse click never leaves the caret anchored, wherever it
  lands.

## Non-goals

- **N1.** Reverting spec 0194 S6 generally. `0`, `^`, `$`, `%`, `Space`,
  the `z` fold family, `J`/`K`, the caret rendering and the jumplist are
  untouched except where S9's table says otherwise.
- **N2.** Reviving the pre-0194 root-level fallback, where `h` on the
  root with no parent folded all root-level siblings. Sibling folding is
  `H`/`L` and `zC`/`zO` now; a key whose meaning changes silently at the
  root is a surprise, not a shortcut.
- **N3.** Giving `H`/`L` a caret-column or anchor condition. They are
  fold commands, not motions — G5 — so there is no caret meaning for
  them to defer to and no state in which they would do something else.
- **N4.** Making `H`/`L` move the cursor. Their previous parent/child
  duty moves onto `h`/`l` at the anchors, and nothing replaces it: a key
  that both refolds a whole level *and* relocates the cursor gives the
  user two things to undo.
- **N5.** Changing what `Alt`-arrows do inside the override pane. There
  they pan the main pane behind the focus lock (spec 0185 S5) and stay
  as they are; S8 binds the main pane's own `Alt`-arrows, which were
  unbound.
- **N6.** A configurable binding scheme. The table in S9 is the table.

## Specification

### S1. The caret anchor

```rust
/// Whether the caret's position at an end of its row was chosen or
/// merely arrived at (spec 0199 G1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum CaretAnchor {
    /// Somewhere in the middle of the row, or pushed onto an end by a
    /// vertical move's clamp rather than by a key that meant it.
    Free,
    /// Deliberately on the row's first non-blank character.
    Home,
    /// Deliberately on the row's last reachable column.
    End,
}
```

`App` gains `caret_anchor: CaretAnchor`, initialized to `Home` — the
first frame's cursor is at the first node's first non-blank, and it got
there by the same node-level move that every later one does.

Three ways to acquire an anchor, and they are exhaustive:

1. **Declared.** `reset_caret_column` (so every `set_cursor`, i.e. every
   node-level move) and `caret_to_line_start` (`0`/`^`) set `Home`;
   `caret_to_line_end` (`$`) sets `End`.
2. **Earned.** The caret motions of S5, S6 and S8 call `settle_anchor`
   after moving, which derives the anchor from where they landed.
3. **Forfeited.** Everything that places the caret without the user
   having aimed at a row *end* sets `Free`: a mouse click (G8), a
   jumplist return, `%`, a search landing.

```rust
/// Derive the anchor from the column a caret motion just reached: an
/// end reached by walking there is as deliberate as one reached by
/// `^`/`$` (spec 0199 S1, rule 2).
fn settle_anchor(&mut self) {
    let (first, last) = self.caret_bounds();
    self.caret_anchor = if self.cursor_column == first {
        CaretAnchor::Home
    } else if self.cursor_column == last {
        CaretAnchor::End
    } else {
        CaretAnchor::Free
    };
}
```

On a one-column row `first == last` and `Home` wins. That is the useful
choice: such a row is a `}` footer or an empty line, where folding is
the plausible intent and descending is not.

A vertical move does **not** change the anchor — that is exactly what
makes it a record of intent rather than of position, and it is what S3
consumes.

### S2. Why the anchor is not derived from `desired_column`

Tempting, and wrong. `carry_caret`'s sticky branch sets `cursor_column`
without touching `desired_column`, so after `^` `j` `j` the caret is
voluntarily at Home on the third row while `desired_column` still holds
the first row's indent. `desired_column == cursor_column` would report
that position as involuntary — the one case it most needs to get right.

The two quantities answer different questions: `desired_column` is *what
column to aim for on the next row*, the anchor is *whether the user
chose this end*. S3 makes the split explicit by giving each of the three
anchors its own carry rule, so `desired_column` is consulted only in the
`Free` case, which is the only case it was ever meaningful in.

### S3. Both ends are sticky across a vertical move

```rust
fn carry_caret(&mut self) {
    let (first, last) = self.caret_bounds();
    self.cursor_column = match self.caret_anchor {
        CaretAnchor::Home => first,
        CaretAnchor::End => last,
        CaretAnchor::Free => self.desired_column.clamp(first, last),
    };
}
```

`move_down`/`move_up` lose their `sticky` local and the argument
(`navigation.rs:386`, `:404`): the anchor already holds what that
boolean was computing, and holds it more accurately, because it
distinguishes the two ways of being at the first column that
`caret_at_first_non_blank()` conflates.

`End` becoming sticky is new (spec 0194 S5 had only the `Home` case,
vim's `'startofline'`). It is the direct consequence of G4 giving the
last column a meaning: without it, `$` `j` `l` would need an extra press
to re-anchor before it could descend, and `$` `j` `j` would drift off
the ends of rows for no reason the user could see. It is also what vim
does after `$` (`curswant = MAXCOL`).

`caret_at_first_non_blank` is deleted. Its only two callers were the
`sticky` computation above and the original draft of S5/S6, and both now
compare against `caret_bounds()` directly alongside the anchor — keeping
a helper that answers half of a two-part question invites using it for
the whole one.

### S4. One predicate pair for the fold state

```rust
/// The cursor node is foldable and currently open — the state in which
/// a leftward key folds instead of moving (spec 0199 G3).
fn cursor_expanded(&self) -> bool {
    self.has_children(self.cursor) && !self.folded.contains(&self.cursor)
}

/// The cursor node is currently folded — the state in which a rightward
/// key unfolds instead of moving (spec 0199 G4).
fn cursor_folded(&self) -> bool {
    self.folded.contains(&self.cursor)
}
```

`has_children` is the *bracketed-node* test, not the has-descendants
test — see its doc comment (spec 0142): an empty-but-bracketed message
is foldable and must fold, even though it has no children. That is
already the property this spec needs, which is why the predicate reuses
it rather than testing `first_child`.

### S5. `caret_left`

```rust
pub(super) fn caret_left(&mut self) {
    self.clamp_caret_column();
    let (first, _) = self.caret_bounds();
    if self.cursor_column > first {
        self.cursor_column -= 1;
        self.desired_column = self.cursor_column;
        self.settle_anchor();
        return;
    }
    if self.caret_anchor != CaretAnchor::Home {
        // G2: at Home without having chosen to be — adopt the position
        // rather than act on it.
        self.caret_anchor = CaretAnchor::Home;
        self.desired_column = self.cursor_column;
        return;
    }
    self.parent_move();
}
```

`clamp_caret_column` runs first, so the comparison is against the row's
real current bounds rather than a stale column (spec 0194 S11).

The adopting branch is not a wasted keystroke even though the caret does
not move. It collapses `desired_column` onto the real column, so a
subsequent `j`/`k` no longer springs back to the column the caret was
carrying — which is vim's own rule for a horizontal motion, and is why
this reads as a motion rather than as a swallowed press.

The final delegation is to `parent_move`, i.e. to the whole of S7
including its fold-first branch and its `no parent` message. G3's "once
it is folded, it moves to the parent" is therefore not separately
implemented; it is what `parent_move` does once the node is folded.

### S6. `caret_right`

```rust
pub(super) fn caret_right(&mut self) {
    self.clamp_caret_column();
    let (first, last) = self.caret_bounds();
    if self.caret_anchor == CaretAnchor::Home
        && self.cursor_column == first
        && self.cursor_folded()
    {
        self.toggle_fold(self.cursor);
        return;
    }
    if self.cursor_column < last {
        self.cursor_column += 1;
        self.desired_column = self.cursor_column;
        self.settle_anchor();
        return;
    }
    if self.caret_anchor != CaretAnchor::End {
        self.caret_anchor = CaretAnchor::End;
        self.desired_column = self.cursor_column;
        return;
    }
    self.first_child_move();
}
```

The unfold branch is tested **before** the move branch and is gated on
all three of anchor, column and fold state. Each part earns its place:

- **The fold state.** On an expanded node there is nothing to unfold, so
  the key is plain caret motion — otherwise `l` at Home would be dead
  wherever `h` at Home is live, which is the defect this spec exists to
  remove, transplanted.
- **The column and the anchor.** A folded node's header row still
  carries a full line of text (`Name { ... }`). A key that unfolds from
  anywhere on it makes that text unreachable by caret, and one that
  unfolds from an involuntary Home unfolds a node the user was passing
  over.

Order matters on a one-column row, where `first == last`. The unfold
branch runs first, so `l` on a folded footer-like row opens it; if it is
not folded, control reaches the `End`-adopting branch and the row's
single column can still be anchored and descended from.

### S7. `parent_move` folds first, `first_child_move` unfolds first

```rust
pub(super) fn parent_move(&mut self) {
    if self.cursor_expanded() {
        self.toggle_fold(self.cursor);
        return;
    }
    if let Some(parent) = self.tree[self.cursor].parent {
        self.record_jump();
        self.set_cursor(parent);
    } else {
        self.message = "no parent".to_string();
    }
}

pub(super) fn first_child_move(&mut self) {
    if self.cursor_folded() {
        self.toggle_fold(self.cursor);
        return;
    }
    let Some(child) = self.tree[self.cursor].first_child else {
        self.message = "no children".to_string();
        return;
    };
    self.record_jump();
    self.set_cursor(child);
}
```

Both doc comments currently record the *removal* of this behavior in as
many words. They are rewritten, not appended to — a comment that says
the opposite of the code is worse than no comment.

`first_child_move`'s unfold branch used to unfold **and** descend in one
press, on the reasoning that "a folded node's children are not on
screen, so there would otherwise be nothing to move to". That is
answered by the fact that after the first press there is. What the split
buys is that `l` at End and `h` at Home become inverses: closing a node
and reopening it returns the tree and the cursor to where they were,
which the one-press form made impossible.

After the descent the child is reached through `set_cursor`, so the
anchor is `Home` and the caret sits on the child row's first non-blank —
the child's *name*, which is what the user descended to read. This is
deliberately not `End`; see Q2.

### S8. `Alt`-arrows move by word

Two motions on `App`, over the cursor row's text:

```rust
pub(super) fn caret_word_left(&mut self);
pub(super) fn caret_word_right(&mut self);
```

Both clamp into `caret_bounds()`, assign `desired_column`, and call
`settle_anchor` — so walking left by words onto the first non-blank
anchors Home exactly as walking there by characters does.

The word definition is the command line's, not a second one. Its two
methods (`command_line.rs:414`, `:435`) are whitespace-delimited scans
over a `Vec<char>` and an index; they are refactored into free functions
taking `(&[char], usize)`, with the existing methods delegating. Two
call sites and one definition of a word — a main pane that broke words
differently from the `:` prompt on the same screen would be a defect,
not a feature.

The scan runs over `row_text(DisplayRow::Committed(self.cursor_line()))`,
which is the same text `caret_bounds` measures its first component from.
Columns past the row's own text belong to the heat suffix (spec 0194 S3)
and are reached by clamping, not by scanning.

### S9. The amended rows of spec 0194 S6's table

| key | spec 0194 S6 | here |
|-----|--------------|------|
| `h` / `Left` / `Backspace` | caret one character left | caret one character left; at Home, fold then parent |
| `l` / `Right` | caret one character right | caret one character right; at Home, unfold; at End, first child |
| `Shift-Left` | move to parent | *as `Left`'s shifted twin* — see `H` |
| `Shift-Right` | unfold **and** move to first child | *as `Right`'s shifted twin* — see `L` |
| `H` / `Shift-Left` | move to parent | fold all siblings |
| `L` / `Shift-Right` | unfold **and** move to first child | unfold all siblings |
| `Ctrl-Left` / `Ctrl-Right` | pan horizontally | unchanged |
| `Alt-Left` / `Alt-Right` (and `Alt-h` / `Alt-l`) | *unbound* | caret one word left / right |
| `a` | toggle annotations | toggle annotations, **only** with no modifiers |
| `zC` / `zO` | fold / unfold all siblings | unchanged |

`Backspace` moves from `parent_move` to `caret_left`. Its old binding
was a protolens invention; in vim `<BS>` is a plain left motion in normal
mode, which is both what a user's fingers expect and what "`Left` is
`h`" implies.

`H` and `L` reuse `fold_all_siblings`/`unfold_all_siblings` unchanged —
the same functions `zC`/`zO` call. Both bindings stay: the chord is the
discoverable one, the shifted arrow is the fast one.

The organizing rule of spec 0194 S6 survives in amended form: *motion
happens in the text until the text runs out and then continues into the
tree; `Shift` widens a fold to the sibling level; `z` folds.*

### S10. Everything else that places the caret forfeits the anchor

Four sites set `caret_anchor = CaretAnchor::Free` (S1, rule 3):

- `mouse.rs::set_caret_from_click` — G8, and stated by the user as a
  rule rather than derived: a click expresses *where*, never *why*, so
  it must not arm a fold.
- `key_dispatch.rs::restore_cursor_pos` — the jumplist already declines
  to restore `desired_column` for the same reason (its doc comment:
  "a jump back reinstates a position"), and the anchor is the same kind
  of intent.
- `navigation.rs::jump_matching_brace` — `%` lands on a brace, which on
  a header row *is* the last column. Anchoring `End` there would make
  the next `l` descend, which is not what `%` promised.
- `override_select.rs::jump_to_match` — a search landing is a position,
  and a match at the end of a row is a coincidence.

`CursorPos` (spec 0194 S10) does **not** gain an anchor field. It records
where the cursor was, and the anchor is not part of where.

## Alternatives considered

### A1. A bare column test, with no anchor

The first draft of this spec. Rejected once `j` onto a short row
followed by `h` was seen to fold a node in passing; see Background. It
also cannot support G4's End behavior at all, since a caret clamped onto
a short row's last column would then descend.

### A2. Two booleans instead of a three-state enum

`at_home` and `at_end` can both be false, and can be made both true on a
one-column row, so the type would permit two states that mean nothing
and demand a tie-break the enum settles by construction.

### A3. Keep `H`/`L` as parent/child motion and put sibling folding elsewhere

What spec 0194 S6 does today, and what the first draft of this spec
kept. Rejected: it forces `Shift-Left` to differ from `Left`, which is
the distinction Background's third section argues nobody tracks. Once
`Shift-Left` must equal `H`, and `Left` must equal `h`, and `H` must not
equal `h`, the only consistent assignment is the one in S9.

### A4. Let `H`/`L` also move to the parent/first child after folding

Rejected as N4, on the user's reasoning: the point of the shifted key is
to normalize the *level* the user is looking at. Moving the cursor as
well means a press that both changes what is on screen and changes where
you are, with no way to have asked for only the first.

### A5. Make `Alt`-arrows the sibling fold instead of word motion

Would leave `H`/`L` free. Rejected: `Alt` is the word-motion modifier on
this project's own command line already, and a modifier that means
"wider unit of text" in one row of the screen and "fold" in another is
not a modifier, it is two.

## Test plan

Anchor mechanics:

1. **A vertical move onto a shorter row leaves the anchor `Free`**, even
   though the caret is at that row's last column. G1's whole point.
2. **`^` then `j` keeps the caret on each row's first non-blank**, and
   the anchor `Home`, across rows of differing indent. G7, and spec 0194
   S5's `'startofline'` rule preserved through the rewrite.
3. **`$` then `j` keeps the caret on each row's last column.** G7's new
   half.
4. **A click at the row's first non-blank leaves the anchor `Free`**, so
   the next `h` does not fold. G8.

`h`/`Left`:

5. **`h` away from an end moves one column and does not fold.** The
   spec 0194 behavior this must not break.
6. **`h` at an involuntary Home is consumed: anchor becomes `Home`,
   nothing folds, `desired_column` collapses onto the column.** G2.
7. **The next `h` folds an expanded node, and the cursor does not
   move.** G3's first press.
8. **A further `h` moves to the parent.** G3's second press.
9. **`h` at a voluntary Home on a *scalar* moves to the parent
   immediately.** `has_children` is false, so there is no fold to do
   first: one press, not two.
10. **`h` on the expanded root folds it; a further `h` reports `no
    parent`.** N2 — it must not fold the root's siblings.

`l`/`Right`:

11. **`l` at a voluntary Home on a folded node unfolds it and the cursor
    does not move.** G4.
12. **`l` at a voluntary Home on an expanded node moves the caret
    right.** The fold-state gate of S6.
13. **`l` on a folded node away from Home moves the caret right and does
    not unfold.** S6's column gate, which nothing else covers.
14. **`l` at an involuntary End is consumed; the next `l` moves to the
    first child.** G2 and G4 together.
15. **`$` then `l` moves to the first child in one press**, landing on
    the child's first non-blank with the anchor `Home`. S7's closing
    paragraph.
16. **`l` at End on a folded node unfolds instead of descending; a
    further `l` descends.** S7's split, from the `caret_right` side.
17. **`h` at Home undoes `l` at End**, returning both `folded` and
    `cursor` to their prior values. The inverse property S7 exists for.

`H`/`L` and the rest of the table:

18. **`H` folds every sibling at the cursor's level, at an arbitrary
    caret column and from a `Free` anchor, and does not move the
    cursor.** G5 and N3/N4 together.
19. **`L` unfolds every sibling, likewise.** G5's other half.
20. **`Shift-Left` and `H` produce identical state**, as do
    `Shift-Right` and `L`. The equality Background's third section
    asserts, pinned so it cannot drift apart again.
21. **`Ctrl-Left` still pans and does not move the caret.** G6, and the
    regression this table is most likely to cause.
22. **`Alt-Right` moves the caret to the end of the next word**, and
    `Alt-Left` back, agreeing with `command_line.rs`'s boundaries on the
    same text. G6 and S8's shared definition. `Alt-h`/`Alt-l` produce
    the same state, as the letters do everywhere else in the table.
23. **`Ctrl-a` does not toggle the annotation display, and `a` does.**
    The Background's smaller defect.
24. **`Backspace` moves the caret one column left**, not to the parent.
    S9.

Spec 0194's and the first draft's tests are the ones at risk: any that
press `Shift-Left` expecting a parent move, `Shift-Right` expecting a
descent, or `h`/`Left` at the start of a row expecting inertness assert
behavior this spec reverses, and are rewritten rather than added to.

## Open questions

**Q1. Should the folding press record a jump?** It does not — only the
press that actually moves does, via `parent_move`. A fold leaves the
cursor where it is, so there is no position to return to. That matches
`Space` and the `z` family, which do not touch the jumplist either.

**Q2. Should `l` at End leave the anchor at `End` on the child?** It
does not: `set_cursor` anchors `Home`, so the caret lands on the child
row's first non-blank and `l l l l` does not walk down the tree the way
`h h h h` walks up it. The asymmetry is deliberate — descending is done
in order to *read* the child, and its name is at the start of the row,
whereas ascending is done in order to *collapse*, where the cursor's
column is irrelevant. Revisit if repeated descent turns out to be a
common gesture; `j` is the natural key for it.

**Q3. What does `l` do at Home on a folded node with no children — an
empty bracketed message?** It unfolds it (S6 tests `cursor_folded`, not
`has_children`), revealing the node's own footer line. That is what
`Space` does there and is correct; noted only because
`first_child_move`'s `no children` message makes the two keys differ.

## Measured outcome

Implemented 2026-07-28. 502 protolens tests pass (up from 492), and the
whole workspace suite is green.

Three tests written before the anchor existed had to be amended, all for
the same reason and all of them mine to fix rather than the spec's to
accommodate:

- `a_detour_across_a_short_row_restores_the_desired_column` and
  `a_caret_on_the_first_non_blank_sticks_to_it_across_rows` poke
  `cursor_column`/`desired_column` directly on a fresh `App`. S1 starts
  the anchor at `Home` (the first frame's caret really is on the first
  node's first non-blank), so `carry_caret` pinned the caret to each
  row's start instead of clamping the desired column. Both now declare
  the anchor they mean. The second one is the more interesting of the
  two: its two cases used to differ only by their starting *column*, and
  now differ by their *anchor* — which is S2's argument restated as a
  test, since column 4 of `"    abcd"` is the first non-blank either way
  and only the chosen one sticks.
- `the_reassigned_keys_dispatch_where_the_table_says` asserted `L` moves
  to the first child. That is precisely the row S9 reverses.

Two additions beyond the specification as written, both requested during
implementation and both folded back into S9's table above:

- `Alt-h`/`Alt-l` alias `Alt-Left`/`Alt-Right`, since every other row of
  the table gives the letters and the arrows the same meaning.
- The `F1` help overlay's Movement section was rewritten. It described
  the old parent/first-child bindings, and its heading ("unshifted moves
  in the text, shifted moves in the tree") was the *old* organizing rule
  in as many words. `H`/`L` moved down into the Fold section, where they
  now sit beside the `zC`/`zO` chords they call.

One thing the user confirmed rather than the spec deriving: the caret is
anchored `Home` at startup, "voluntarily" — so the very first `h` on a
freshly opened file folds the root node rather than being spent adopting
a position.
