<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0298 — a slot that shows nothing changes nothing

Status: implemented
Implemented in: 2026-08-15
App: protolens
Refs: docs/specs/0247-a-fold-toggle-carries-the-worst-news-below-it.md
        (S7, the one full pass this re-sizes),
      docs/specs/0249-a-large-document-answers-the-user-first.md (S1's
        row budget and S12's `Unbaked` rung),
      docs/specs/0257-the-first-pane-does-not-wait-for-the-last-line.md
        (the change that made S7 the wrong size),
      docs/specs/0297-the-stops-are-read-in-the-order-they-are-walked.md
        (the baseline this is measured against)

## Background

Spec 0247 S7 computes every node's status in one reverse linear pass
over the arena. That was the right size when it was written: startup
rendered the whole document, so every slot had rows to read.

**Spec 0257 changed startup to render a screenful**, and nothing
re-sized the pass. Counters on googleapis:

| | |
| --- | ---: |
| slots visited | 4 737 284 |
| slots that render anything | 7 798 |
| slots with rendered children | **6** |
| slots ending non-`Ok` | 7 773 |

One useful answer per 610 slots, at 323 M Ir — 52% of `App::new`. Each
iteration streams a 16-byte `Option<Box<str>>` and then chases two
*random* reads: `tree[first_child[idx]]`, to find out whether the child
block is rendered, and `status_rolled[children]`, to max over it. Both
answer "nothing here" 4 737 278 times out of 4 737 284.

## Goals

- **G1.** The work is proportional to what is rendered, not to the
  arena.
- **G2.** The arrays are byte-for-byte what the unmodified pass
  produced. Not "close enough to tint a fold toggle" — identical.
- **G3.** The property the fast path rests on is checked by the suite,
  not asserted in a comment.

## Non-goals

- **N1.** No list of rendered slots threaded out of `decode`. It would
  make the pass O(rendered) outright rather than O(arena) with an O(1)
  body, but it crosses a module boundary and needs a sort to restore
  descending order, for the ~135 M Ir the surviving scan costs.

- **N2.** No early exit from `children_status` when a child is already
  `Invalid`. Sound in principle — `Invalid` is the top of the lattice —
  but it cannot pay: of 4 737 284 calls, **6** have a non-empty slice,
  so there is no loop to leave early. The 150 M is call overhead, not
  iteration. It would also force a vectorized `max` over a `&[u8]` back
  to scalar.

## Specification

- **S1.** `rebuild_status` skips a slot that renders nothing:

  ```rust
  let is_stop = stops[idx / 64] & (1 << (idx % 64)) != 0;
  if self.node_text[idx].is_none() && !is_stop {
      self.status_own[idx] = Status::Ok;
      self.status_rolled[idx] = Status::Ok;
      continue;
  }
  ```

  `node_text` is the load `own_status` would make first, so the fast
  path adds nothing: one sequential read, two stores.

- **S2.** The stop bit is *tested*, not inferred from the absent text.
  A stop always has text, so the two agree — but leaning on that would
  assume away the `Unbaked` rung, and several tests seed `auto_folded`
  by hand.

- **S3.** Skipped slots are **written**, not merely passed over.
  `rebuild_status` must produce what a from-scratch computation would,
  because that is exactly what `assert_status_is_exact` compares it
  against.

- **S4.** `assert_ancestor_closed`, `#[cfg(test)]`, checks over the
  whole arena that a slot with no text shows no children and is not a
  stop. Called from `assert_status_is_exact`, so it runs after every
  `finalize_override_batch` in the suite (G3).

## Why it is exact

`Ok` is the bottom of the lattice (`Ok < Unbaked < Unknown <
NonCanonical < Invalid`), so `max(Ok, x) == x`. A slot contributing
`Ok` — as its own status and as its roll-up — cannot change any
ancestor's answer, in the way that adding zero cannot change a sum.

That leaves one thing to establish: an unrendered slot has no rendered
children.

**The rendered set is closed under parent.** A child's rows are emitted
inside its parent's body, and spec 0249's row budget gates `descend` —
entry into a body — never the iteration over a sibling list. A node the
budget stops at is *"still opened and closed… and loses only its
children"*. So a sibling block is all-or-nothing, which is the same fact
`child_slots` already relies on when it tests `block.start` alone; a
partially rendered block does not exist. Contrapositively, a slot that
is not rendered has no rendered children.

**This is also why `Unbaked` must rank above `Ok` and not below.** A
stop is rendered, so it keeps the full body and carries `Unbaked` in its
own right; its vacant descendants are exactly the slots skipped here,
and spec 0249 S12 needs them to contribute nothing so that the stop's
own rung is what climbs to the ancestors. Were `Unbaked` the bottom of
the lattice, `Ok` would not be the identity of `max` and this
optimization would be unsound.

The measurement shows that climb happening: **7 771 stops, 7 773
non-`Ok` slots.** The two extra are the stops' parent and the root,
non-`Ok` *only* because the roll-up carried `Unbaked` up to them.

## Test plan

1. `assert_ancestor_closed` over the whole arena, after every splice in
   the suite.
2. `assert_status_is_exact` — the incremental path has no fast path, so
   a violation would also show up as a disagreement there.
3. The full workspace suite.
4. `protolens … export /` over googleapis is byte-identical to 0297's.

## Measured outcome

Dev VM (8 E-cores, two L2 clusters), googleapis (25.6 MB descriptor set,
49 255 roots), `--descriptor-set $SET $SET quit`.

| | 0297 | 0298 |
|---|---|---|
| wall clock `-j 1`, `taskset -c 4`, median of 9 | 2.548 s | 2.501 s |
| wall clock `-j 8`, `taskset -c 0-7`, pooled n=22 | 1.325 s | 1.300 s |
| instructions (`-j 1`) | 18.62 G | 18.30 G |

**−1.72% instructions, −1.8% at `-j 1`, −1.9% at `-j 8`.**

`App::new` falls from 619 M to 299 M Ir. `own_status` and
`children_status` are each called **7 798 times instead of 4 737 284** —
exactly the rendered count — and `children_status` falls from 150 M to
0.32 M.

The `-j 8` figure is pooled over two interleaved runs of eleven pairs;
the first alone read **+1.6%**, which the second overturned. The spread
at `-j 8` is 1.22–1.55 s in *both* arms, so a single batch of eleven
cannot resolve a 2% effect there. `-j 1` reproduced to ±1% and agreed
with the instruction count first time, which is what settled it.

`export /` over the whole corpus is byte-identical to 0297's output,
5 278 322 lines.
