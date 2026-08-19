<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0332 — folding is a question about the bytes

Status: implemented
Implemented in: 2026-08-19
App: protolens
Refs: docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (the
        maximal arena, level order, and the title this one borrows),
      docs/specs/0323-a-document-opens-closed.md (`FoldSet`, and the
        state every bracketed slot starts in),
      docs/specs/0249-a-large-document-answers-the-user-first.md (S3's
        `auto_folded`, S8's stop, and the `open`/`unfold` split this
        removes from the gesture path),
      docs/specs/0255-the-document-finishes-itself-while-nobody-waits.md
        (the bake, the only thing that clears a stop after this),
      docs/specs/0271-a-script-walks-the-reader-through-the-blob.md
        (`script_reset_folds`, which already states this spec's rule),
      docs/specs/0194-the-cursor-is-a-caret.md (the `0`/`^` motion this
        takes a key from, and S11's caret clamp after a fold),
      docs/specs/0210-a-node-counts-its-own-lines.md
        (`refresh_line_counts`, and the order a bulk fold must call it
        in)

## Background

**There is no way to ask for a shape.** The pane has two fold gestures
at a node and they reach two extremes: `z` toggles the node, `Z` forces
its whole subtree open or closed. On the documents protolens exists for
neither is what a reader wants. A `FileDescriptorProto` closed says
nothing; open is thousands of rows. What is wanted is nearly always the
first two or three levels — enough to see the shape and the names, with
the bodies out of the way. Reaching that today is `Z`, then `z`, then
`z` on each child in turn, redone at every sibling.

**And a fold is currently a question about the render, in two ways that
both make it less useful than it could be.**

The first is the walk. `structure.rs:85` is the seam:

```rust
let block = first_child[idx]..first_child[idx + 1];   // the bytes
if block.is_empty() || !self.tree[block.start].is_rendered() {
    return 0..0;                                       // this rendering
}
```

The arena above the mask is a function of the bytes (spec 0216) and
holds every node a byte-level walk found. `child_slots` hides the ones
the *current interpretation* does not print, and `collect_descendants`
— which `Z` walks — therefore sees only what is on screen. A LEN field
being shown as `bytes` has arena children and no rendered ones.

The second is `open()`. Spec 0249 S8 makes opening a stop a *render*:
`toggle_fold` calls `open`, which calls `expand_auto_fold`, which
splices. So a fold gesture's reach depends on how far the bake has got,
and on a large document the same keystroke does different things a
second apart.

`script_reset_folds` already states the rule the rest of the pane does
not follow: *"`auto_folded` is deliberately untouched: it is not a fold
the reader or the script chose, it is rendering that has not happened
yet."*

**The state this spec needs already exists.** `App::folded` is built as
`FoldSet::new(arena.len())` (`decode.rs:948`) — one bit per *arena*
slot, not per rendered node. Because the arena is a function of the
bytes, that bit is already there for a node the bake has not reached and
for a node no current typing prints. This spec adds no state; it widens
which slots the gestures are willing to write, and separates that
writing from the three conditions rendering adds on top:

| question | where it is answered |
| --- | --- |
| Does the reader want this node open? | `folded` — per arena slot, always answerable |
| Has this node's body been rendered yet? | `auto_folded` |
| Does this node exist under the current typing? | `child_slots`' `is_rendered` mask |

`is_folded` is `folded \|\| auto_folded` and stays so: both mean "draw no
body". The point is that only the first is intent, and only the first is
what a keystroke may write.

## Goals

- **G1.** Ten main-pane bindings, `0` through `9`. Digit `n` sets the
  cursor node's subtree so that everything at relative depth below `n`
  is open and everything at depth `n` or deeper is folded — the cursor
  node itself being depth 0. `0` closes the node and everything in it;
  `1` opens the node and folds its children; `2` opens the node and its
  children; and so on.
- **G2.** A digit is absolute. Pressing it twice does what pressing it
  once did, and `3` then `1` leaves the same shape as `1` alone. A digit
  names a shape; it does not describe a change.
- **G3.** **A fold gesture walks the bytes, not the render.** The
  subtree it acts on is the arena's, so a slot the current typing prints
  as a scalar still takes a fold bit. Overriding that node later then
  reveals it at the depth already asked for, with no second gesture.
- **G4.** **A fold gesture writes `folded` and renders nothing.**
  `auto_folded` is the bake's bit and no keystroke touches it. The same
  keystroke does the same thing whenever it is pressed.
- **G5.** `z` and `Z` obey G3 and G4 too, and `Z` becomes one call to
  the digits' function.

## Non-goals

- **N1.** *No depth past 9, and no multi-digit count.* A count prefix
  (vim's `3z`) needs a pending-input state that every other key in the
  pane must then be defined against, and it spells the common case as
  two keystrokes. Deeper than 9 is what `Z` is for, and `Z` has no bound.
- **N2.** *`z` on a stop stops rendering it — accepted.* Today `z` on a
  node the bake has not reached expands it on the spot. After this it
  records the reader's request in `folded` and the node stays drawn
  collapsed until the bake clears its own bit, which is the whole
  content of G4. The cost is latency on a not-yet-baked frontier, and
  it is bounded: `auto_folded` is empty when the bake converges (18.9 s
  on googleapis, the largest corpus), and `bake.rs` drives
  `expand_auto_fold` with no help from any gesture. What is bought is
  that no fold gesture can fire a splice, so none of them can stall.
- **N3.** *`scrub_folds_under` is not revisited.* Its premise — "a fold
  flag can only stand on a node some rendering showed" — is what G3
  breaks, and its comment must be corrected; its *behavior* (clearing
  the rendered subtree's flags on a splice) belongs to spec 0256 S4.
  This leaves flags on unrendered slots surviving a re-splice, which is
  arguably right: the arena is a function of the bytes, so slot `k`
  covers the same byte range under every typing, and a bit on it is a
  statement about those bytes.
- **N4.** *No context-menu rows.* Ten entries for one setting would
  swamp a menu whose purpose is to name the handful of gestures a mouse
  user cannot guess.
- **N5.** *`0` loses its caret meaning outright.* `^` and `Ctrl-a` are
  both already bound to `caret_to_line_start` and both already in the
  help text. Vim's two column-zero motions were only ever collapsed
  into one here because column zero is the fold gutter; dropping one
  spelling costs nothing a reader can reach.
- **N6.** *The cursor does not move and no view is pinned.* The cursor
  is already on the node being reshaped, and a digit only changes rows
  after that node's header — `toggle_fold`'s own argument.
- **N7.** *No third fold state.* G3's corollary runs one way only, and
  measurement is what says so: a *fold* recorded on a slot no row stands
  for survives an override of the node above it, but an *unfold* does
  not, because the render that first draws such a slot inserts it into
  `folded` (spec 0323 S2) and there was no bit there for it to leave
  alone. Reversing that needs a state the fold sets do not have — "the
  reader has an opinion about this slot", as distinct from "the reader
  wants it open" and "nobody has said". Adding it would put a second bit
  per arena slot beside the two there are and would have spec 0323's
  default answer to it on every render.
  `an_unfold_of_a_slot_no_row_stands_for_does_not_survive_an_override`
  pins the boundary, so that it becomes visible the day someone wants
  it moved.

## Specification

- **S1.** The fold code reads the arena's child block directly —
  `first_child[idx]..first_child[idx + 1]`, as `absolute_start` and
  `push_subtree_lines` already do — and no new accessor is added.
  `child_slots` keeps its `is_rendered` test untouched: that mask is
  load-bearing for everything that navigates or draws, because a vacant
  slot's span is a placeholder that must not be read.

- **S2.** `App::set_folded(&mut self, idx: usize, fold: bool) -> bool`
  is the one writer of reader intent, reporting whether the bit moved:

  - `fold` → `is_foldable(idx)`, and `folded.insert(idx)`;
  - `!fold` → `folded.remove(idx)`.

  It writes the existing `folded` set and introduces no state of its
  own. Its whole content is that the intent is recorded for the slot
  whether or not anything is drawn for it.

  `is_foldable` is the union of two facts, because neither alone is
  right: the arena gives the slot a child block, **or** the slot is
  drawn bracketed. The second disjunct is not a concession — an empty
  message has no children in the bytes at all and is still `Name {`
  then `}`, foldable and marker-worthy, which is the case
  `has_children`'s own doc comment exists to make. What the union
  excludes is the only thing it must: a scalar, which would enter
  `folded` with nothing to ever take it back out.

  **The leaf guard moves from `has_children` to that block, and this is
  the step G3 fails silently without.** `has_children(i)` is
  `tree[i].is_bracketed()` is `tree[i].span.is_message`; a slot the
  current typing does not print holds `TreeNode::vacant()`
  (`decode.rs:678`), whose span is a placeholder with `is_message:
  false` and a doc comment saying it must not be read. So the old guard
  is false on every vacant slot — precisely the slots G3 exists to write
  bits on. Left in place it would compile, pass on a fully-rendered
  fixture, and do nothing in the one case that matters.

  The arena answers the question a vacant slot cannot: *does this slot
  have a child block*. That is also what the guard always meant — its
  stated reason, "a leaf must never enter `folded` because nothing would
  take it back out", is a fact about the bytes, not about the typing.

  `auto_folded` appears nowhere in it. That is G4.

- **S3.** `App::set_cursor_fold_depth(&mut self, depth: usize)`, in
  `navigation.rs`. Guard first: a cursor node with no arena children
  leaves `not foldable` and moves nothing.

  It collects the subtree breadth-first through the arena blocks into
  one `Vec<usize>`, recording `cut` — the index at which relative depth
  `depth` begins — as it passes that level. `depth` larger than the
  subtree leaves `cut = out.len()`, so `usize::MAX` means "open
  everything". Applying is then one reverse pass: `out[k]` is folded iff
  `k >= cut`.

  Level order (spec 0216) is why the reverse pass is correct and why no
  sort is needed: the collection is level by level, so reversing it
  visits every node before its parent. That matters because
  `refresh_line_counts` climbs *upward* and stops at the first ancestor
  whose count is unchanged — refreshing a parent first would propagate
  counts about to move again.

  `refresh_line_counts` is called only for a slot that moved **and** is
  rendered. An unrendered slot has `lines_total == 0` and contributes to
  no count, so calling it there would be a climb that provably changes
  nothing. `folds_changed` runs once at the end if anything moved.

  The walk covers the *whole* subtree, not just its first `depth`
  levels. Stopping at the cut would be cheaper and pixel-identical on
  the frame it draws, since everything under a folded node is hidden —
  but it would leave the deeper slots as history left them, and a later
  `z` at the cut would reveal an arbitrary shape. G2 is what pays for
  the full walk.

- **S4.** `Z` becomes
  `set_cursor_fold_depth(if is_folded(cursor) { usize::MAX } else { 0 })`
  — open all the way down, or close all the way down. This is G5, and it
  is the claim that the digits generalize `Z` rather than reimplement
  it. The target state still comes from the cursor node, for the reason
  `toggle_cursor_fold_recursive` already gives: a subtree in mixed
  states has no meaningful opposite.

- **S5.** `toggle_fold` (`z`, the fold-marker click, `l` into a folded
  node) becomes `set_folded(idx, !is_folded(idx))` in place of its
  `if !self.open(idx)` line, and `set_all_siblings_folded`
  (`Ctrl-h`/`Ctrl-l`) becomes one call to the same. Note the asymmetry
  this creates and that N2 accepts: on a node in `auto_folded` only,
  the reader's `z` is recorded and nothing is drawn differently — and
  the bake honors it when it arrives, since a stop the reader took out
  of `folded` renders open.

  `open()` keeps exactly one caller, and it draws the line this spec is
  about: **a keystroke never renders; a script's `unfold:` step
  declares that a node shall be visible at this beat, and a script that
  walked a reader to a node they could not see would have failed at its
  one job.** `unfold()` also stays, for `scrub_folds_under` and
  `unfold_ancestors`, neither of which is a gesture either.

- **S6.** `key_dispatch.rs`: `KeyCode::Char(c) if c.is_ascii_digit()` in
  the main-pane letter tier beside `z`/`Z`, taking its depth from `c`.
  The tier already sits behind `ctrl_or_alt(key)`, so no modifier guard
  of its own is needed, and `Alt-<digit>`/`Ctrl-<digit>` stay free. The
  arm at `key_dispatch.rs:924` loses its `Char('0')` and keeps
  `Char('^')`.

- **S7.** `cursor_line_in_node` is reset to 0 and `clamp_caret_column`
  runs, per spec 0194 S11 — the cursor row's text is rewritten
  underneath the caret when its `{ ... }` summary appears or goes.

- **S8.** `HELP_TEXT`'s "Fold / unfold" section gains the digits and its
  "Movement" line drops `0` from `0 / ^ / Ctrl-A`.

## Alternatives considered

**A vim-style count prefix, `3z`.** Two keystrokes for the case a reader
repeats most, plus a pending-input state. Rejected per N1.

**Stopping the walk at the cut.** Cheaper and identical on the frame it
draws. Rejected because it makes the digits history-dependent, which is
the one property they exist to remove — S3.

**Depth measured from the document root.** Then `2` pressed deep inside
a document collapses everything the reader opened to get there. Every
fold gesture in this pane acts at the cursor.

**Leaving `0` on the caret and starting the digits at `1`.** Nothing
would be unreachable, since `Z` gives depth 0. Rejected because it
breaks the one thing that makes ten bindings learnable — that the digit
*is* the depth.

**Keeping `open()` on the gesture path, so a digit renders the stops it
meets.** Considered and rejected as the primary design: it makes a
keystroke's effect depend on the bake's progress, and a digit on a large
subtree can fire one bounded splice per stop. The narrower variant —
render the cursor node only, so the gesture always visibly lands — was
also rejected, as one special case in the walk buying back a latency
that N2 bounds anyway.

## Test plan

1. `a_digit_sets_the_subtree_to_that_depth` — on a fixture at least four
   levels deep, press `0`, `1`, `2`, `3` and assert `folded` membership
   by relative depth: folded exactly where depth `>=` the digit and the
   slot has arena children.
2. `a_digit_folds_slots_this_typing_does_not_show` — G3, the corollary.
   A node rendered flat, whose arena block is non-empty, takes bits on
   its children; overriding it to a message type then draws them folded
   with no second gesture.
3. `a_digit_is_absolute_not_a_toggle` — G2: `2` twice equals `2` once,
   and `3` then `1` equals `1` alone, compared as `FoldSet`s.
4. `a_deeper_digit_reopens_what_a_shallower_one_closed` — `1`, `3`, `1`,
   returning to the first shape.
5. `shift_z_is_the_two_extremes_of_the_digits` — S4: `Z` closing equals
   `0`, and `Z` opening equals a digit past the subtree's depth.
6. `a_fold_gesture_never_renders` — G4: with a node in `auto_folded`,
   `z`, `Z` and a digit each leave `auto_folded` unchanged and add no
   rendered lines; the reader's request is visible in `folded`.
7. `a_digit_on_a_leaf_says_not_foldable` — the message and an unmoved
   fold set.
8. `a_digit_leaves_the_cursor_where_it_was` — N6.
9. `line_counts_stay_exact_after_a_digit` —
   `assert_line_counts_are_exact` (spec 0210) after a digit that both
   opened and folded. The reverse pass of S3 is what this pins.
10. `zero_folds_and_the_caret_still_reaches_column_zero` — the
    rebinding. The two existing tests that press `0` for the caret are
    re-pointed at `^` rather than deleted.
11. `an_unfold_of_a_slot_no_row_stands_for_does_not_survive_an_override`
    — added during implementation, for N7: the boundary of item 2's
    corollary, so that a later change of mind about the third fold
    state has something that fails.

## Measured outcome

Eleven new tests in `protolens/src/tui/tests/folding.rs`; the whole
suite is 1 161 tests green, and the four gates are clean.

Three things the implementation settled that the draft had wrong or
left open.

**The leaf guard cannot be the arena alone.** S2 as drafted said "the
arena block is non-empty", and `empty_bracketed_message_is_foldable`
failed on it immediately: a message with zero populated fields has no
child block in the bytes and is still drawn `Name {` then `}`, with a
fold marker. So the guard is the union — arena block, *or* bracketed —
and the draft's claim that the arena test is simply the better one was
half right. It is the better test of *the bytes*; the drawn shape is a
second, independent reason a slot may hold a fold. S2 now says so.

**G3's corollary runs one way only.** A fold recorded on a slot no row
stands for survives an override of the node above it, exactly as
intended — `scrub_folds_under` clears the *rendered* subtree, and a
node drawn flat has no rendered descendants, so the bits live. An
unfold does not survive: `decode.rs`'s render inserts every bracketed
slot it writes into `folded` (spec 0332 S2's neighbor, spec 0323 S2),
and a previously vacant slot has no bit there for it to leave alone.
Measured, not deduced, and now pinned by
`an_unfold_of_a_slot_no_row_stands_for_does_not_survive_an_override`.
N7 records why it is not fixed here.

**`open()` did not lose all its callers.** S5's draft said it would.
Three remained: `set_all_siblings_folded`, which is a gesture and moved
to `set_folded` with the rest; two fixtures, which moved; and the
script's `unfold:` step, which stayed. That last one turned out to be
the sharp edge of the rule rather than an exception to it — a keystroke
never renders, and a script step declares that a node shall be *visible*
at this beat, which is the one thing a demo script exists to guarantee.

Six existing tests changed. Two only because `0` moved
(`the_reassigned_keys_dispatch_where_the_table_says`,
`a_click_in_the_left_margin_forfeits_the_anchor` — both re-pointed at
`^`, and the first now also exercises the `Ctrl-A` alias). One because
of the union guard. Three because they pinned the behavior N2 replaces,
and the most useful of those is the rewrite of what was
`opening_an_auto_fold_renders_the_body_it_stood_for`: it now asserts
that `z` on a bake stop withdraws the fold, changes not one row, and is
honored when the bake arrives. Both versions guard the same hazard from
opposite sides — the row must never stop saying "not shown here" and
start saying "nothing here". Then the splice firing is what prevented
it; now it is `auto_folded` standing while `folded` clears.
