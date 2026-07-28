<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0162 — protolens: general reclamation of abandoned tree nodes

Status: draft
App: protolens
Refs: docs/protolens/rendering-flaws.md (D5, P3),
      docs/protolens/rendering-scaling-roadmap.md (S7),
      docs/specs/0160-protolens-render-overrides-batch-scaling.md,
      docs/specs/0161-protolens-preview-node-reclamation.md,
      docs/specs/0183-prune-the-override-walk.md,
      docs/specs/0186-the-commit-touches-only-what-moved.md,
      docs/specs/0188-the-batch-updates-what-changed-not-what-exists.md

## Background

`splice_override` (`override_apply.rs`) never removes anything from
`self.tree: Vec<TreeNode>`. Every call — whether previewing a
candidate or confirming a real override — decodes a fresh local
subtree, appends it to `self.tree`'s tail, and rewires the overridden
node's own pointers to the new content; the previous content becomes
unreachable via live traversal but stays physically resident in the
`Vec` forever ("abandon in place", `override_apply.rs:1223-1226`).
`self.heat_states` is kept parallel to `self.tree` and grows the same
way.

Spec 0161 bounds this growth for the *disposable live-preview* path
specifically (repeatedly moving the highlight in the override pane),
since a preview's content is known, by construction, to be throwaway
the moment a new preview supersedes it.

