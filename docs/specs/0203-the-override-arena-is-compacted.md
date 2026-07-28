<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0203 — the override arena is compacted

Status: implemented
Implemented in: 2026-07-28
App: protolens
Refs: docs/specs/0118-protolens-recursive-override-rendering.md (§7, a
        retyped node keeps its own index),
      docs/specs/0135-protolens-override-raw-tag-rewrap.md (G1, the
        packed sibling merge and its orphans),
      docs/specs/0152-protolens-heat-cue-background-scoring-thread.md
        (G6, `heat_states` is parallel to `tree`),
      docs/specs/0167-protolens-render-overrides-deferred-line-splice.md
        (the batch defers every write to `lines`),
      docs/specs/0183-prune-the-override-walk.md (the descent marks),
      docs/specs/0186-the-commit-touches-only-what-moved.md (the
        per-batch verification hook this reuses),
      docs/specs/0188-the-batch-updates-what-changed-not-what-exists.md
        (S4, the marks are kept across batches),
      docs/specs/0192-a-frame-costs-the-same-wherever-the-cursor-is.md
        (S4, the event loop's sliced-work interleave),
      docs/specs/0202-an-override-is-refused-rather-than-fatal.md (the
        stop-gap this replaces),
      docs/specs/0206-the-arena-reuses-its-dead-slots.md (slot reuse,
        which revises the free-list rejection below and narrows this
        spec's job),
      docs/specs/0207-where-the-override-memory-work-stands.md (the
        wrap-up: what this spec's N2 peak is actually made of)

## Background

Spec 0202 measured the defect and shipped a guard for it. The defect
itself is unchanged: `App::tree` is an append-only arena, and every
`render_overrides` batch appends a fresh copy of everything it retypes
without ever removing what that copy supersedes.

| after | `tree.len()` | reachable | garbage |
|---|---|---|---|
| startup | 4 501 014 | 4 501 014 | 0 |
| apply | 9 000 349 | 4 499 336 | 4 501 013 (50%) |
| remove | 13 499 684 | 4 499 336 | 9 000 348 (67%) |

The live set is flat. Only the garbage grows, at ~305 B/node, and
`Vec` doubling reserves ahead of it (capacity 18 004 056 for 13 499 684
entries, ~5.5 GiB). Spec 0202 turns the resulting OOM kill into a
refusal; it does not let the override run.

### Why compaction and not a free list

**Superseded in part by spec 0206** (revised 2026-07-28). Two of the
three grounds below did not survive the question "what is a slot?",
and reuse is now specified rather than rejected. The section is kept
because the third ground stands and 0206 has to answer it, and because
which argument failed is worth recording.

The cheaper fix is to push each splice's superseded subtree onto a
free list and reuse those slots.

**It does not bound memory.** As written, this ground held that a free
list is subject to external fragmentation, the arena's allocation
pattern being close to the worst case for it: subtrees are *contiguous
extents* of wildly different sizes (measured: 260 944 nodes for `/5767`
down to single nodes), a freed extent can only be reused by a request
that fits inside it, and a 27 000-node hole satisfying a 30 000-node
request is worth nothing.

Void as written (revised 2026-07-28). It is an argument about
*extents*, and the arena does not allocate extents — it allocates
slots, every one an identical 264-byte `TreeNode`. At slot granularity
a hole always fits a request, so external fragmentation cannot occur.
The extent framing came from the splice's affine addressing
(`translate = |i| i + base`), which is self-imposed and which spec 0206
replaces.

The bound is in fact the other way round. Because `splice_override`
frees before it allocates, reuse holds `tree.len()` at the high-water
mark of the *live* set — 4 499 336 rather than 8 998 671 — tighter than
what compaction achieves *for the arena*, since it prevents the arena's
peak instead of reclaiming after it. That peak is this spec's own N2.

It does not follow that the *process* peak goes with it, and the second
revision of this paragraph is where that was noticed. A splice also
materializes `local_tree`, a second full `Vec<TreeNode>` living outside
the arena (`override_apply.rs:2566`), and the render it is built from
is held twice over by `render_cache`. Spec 0206's Background carries
the arithmetic. The point here is only that "reuse bounds the arena"
and "reuse bounds the process" are different claims.

**It has no checkable postcondition.** Half right. There is indeed no
"the arena now holds exactly the live set" moment, which is
compaction's own G1. But there is an invariant that is just as
checkable and is true continuously rather than once per pass: every
index is either reachable or marked in `dead`, never both and never
neither. `verify_arena` (S6) computes both sides of it. The
postcondition moved; it did not disappear.

**It fails dangerously rather than loudly.** Stands, entirely, and is
now spec 0206's principal risk rather than a reason not to proceed.
Reusing a slot means an index that some other structure still holds
now names a *different* node, and the arena carries no generation
counters to detect it. The failure mode is a fold or a heat cue
silently attaching to unrelated content, which no test catches and no
user reports as anything but "protolens is confusing".

What changed is not the argument but what is available to answer it.
Spec 0162's A2 rejected reuse on the grounds that a free list "has no
such moment" at which the arena can be asserted correct; this spec
then built one. `verify_arena` decides exactly the property that fails
when a stale index survives, and spec 0186's hook runs it after every
batch in every override test. Compaction still has the stronger
guarantee — it rewrites every holder as it moves each node, so a slot
is never recycled while anything can point at it — and that is why it
is kept alongside reuse rather than replaced by it.

### Why incremental, and not a stop-the-world pass

The first draft of this spec specified a four-phase mark-compact
(mark, address, move, relink) with a `remap: Vec<u32>` table, run to
completion at the end of a batch. It is the textbook shape and it is
the wrong one here, for a reason that only shows up at this document's
scale: it would add a second multi-second freeze to an application
whose existing multi-second freeze is itself the subject of two other
specs (0204, 0205). Reclaiming memory by making the hang longer is a
poor trade.

The arena admits something better, and the whole of this spec rests on
it:

> **Every reference to a node is discoverable from that node.**

A node's parent, siblings and document-order neighbors are named by
its own link fields, and the reverse link in each of them is the one
that has to change. Its children are reachable by walking its own
child chain. Its two line-map entries are found through its own
`text_range`. The remaining holders are keyed by index, so a hash
lookup or an equality test finds them.

So a *single node* can be relocated in time proportional to its own
degree, with no arena-wide pass and no remap table — and, critically,
the arena is fully consistent again the instant that one node has
moved. Summed over the arena that is `O(edges)`, i.e. `O(nodes)`: the
same total as a mark-compact, but divisible at *arbitrary*
granularity. A pass can therefore be sliced into the event loop a few
thousand nodes at a time, abandoned halfway with no cleanup, and
restarted later, without the document ever being observably wrong.

This is also what distinguishes compaction from an override batch, and
why it is safe to interleave when a batch is not: a batch mutates the
arena in place with no undo and no coherent intermediate state, whereas
compaction has no unsafe intermediate state at all.

## Goals

- **G1.** After a compaction pass, the arena holds only reachable
  nodes — `tree.len()` equals the reachable count exactly.
- **G2.** A session's arena is bounded at a constant factor of the
  live set, regardless of how many overrides are applied and removed.
- **G3.** The arena is fully consistent after every *individual* node
  move, so a pass can be interrupted anywhere.
- **G4.** Nothing user-visible changes except memory: the rendered
  document, the cursor, the folds, the heat cues and the override
  entries are identical across a pass.
- **G5.** Reclamation costs no arena-wide scan. Liveness is recorded
  where it is created, not recovered afterwards.

## Non-goals

- **N1.** Narrowing the per-node footprint (`u32` links,
  interned/shared `type_fqdn`). This spec is what makes `u32` sound —
  the arena is unbounded *in time* today, so no width is safe — but
  the narrowing is separate work.
- **N2.** Reducing a batch's transient peak. This spec removes the
  *permanent* retain only. The premise stated here — that a splice
  materializes the new subtree before the old one can be freed, so the
  momentary double is inherent — turned out to be wrong: the free
  happens first (`override_apply.rs:2436`, before the push loop at
  `:2555`), so the holes exist when the request arrives. Spec 0206
  takes the peak on that basis.
- **N3.** The batch stall (~4.8 s on a document-wide override).
  Separate specs.
- **N4.** Removing spec 0202's guard. It stays. It should simply stop
  firing, and if it ever fires again that is information.
- **N5.** A general collector. This is one arena, with one shape, one
  producer of garbage, and no cycles to worry about.
- **N6.** Compacting *during* a batch. See S5.

## Specification

### S1. Garbage is recorded, not discovered

A node is live if and only if it is reachable by
`first_child`/`next_sibling` from the **top level**.

The top level is a *forest*, not a single root: a document's top-level
fields are a sibling chain of parentless nodes (7 771 of them on the
reported `FileDescriptorSet`). Reachability must seed every member of
that chain — climbing `parent` from `first_node` lands on one
arbitrary member, and walking down from it alone reports most of the
document as unreachable. `build_tree` is post-order, so none of this
is arena index 0, which is a leaf.

Garbage enters at exactly two sites, both inside `splice_override`:

- the target's previous subtree (`old_descendants`, plus the
  `packed_orphans` absorbed by a packed sibling merge, spec 0135 G1);
- the pushed copy of the local root (`new_self_idx`), whose span and
  child links are folded into the surviving `tree[idx]` entry and
  which is then never referenced again.

The target `idx` itself is *not* garbage: it keeps its own arena index
across a retype (spec 0118 §7).

`splice_override` already computes both sets, so it records them in a
`dead: Vec<bool>` parallel to `tree` as it goes (G5). There is no mark
phase. The accounting was checked against the measured probe data:
4 501 014 initial − 7 772 survivors + 7 771 pushed local roots =
4 501 013 dead, and 9 000 349 − 4 501 013 = 4 499 336, exactly the
measured reachable count.

`dead_count` is maintained alongside the flags, because the event loop
asks whether a pass is worth starting on every idle iteration and
counting four and a half million booleans that often is precisely the
arena-wide work this design exists to avoid.

### S2. One node at a time

`relocate_node(from, to)` moves a live node into a dead slot and fixes
every reference to it:

- the parent's `first_child` and `last_child` (both, since for an only
  child both name it);
- `prev_sibling.next_sibling` and `next_sibling.prev_sibling`;
- `doc_prev.doc_next` and `doc_next.doc_prev`;
- every child's `parent`, by walking the moved node's own child chain
  — the one unbounded step, and the reason per-node cost is "degree"
  rather than a constant;
- `tree`, `heat_states`, `descend` and `dead` are swapped together,
  being parallel arrays;
- the two line maps, located through the moved node's own
  `text_range` and updated only where they actually name it;
- `folded`, `pending_heat_recheck`, `cursor`, `first_node` and
  `override_target`, by hash lookup or equality.

### S3. Why this is safe

The argument has the shape: the arena satisfies four properties; each
property is *checkable*; `relocate_node` preserves all four; and under
them, both the relocation and the final truncation are sound.

**W1 — `dead` is sound.** A node marked dead is genuinely unreachable.
The converse is not required: an unmarked dead node merely leaks, and
leaking is the status quo. Only this direction can lose data, because
a mismarked node's slot is treated as a hole, overwritten, and
truncated away.

**W2 — closure.** Every link field of a live node names a live node,
and every index holder names a live node.

**W3 — inverse links.** The four link pairs are mutually consistent
(`a.next_sibling = b` iff `b.prev_sibling = a`, and likewise for the
document chain); a node's child chain is exactly the set of nodes
claiming it as parent, and ends where `last_child` says; the document
chain covers exactly the live set.

**W4 — the line maps are filed under the node's own range.**
`line_to_node[l] = n` implies `n.text_range.start = l`, and
`footer_line_to_node[l] = n` implies `n.text_range.end = l + 1`.

*Completeness of the fix-up.* Enumerate who can hold index `n`. For
each of the seven link fields, the holder is uniquely determined by
`n`'s own fields: `x.first_child = n` or `x.last_child = n` implies
`x = n.parent`; `x.prev_sibling = n` implies `x = n.next_sibling`, and
symmetrically; likewise the document chain; and `x.parent = n` implies
`x` is on `n`'s child chain. Every one of these implications is an
instance of W3 — that is exactly what W3 buys, and without it the
child-chain walk in particular could miss a child. W4 does the same
job for the line maps. What remains is the five index-keyed holders,
found by lookup. So the list in S2 is complete, and `relocate_node`
leaves no reference behind.

*Safety of overwriting the destination.* `to` is dead, so by W2 no
live node and no holder names it. Nothing can observe the overwrite.

*Preservation.* After the swap, `to` holds what `from` held and every
reference has been rewritten; no other node's content changes; the
`dead` flags swap. W1–W4 therefore hold again — which is G3, and it
holds after each single move, not merely at the end.

*Safety of truncation.* The pass ends when no live node remains above
the lowest hole, so `[live, len)` is entirely dead. By W2 nothing
names any of it, and truncation is sound.

*Correctness of a reader between slices.* Readers follow links from
live nodes and read the index holders; by the above, all are correct.
This relies on one further property, which is pre-existing rather than
new: **no reader enumerates the arena by index**. A `0..tree.len()`
loop would see abandoned nodes — and would already do so today, since
they are equally present.

**What this change makes worse, and the mitigation.** Today a dangling
reference is *survivable*: an abandoned node is never freed and never
reused, so a stale index still resolves to a plausible node and the
symptom is a wrong render. Once slots are reused and the arena is
truncated, the same dangling index is an unrelated live node, or out
of bounds. Compaction removes the arena's tolerance for a class of bug
it does not itself introduce. That is the principal risk of this spec,
and the reason for S6.

The property was not merely assumed. Enforcing W2 on the index holders
turned up a real pre-existing violation: nothing repaired `cursor`
when a splice abandoned the node it named, so a `:type-as` on a node
inside the retyped subtree left the cursor on a dead node. Three tests
in the existing suite were in that state. The fix belongs to the
mutator, not to compaction — `splice_override` now redirects `cursor`,
`first_node` and `override_target` to the surviving `idx` and drops
abandoned heat rechecks, alongside the `folded` scrub it already did.

### S4. The pass

Two cursors over the arena. `compact_dst` seeks the lowest hole,
`compact_src` the lowest live node above it, and the latter is moved
into the former; `compact_dst < compact_src` always. Everything below
`compact_dst` is live and packed. The pass ends when either cursor
runs off the end, at which point the arena is truncated and
`shrink_to_fit` returns the reserved capacity — without which nothing
reaches the allocator (measured capacity 18 004 056 for 13 499 684
live entries).

**Revised by spec 0206 S6.** The `shrink_to_fit` is removed there.
Once slots are reused, shrinking capacity to exactly the live count
puts the release threshold and the `Vec` doubling threshold at the
same point, so the next allocation past it reallocates and copies the
whole arena to gain one slot. Plain `truncate` lowers `len` without
touching `capacity`, which returns nothing to the operating system and
is deliberately where 0206 stops. A release policy with hysteresis is
recorded there as future work.

It is **order-preserving**: the lowest hole takes the lowest live node
above it, so survivors keep their relative order. This is not free
elegance, it is required — `descend.len()` doubles as spec 0188 S4's
already-examined watermark, and every survivor was examined before the
pass, so truncating it to the new length keeps that meaning exact.

A pass is worth starting only when it reclaims a real share of the
arena, since it ends in a full reallocation. The gate is a fraction,
not an absolute floor: refusing below a garbage share of `1/N` bounds
the arena at `N/(N-1)` times its live set, which is G2 stated as a
guarantee. An absolute node count says nothing about a 4.5-million-node
document. `N = 8`.

### S5. When it runs, and what abandons it

A slice runs from `run_loop`'s idle path, strictly *behind* read-ahead
(spec 0192 S4): read-ahead is latency a user will feel, compaction is
housekeeping nobody is waiting on. A slice is bounded at 4 096 moves
and yields to any pending event exactly as read-ahead does.

A pass never runs while `override_batch_depth > 0` or while
`pending_line_patches` is non-empty. Mid-batch, spans and line maps
are transiently out of step (W4 fails) and patches name nodes by
index, so N6 is not a preference but a precondition.

Any splice abandons a pass in flight, by resetting both cursors.
Abandoning is free — a partially compacted arena is a fully consistent
one, and the cursors are the only state a pass carries. The reset
lives in `splice_override` and not in `render_overrides` because
`splice_override` is the arena's **only** mutator (the crate's only
`tree.push`) and is reachable outside any batch, via the live preview
splice in `override_select.rs`. Placing the reset on the batch alone
would leave the preview path silently corrupting a pass.

