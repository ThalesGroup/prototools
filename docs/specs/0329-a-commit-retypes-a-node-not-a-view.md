<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0329 — a commit retypes a node, not a view

Status: implemented
Implemented in: 2026-08-19
App: protolens
Refs: docs/specs/0323-a-document-opens-closed.md (the uniform fold rule,
        and the "a commit is a gesture" clause this amends),
      docs/specs/0259-the-rows-on-screen-stay-where-they-are.md (the
        scroll anchor, and the overlay exclusion this amends),
      docs/specs/0185-the-preview-is-an-overlay.md (the overlay, which
        must not be alive while a splice runs),
      docs/specs/0318-a-preview-ends-where-a-record-ends.md (the preview
        the reader has just been reading, which is why the commit
        should be the quiet part)

## Background

Confirming an override is meant to be the moment the proposal the reader
has been looking at becomes the document. Two things move that should
not.

**The node opens.** Spec 0323 S4 made a commit a gesture: `resettle_node`
clears the retyped node's fold bit, on the argument that the reader has
just watched the preview draw the body and should not press `z` to see it
again. In use it reads the other way round. The preview *was* the look
inside; what the reader wants back afterwards is the document they were
navigating, at the shape they had folded it to. Overriding a node that
was deliberately collapsed re-expands it, and on a wide node that is a
screenful of content nobody asked for.

**The document jumps.** `finalize_override_batch` calls
`restore_scroll_anchor` (0259 S3) and it does nothing, every time, on
every commit made from the override pane. `capture_scroll_anchor` clears
the anchor and returns while a preview overlay is held (0259 S5) — and
the override pane always holds one, from the frame it opens to the frame
it confirms. So the last frame that could have captured an anchor did not,
and the restore has none to use. The row the reader was pointing at lands
wherever the new line counts put it.

Restoring 0259's anchor would not be enough on its own. It names *the
pane's top row*, which is the right answer for the splices it was written
for, and the wrong one here: an origin kind decides how far a commit
reaches, and a `path-field` or `fqdn-field` origin retypes every node in
the document that matches — including nodes **above** the one the reader
is pointing at. Rows above the target then change count too, and putting
the top row back does not put the target back.

The two defects are the same one seen twice: the commit is doing things
that belong to a reader's gesture, and the reader made no gesture.

## Goals

- **G1.** A commit changes no fold bit that was not already going to
  change. Whatever the fold sets remember is what is drawn.
- **G2.** The node the caret was on — the commit's primary target — is
  drawn on the same terminal row after the commit as before it, however
  many other nodes the origin kind swept up with it.

## Non-goals

- **N1.** *The uniform rule is untouched.* Spec 0323 S4's "a body no
  reader has asked to see is closed" still governs the subtree a splice
  writes: the new children arrive folded, one level at a time. This spec
  changes only what happens to the retyped node's **own** bit.
- **N2.** *The preview still draws unfolded.* Spec 0323 S6's asymmetry is
  the point of previewing and is not being "fixed" — it is in fact what
  makes G1 defensible, since the reader has already seen the body.
- **N3.** *Two anchors, one rule.* This was drafted as "not a second
  anchor" — `ScrollAnchor` gains a term and a second way to be captured
  but not a sibling — on the argument that two anchors alive at once
  would name two scroll positions with nothing to rule on which wins.
  Implementation showed the single field cannot work, and the rule the
  draft was missing turns out to be one line.

  A commit's anchor cannot be written over `scroll_anchor`, because the
  two have different lifetimes. 0259's is a standing fact about the frame
  last drawn, re-taken by every frame, and true of whatever splice comes
  along next. A commit's is a fact about *one batch*, and if it outlives
  that batch it is read later with geometry that has since moved. The
  measured symptom: the fixture builders in
  `override_apply`'s tests splice during setup, and a test that later
  ran a standalone `splice_override` consumed the anchor that setup had
  left behind, restoring to a scroll position from another document
  state.

  So `App` has `target_anchor` beside `scroll_anchor`, and the
  precedence is stated once, at the single place either is read:
  `target_anchor.take().or(scroll_anchor)`. **One batch, one restore** —
  the `take` is what keeps the pair from becoming the ambiguity the
  draft feared. Anything with no commit anchor of its own falls back to
  0259's, which is still current because it is re-taken every frame.
