<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0183 — prune the override walk to the subtrees that can change

Status: draft
Implemented in:
App: protolens
Refs: docs/protolens/rendering-flaws.md (P1, P2, P3),
      docs/protolens/rendering-scaling-roadmap.md (S1),
      docs/specs/0119-protolens-override-fidelity-and-workflow.md,
      docs/specs/0120-protolens-any-messageset-as-auto-overrides.md,
      docs/specs/0160-protolens-render-overrides-batch-scaling.md,
      docs/specs/0174-preview-interior-truncation-and-node-budget-removal.md
      (G1),
      docs/specs/0184-packed-records-are-the-addressable-unit.md

## Background

`render_overrides_inner`'s child-recursion gate
(`protolens/src/tui/override_apply.rs:1283-1289`) is four disjuncts:

```rust
if self.tree[c].span.is_message
    || self.is_auto_expand_candidate(c)
    || self
        .resolve_active_override_entry_index_by_path(c, &c_path)
        .is_some()
    || self.tree[c].rendered_as.is_some()
```

The first disjunct is what makes the walk O(document), and it is worth
being precise about *why* it is there. Mostly it is not there because a
message node needs work; it is there because a message node is the only
kind that can *contain* a node needing work, and the walk has no other
way to find out. `is_message` is a stand-in for "something under here
might matter" — the weakest such predicate available, and it is true of
essentially every interior node in a real document.

**But not only that**, and the exception is a trap for this spec. See
S1's second half: `is_message` is also the *sole* discovery mechanism
for MessageSet tier-1 wrappers. Deleting the disjunct without replacing
that role silently breaks MessageSet auto-expansion.

Measured on the 1.1 MB `FileDescriptorSet` (622,922 nodes / 193,072
lines), release:

| | |
|---|---|
| `App::new` — dominated by the startup `render_overrides` | **5.22 s** |
| `Enter` (commit override #1) | **10.6 s** |
| `Enter` (commit override #2) | 8.2 s |

[rendering-flaws.md](../protolens/rendering-flaws.md) P1 records the
sharpest form of the waste: on a blob containing no `Any` and no
`MessageSet` — the common case for a `FileDescriptorSet` — that entire
5.22 s "produces literally no change to the rendering."

Each visited child also pays
`resolve_active_override_entry_index_by_path` (`:881-903`), which is
**three** successive linear scans over `overrides.entries()` — one per
origin kind. That is P2's cliff, and the gate pays it once per node per
pass.

### The gate has two halves, and they want different fixes

Disjunct 2 is schema-driven: `is_auto_expand_candidate` asks whether a
node's *parent* is `Any`-typed, or its parent is a MessageSet `Item` —
a structural property of the decode, with no relation to what the user
has overridden. Disjunct 1 carries a second, hidden schema-driven role
alongside its structural one (S1).

Disjuncts 1, 3 and 4 are override-driven: they exist to reach nodes
whose rendering an override changes.

The override-driven half admits a prefix argument. Overrides are keyed
by `OverrideOrigin` (`override_pane.rs:140-147`), and two of its three
variants — `Path { path }` and `PathField { path, field }` — are
*paths*. A subtree rooted at path `P` cannot be affected by an override
whose path does not have `P` as a prefix. So the question "must I
descend into this subtree" is answerable without descending into it.

The schema-driven half admits no such argument, but it does not need
one: the candidate set is computable in a single cheap structural pass,
which is P1's proposed correction.

**Neither fix alone removes `is_message`.** Precomputing auto-expand
seeds still leaves the walk descending everywhere to find overrides;
prefix-pruning overrides still leaves it descending everywhere to find
`Any` nodes. Only both together let the disjunct go. That is why this
is one spec and not two.

### Why this reuse argument is sound, where the root-retype one is not

It is worth recording the contrast, because the two look alike and only
one works.

Re-rendering after a **root retype** cannot reuse subtree structure:
whether a subtree's shape survives depends on the new schema in
non-local ways (opaque↔nested, `Any`/MessageSet expansion, packed↔
unpacked repeated scalars), so deciding "did this subtree change" is a
schema-directed walk of the subtree — the very work being avoided. The
predicate costs what it saves.

Re-rendering after an **override** is the opposite: the set of nodes
whose rendering changed is *exactly* the set of override targets and
their descendants, and that set is known before the walk starts. The
predicate is a string prefix test.

## Goals

- **G1** — `is_message` leaves the recursion gate. Descent visits the
  marked subtrees and the freshly spliced content, not the document.
- **G2** — the startup pass costs O(auto-expand seeds). On a document
  with no `Any` and no MessageSet it does no work at all, instead of
  5.22 s of work with no effect.
- **G3** — the gate's per-node cost stops being three linear scans over
  `entries()`.
- **G4** — pruned and unpruned renders are byte-identical, asserted
  differentially rather than argued.
- **G5** — no change to any rendered byte, ever, under any input. This
  is a pure work-avoidance change.

## Non-goals

- **N1** — the four O(document) passes that follow a splice. They are
  downstream of `finalize_override_batch` and pruning cannot touch any
  of them:
  1. `materialize_line_patches` deep-cloning ~193k `String`s to apply
     one patch;
  2. the `doc_next` walk at `:1338-1342`, shifting every node after the
     target;
  3. the unconditional `line_to_node`/`footer_line_to_node` clear and
     full-document reinsert;
  4. the two `HashMap::retain` passes in `override_select.rs:829-830`.

  These are P3's problem and want their own spec. **A reader should not
  expect this spec to move the 10.6 s commit figure much.** Its target
  is the 5.22 s startup pass, where there is no splice and therefore
  none of the above.
- **N2** — the live-preview path. Previews are moving to an overlay,
  which is a different and already-cheap path; this spec governs the
  committed walk only.
- **N3** — arena reclamation (spec 0162, tree-node reclamation / W23).
- **N4** — any change to override semantics, precedence between the
  three origin kinds, or the `rendered_as` fallback rule.
- **N5** — whether a root retype should rebuild from scratch rather
  than splice. Related, separately decided.

## Specification

### S1. Precompute auto-expand seeds (rendering-flaws P1)

`App::new` currently calls `render_overrides(cursor)` on the root purely
so that `is_auto_expand_candidate` gets evaluated at every node. Replace
it with a structural scan collecting `auto_expand_seeds: Vec<usize>`,
then call `render_overrides` once per seed, wrapped in a single explicit
batch (increment `override_batch_depth` around the loop, finalize once)
so `k` seeds do not pay `k` finalizations.

The scan must stay cheap. `is_any_typed` (`:637`) is a string compare
and costs nothing, but `is_message_set_typed` (`:649`) does a
`pool().get_message_by_name` lookup — so the scan must apply the cheap
`field_number` and `type_fqdn` discriminators first and reach the pool
only for the few nodes that pass them. This ordering is already what
`is_auto_expand_candidate` does; the requirement is not to lose it.

**The seed predicate is not `is_auto_expand_candidate`.** This is a
defect in the obvious implementation, and it is invisible on the current
fixtures. `is_auto_expand_candidate`'s own comment (`:730-733`) says:

> MessageSet tier 1 (the "Item" group wrapper itself) needs no entry
> here: it's already `is_message == true` naturally (a real decoded
> group), so `render_overrides`'s own `is_message` half of its recursion
> gate already reaches it.

So the predicate deliberately returns `false` for tier-1 wrappers, on
the explicit grounds that the disjunct this spec deletes will find them.
It is `auto_expand_type` (`:777`) that handles them, and it is never
reached if the gate does not let the walk in. Delete `is_message`
without acting on this and MessageSet auto-expansion stops working,
quietly.

The seed predicate must therefore be `is_auto_expand_candidate(c)`
**or** the tier-1 wrapper shape: field number 1, `WT_START_GROUP`, and
`is_message_set_typed(parent)`. The cleanest form is to widen
`is_auto_expand_candidate` itself to include tier 1 and delete the
comment above, so one predicate defines "auto-expandable" for both the
seed scan and the gate — but that changes an existing predicate's
meaning, so whichever way it goes, it must be deliberate.

Note how nearly this shipped unnoticed: the 1.1 MB fixture is a
`FileDescriptorSet`, which contains no MessageSet at all, so the
differential test in the test plan would have passed on it. A MessageSet
document has to be added to the corpus deliberately or the test proves
nothing about this path.

At startup the only override entry that can exist is the seeded root
type, already reconciled by the `rendered_as` write at `mod.rs:1223`, so
no other node can need resettling and the seed set is provably complete.
This scoping is load-bearing — see L1.

### S2. A descent mark, computed before the walk

Introduce a bitset `descend: FixedBitSet` of length `tree.len()`, where
bit `i` means "node `i` is itself a visit target, or has one in its
subtree". Compute it at the start of each batch from three sources:

1. every `OverrideEntry` in `overrides.entries()` whose origin is
   `Path` or `PathField` — **regardless of `active`** (see L3);
2. every node with `rendered_as.is_some()`;
3. the auto-expand seeds, where S1's scan is still valid.

Mark each target, then walk `parent` links upward from it, setting bits
until an already-set bit is reached. That is O(targets × depth) with
early termination, plus the O(n) scan for source 2.

The gate becomes:

```rust
if fresh || self.descend[c] {
    self.render_overrides_inner(c, child_owed, &c_path, child_scope, fresh);
} else if child_owed != 0 {
    Self::shift_span(&mut self.tree[c].span, child_owed);
}
```

**On the honest cost.** Source 2 is an O(n) pass over the tree, so this
does not make the walk sublinear, and the spec should not be read as
claiming it does. What it removes is the *constant*: the current O(n)
pass formats a path `String` per node, runs three linear scans over
`entries()`, and calls `resettle_node`. The replacement O(n) pass reads
one `Option` per node. That is the whole win, and it is a large one
precisely because the current constant is enormous — but it is a
constant-factor win, not an asymptotic one.

Making it genuinely sublinear means maintaining `descend` incrementally:
`rendered_as` changes only at splice sites, seeds only inside fresh
content, entries only on user action. That is a strictly better end
state and a strictly worse place to introduce a silent bug, so it is
deliberately *not* specified here. Land the rebuild, measure, and open a
follow-up only if the residual O(n) shows up.

### S3. Freshly spliced content is descended in full

A subtree that was just re-decoded contains nodes that did not exist
when `descend` was computed, and may contain new auto-expand candidates
(see L1). Carry a `fresh: bool` parameter, set when `resettle_node`
returned a patch, and descend unconditionally beneath it — bounded by
the size of the spliced content, which is the work the splice implies
anyway.

### S4. Prefix queries use the existing sort invariant

`OverrideCollection` already keeps `entries` sorted lexicographically by
`OverrideOrigin::label()` (`override_pane.rs:225-234`), and there is
already a test pinning that (`:982`). A prefix query is therefore a
`partition_point` to the first label ≥ the prefix, then a short forward
scan while the prefix still matches — no new data structure, and no new
invariant to maintain.

The exact boundary condition must remain `origin_is_at_or_under`'s
(`:169-195`): a genuine path-segment boundary, so `/3` matches `/3/2`
and `/3:5` but **not** `/30`. A raw `starts_with` is wrong and the
existing helper already says so.

This is also the replacement for `resolve_active_override_entry_index_
by_path`'s three linear scans (G3), which should be rewritten against
the same sorted access.

**Precondition: a stored path must keep designating the same node.**
Prefix pruning is only as sound as the paths it prunes with. Today a
packed run's elements are separately numbered children, so committing an
override on a run collapses N siblings to 1 and shifts every later
sibling's ordinal — an override entry stored earlier then designates a
different node. Under the current full walk that is a pre-existing bug
with limited blast radius; under prefix pruning it becomes a *pruning*
error, which fails as stale text rather than as a wrong override. Spec
0184 makes the packed record the addressable unit and removes the
instability. It should land first.

### S5. `FqdnField` origins

`FqdnField` is not path-shaped, and `origin_is_at_or_under`'s own doc
comment states it "never prefixes, nor is prefixed by, anything but its
own label." It therefore cannot be pruned by path.

Before choosing a mechanism, reframe the problem, because it is less
hard than it looks and hard in a way no mechanism can fix. **An
`fqdn:field` override on a common type genuinely does affect nodes all
over the document.** When the marking work comes out large, that is not
the criterion failing — it is the true blast radius. The goal is a
criterion whose cost is proportional to real impact, not one that makes
a genuinely global change cheap.

**Specified: an exact FQDN index.** Every node already carries
`type_fqdn`, so one O(n) pass at decode builds `fqdn → Vec<node_idx>`.
An active `FqdnField { fqdn: F, field: k }` then expands to
`nodes_by_fqdn[F]`, and for each, its children bearing field number `k`.
Mark those. Cost is O(|nodes of type F|), paid **on user action**, not
per node per render. It is exact, so no wasted descents, and it collapses
`FqdnField` into the same marking mechanism as everything else — one
pruning mechanism rather than two, which was the point.

Maintenance: `type_fqdn` changes when a node is retyped, so the index
needs patching on splice — but only for nodes inside the spliced
subtree, which is bounded by the splice itself.

**Fallback if that maintenance proves awkward: a subtree FQDN mask.**
Hash each distinct FQDN to a bit in a `u64`; store per node the OR of
its subtree's FQDN hashes; test `(subtree_mask[X] & query_mask) != 0`.
One bottom-up O(n) pass, 8 B/node (~5 MB at 622k nodes), false positives
cost a wasted descent but never a missed one. Cheaper to maintain, less
precise.

**Rejected: the conservative blanket.** Setting every bit whenever any
`FqdnField` entry is active is trivially correct and degrades exactly to
today's behavior — but it degrades the moment a user creates one
`fqdn:field` override, which is not a rare action, so the pruning would
be absent precisely when a user is working hardest. Keep it only as a
temporary scaffold if Q2 forces it.

## Correctness argument

Three claims carry this spec. Each must be tested, not just believed.

**L1 — new auto-expand candidates only appear inside fresh content.**
`is_auto_expand_candidate(c)` depends only on `c`'s field number and
wire type, and on the `type_fqdn` of `c`'s parent (and grandparent).
A node's `type_fqdn` changes only when it is retyped, which happens only
under an override or inside a re-decode. So a node whose auto-expand
eligibility *changes* necessarily lies inside a subtree that was
re-decoded in this batch — which S3 descends in full. Seeds computed at
startup therefore remain sufficient for all non-fresh content.

This is the claim most likely to be wrong, because it is a claim about
every way `type_fqdn` can change. It is the first thing a reviewer
should attack.

**L2 — a subtree's rendering does not depend on its position.** Spec
0174 G1 removed `DecodeRenderOpts::node_budget`, so there is no global
running budget whose exhaustion point could move when earlier content
changes length. The one remaining budget,
`OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT`, bounds the renderer's input for
*live previews only* (`override_apply.rs:1490-1496`) — and previews are
the overlay path, out of scope by N2. `MAX_WIRE_DEPTH` depends on depth,
not on preceding content. So skipping a subtree cannot change what it
would have rendered.

**L3 — `rendered_as` needs a subtree-level test, not a node-level
one.** This is the trap in the gate, and it is worth spelling out
because disjunct 4 is the one that looks safe to leave alone.

Disjunct 4 (`tree[c].rendered_as.is_some()`) is an O(1) field read. It
costs nothing, it does not scale with the document, and so the obvious
plan is: delete disjunct 1, keep disjunct 4 as it stands. That plan is
wrong, and the reason is that disjuncts 1 and 4 are not independent —
disjunct 1 has been silently supplying the *reachability* that disjunct
4's correctness depends on.

Disjunct 4 only ever speaks about `c` itself, and it is only consulted
if the walk arrives at `c` in the first place. Today the walk always
arrives, because disjunct 1 lets it through every message on the way
down. Take disjunct 1 away and consider a node `D` carrying
`rendered_as`, sitting under a chain of plain message ancestors that no
override touches. Every one of those ancestors now fails the gate, the
walk stops at the topmost one, and `D` is never visited — so its own
disjunct 4, which is perfectly correct, is never evaluated. The rule it
encodes (a once-spliced node must keep being revisited, so that it can
fall back to its natural type when the override that spliced it is
deactivated — the spec 0135 follow-up) stops holding.

The failure mode is the bad one: no panic, no assertion, no index out of
range. `D` simply keeps the text it was last rendered with, forever. It
is stale, it is plausible, and it is only wrong relative to a state the
user changed several actions ago.

Hence source 2 of S2 marks *ancestors*, not just the node: a node-level
predicate has to be lifted into a subtree-level one before it survives
the loss of a blanket descent. The same lifting is what sources 1 and 3
do for their own targets; disjunct 4 is easy to overlook only because it
was already O(1) and therefore already looked finished.

Note that `OverrideEntry` persists with `active: false` after
deactivation, so a deactivated override still marks its own path — which
is exactly what the fallback-to-natural-type rule requires. Entry
*deletion* is the case to check (Q3).

## Test plan

- **The differential test is the acceptance criterion.** Render a
  document with pruning enabled and with the gate forced to its old
  `is_message` form, and assert byte equality of `lines`, of every
  `NodeSpan::text_range`, and of `line_to_node`. Run it over the
  existing fixtures and over spec 0182's generated corpus once that
  lands. A wrong pruning predicate produces *stale text with no panic*,
  so nothing weaker than byte equality is worth running.
- **L1 gets its own test:** a document where an override on node X
  retypes it such that a descendant becomes `Any`-typed, asserting the
  newly eligible node is auto-expanded in the same batch.
- **L3 gets its own test:** splice a node under an override, deactivate
  the override, and assert the node falls back to its natural type —
  with the node deep enough that an unmarked ancestor would have
  pruned it.
- **P1's own assertion** (from rendering-flaws): the seed list equals
  the set of nodes the current full walk would splice.
- **A timing check, through `bin/bench`,** on `App::new` for the 1.1 MB
  fixture. The predicted result is that the pass approaches zero on a
  fixture with no `Any`/MessageSet; if it does not, S1 is not doing what
  this spec claims and the reason must be found before landing.
- `reuse lint` and `nix-build -A ci`.

## Open questions

- **Q1 — settled: the arena is append-only.** The concern, in plain
  terms: S2's bitset identifies nodes by their seat number in the arena.
  If a splice were to reseat everyone — shuffling nodes so that the node
  formerly at seat 900 is now at seat 850 — then every bit set before the
  splice would point at the wrong node afterwards, and the walk would
  descend into unrelated subtrees while skipping the ones that changed.
  Silently, and only after the second override in a batch.

  It does not happen. `splice_override` (`:1878-1918`) takes
  `base = self.tree.len()`, translates the freshly decoded local tree
  into global coordinates by *adding* `base`, and pushes. Superseded
  nodes are not removed; they are "abandoned in place" (`:1928-1931`),
  which is exactly why the arena grows across commits (622,922 →
  1,690,153 → 2,709,031 on the 1.1 MB fixture) rather than staying flat.
  No existing index ever moves.

  So the bitset stays valid across a splice, and needs only to be grown
  to the new `tree.len()` — with the new bits left clear, since the fresh
  content is descended by S3's `fresh` flag rather than by a mark. The
  roadmap's "renumber" language at `:354` describes the *ordinal* within
  a parent's child list, which is a different numbering entirely (and is
  spec 0184's subject).
- **Q2** — is there an existing FQDN→nodes index (on the heat or
  scoring side) that S5's first option can reuse, or must one be built?
- **Q3** — when a user *deletes* an override entry from the manage
  pane, what re-render is triggered, and does it reach the nodes that
  still carry `rendered_as` from it? Under today's full walk this is
  invisible; under pruning it becomes load-bearing.
- **Q4 — a possible refinement, not an alternative.** It is tempting to
  say `render_overrides` should simply start at the override's own node
  instead of at the root, and that S2's marks then become unnecessary.
  They do not. The entry point answers "where do I start?"; the marks
  answer "where do I stop?" — and those are different questions. A batch
  can carry several scattered overrides, so there is no single "the
  override's node" to start from; and even from a correct starting node,
  the walk still has to decide at every child whether to descend, which
  is precisely what `descend` is for. Lowering the entry point is a
  worthwhile refinement *on top of* S2 (it skips the marked spine above
  the topmost target) and can be taken separately.