### S6. The invariants are checked, not asserted in prose

`App::verify_arena` checks W1–W4 directly and names the first
violation. It is `O(nodes + lines)`, so it is a test instrument, not
something a slice can afford, and it is compiled `#[cfg(test)]`.

That is a real limitation and not a tidy one: the documents most
likely to expose a violation are the multi-million-node ones no
fixture reaches, and those are reachable only through the shipping
binary. A hook that let it be run on demand there would close the gap.
It is left out of this spec because the change it would verify is
already the risky one, and because it needs a way to report a failure
from a process that owns the terminal — the same problem `trace.rs`
solves for instrumentation, and the natural place to solve it again.

It runs from `assert_repair_matches_full_rebuild`, spec 0186's
existing per-batch verification hook, which means every override test
in the suite — including `randomized_override_sequences_keep_every_
span_consistent` — becomes a witness for the properties compaction
depends on, at the one moment they are required to hold. This is
deliberately much broader coverage than compaction's own fixtures
could give, and it is what found the `cursor` violation in S3.

The list of index-keyed holders in `relocate_node` and `verify_arena`
is the one thing here that no compiler enforces: a new `App` field
holding a node index must be added to both. Stated as a standing
obligation because there is no way to make it fail otherwise.

### S7. A note on the latent `u32` truncation