- **N5.** *Nothing else in the batch is promised a position.* An
  `fqdn-field` commit may retype fifty nodes; forty-nine of them move,
  because the document above them changed and there is no coherent
  position to hold them all at. One node is where the reader was
  pointing, and that is the one with a claim.
- **N4.** *`splice_override` still is not a gesture.* The bake reaches it
  through `expand_auto_fold` and must open nothing. That was already
  true and is why the fold policy was placed in `resettle_node` rather
  than in the splice; this spec removes the policy, not the placement
  rule.

## Specification

- **S1.** `resettle_node` no longer clears the retyped node's fold bit
  after a successful splice. `splice_override` already remembers the bit
  across `overlay_spans` — it reads `idx_was_folded` before and restores
  it after, because the uniform rule would otherwise fold every bracketed
  slot it writes — so removing the clear leaves exactly what the set
  remembered, which is G1 stated as code.

  Spec 0323 S4's clause naming `resettle_node` as one of the two callers
  that *are* gestures is superseded. One caller remains a gesture:
  `App::open`, which clears the bit before it calls in.

- **S2.** `ScrollAnchor`'s third term is restated: `skip` — the
  viewport's remainder within its top row — becomes `above`, **how many
  terminal rows sit between the pane's top and the anchored row**.

  This is a renaming and a negation, not a new field. `PaneScroll::top`
  is `offset(index) + skip` and `terminal_row_of(row)` is
  `offset(row) − scroll_top()`, so for the one row 0259 ever anchored —
  `scroll.index`, the top one — the two are the same number with
  opposite signs. `restore_scroll_anchor` subtracts where it added and
  the existing path is unchanged in behavior, not merely in shape;
  both captures then reduce to the one expression `terminal_row_of`.
  What the renaming buys is that the term also holds an anchor on a row
  *inside* the pane, which is the row this spec needs.

  The term is signed. A target scrolled off the top of the pane keeps its
  distance above it rather than being dragged into view, which is the
  same promise for a reader who has deliberately scrolled past.

- **S3.** A commit captures its own anchor, on its **primary target**,
  at the commit — not from the frame that happens to be on screen. The
  primary target is the node the caret is on, which is also
  `override_target` for every commit made from the selection pane
  (`open_override_on_type` sets one from the other). One rule, always
  defined, and the same node the reader was pointing at whichever key
  started the commit.

  The capture goes at the two **call sites** that are commits — the
  `Enter` arm that confirms the selection pane, and `run_override_cmd`
  — and explicitly **not** inside `render_overrides`, which is the one
  function they share.

  In `run_override_cmd` that is `override_cmd_subject()`, read before
  `close_override` clears `override_target`, and deliberately *not* the
  `origin_subject_node(&origin)` the rest of the command uses. That one
  is the origin's **first match**, which for an `fqdn-field` origin is
  the topmost occurrence in the document: anchoring there would hold a
  node the reader may never have seen and let the one they were
  pointing at move, which is the very case this spec exists for.
  The bake enters `render_overrides` too, through `expand_auto_fold`,
  and a bake is not a commit: it has no primary target and it is
  already promised the *top* row (0259, and five tests that say so).
  Putting the capture in the shared function made all five fail, which
  is the same distinction N4 draws for `splice_override` — the batch
  machinery is not the gesture, the key press is.

- **S4.** The capture is arithmetic, not a window read: the target's
  header line is `absolute_start(idx)`, its visible row is
  `visible_row_of_line`, and the distance above is that row's terminal
  offset less the current scroll top — the exact inverse of the two lines
  `restore_scroll_anchor` ends with.

  This is what lets the capture run with a preview overlay up, and it is
  the distinction spec 0259 S5 was reaching for. That exclusion is about
  reading a *window*, whose rows are display rows under an overlay and
  committed rows otherwise. `self.scroll` and `visible_row_of_line` are
  in committed terms either way, because the overlay is a substitution
  made at draw time on top of them.

- **S5.** With no primary target — `--load-overrides` at startup, a
  scripted batch — there is no caret to honor and 0259's top-row anchor
  stands as it is. To keep that fallback from being empty on the paths
  that do hold an overlay, `capture_scroll_anchor` stops *clearing* the
  anchor when one is held: 0259 S5 is right that capturing from an
  overlay window would record nonsense, and wrong to also throw away the
  good anchor taken on the frame before the overlay went up.