Confirmed override commits are different: each one permanently
replaces a node's content, and a long interactive session can
confirm overrides on many distinct nodes, or repeatedly re-override
the same node many times, or apply a batch operation touching many
nodes (spec 0160) — all of it appended to the same `self.tree` with
none of it ever reclaimed. Unlike a preview, none of this is knowable
in advance to be safely truncatable by a simple watermark: many
different, unrelated nodes' abandoned subtrees can be interleaved
with live content across the session's lifetime, and other `App`
state (cursor, `folded`, `line_to_node`/`footer_line_to_node`,
override-management-pane state, etc.) may hold indices into any of it.
Reclaiming this general case therefore requires an actual compaction pass
and remapping of every index-based reference across `App`'s state, not
just a truncation of the most recent addition. (It does *not* require a
reachability analysis, as this section originally assumed — see "Garbage
has exactly one source" below.)

Over a sufficiently long interactive session with many confirmed
overrides, `self.tree`/`self.heat_states` therefore still grow
unboundedly even after spec 0161 lands, with no way to reclaim any of
it short of restarting protolens.

### What the growth actually costs (measured 2026-07-27)

On `/tmp/pdb.desc`, `tree.len()` over a session's first few structural
operations:

| after | `tree.len()` | arena bytes |
|---|---|---|
| initial decode | 146,511 | 37 MB |
| root-type resolution | 382,032 | 98 MB |
| first nested commit | 1,213,544 | 311 MB |
| second nested commit | 2,232,224 | 572 MB |

`size_of::<TreeNode>()` is **256 bytes** (`size_of::<NodeSpan>()` = 96,
seven `Option<usize>` link fields at 16 each). The bytes column is
`tree` alone; it excludes the heap each node's `NodeSpan.type_fqdn` and
`TreeNode.rendered_as` own, and excludes the parallel `heat_states`.

Growth compounds with everything else: spec 0183's mark pass, spec
0188's target scan, and every future per-node index are all sized by
`tree.len()`, so they are sized by *cumulative* session history rather
than by the document. Reclamation is the only thing that breaks that
coupling.

### Deadness is monotone

Established by reading the code rather than assumed, because the whole
design below rests on it.

There is no undo path in protolens. Every assignment to a tree link
(`parent`, `first_child`, `last_child`, `next_sibling`, `prev_sibling`,
`doc_next`, `doc_prev`) occurs in exactly two places: `build_tree`
(`decode.rs`) and the splice (`override_apply.rs`). The splice only ever
rewires a node that is *itself still live* — the overridden node
survives and receives freshly appended tail content, while its previous
children are orphaned in place (`override_apply.rs:2209`). Nothing
anywhere re-links an orphan back into the live structure, and the arena
never reuses an index, so no new node can alias an old one.

**Therefore: once unreachable, always unreachable.** A reachability
analysis computed against a stale snapshot is a *sound
underapproximation* — it can miss garbage that appeared after the
snapshot, but it can never classify a live node as dead. This is what
makes an off-thread analysis safe without write barriers, without
locking, and without invalidation on mutation.

### Garbage has exactly one source, and it is already enumerated there

Added 2026-07-27, after spec 0188. This is the finding that most changes
the design below, so it is stated before the design rather than buried
in it.

Garbage is created in exactly one place: `splice_override`
(`override_apply.rs`). It re-decodes the overridden node `idx`'s payload
into a fresh subtree appended at `base = tree.len()`, rewires `idx`'s own
child and document links to that new content, and leaves `idx`'s previous
descendants in place. `idx` itself always survives — which is why
`first_node` never moves, and why spec 0188 S8 can read the root off a
field instead of searching the arena for it.

So the dead set never has to be *discovered*. It is already materialized
at the moment of death, for an unrelated reason: `splice_override`
computes

```rust
let mut old_descendants = Vec::new();
self.collect_descendants(idx, &mut old_descendants);
old_descendants.extend(packed_orphans);
for d in &old_descendants {
    self.folded.remove(d);
}
```

(`override_apply.rs:2146-2152`) purely to scrub stale fold state, and
that vector is precisely the nodes this splice just killed.
`collect_descendants` follows *live* child pointers, so each splice
enumerates only the newly dead: a node orphaned by an earlier splice is
no longer reachable from `idx` and was enumerated at its own splice. The
union over splices is therefore the complete garbage set — no omissions,
no double counting. Monotonicity (above) is what makes accumulating it
valid: an index put into that union never has to come back out.

This deletes the expensive half of the collector. The mark phase was the
part that needed a snapshot, a worker thread, a version stamp and a
discard-and-retry, and it existed to answer a question the mutator can
simply answer as it goes. What remains is a mechanical copy with no
analysis in it.

### The side tables are the real obstacle, and they are shared

What is *not* solved by the above is that `App` holds bare arena indices
outside the tree, and a structurally dead node that one of them still
names is not safe to remove:

| holder | what it holds | scrubbed at the splice? |
|---|---|---|
| `cursor` | node index | no |
| `back_stack` / `fwd_stack` | node indices (`record_jump`, `key_dispatch.rs:181`) | no |
| `folded` | node indices | **yes** — the scrub quoted above |
| `override_target` | node index | no |
| `pending_heat_recheck` | node indices | no |
| `heat_states` | parallel array | kept parallel; `heat_states[idx]` reset |
| `line_to_node` / `footer_line_to_node` | node indices as *values* | repaired per batch |

Two entries matter for what they are *not*: `render_cache` is keyed by
`(Range<usize>, Option<String>, bool)` — a payload **byte** range, not a
node index (`render_cache.rs:32`) — and `prefetch_walk` holds rows plus a
`structural_version` guard rather than arena indices. Neither needs
remapping.

Making that column say "yes" everywhere is the shared prerequisite for
*both* candidate designs: the copying collector needs it so the remap has
a complete list, and slot reuse (A2) needs it so a reused slot cannot be
reached through a stale index. It is also worth doing on its own merits —
a `cursor` left on an abandoned node is a latent bug today. See Q2.

## Goals

- **G1**: Reclaim (physically remove from `self.tree`/`self.heat_states`)
  nodes that are no longer reachable from the live document structure
  (`first_node`'s doc-chain and/or parent-child/sibling pointers),
  once nothing in `App`'s state still references them.
- **G2**: Remap every index-based reference across `App`'s state
  consistently whenever reclamation runs, so no field is silently
  left pointing at a stale or now-invalid index. This includes (at
  least) `cursor`, `folded`, `heat_states`, `line_to_node`/
  `footer_line_to_node`, `override_target`, override-management-pane
  state, and `active_override_range`/`override_seek_target`.
- **G3**: Bound total steady-state memory/tree size for arbitrarily
  long interactive sessions with many confirmed overrides, independent
  of how many overrides have cumulatively been applied over the
  session's lifetime.
- **G4**: Run reclamation at a cost and/or frequency that does not
  itself introduce a new user-visible stall — triggered once accumulated
  garbage crosses a threshold (S5), and off-thread if S3's synchronous
  cost turns out to warrant it (Q3).

## Non-goals

- **N1.** Reclaiming anything other than `tree`/`heat_states`. The
  render cache, the blob, and the override collection have their own
  bounds (specs 0163, 0174; flaw P4).
- **N2.** Reducing `size_of::<TreeNode>()`. Worth doing separately —
  seven `Option<usize>` at 16 bytes each would be seven `Option<u32>` at
  4 if the arena were `u32`-indexed, halving the struct — but it is an
  orthogonal change that does not need this spec and would conflict with
  it if done concurrently.
- **N3.** Reclaiming *during* a batch. Collection runs between
  user-visible operations, never inside `render_overrides`.
- **N4.** Making the collector concurrent with mutation. Whether the copy
  runs off-thread at all is Q3; either way the swap does not overlap a
  commit.

## Specification

The design is a **copying collector fed by the mutator**. The splice
records what it kills; collection is then a single linear pass that
rebuilds the arena without the dead and remaps every index that named a
survivor. There is no reachability analysis, and consequently no
snapshot, no version stamp and no retry protocol.

### S1 — the root set

A node is *live* if it is reachable from `first_node` by `doc_next`, or
from a live node by `first_child`/`next_sibling`. That is the structural
definition, and S2 maintains its complement directly.

It is **not** sufficient on its own, because of the bare indices
tabulated in Background ("The side tables are the real obstacle"). Two
admissible resolutions:

- **(a)** retarget each such field whenever a splice orphans the node it
  names, and treat the structural definition as complete; or
- **(b)** add each field to the root set, keeping its node (and its
  ancestors) alive until it moves.

(a) is preferable — a stale `cursor` is a latent bug independent of this
spec, and (b) pins arbitrary amounts of garbage behind one stale
reference. It must be *established*, not assumed. See Q2.

### S2 — the mutator maintains the dead set

`App` gains a `dead: Vec<u32>` (or a bitset sized to `tree.len()`; the
choice follows from the survivor ratio, which S5 now measures for free).
`splice_override` appends `old_descendants` to it, at the site where that
vector is already built for the `folded` scrub — so the marginal cost of
knowing the entire garbage set is one `extend`.

This replaces the mark phase. Its correctness rests on the two
Background results: the splice is the only producer of garbage, and
deadness is monotone, so the accumulated set is exact and never needs a
removal operation.

`dead` is cleared by S3, since everything in it has by then been dropped.

### S3 — the copy

At a quiet moment — no batch in flight, no pending patch (N3):

1. Build `old_to_new: Vec<Option<u32>>` from `dead`, assigning survivors
   consecutive indices in arena order — a prefix-sum pass over `dead`
   in bitset form (materialized from the `Vec` first, if S2 took that
   shape), no walk.
2. Allocate the new `Vec<TreeNode>` at exactly the survivor count and
   fill it, rewriting each link field through `old_to_new` as it goes.
   The same map rewrites `heat_states`.
3. Rewrite `App`'s own indices through `old_to_new` (S1), and remap
   `line_to_node`/`footer_line_to_node`, whose *values* are node indices.

Arena order, not document order. Reordering into document order would be
a real locality win — after enough splices a node's children sit
arbitrarily far from it — but it costs a `doc_next` chain walk over every
survivor, which is exactly the pointer-chasing this design otherwise
avoids, and it is separable: the collector is correct either way, so the
reordering can be added later behind its own measurement.

As written, every step is O(arena) with a small constant, with no walk
and no hashing anywhere. That is the same cost class as the full line-map
rebuild a commit used to perform on every batch before specs 0186/0188,
which is the evidence for expecting it to be affordable synchronously —
but it is an expectation, not a measurement, and Q3 is what the design
now reduces to.

### S4 — if the copy turns out to need a worker after all

Recorded because the reasoning is non-obvious and would otherwise have to
be rediscovered. Not part of the design unless Q3 says so.

Should S3 prove too slow to run on the main thread, it can be handed an
`Arc<Vec<TreeNode>>` snapshot of the arena at length `N` and run
off-thread — and, unlike the original design, its result never has to be
discarded when a commit lands during the copy. The main thread's
mutations in that window are an append of fresh nodes `N..M` plus
additions to `dead`, so the result is applied by concatenating the
carried-over tail and remapping its links: any pre-snapshot node a fresh
node links to is live *now*, hence was live at snapshot time by
monotonicity, hence has a mapping. The dead indices recorded during the
window are mapped through `old_to_new` and kept for the next collection
rather than cleared.

So the version stamp, the staleness check and the discard-and-retry of
the original design are all unnecessary in either arrangement. What
remains open is only the snapshot's own cost (Q3).

### S5 — triggering

Collection is requested when `dead.len()` crosses a fraction of
`tree.len()` — an exact count now, not an estimate, since S2 accumulates
it. A simple ratio (e.g. collect when the dead are more than half the
arena) satisfies G4 without a policy that needs tuning. It must never be
requested from inside a batch (N3).

The same counter answers Q1's survivor-ratio half as a side effect: after
S2 lands, `dead.len()` at any moment *is* the measurement, so the
question can be settled by a `dbg!` rather than by a study.

## Alternatives considered

### A1 — worker queues chunks, main thread compacts them incrementally

The natural first design: have the worker identify regions worth
compacting and queue them, so the main thread does bounded work per
frame at its own pace.

**Rejected: it makes the total cost worse, not better.** To move a
chunk's survivors, every link *pointing into* that chunk must be
rewritten — and after repeated splices, a parent and its child sit
arbitrarily far apart in the arena, so those links can be anywhere.
Without a reverse index, locating them requires a full arena scan **per
chunk**, turning one O(arena) pass into K × O(arena). Chunking pays off
when fix-up is local to the chunk; here it provably is not.

The instinct behind it is right, though, and S2/S3 are that instinct
with the handoff moved: rather than queueing chunks of *work*, the
worker queues the finished *result*. The main thread's blocking cost is
then bounded by the root remap alone, independent of how much garbage
accumulated.

### A2 — free list, reusing dead slots for future nodes

Keeps survivor indices stable, so nothing needs remapping, and the sweep
is trivially incremental.

**Adopted, as spec 0206** (revised 2026-07-28). It was rejected here on
one ground — aliasing — and that ground was answered by work this spec
did not anticipate: spec 0203 built `verify_arena`, which is precisely
the "moment" the paragraph below says a free list lacks. The rest of
the analysis held up and 0206 uses it, so it is left standing rather
than rewritten, with the verdict corrected.

The original reasoning follows.

**Rejected, but on one ground rather than two** (revised 2026-07-27).

The contiguity objection is weaker than it first looked. Both the splice
(`override_apply.rs:1467`) and the preview path (`:2157`) take
`base = self.tree.len()` before appending and then treat the *contiguous
range* `base..tree.len()` as "the nodes this operation just created" —
`mark_fresh_subtree` passes exactly that range to
`collect_descend_targets` (`:1221`), as does spec 0183's fresh-node
marking. But a free list can *supply* contiguity: the orphans one splice
creates are usually already a contiguous run, since a post-order decode
lays a subtree out contiguously and a splice appends its replacement in
one block. A free list of runs, serving each request from a single run
and reporting the run actually used, keeps `base..base + len` meaningful.
Spec 0188's `descend` watermark would need the same treatment, since it
uses `descend.len()` to mean "the arena prefix already examined" and
that identity assumes append-only growth.

What does stand is aliasing, and it is the serious one. Today a stale
index points at abandoned garbage, which renders oddly and is noticed;
with slot reuse it points at a *different live node*, which renders
plausibly and wrongly. That is the same hazard the copying collector
has — both need the side-table scrub of Background — with one difference
that decides it: the collector remaps every index it knows about in one
pass and can be *asserted* correct afterwards (test plan 2), whereas a
free list has no such moment, and a table entry someone forgets to scrub
stays wrong indefinitely.

> That last clause is what stopped being true. `verify_arena` (spec
> 0203 S6) decides the same property — every link and every index
> holder names a reachable node — without needing a collection cycle to
> hang off, and spec 0186's hook runs it after every batch in the
> override suite. The hazard is unchanged and is spec 0206's principal
> risk; what changed is that it is now detectable.

And it still does not return memory: peak `tree.len()` never drops, it
only stops rising.

> Also still true, and now the division of labour rather than an
> objection: spec 0206 stops the rise, spec 0203 does the returning.
> The contiguity paragraph above is the one 0206 leans on hardest —
> its point that a free list can *supply* contiguity is right, and
> address-ordered first fit over a bitmap supplies it without needing
> a list of runs at all. The `descend` watermark it flags is spec 0206
> S4.

### A3 — payload-only drop

Keep every slot, but free the heap a dead node owns (`NodeSpan.
type_fqdn`, `TreeNode.rendered_as`'s strings).

**Rejected as a substitute; retained only as a stopgap** (revised
2026-07-27).

Its constraint profile is excellent: no index moves, nothing to remap, no
root-set question, incremental at arbitrary granularity (N nodes per
frame, stop whenever), safe with no snapshot or version protocol at all.
What it does not do is recover the 256-byte slots, so `tree.len()` keeps
growing and G3 is not met.

The earlier draft left open whether it might nevertheless be *most of the
win*. It is not, and the split is settled by inspection rather than
needing a heap profile. `size_of::<TreeNode>()` is 256 bytes against, per
node: `NodeSpan.type_fqdn: Option<String>`, which is `None` for every
scalar field and a fully-qualified name (tens of bytes) where present,
and `TreeNode.rendered_as`, a pair of small strings present only on nodes
a splice has rendered. Even taking the generous end of both, the slot
outweighs the heap by roughly 7:1, so a payload-only drop recovers on the
order of 10% — it defers the problem by a few overrides and no more.

The corollary is worth stating plainly, since it runs against intuition:
in this arena it is the *indexes* that are worth reclaiming, not the
rendered text they carry.

### A4 — handle indirection

Give every node a stable handle, resolved through an indirection table,
so compaction never invalidates an external reference.

**Rejected:** it adds a dereference to every node access, including the
render and navigation hot paths, to avoid a remap that happens at most
a few times per session.

## Test plan

1. **The accumulated dead set matches a from-scratch reachability
   analysis.** After a sequence of commits, `dead` must equal exactly the
   complement of what a live-structure walk reaches. This is the pair of
   Background claims S2 rests on — one producer, monotone deadness —
   checked together and directly, and it subsumes the weaker
   "monotonicity holds" test the earlier draft proposed.
2. **The collection is behavior-preserving.** After a collection, the
   rendered document, `visible_rows`, `line_to_node`,
   `footer_line_to_node` and the cursor's resolved node are identical to
   what they were before — compared directly, in the style of spec 0186
   G3's equivalence assertion.
3. **Every root survives.** Fold some nodes, place the cursor deep,
   open the manage pane with a selection, collect, and assert each of
   those still resolves to the same logical node.
4. **The arena actually shrinks**, on the pdb.desc-shaped fixture: after
   two nested commits and a collection, `tree.len()` is close to the
   live count rather than the cumulative count.
5. **Collection never runs inside a batch** (N3), asserted from the
   trigger site.
6. Only if S4 is taken: **a commit during the copy phase is carried, not
   discarded.** Force a commit between snapshot and swap and assert the
   post-snapshot tail survives with its links remapped, and that the dead
   recorded in the window are preserved for the next collection.
7. Existing suite unchanged: `cargo test --release --bin protolens`,
   `cargo clippy --release --no-default-features --workspace -- -D
   warnings`, `reuse lint`, `nix-build -A ci`.

## Open questions

- **Q1 — what is the survivor ratio?** *Half-answered, 2026-07-27.* The
  heap/slot half is settled: the slot dominates ~7:1, so A3 is not a
  substitute and copying is the only design that meets G3. The survivor
  half stops being a gating study — S2's `dead` counter measures it
  exactly, for free, and the growth table above already bounds it well
  below 1 (the arena grew 5.8× across two nested commits while the
  document gained two retyped subtrees). Land S2 first and read the
  counter.
- **Q2 — is `cursor` (and each other bare index in Background's table)
  always retargeted when a splice orphans the node it names?** Determines
  whether S1 takes branch (a) or (b), and it is the prerequisite shared
  with A2. Worth answering regardless: if the answer is no, that is a
  latent bug today, independent of reclamation.
- **Q3 — is S3's copy affordable on the main thread?** This is what the
  old "can the worker be snapshotted without doubling peak memory?"
  reduces to now that the analysis is gone. If yes, S4 and the whole
  worker apparatus are deleted. If no, S4 applies, and the snapshot
  question returns in its original form: `Arc`-sharing costs a full
  duplicate at exactly the moment memory is scarcest, and the mitigation
  is to run the copy only while genuinely idle and abandon it on the
  first keystroke.
- **Q4 — does `heat_states` need the same treatment, or can it be
  rebuilt from scratch after a swap?** It is a parallel array, so
  remapping it is free given `old_to_new`; but if its contents are
  cheaply recomputable, dropping it is simpler.