`line_to_node` and `footer_line_to_node` store node indices as
unchecked `as u32` casts. Today that is a live hazard, because the
arena is unbounded — enough batches on any document eventually
truncate a real index and silently mis-map a line. Bounding the arena
does not make the cast checked, but it does make it unreachable for
any document that fits in memory. N1's narrowing should add the
checked conversion.

## Test plan

Every test asserts through a *content* projection (level, field
number, type, text range), never through raw indices: renumbering is
the entire point of the operation, so an index comparison would report
differences that say nothing about what the user sees.

1. `a_completed_pass_reclaims_the_garbage_and_preserves_the_document`
   — the arena shrinks, `dead_count` is zero, and the lines, the live
   tree, both line maps, the cursor and the viewport are unchanged.
2. `the_arena_is_consistent_after_every_single_move` — the same
   comparison plus `verify_arena` after *every* move, at a budget of
   one. This is the test for G3, and the one the design rests on; test
   1 alone only checks the endpoint.
3. `index_keyed_state_follows_the_node_it_names` — folds, heat
   rechecks and the override target, which are repaired by a mechanism
   separate from the pointer fix-up.
4. `a_splice_abandons_the_pass_in_flight` — the cursors reset, the
   arena stays well-formed, and it remains compactable afterwards.
5. `a_clean_arena_is_left_alone` — the gate is a counter read and
   truncates nothing.