- **S6.** Nothing else on the confirm path moves. The overlay is still
  cleared before `render_overrides` (0185 S6), the restore still runs at
  the top of `finalize_override_batch` before `clamp_pan_offset` (0259
  S3), and the clamp still has the last word if the caret ends up outside
  the pane.

## Alternatives considered

**Restoring 0259's top-row anchor and nothing more.** One line — stop
clearing it — and it is right for a commit that touches one node, since
nothing above that node's header changes and the top row and the target
then move together. It is wrong the moment the origin kind reaches
further: a `path-field` or `fqdn-field` commit retypes matching nodes
throughout the document, rows above the target change count, and the top
row comes back while the target does not. The narrow fix would work on
the common case and fail silently on the case the reader is least able to
predict. It survives as S5's fallback, where there is no target to prefer
to it.

**Capturing when the preview is installed rather than at the commit.**
The overlay is built and rebuilt on every arrow key in the selection
pane, so the capture would run dozens of times to be used once, and it
would have to be added at each of `preview_override_highlight`'s callers
or hidden inside it, where "remember the viewport" has no business. The
commit is the one moment the anchor is for.

**Anchoring on the byte range instead of the node.** A splice can leave a
node with no row of its own — 0259 S4's flattened parent — and a range
would then resolve to nothing at all, where a node index climbs to its
parent and still answers. The arena is immutable, so a slot index is
valid for the life of the document; that is why 0259 chose a node and the
reason has not changed.

**Keeping the commit-opens rule behind a setting.** A setting is two
behaviors to test and a question to answer at every future fold change.
The reader who wants the node open presses `z`, which is what it is for.

## Test plan

1. `a_commit_leaves_the_fold_alone` — a folded node overridden through
   the pane's confirm key is still folded, and an open one is still open.
   Both directions, since S1 removes a rule rather than inverting it.
2. `a_committed_subtree_still_arrives_folded` — spec 0323 S4's uniform
   rule over the *new* children is unaffected by item 1. This is 0323's
   own `an_override_subtree_arrives_folded` extended to the
   `resettle_node` path, which it did not previously cover.
3. `a_commit_keeps_the_target_where_it_was` — with the document scrolled
   so the target sits mid-pane, the terminal row its header is drawn on
   is the same before and after confirming. Fails today by the change in
   line count.
4. `a_wide_origin_still_keeps_the_target` — the same assertion for an
   `fqdn-field` commit whose matches include a node **above** the target,
   so rows above it change count. This is the case the top-row anchor
   cannot do and the reason S2 exists; it must be a distinct test from
   item 3, which the top-row anchor would pass.
5. `an_anchor_with_no_offset_is_todays_anchor` — capture then restore is
   the identity on the row 0259 anchors, at both viewport shapes. Pins
   S2's claim that the existing path did not change behavior. The
   comparison is on the viewport's absolute top rather than on
   `PaneScroll`: a `skip` past the end of its index's display rows is the
   same viewport spelled differently, and `set_scroll_top` normalizes it
   on the way back in.
6. `an_overlay_does_not_discard_the_anchor` — capture on a committed
   window, install an overlay, render again, and the anchor is still the
   first one. Pins S5 against a later "clear it for safety".
7. The existing `assert_line_counts_are_exact` and
   `assert_status_is_exact` run on every splice in the suite and must
   keep passing; they are what proves S1 did not leave a count stale.

## Measured outcome

Implemented as specified. N3 and the two added paragraphs in S3 were
written *from* the implementation and record what the draft had wrong:
one anchor field cannot serve two lifetimes, and the capture cannot go in
the function the two commit sites share.

Two further things the tests forced, both of them about the fixture
rather than the code:

- **A shrinking commit cannot test G2 at all.** The obvious fixture —
  retype a message node to something flatter — makes the document
  *shorter* than the scroll position the anchor would restore, and
  `set_scroll_top`'s clamp, which S6 deliberately leaves with the last
  word, wins. So `opaque_items_fixture` declares its elements `bytes`:
  each draws one row until it is retyped and three afterwards, and the
  commit grows the document by forty rows, including above the caret.
- **The `anchored ≠ origin_subject_node` distinction was a live bug, not
  a hypothetical.** `run_override_cmd` was written to pass the origin's
  first match, and `a_wide_origin_still_keeps_the_target` — whose origin
  matches twenty nodes, the first of them twenty rows above the caret —
  is what separates it from item 3.
