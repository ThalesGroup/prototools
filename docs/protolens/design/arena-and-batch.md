<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Asset: the node arena and the override batch

*last verified: 2026-07-31*

**This file used to be a redesign brief for protolens's eventual-OOM
defect.** That redesign shipped as
[spec 0216](../../specs/0216-the-arena-is-a-function-of-the-bytes.md) on
2026-07-31 and the brief's premise — that a splice allocates — is no
longer true, so it has been replaced by a description of what exists.
The brief itself, with its measured peak breakdown, its ten constraints,
its five ranked levers and its annex on the 264-byte slot, is in `git
log` for this path; specs 0209-0213 cite it and those citations point at
the historical version.

## Executive summary

The **arena** is protolens's node store: one slot per structural element
the blob's bytes admit, built once when the document loads and never
changed afterwards. It is a function of the bytes alone — no schema, no
descriptor pool, no type assignment enters into it.

Applying an override therefore does not allocate. It **vacates and
rewrites an overlay** under one slot: which slots are shown, what each is
labelled, how many lines each occupies. The structure underneath is the
same structure it was before, because the bytes are the same bytes.

That is the property worth carrying away, because almost everything else
follows from it. A node index stays valid for the life of the document.
There is no garbage, so nothing compacts and nothing is reused. There is
no local tree to build and translate. Peak memory during an override is
bounded by the render, not by the tree.

## How the arena is built

Two phases, in `prototext-core/src/serialize/render_text/arena.rs`
(`build_arena` → `Arena`):

1. A **depth-first, document-order wire walk**. It is *maximal* and
   *greedy*: every length-delimited payload is recursed into, without
   probing whether it is really a message. A string that happens to
   parse as fields gets children. About a quarter of the slots on the
   reference corpus are such spurious children, and that is intended —
   see "the superset property" below.
2. A **counting sort by depth** into **level order**. Roots first, then
   every depth-1 node, and so on.

The level-by-level build one might reach for instead is impossible:
groups carry no length prefix, so a level cannot be enumerated without
having walked the level below it.

## What level order buys

Children occupy one contiguous block; blocks run in parent order.
Therefore:

| relation | cost |
|---|---|
| next/previous sibling | `idx ± 1`, bounds-checked against the parent's block |
| *k*-th child | one add — `first_child[parent] + k` |
| sibling ordinal | `idx - first_child[parent] + 1` |
| document-order successor | descend, else step sideways, else climb |

Every one of these used to be a stored link. `protolens/src/tui/
structure.rs` is the whole of it, and it stores nothing.

Consequently the slot carries **no links at all**:
`size_of::<TreeNode>() == 44` (a 32-byte `NodeSpan`, two `u32` line
counters, a 4-byte `ProvenanceId`), pinned by a compile-time assertion at
`decode.rs:595`. The shape lives beside it in one `Vec<u32>` of `4n + 1`,
sliced into `parent`, `first_child`, `raw_start`, `raw_end`.

**The root is slot 0** — the whole blob seen as field 1 of a virtual
encompassing message (the [synthetic wrapper](target-blob.md)). It used
to be the *last* slot, because the render emitted post-order. Code
written against the old layout that reaches for a top-level field by
scanning for `field_number == 1` now finds the wrapper instead; use
`nth_child`.

## The superset property, and why it needs checking

The arena describes all the structure the bytes admit; a rendering shows
only part of it. A payload the walk descended into may well be printed as
a string. `TreeNode::is_rendered` is what tells the two apart, and every
accessor in `structure.rs` answers about the *rendered* tree.

Soundness rests on one property: **a rendered node consumes either the
whole of its slot's child block or none of it, never a subset.** Were
that false, a positional path would count rendered children while
`first_child[idx] + k` counted arena children, and the two would drift
apart silently. `decode::the_arena_covers_a_real_corpus` checks it —
coverage, agreement, all-or-nothing — over the whole reference corpus in
both the raw and the `FileDescriptorSet` interpretation. It is
`#[ignore]`d and needs `--bin protolens`.

A **packed run is one slot**, drawing one row per element. The rule spec
0184 had to state — that a run's N spans share one ordinal — is no longer
a rule, it is arithmetic.

## The splice

`splice_override` is still the one function that applies an override, and
`render_node_as` is still the half that answers "what does this node look
like as *target*". What is gone is everything structural: no local tree,
no coordinate translation, no pointer repair, no abandoned slot, nothing
to compact. `idx` keeps its slot, and so does every node under it that
the new interpretation still shows.

It does not write the rendered text buffers directly. It records the
affected line range and its replacement lines as a **deferred patch**;
the buffers are rewritten once per *batch* — one `render_overrides` pass,
or one standalone splice such as a live-preview update — by
`finalize_override_batch`. See
[override-collection.md](override-collection.md)'s render-pass section.

The **live preview does not splice at all** (spec 0185). It renders into
a read-only overlay, so a preview mutates nothing and a failed one leaves
the committed document on screen. A preview additionally caps the
interior bytes handed to the renderer at
`override_preview_byte_budget`; a confirmed override never does.

## Where the memory goes now

Reference corpus is `googleapis.desc` (25.7 MB), driven through a pty,
`:type-as-raw` on line 0 — the root retype, the only action that
re-renders the whole document.

| | before the 0211-0213 slot narrowing | today |
|---|---|---|
| VmRSS at rest | 1 959 900 kB | **988 280 kB** |
| VmHWM, root retype | 4 379 916 kB | **1 740 056 kB** |

Peak **4.18 → 1.66 GiB (−60%)**. The arena holds **4 737 284 slots**
against 4 501 014 nodes in the old render-derived tree (1.05x) — roughly
1.6x either single interpretation, which is the price of describing all
the structure the bytes admit rather than one reading of them.

The original OOM reproduction (`Down`, then three rounds of `t`, `Enter`,
`o`, `d`, `Esc`) reported 2045 → 3889 → 5256 MiB → killed. It now runs
**flat at 995 MiB across all three cycles**, because a splice allocates
no slots and a second cycle has nothing to add.

**A per-slot cost is not only the slot.** Anything held one-per-slot is
multiplied by the same 4.7M. The largest such term after the slot
itself was `heat_states`, at 40 bytes an entry — 190 MB, of which 14
bytes per entry were alignment padding. [Spec
0220](../../specs/0220-the-heat-state-is-three-numbers.md) narrowed it
to 12, taking **at rest 1 004 036 → 874 676 kB (−12.9 %)**. Note what
that measurement did *not* move: the peak, which on the startup path is
set by the decode/render phase, `heat_states` being allocated after its
temporaries are freed. Per-slot narrowing buys steady-state footprint;
the peak has to be attacked in the render.

**One term is left, and it is the render, not the tree.**
`RenderCache::get` clones on every hit and `splice_override` clones again
on insert. See
[spec 0207](../../specs/0207-where-the-override-memory-work-stands.md)
for the shape a fix has to take — a plain `Arc` does not work, because
the confirmed splice *moves* the rendered lines into a buffer that must
own its `String`s.