6. `the_verifier_rejects_a_broken_arena` — a dangling link, a
   mismarked live node, a miscounted `dead_count` and an inconsistent
   child chain are each rejected. Without this the suite could pass by
   checking nothing.
7. The spec 0202 reproduction end to end: three `t`/`Enter`/`o`/`d`
   cycles on `googleapis.desc` doubled, with no refusal and a flat RSS.

## Measured outcome

Implemented 2026-07-28. The reproduction from Background, run to six
batches instead of the three that used to reach the OOM killer
(`Down`, then three times `t`, `Enter`, `o`, `d`, `Esc`):

| batch | `tree` at start | `tree` at end | settled RSS |
|---|---|---|---|
| 1 | 4 499 336 | 8 998 671 | 2594 MiB |
| 2 | 4 499 336 | 8 998 671 | 2614 MiB |
| 3 | 4 499 336 | 8 998 671 | 2614 MiB |
| 4 | 4 499 336 | 8 998 671 | 2615 MiB |
| 5 | 4 499 336 | 8 998 671 | 2642 MiB |
| 6 | 4 499 336 | 8 998 671 | 2722 MiB |

The arena is flat. Before this spec the same figures were 4 501 014,
9 000 349, 13 499 684 and then death; every batch now begins from the
live set, because a pass has run to completion in the idle time
between keystrokes. G1 holds, and with room to spare: the observed
ratio is 1.0, not the 8/7 the gate permits.

Two things the table does not show, both worth stating plainly:

- **The peak is unchanged**, at 3.9–4.0 GiB. Compaction reclaims the
  superseded copy but does not stop it existing, so the arena is flat
  when observed between batches and doubled during one. That is N2
  here, not a shortfall of this spec — but the reason given for it
  above ("a batch materializes every replacement subtree before any of
  the originals become garbage") does not hold, and spec 0206 takes
  the peak by allocating from the holes the batch has already made.
- **A slow drift**, +128 MiB across six batches, in settled RSS while
  `tree.len()` is exactly constant. It is therefore not the arena. The
  remaining candidates are the heat and render caches and allocator
  retention around the `shrink_to_fit`; unmeasured, and left as such
  rather than guessed at.

The spec 0202 refusal never fired during the run — which is the point
of it now being a backstop rather than the mechanism.
