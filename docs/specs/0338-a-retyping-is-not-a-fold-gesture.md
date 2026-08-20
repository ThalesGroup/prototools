<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0338 — a retyping is not a fold gesture

Status: implemented
Implemented in: 2026-08-20
App: protolens
Refs: docs/specs/0323-a-document-opens-closed.md (whose uniform "born
        folded" rule this moves off the splice path and onto the open),
        docs/specs/0332-folding-is-a-question-about-the-bytes.md (whose
        one-way corollary this closes),
        docs/specs/0124-protolens-manage-pane-navigation.md (the
        management pane's `Left`/`Right` circulation)

## Background

Committing an override throws away the reader's folds under the
retyped node. Measured on `Root { a: Mid { x: Leaf { v: 7 } }, b }`,
with `a` and `a/x` opened by hand and then `a` re-typed:

```
BEFORE                          AFTER
1 {                             1 {
  a {                             a {
    x {                             x { ... }     <- refolded
      v: 7                        }
    }                             b { ... }
  }                             }
  b { ... }
}
```

The reader's fold set goes `a=false ax=false` → `a=false ax=true`. The
retyped node keeps its own bit; everything below it is closed again.

Two lines in `splice_override` do it, in this order:

1. `scrub_folds_under(&old_descendants)` clears the bit on every slot
   the previous rendering showed;
2. `decode::overlay_spans` inserts **every bracketed slot it writes**
   into `folded` — spec 0323 S4's "a body no reader has asked to see is
   closed", applied to a splice as well as to a fresh document.

So a *fold* under the target survives (it would be re-applied anyway)
and an *unfold* does not. Spec 0332 recorded that asymmetry as
accepted, on the grounds that fixing it needed a third state — "the
reader has an opinion" vs. "wants it open" vs. "nobody asked". It does
not. What it needs is for the set to have a *complete* initial value,
so that "nobody asked" is not a state the set can be in.

Separately, and for the same reader: `Left`/`Right` in the management
pane circulate the main-pane cursor among the nodes an entry impacts
(spec 0124 G1), but land it on a node its ancestors may be hiding.

## Goals

- **G1.** A splice does not write the reader's fold set. Not the
  target's bit, not a descendant's, not a newly revealed slot's.
- **G2.** A document still opens closed, and still bakes closed.
- **G3.** `Left`/`Right` in the management pane put the node they
  select where the reader can see it.
- **G4.** None of it costs measurable time at open. The reproduction is
  a correctness fix; paying 4% of a startup for it would be trading one
  defect for another.

## Non-goals

- **N1.** No third fold state, and no per-slot "the reader has an
  opinion" flag. G1 removes the need for one rather than answering it —
  see S1.
- **N2.** `auto_folded` is untouched by this spec. It is not the
  reader's set: it means "this body has not been rendered", it is
  written and cleared by the render, and a splice going on writing it
  is correct.
- **N3.** No change to what a *gesture* folds. `set_folded`,
  `toggle_fold`, the digits and `Z`/`z` keep spec 0332's semantics
  exactly.

## Specification

- **S1.** **The set is born full, and only the reader writes it
  afterwards.** `build_tree` constructs it as
  `FoldSet::full(arena.len())` — a new constructor that hands back a
  bitmap with every bit already set, masked off past the last slot.

  This is how "a document opens closed" (spec 0323) is *said*, rather
  than something written out slot by slot. Every slot is a member, so
  the answer to "does the reader want this open?" is total before any
  render asks, no later render has to invent a default for a slot it
  has not seen, and spec 0323 S3's "the root alone is open" stays the
  one exception, cleared by `App::new`.

  The set is therefore deliberately **wider than foldability**: a
  scalar leaf is a member too. That is the whole difference from the
  version that shipped first, which walked the arena testing each slot
  with `is_foldable` and inserted the ones that passed. Excluding
  leaves was the only thing that walk was still doing, and it cost
  227 MiB of reads and 115.6 M instructions to arrive at a value the
  constructor can start from. **A default belongs in an initial value,
  not in a loop that writes the default out.**

- **S2.** `Overlay` loses its `folded` field and `overlay_spans` stops
  writing the set. It is no longer a writer of reader state at all.

- **S3.** `overlay_spans` therefore cannot write a collapsed
  `lines_visible` either, since it no longer knows the fold. It writes
  the open count (`line_count`) for every slot, and each caller settles
  the counts it is responsible for:

  - `build_tree` sets `lines_visible = 1` on every bracketed slot, all
    of which S1 has folded — one pass, no climb, exactly what spec
    0323 S2 achieved inline. Over the slots `overlay_spans` *reports*,
    not over the arena: it is the one place that knows which slots it
    drew bracketed without having to look, and on `googleapis.desc`
    that is 7 777 of 4 936 532;
  - `splice_override` walks the new subtree **deepest-first** and calls
    `refresh_line_counts` on each node that `is_folded`. Reverse
    pre-order is deepest-first, so `collect_descendants(idx)` reversed
    is the order; a folded node's climb repairs its ancestors, and a
    node with nothing folded beneath it is already right.

- **S4.** `splice_override` drops `scrub_folds_under` and the
  `idx_was_folded` save-and-restore around `overlay_spans`. Both existed
  to undo a write that no longer happens. `scrub_folds_under` goes with
  them; it was its only caller.

- **S5.** `manage_circulate_cursor` calls `unfold_ancestors` on the node
  it picks, before `set_cursor`. The same call every other
  jump-to-a-node path makes — the search sweep, `Ctrl-o`'s jump, the
  script's focus.

- **S6.** **The read gates on foldability; the write does not.**
  `App::is_folded` becomes

  ```rust
  (self.is_foldable(idx) && self.user_folded.contains(idx))
      || self.auto_folded.contains(idx)
  ```

  and `set_folded` drops its leaf guard, writing the bit for any slot.
  Every read of fold state already goes through `is_folded`, so a
  scalar's bit is never consulted and needs no permission to be
  written.

  Read-gating is not merely the cheaper way to say the same thing. It
  is the *correct* one, because foldability is not constant: an
  override can leave a slot bracketed that was flat before. A bit
  decided once at open would be stale for that slot and it would draw
  open, against spec 0323. Asking `is_foldable` at the point of the
  question cannot go stale.

- **S7.** `folded` is renamed **`user_folded`**, at its declaration in
  `App`, in `BuiltTree`/`Decoded`, and at every use. Spec 0332 S2
  already defined the set as the reader's intent — "does the reader
  want this node open?" — but the old name says only how the answer is
  encoded, and a set that now holds every slot in the arena invites
  exactly the misreading that produced the arena-wide walk. Nothing
  about the polarity changes: a member is a node the reader has not
  asked to see inside.

  Two test-only accessors come with it, because `contains` and `len`
  no longer answer what a test means to ask: `is_user_folded(idx)` is
  the old membership test, and `user_folds()` is the members filtered
  by `is_foldable`.

## Alternatives considered

### Snapshot the old descendants' bits and restore them after the splice

Preserves the bit for slots both renderings show, and lets a
first-time-revealed slot arrive folded. It works, but it keeps the
splice a writer of the reader's set and buys that with a snapshot
vector per splice — and the bake takes 70 893 of them on
`googleapis.desc`. S1 gets the same outcome by giving the set a
complete initial value instead.

### Fold only what `build_tree` renders, and re-fold in `expand_auto_fold`

Keeps the arena-wide pass out of the open, at the price of making the
bake a writer of `folded` — so the question "may this code touch the
reader's set?" would still have two answers depending on the caller.
It also cannot distinguish a stop the reader unfolded with a digit from
one nobody has asked about, which is the very confusion S1 removes.

### Materialize the set: walk the arena and insert every foldable slot

This is what shipped first, and it is the reason S1 says what it says.
It produces the right value — the set is total, and leaves stay out of
it — but it computes at open something that does not depend on the
document at all. Measured on `googleapis.desc`: **+115.6 M instructions
(+4.27%)** and **+227 MiB of last-level read misses**, for about +28 ms
of a 0.6 s startup, slower in 24 wall-clock pairs out of 24.

The mechanism is worth recording, because "it is one pass over the
arena" understates it. The loop is driven by the wrong array. It reads
`tree` — `Vec<TreeNode>`, 44 B per slot, 207 MiB over 4 936 532 slots,
once, with no reuse — to consult `span.is_message`, which is true of
7 777 of them. The decision it actually needed comes from
`first_child`, a `u32` array a tenth the size. So 207 MiB was streamed
for a 0.158% hit rate. The cachegrind delta of 3 720 888 LL read misses
matches the 3 702 398 lines that shape predicts to within 0.5%.

Under S1 none of that is needed, because the answer is a constant and
constants belong in initial values. What remains is one 617 KB memset
and a loop over the 7 777 bracketed slots the render already reported.

### Leave it, per spec 0332 N7

That is what shipped, and the reproduction above is the reason it
cannot stay. The judgement it rested on — that a complete initial value
would need a third state — is wrong: the arena is a function of the
bytes and is complete at open (spec 0216 S1), so the set can be too.

## Test plan

1. `an_unfold_survives_an_override_of_the_node_above_it` — the inverse
   of spec 0332's `an_unfold_of_a_slot_no_row_stands_for_does_not_survive_an_override`,
   which this replaces. Both halves of that spec's corollary now run the
   same way.
2. `a_commit_leaves_the_fold_set_exactly_as_it_found_it` — the
   reproduction above, asserted as set equality over the whole arena
   rather than over the rows, so a bit moved on a slot no row stands for
   still fails it.
3. `a_document_opens_closed` and the rest of `tui/tests/folding.rs`,
   unchanged — G2 is the promise most at risk from S1.
4. `arrows_in_the_manage_pane_reveal_the_node_they_select` — the node
   `Left`/`Right` lands on is on a visible row.
5. `assert_line_counts_are_exact`, which every splice in the suite
   already runs, is the check on S3.
6. `apply_override_splices_tree_and_lines_repeatedly` compares
   `user_folded` against a snapshot after each of three splices over
   the same node, with the target opened first so the snapshot is not
   uniformly full — equality then catches a splice that sets a bit as
   well as one that clears one.
7. Callgrind on `googleapis.desc` is the check on G4, wall clock being
   two orders of magnitude too coarse to see 0.022%.

## Measured outcome

The reproduction is gone. On `Root { a: Mid { x: Leaf { v: 7 } }, b }`
with `a` and `a/x` opened by hand, committing an override on `a` now
leaves `user_folded` bit-for-bit as it found it, asserted over the
whole arena rather than over the rows. Both halves of spec 0332's
corollary run the same way.

**S1 is free.** Callgrind measures the open deterministically
(`--raw -j 1 quit` on `googleapis.desc`, same document throughout):

| build | I refs | vs. before |
| --- | --- | --- |
| before 0338 | 2 706 896 046 | — |
| the materialized set (rejected above) | 2 822 463 205 | +115.6 M, **+4.27%** |
| S1, `FoldSet::full` | 2 707 495 056 | +0.6 M, **+0.022%** |

The 599 010 instructions that remain are the 617 KB memset and the loop
over the 7 777 bracketed slots — three orders of magnitude under the
walk they replace, and two under the run-to-run wall-clock noise.

Wall clock says nothing, which is the expected answer at 0.022%. Eight
interleaved pairs on mains power: base 552.2 ms, S1 554.8 ms, +0.48%,
with S1 slower in **4 pairs out of 8**. Set against the materialized
version's 24 out of 24, that is the difference between a real effect
and none.

Two notes on how the measurement had to be taken, both of which cost
time to learn:

- **On battery the measurement is worthless.** Runs fall into a bimodal
  0.64 s / 0.93 s band with identical instruction counts and identical
  page faults, and a build doing *strictly less* work came out slowest
  of three. On mains the same command runs 0.59-0.62 s with a 3.7%
  spread. This is a second, distinct fingerprint from the known uniform
  clock cap.
- **`std::hint::black_box` cannot isolate a loop's cost**, being an
  optimization barrier. A "walk only, bit set removed" build read
  2 775 828 901 and implied a 2:3 split between the two halves; the
  number was an artifact. Diffing two *unmodified* builds per
  `file:function` gave the real split (four fifths walk, one fifth bit
  set) and is the instrument to reach for.
