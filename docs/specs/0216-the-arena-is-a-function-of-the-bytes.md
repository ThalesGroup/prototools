<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0216 — the arena is a function of the bytes

Status: draft — steps 1-8 implemented (everything but S10, moving the
        walk off the main thread, which is pure scheduling)
Implemented in: 2026-07-31 (steps 1-8)
App: protolens
Refs: docs/specs/0097-raw-recursive-lendel.md (the unknown-LEN cascade
        whose probe S14 bypasses),
      docs/specs/0114-protolens-range-type-override.md §1.1 (the
        encompassing wrapper, which S1 makes slot 0 and S28 builds once),
      docs/specs/0115-protolens-packed-element-nodespans.md
        (`packed_record_start`, which S22 reuses),
      docs/specs/0117-protolens-override-collection.md
        (`OverrideOrigin::Path` — the identity scheme S1a adopts),
      docs/specs/0171-render-recursion-depth-cap.md (`at_depth_cap`,
        which N3 leaves to the renderer alone),
      docs/specs/0184-packed-records-are-the-addressable-unit.md
        (S6 keeps its conclusion, dissolves its mechanism),
      docs/specs/0192-a-frame-costs-the-same-wherever-the-cursor-is.md
        (`sibling_ordinal`, which S17 derives and deletes),
      docs/specs/0203-the-override-arena-is-compacted.md (superseded),
      docs/specs/0206-the-arena-reuses-its-dead-slots.md (superseded),
      docs/specs/0210-a-node-counts-its-own-lines.md (its S1, which S7
        strengthens),
      docs/specs/0212-the-span-is-a-third-as-wide.md (`NodeSpan`'s u32
        offsets and `MAX_INDEXED_BUFFER`, which S18 inherits),
      docs/specs/0215-the-cursor-knows-which-line-it-is-on.md
        (implemented 2026-07-30, steps 1-2; S23 withdraws its step 3
        and states what it did *not* fix),
      docs/protolens/design/arena-and-batch.md

## Background

protolens's arena is built from *render output*: `build_tree`
(`decode.rs`) consumes the `Vec<NodeSpan>` that `IndexingTextSink` emits
while rendering under the current type assignment. So the arena is a
picture of one interpretation, and every re-interpretation must build
another — hence `local_tree`, hence splicing, hence slots abandoned in
place, hence unbounded growth across a session. Specs 0203 (compaction)
and 0206 (slot reuse) exist only to manage that growth, which is the
bulk of the 3.9 GB peak measured on `googleapis.desc`.

The observation this spec is built on: **the child decomposition of a
byte range is schema-free.** Cutting a payload into field occurrences —
including deciding where a malformed tail begins, since a bad wire type
or a zero field number is a byte-level fact — uses no schema. A schema
supplies names, types and presentation; it never moves a boundary.

Two things must be kept apart, and the rest of the design rests on it.
The *cut* is structural: an ordered list of ranges, in which a child's
identity is its **position**. The tag's contents — field number, wire
type — are read only to find where an occurrence ends and the next
begins; that done, they are interpretation. A field number may repeat,
may be absent from the schema, and does not exist for a malformed tail.
A position always exists and is always unique.

It follows that there is a single tree implied by the bytes alone, of
which every interpretation's tree is a *pruning*. If the arena is that
tree, it is immutable for the session, because the blob is.

The second observation: **a frozen tree needs no path index, because the
layout can be the index.** Lay the nodes out in level order and path
lookup is arithmetic. That turns a memory fix into a performance one.

## Goals

- **G1.** The mapping from path to slot is fixed at load and never
  mutated.
- **G2.** No structural allocation after load: no `local_tree`, no
  splice, no abandoned slots, nothing to compact or reuse.
- **G3.** Applying, removing or changing an override is a *re-labeling*
  of existing slots plus a change of which are displayed.
- **G4.** If the blob loaded, every override expressible over it is
  representable. No override can fail for want of a slot.
- **G5.** The arena is built in one sweep by prototext-core and handed
  over by move, never copied out.
- **G6.** Path lookup and the sibling scans that dominate
  `absolute_start` are cheap *because of the layout*, not because of a
  cache.

## Non-goals

- **N1.** Changing the override model, file format, or inference.
  `OverrideOrigin::Path` already keys by canonical positional path; this
  spec makes the arena agree with it.
- **N2.** Lazy or incremental construction. The walk is eager, as
  today's decode is.
- **N3.** Changing `MAX_WIRE_DEPTH` or altering what it does. It stays a
  bound on prototext-core's *recursive render*, enforced by
  `at_depth_cap` with its existing degradation. S9's arena bound answers
  a different question and has a different response to hitting it; it
  merely reuses the same value, 1000.
- **N4.** Degrading an over-deep node the way the renderer does. The
  arena's response to depth is refusal of the whole blob (S9), never a
  truncated arena — a partial arena is the missing-slot failure this
  design exists to eliminate.
- **N5.** Changing rendered output. The new mode is additive and
  existing output is byte-for-byte unchanged; the 0097 probe is neither
  modified nor conditionally bypassed, because S14's walk never reaches
  it.
- **N6.** Making `Sink` public, or expressing the traversal as a macro.
  See S26.

## Specification

### The tree

- **S1.** **A node is a byte range, and slot 0 is the blob.** `wrap_blob`
  (`decode.rs:1144`) already prepends a real tag and length, so the blob
  is field 1 of a virtual encompassing message (spec 0114 §1.1). **That
  node is slot 0**; the top-level occurrences (7 771 on the reference
  corpus) are its children, at depth 1. Every node has a tag, the root
  included, which is what lets S19 hold with no special case at the top.
  `positional_path` already drops the leading `/1` leg
  (`navigation.rs:884`), so no path changes.
- **S1a.** **Identity is positional, and only positional.** A path is a
  sequence of *child indices* within the parent's block — the canonical
  form `OverrideOrigin::Path` already stores — and **not** a sequence of
  protobuf field numbers. Wherever this spec says "field", read "field
  occurrence at a position".

  Wire type carries no structural weight either. It fixes how many bytes
  an occurrence spans, hence where the next sibling starts; past that it
  has no say. It is not identity, it is not stored, and no invariant
  here is conditioned on it.
- **S2.** **Always recurse, never judge.** The walk descends into
  *every* length-delimited payload, whatever it looks like, and never
  declines on a heuristic. That is what makes the tree maximal, and
  maximality is what makes it a superset: any payload the walk declined
  is one some schema — or some user override, which is the entire point
  of the app — could still declare a message, and the render would then
  need slots the arena lacks. Today's raw walk does not obey this (S14).
- **S3.** A payload yielding no readable children yields a childless
  node, exactly as a scalar leaf does. Refusal and emptiness are the
  same shape, so the walk has no failure mode.
- **S4.** Malformed regions get nodes, as today, at positions among
  their siblings like any other occurrence. The boundary at which a
  malformed tail begins is byte-determined, so those positions are the
  same in the maximal tree as in any interpretation.
- **S5.** Value-level complaints — a varint too wide for a declared
  `int32`, invalid UTF-8, an out-of-range enum — annotate an existing
  leaf. They create no nodes and do not perturb structure.

### The layout

- **S16.** **Level order.** Every node's children occupy one contiguous
  block, blocks ordered by parent index. Breadth-first, and a
  *requirement*: S17 depends on it. It is a property of the finished
  arena, produced by S8's phase 2; the walk that fills it is depth-first.
- **S17.** **Path-to-slot is not stored, because it is arithmetic.**

  ```rust
  let mut i = 0;            // S1: slot 0 is the wrapper
  for step in path { i = first_child[i] + step; }
  ```

  One add per level; no hash, no string, no allocation, and no failure
  mode but a step past `first_child[i+1]`. The inverse is equally free:
  `i - first_child[parent[i]]` *is* the `sibling_ordinal` spec 0192 had
  to store and `splice_override` has to repair. **That field is
  deleted**, and with it 0192's one repair site.

  Cost is O(depth), bounded by S9's D. The 13 measured on the reference
  corpus is an observation about that blob, not a term in the
  complexity. What makes the primitive cheap is that each level is one
  indexed read.
- **S18.** **Four arrays, struct of arrays.**

  | array | width | entries | meaning |
  | --- | ---: | ---: | --- |
  | `first_child` | u32 | n + 1 | children of `i` are `first_child[i] .. first_child[i+1]` |
  | `parent` | u32 | n | the climb, and via S17 the sibling ordinal |
  | `raw_start`, `raw_end` | u32 x2 | n | the byte range |

  16 bytes per node. Only `first_child` carries the trailing sentinel,
  and it is what makes child count free —
  `first_child[i+1] - first_child[i]` — which holds because S16
  allocates child blocks in parent order, and which requires
  `first_child[i]` to be written for childless nodes too. Struct of
  arrays because the passes are disjoint: path descent touches only
  `first_child`, the line sums only the overlay, byte ranges only at
  render.

  **The `u32`s need no new bound.** `MAX_INDEXED_BUFFER`
  (`helpers/bounds.rs:143`) is `u32::MAX / 8` — 512 MiB — and
  `decode_and_render_indexed` already refuses any buffer above it, for
  the same reason: `NodeSpan` stores `u32` offsets. The arena inherits
  that check rather than adding one. Slot *indices* are bounded by the
  same constant through S24's one-node-per-byte worst case.
- **S19.** **Store nothing recoverable from the blob — including
  malformity.** `raw_start` points at the first byte of the *tag*, not
  of the payload; a node's range is tag-through-end. Everything the tag
  says is therefore one parse away, and that includes whether the
  occurrence is well-formed at all: re-parse at `raw_start` and it is
  malformed exactly when the parse fails (invalid wire type, zero field
  number, truncated varint) or yields an extent other than
  `raw_end - raw_start`. No flag, no bitset, no fifth array. The blob is
  retained for the session anyway, and by S1a none of what the tag holds
  is part of what the arena is *for*. The structure is the one thing
  that cannot be recovered without re-walking, and S18 holds exactly
  that and nothing else.

### The walk prototext-core must provide

- **S14.** **Greedy belongs to the driver, not to a render policy.**
  Today's raw walk judges, and S2 forbids that. For an unknown LEN
  field, `render_len_field`
  (`serialize/render_text/helpers/len_field.rs`) runs spec 0097's
  cascade: it renders the payload into a `ProbeSink` and accepts it as a
  message only if `probe.malformity_count() == 0 && next_pos ==
  data.len()`. A payload that fails becomes `ScalarValue::Bytes` with
  **no child nodes**.

  The escape is not hypothetical. The *schema-driven* branch of the same
  function (`Kind::Message`, not a group) **never probes** — it calls
  `begin_nested` and recurses whatever the payload contains. (This is
  unconditional on the payload's *contents* only; the depth cap still
  applies, since `at_depth_cap()` is checked at `len_field.rs:64`, ahead
  of both branches.) So prototext-core is already always-recurse
  wherever a type says message, and an override is exactly a way to
  supply a type. Override the node — **or any ancestor of it** — and the
  descent reaches the rejected payload through the unconditional branch,
  needing slots the arena never allocated. That ancestor case is also
  why the narrower fix, protolens refusing to override a node the walk
  left childless, does not work.

  **The fix is one policy value, and it is the only one this spec
  adds.** The probe decides *whether to recurse*; phase 1 must always
  recurse. `render_len_field` runs the cascade only after failing to
  find a `field_schema` (line 76), so what is needed is a way to say
  "unknown LEN is a message, do not probe" — a `Sink` predicate,
  `unknown_len_is_message`, default `false`, checked where the cascade
  begins. It const-folds away in the three existing monomorphizations
  (S26), so the hot path is untouched.

  **Done 2026-07-31.** `sink.rs` gains the predicate and
  `IndexingTextSink` forwards it to its inner sink, like the two
  predicates beside it. In `len_field.rs` the probe moved behind
  `sink.unknown_len_is_message() ||`, so for a sink keeping the default
  the branch is the same code it was, and for one that asks the
  `ProbeSink` construction and its `render_message` call fold away
  entirely.
- **S15.** **The decomposition lives in prototext-core.** Not for
  convenience: the superset property is a claim about *boundaries*
  — where a malformed tail begins, how an unterminated `START_GROUP`
  recovers, where the depth cap hands a payload back opaquely. Two
  walkers that disagree fail identically to a missing slot. One
  decomposition, used by both. (`examples/maximal_walk.rs` is a
  measurement tool, not a candidate implementation, and reimplements
  those rules by hand for exactly that reason.)

  **Boundaries, not traversal.** This item says the arena walk and the
  render walk must agree on where every field begins and ends. It does
  not ask them to share a traversal: they have opposite recursion
  policies by design (S14), and that is the whole point of routing the
  difference through `Sink`.

  **prototext-graph is outside this item, deliberately.**
  `score_message_multi` interprets one byte stream as N candidate types
  at once, assigning a per-candidate verdict to the same tag; a `Sink`
  is single-semantics by construction, so it cannot host that walk. What
  scoring shares is the layer below — `parse_varint` and `parse_wiretag`
  are prototext-core's, so the boundary rules cannot drift even though
  the traversals stay separate. On the two points where the walks do
  differ (unterminated group at EOF, depth cap) they differ in *verdict*
  — veto versus degrade — not in where a field ends, which is what this
  item governs.
- **S20.** **Groups need no special case.** A group is one node like any
  other occurrence, well-formed or not, and under S8's depth-first phase
  1 its extent is simply where its own walk returned — allocate the slot
  on the way down, backpatch `raw_end` on the way up, exactly as
  `render_group_field` already does with `begin_nested`'s `mark`. No
  extent scan, no second policy value.
- **S21.** **`gar` is a borrow, not an owned copy.** **Done
  2026-07-31.** `parse_wiretag` and `parse_varint`
  (`helpers/varint.rs`) did `buf[start..].to_vec()` on an invalid wire
  type or a truncated varint. Today that is exceptional; under
  always-recurse it is the common case — ~1.15 M malformed nodes, a
  quarter of all nodes. Both are now `Option<&'a [u8]>`: the exact
  bytes, no allocation, zero call-site changes. The input buffer is
  read-only and outlives every parse, and `Sink::malformed` already took
  `&[u8]`.

  **`gar` must stay bytes.** It is roundtrip payload, not diagnostics:
  it reaches `Sink::malformed`, which renders it, and the encoder reads
  those bytes back. A `bool` plus re-slicing at the use site is also a
  trap — at `render_text/mod.rs:544` and `:617` the garbage is
  `buf[pos..]` (after the tag) while the `raw_range` argument on the next
  line is `field_start..buflen`.

  The saving is real but modest, and this item never blocked the rest:
  every `gar` site returns `(buflen, None)`, so garbage is *terminal* —
  once per message **body**, not once per malformed node — and
  `render_message` recurses on re-sliced sub-buffers, so the remainder
  copied is the local payload's. Roughly blob × depth ≈ 330 MB of
  `memcpy`.

  The same pass deleted prototext-graph's hand-copied
  `parse_varint`/`parse_wiretag` (`score/walk.rs`) in favor of
  prototext-core's (S15) and removed `render_as_bytes`'s pass-through
  `data.to_vec()` by returning `Cow` (S28).
- **S25.** **prototext-core allocates, fills, shrinks, hands over by
  move.** Forced, not chosen: by S8 the count is the walk's *output*, so
  no arrangement lets protolens size the arrays and the library fill
  them. Four `Vec<u32>` move out; nothing is copied.

  Both of S8's phases are the library's. Phase 1's arrays are internal
  and never leave — they are dropped when phase 2's rewrite is done.
  What crosses the seam is phase 2's output, and only that.

  The seam is **frozen structure against per-interpretation overlay**,
  not "some fields each". prototext-core owns all four arrays because
  all four are functions of the bytes; protolens owns S12's overlay and
  everything above it — folds, cursor, display — and none of that
  crosses into the library.

  The governing test is `node_budget`'s: *whose concern is it*, not who
  allocates it. `node_budget` was kept out as a preview-pane concern
  wearing a library interface. The structural decomposition passes — a
  fact about the bytes, needing the codec's own boundary rules to be
  right (S15), carrying no protolens semantics.
- **S26.** **Monomorphization is the extension mechanism.** `Sink` stays
  `pub(super)` and the arena sink is written inside prototext-core
  against it. `render_message` is generic over `S: Sink`, so that sink
  gets a specialized copy with `treat_len_as_opaque` and
  `unknown_len_is_message` const-folded and the dead branches gone — precisely
  what a macro or a public trait would buy, already in place. The
  alternatives (N6) were rejected: a public `Sink` would freeze
  `FieldOrExt`, `TagFacts`, `ScalarValue`, `MalformedKind`,
  `NestedKind` and `GroupCloseFacts` as API; a macro would move ~300
  lines of decode logic into a body type-checked only at expansion
  sites, in a crate with no `macro_rules!` today.

### What does not get a node

- **S6.** Display artifacts get no slot:
  - **Packed repeated scalars** — the element lines are a bloated
    rendering of the one length-delimited field holding them, generated
    as offsets within that node's own lines. This dissolves spec 0184's
    run absorption and the `sibling_ordinal` repair it forced: packed
    elements are not siblings, so there is nothing to renumber.
  - **Any / MessageSet expansion** — `begin_virtual_nested`'s wrapper is
    presentation. The embedded message's fields are already nodes, found
    by S2's recursion into the enclosing payload.
  - **`virtual_scalar` rows** (Any's `type_url`, MessageSet's
    `type_id`) — these already emit no `NodeSpan`; that stops being a
    defect and becomes the normal case.
- **S22.** S6 needs a companion rule on the *render* side, because the
  renderer still emits spans for these and protolens must map every
  emitted span onto a slot to maintain the overlay. A packed element has
  no tag of its own and a virtual wrapper's range is synthetic, so
  neither maps by range. The rule: a span with `packed_record_start !=
  NO_PACKED_RECORD`, or with `field_number == 0`, is a **display row
  attached to its owning slot**, not a slot.

  Both discriminators already exist and are already documented as such.
  `field_number == 0` is `NodeSpan`'s stated convention for a virtual
  wrapper (`sink.rs:990`) — Any's `value {}`, MessageSet's `Item {}` and
  `message {}` — since no real field number is 0. The owning slot is the
  span's enclosing node in either case: for a packed element, the record
  whose tag is at `packed_record_start`; for a wrapper, the node whose
  payload it expands.

  **This is not new machinery; it is existing machinery promoted.** The
  codebase already treats a packed run as one addressable thing:
  `same_packed_record` (`decode.rs:465`) is documented as the single
  definition of the record boundary, already shared by `build_tree`'s
  ordinal derivation, `render_overrides_inner`'s forward counter and
  `nth_child`; spec 0184 S2 already gives a run's N spans one ordinal,
  hence one path; and `message_payload_range` (`extract.rs:101`) already
  returns a packed element's range *unstripped*, on the stated grounds
  that it has no tag of its own. So the span-to-slot map keys on
  `same_packed_record` — the same predicate, doing the same job one
  level up. What changes is that the run stops being N nodes that share
  an ordinal and becomes one node with N display rows (S7), which is
  what deletes the absorption pass rather than merely relocating it.

### Where the cursor goes

- **S7.** Spec 0210 S1's invariant — every rendered line belongs to
  exactly one node — **is preserved, not weakened.** A line owned by no
  node is an absorbing barrier (`navigation.rs`, 2026-07-25 bug), and S6
  creates none: it makes the map *many-to-one*, not partial. That is
  already the shape of the problem — a message's header and footer lines
  are two lines of one node, and under S6 a packed run's elements are N
  more of the same kind.

  So the coordinate widens rather than the invariant loosening:
  `LinePos { node, footer: bool }` becomes
  `LinePos { node, line_in_node: u32 }` — the same half-coordinate the
  closing brace already uses. Footers, packed elements and synthetic
  rows become one mechanism instead of three, and `next_visible` /
  `prev_visible` stay O(1) by stepping within a node before stepping to
  the next.

  The footer is the case to keep in view: a node's lines are *not*
  contiguous on screen, since the whole subtree is drawn between the
  header and the footer. `line_in_node` is an index into the node's own
  line list, not a screen offset (the same distinction the render hot
  path already had to make — a node-identity memo over consecutive rows
  collapses a packed run but never a header/footer pair).

### Construction

- **S8.** **Two phases: walk in document order, then sort into level
  order.** Level order is a property of the *result* (S16), not a way to
  build it. It is produced by a linear sort after a single depth-first
  pass, not by sweeping the bytes level by level.

  **Phase 1 — depth-first, one pass over the bytes.** It is
  `render_message`'s own recursion, driven by the arena sink: S15 puts
  the decomposition in one place and S14 routes the greedy difference
  through `Sink`, so phase 1 is not a second walker. Recursion depth is
  therefore `DEPTH`, capped at 1000, and exceeding it aborts the load
  (S9). On the way down, append a slot; on the way up, write its
  `raw_end`. Output, in document order: `parent`, `depth`, `raw_start`,
  `raw_end`.

  The cap being a *recursion* bound has one consequence that must not be
  lost: `render_message` measures ~1408 KiB of stack at full depth, so
  whichever thread runs phase 1 needs a stack sized for it (S10).

  **Phase 2 — a counting sort by depth.** Not a comparison sort: the key
  is a small integer bounded by D, so it buckets rather than compares,
  and the whole phase is O(n + D) with D ≤ 1000.

  1. count nodes per depth — one pass;
  2. prefix-sum to each level's base slot — O(D), a ≤ 4 KB array;
  3. scatter: for each node *i* in document order,
     `new[i] = base[depth[i]]++`.

  Then rewrite `parent` through `new`, permute `raw_start`/`raw_end`,
  and derive `first_child` — one forward pass marking where
  `parent[j] != parent[j-1]`, one backward sweep giving childless nodes
  the value that keeps `first_child[i+1] - first_child[i] == 0`.

  **Why a sort by depth alone is enough.** Stability falls out of step 3
  rather than being engineered — nodes are visited in document order and
  each bucket's cursor only advances — and *within a level, document
  order already equals the order induced by parent order*. Two nodes at
  depth *d+1* with different parents P₁ ≺ P₂ at depth *d*: all of P₁'s
  subtree precedes P₂ in preorder, so P₁'s children precede P₂'s. And
  nothing at depth *d+1* falls between two children of one parent, since
  anything between them in preorder is a descendant of the earlier one,
  hence depth ≥ *d+2*. So the buckets come out with sibling blocks
  contiguous and blocks ordered by parent index — S16 exactly.

  **Why not build level order directly.** A level-order build must know
  each child's extent before it can place the next sibling. For a
  length-delimited field the prefix gives it; for a **group** there is no
  prefix, so the extent is only knowable by traversing the body — which
  is the next level's work. Level *d* could then not be completed until
  the levels below it were. Worse, it makes the build superlinear: a
  chain of *k* nested groups has its innermost bytes traversed once per
  level, so a blob nesting groups around a large payload costs order
  D x blob. Depth-first has neither problem — a group's extent is simply
  where its walk returned, backpatched — and it reads every byte once.

  **Memory.** With n slots from phase 1:

  | | bytes |
  | --- | ---: |
  | phase 1 output (`parent`, `raw_start`, `raw_end` u32; `depth` u16) | 14n |
  | phase 2, peak (phase 1 live + level-ordered arena 16n + `new` 4n) | **34n** |
  | after phase 2 | 16n + 4 |

  34n is 156 MB on the reference corpus, before any rendering, against a
  current peak of 2.09 GiB. **Speed is the priority here, not the
  peak**, and 34n is what buys it: n is known the instant phase 1 ends,
  so every phase-2 array is one exact-size allocation with no growth and
  no reallocation, and the four output arrays are carved from a **single**
  16n + 4 allocation — one `malloc`, one first-touch, one `free`, and
  identical lifetimes. Phase 1's arrays are released together at the end.

  A 20n ordering exists — rewrite the three permuted arrays in separate
  loops, dropping each source as its loop ends — and is the fallback for
  pathological *depth*, not for size: the fused rewrite holds 3D
  concurrent write streams, ideal at the measured D = 13 and TLB-hostile
  near the cap. Recorded, not taken.

  **The node count cannot precede phase 1** — it is knowable only by
  parsing every tag, which *is* the walk. Phase 2 is the opposite: it
  knows n exactly. So over-reservation is phase 1's problem alone (S24).
- **S28.** **The wrapped blob is built once, at load, in one buffer, and
  kept.** S1 puts the wrapper at slot 0 and S19 re-parses tags all
  session, so the bytes `raw_start` indexes are the *wrapped* bytes and
  they must outlive the walk. Today `wrap_blob` runs inside
  `render_resolved` (`decode.rs:1200`) and its buffer dies with the
  call. What changes is the lifetime, not the frequency: `wrap_blob` has
  one live call site (its doc comment's claim that `splice_override`
  also calls it is stale — spec 0135 G1 removed that) and
  `render_resolved` runs once per document.

  Along the way, **a binary blob is copied twice before a byte is
  parsed**, and both copies go:

  | site | why it copies | how it goes |
  | --- | --- | --- |
  | `main.rs:230` `fs::read` | the read itself | — |
  | `lib.rs:139` `Ok(data.to_vec())` | `render_as_bytes`'s binary pass-through copies precisely in the branch where it has nothing to do | **done 2026-07-31** — returns `Cow`, borrowing on pass-through |
  | `decode.rs:1200` `wrap_blob` | the prefix is written before the payload, so a fresh buffer is needed | headroom, below |

  **Nothing forces the prefix to be written first.** Reserve a fixed
  **11-byte headroom** (max tag + max length varint) ahead of the
  payload, fill the payload, then write the real prefix *backwards* into
  the headroom and take `&buf[11 - w ..]`. `w` never has to be
  predicted — which matters, since the encode path changes the length
  between reading the file and wrapping it. Callers wanting the
  unwrapped blob take a subslice; neither view costs a copy.

  Two producers, one shape: **binary** reads into the headroom buffer's
  tail; **`#@ prototext` text** encodes into it instead.

  The API this needs is a **flavor of `render_as_bytes` that appends into
  a caller-supplied buffer**, called only on the text branch. That is
  where it earns anything: the binary branch's job is to *not* copy. So
  the binary branch stops calling `render_as_bytes` at all — the reader
  or the mapping puts the bytes where they belong directly — and the
  text branch appends its encode output into the headroom buffer instead
  of returning a fresh `Vec`. The `Cow` return already landed removes
  the pass-through copy in the meantime, but not the *second* one: the
  text branch still allocates, and only the appending flavor puts its
  output straight into the headroom.

  **Optional: map the file rather than read it.** The headroom survives
  mapping, because the prefix can sit flush against the file bytes:
  reserve `page + round_up(len)` anonymous, `MAP_FIXED` the file
  `PROT_READ` over the tail, keep the leading page writable, write the
  prefix at `base + page - w`. Contiguous, nothing written back, and S18
  is indifferent because it stores offsets, not pointers.

  **The gain is resident memory, not startup latency.** The walk touches
  every byte anyway, so mapping removes one memcpy (milliseconds against
  a ~1.7 s descriptor decode) and defers the faults into the walk. What
  it does buy is that file-backed clean pages are evictable where a
  `Vec` is anonymous and must be swapped — a scaling property, priced
  against two costs: a truncated or network-backed file turns a
  load-time `errno` into a **mid-session `SIGBUS`**, possible at any
  point in a mapping held all session; and the blob is capped at 512 MiB
  by `MAX_INDEXED_BUFFER` regardless (S18), so mapping widens the range
  protolens is comfortable in, not the range it can address. **At that
  ceiling the benefit is bounded and the `SIGBUS` is not** — which is
  why mapping is worth having but not worth much, and is the first thing
  to drop if step 4 proves awkward.
- **S29.** **Peek first, then choose the producer. Mapping is the
  default for binary.** S28's two producers want different buffers, so
  the choice must be made before either is allocated — which needs only
  the first 13 bytes.

  The magic is the text format's: `PROTOTEXT_MAGIC = b"#@ prototext:"`
  (`render_text/mod.rs:30`), tested by the already-`pub`
  `is_prototext_text` (`:258`), a 13-byte `starts_with`. Its **absence**
  is what says binary — which is both the cheap direction and the one
  that preserves today's behavior on an unrecognized file.

  | peek | producer |
  | --- | --- |
  | magic present | read, encode into the headroom buffer |
  | otherwise | map the file behind the headroom page |

  Two orthogonal CLI controls — one selects the **format**, the other
  the **access**:

  - **`--assume-binary`** — treat the blob as true binary whatever the
    magic says, so the peek is skipped. `RenderOpts.assume_binary`
    (`lib.rs:138`) already carries exactly this meaning; protolens
    simply never surfaced it. Mapping then follows from the blob being
    binary — the flag does not itself say anything about access.
  - **`--eager-read`** — pull the whole file in now rather than mapping
    it. Eager against lazy is the real axis: a mapping faults pages on
    demand, a read does not. The consequence is what the flag is for —
    a `MAP_PRIVATE` mapping is a *live view* of a file held open all
    session, whereas a read is fixed at load, after which the file may
    change or vanish safely. That is the documented answer to S28's
    `SIGBUS` exposure on network or removable filesystems.

    **Only binary is affected.** Text input must be read and encoded
    into memory regardless, so it is eager already; the flag is a no-op
    there, not a conflict, and must not be rejected.

    Rejected names: `--no-mmap` (a negation naming an implementation),
    `--snapshot` (names the guarantee, but obliquely, and says nothing
    to someone whose motive is that mapping is slow on their
    filesystem), `--greedy-read` (same axis, but *eager* is the
    conventional word for it against a lazy mapping; secondarily,
    *greedy* is this spec's word for the always-recurse walk — S2, S14 —
    and while that is purely internal today, `--raw` is not, so the two
    would sit together in `--help`), `--blob-access <mmap|read>`
    (extensibility with no second use yet).

  Two fallbacks are automatic and **must not be errors**:

  - **Not a regular file.** `protolens <(gunzip -c foo.pb.gz)` is a
    fifo; it cannot be mapped and must simply be read.
  - **Too small to be worth it.** Below a threshold the mapping setup
    costs more than the read and the benefit (evictable pages) is nil —
    the corpus's 375 per-type instance blobs are all under 1 KB. **One
    MiB**, chosen for being far above the small case and far below the
    one that motivates mapping; nothing depends on the exact value, and
    both sides of it must be exercised (item 13).
- **S24.** **Reserve at the worst case, then shrink — phase 1 only.**
  Phase 2 knows n exactly and allocates exact-size (S8), so this item
  governs the depth-first pass alone. A large `Vec` reservation is served
  by the allocator from fresh anonymous pages that are never faulted in
  until touched, so it costs address space, not resident memory. The
  worst case is **one node per byte** — every node's tag costs at least
  one byte — so reserving `blob.len() + 1` slots removes reallocation
  outright: ~360 MB of address space for the reference blob at phase 1's
  14 bytes per node, of which ~64 MB is ever touched. `shrink_to_fit`
  returns the rest, and each array is well past glibc's 128 KB mmap
  threshold, so the shrink should be an `mremap` tail-unmap — in place,
  no copy, pages genuinely returned. Allocator behavior, so it must be
  measured (item 12). Where overcommit is unavailable, reserve from the
  measured density instead — 5.6 bytes per node, so `len / 5` lands
  within ~10% — with doubling as the backstop.

  Doubling alone would also work and is amortized O(1); what it costs is
  a transient above 14n at the largest reallocation. The escape, if that
  ever matters, is chunked blocks that are never reallocated — one extra
  indirection on a write-only path. Not done now.

  **A run of `START_GROUP` tags is not a build worst case.** It is a
  chain 25 M levels deep, but depth-first reads it once and S9 refuses
  it on depth long before phase 1 finishes.
- **S9.** **The walk is depth-bounded at D, and exceeding D refuses the
  blob.** Phase 1 is `render_message`'s recursion (S8), so the depth is
  `DEPTH` and the check is the one `at_depth_cap` already makes — what
  differs is the response, refusal rather than degradation. The same
  value is recorded per node as S8's sort key. A blob
  whose maximal tree goes deeper is rejected at load with a diagnostic
  naming depth, alongside spec 0168's startup progress lines. Nothing is
  rendered, so nothing is degraded.

  **Refusing is not truncating, and only refusing is safe.** A walk that
  stops at D and hands back a partial arena *is* the missing-slot panic.
  Aborting the load instead leaves G4 intact, since G4 is conditional on
  the blob having loaded — and makes it total: if the blob loaded, its
  true depth is ≤ D, so no render from *any* root can go below D, and
  `at_depth_cap` only ever makes a render shallower.

  **A bound on the arena alone would not be enough**, which is why the
  refusal is not merely tidy. `DEPTH` is a thread-local reset to 0 at
  each entry point (`render_text/mod.rs:354`, `:453`) and protolens
  renders **subtrees** (override previews, `render_node_as`), so a
  subtree rooted at absolute depth *d* renders to *d + 1000* — past
  wherever a truncating arena stopped. That is exactly the case of
  overriding a node inside a folded, deeply nested message. **The
  invariant that saves this is arena completeness, not the size of
  either cap.**

  **D = `MAX_WIRE_DEPTH` = 1000**, reused rather than re-derived: it is
  75x the depth real data reaches, so no ordinary blob is refused, and
  one number means one thing for a reader to hold.

  **Refusal is immediate, and there is no adversarial residual.** A
  crafted `START_GROUP` chain hits depth 1001 after reading 1001 bytes
  — 1001 stack frames, well inside the budget S10 sizes for. No input
  makes the walk superlinear in the blob, so no cumulative-work escape
  hatch is needed.
- **S10.** **Startup: the walk runs concurrently with the schema
  phases.** **The walk depends on the blob alone**, so it starts the
  moment the blob is available, before `DescriptorContext::load`
  (`main.rs:277`) — not merely alongside inference. The overlap window
  is descriptor load *plus* inference, seconds on `googleapis.desc`,
  against a build of a few hundred milliseconds at the pessimistic end.
  The build is hidden outright, and the conclusion survives being wrong
  about the cost several-fold (73 ms is a floor, not an estimate: see
  Measured outcome).

  Two constraints:

  - **Join before the render, not after.** Spec 0168 deleted a detached
    root-type thread because its answer arrived *late* and forced
    `apply_resolved_root_type` → a root splice → a full re-render,
    10.6 s, under a reader already browsing. Inference stays a phase
    completing before `render_resolved`; only the arena walk proceeds
    beside it. Startup still renders exactly once and nothing is
    replaced under the reader, so 0168's G1 is intact.
  - **Whichever arm moves off main needs a spawn, and a sized stack.**
    The heat worker does not exist yet (`run()` spawns it after
    `App::new`), so "a thread protolens already has" describes the
    steady state, not this phase. Both arms need an explicit
    `stack_size`: inference per spec 0180, and phase 1 because it is
    `render_message`'s recursion (S8) — **~1408 KiB at D = 1000, a
    1.45x margin on a default 2 MiB spawned stack**, which is not
    enough. The renderer's comfortable 5.8x margin is a property of the
    8 MiB main thread it runs on today and does not travel with it.
    Size the walk's thread to match, and state the (walker, thread) pair
    when doing so.

  Where inference is skipped (`--raw`, `--type`, no scoring graph) there
  is no second arm, but the walk still overlaps the descriptor load —
  the larger of the two anyway.

### Consequences

- **S11.** `splice_override` ceases to be structural: applying an
  override re-labels slots and changes which subtrees are displayed.
  `local_tree` is deleted.
- **S12.** Per-interpretation state moves off the slot into a parallel
  array indexed by slot: `rendered_as`, `lines_total`, `lines_visible`,
  resolved type. Only one interpretation is live at a time, so the
  overlay does not grow — and because it is indexed by the same slot
  numbers, a node's preceding siblings are a **contiguous run** in it.
- **S13.** Specs 0203 and 0206 are superseded. There is no garbage.
- **S23.** `absolute_start` (`lines.rs`) sums `lines_total` over every
  preceding sibling at every level — ~7 771 random reads into a 4.5 M
  node arena near the end of `googleapis.desc`. Under S12 those siblings
  are contiguous, so the sum becomes a sequential scan of 31 KB.

  **The cost this targets is per-frame, not per-keypress.** Spec 0215
  took the keypress from 13 624 µs to 356 µs; that figure is spent and
  must not be claimed again here. What 0215 did *not* move is `draw`,
  which calls `absolute_start` six or seven times a frame: ~1 292 µs
  near the top of the document against ~3 000 µs deep in it. A frame
  costing more the further down the reader is — that residual is S23's
  target, and it is the symptom the reader reports (the cursor still
  moving after the key stops is the repaint backlog, not navigation).

  Spec 0215's N1 declined to make `absolute_start` cheap because it
  meant storing a line *offset*, repaired across every following sibling
  on every splice. A frozen, sibling-contiguous arena gets most of that
  win storing nothing, and there are no splices left to repair. **Spec
  0215 step 3 is withdrawn** (see Rejected alternatives); steps 1 and 2
  stand.
- **S27.** **Document order stops being stored.** Today `build_tree`
  sorts spans on `raw_range.start` and materializes `doc_next` /
  `doc_prev`, because `IndexingTextSink` emits *post-order* — children
  before parents — and document order cannot be read off that. Under S16
  it needs no storage: sibling ranges increase and child ranges nest, so
  document order is plain pre-order DFS, and `doc_next(i)` is
  `first_child[i]` if that block is non-empty; else `i + 1` while it
  stays inside `first_child[parent[i]+1]`; else climb `parent` until one
  has a next sibling. O(1) amortized over a traversal, O(depth) worst.
  **Two fewer arrays**, and `build_tree`'s sort goes with them.

## Rejected alternatives

One line each; the argument is at the item named.

- **A tagless synthetic root.** Slot 0 would then be the only node whose
  `raw_start` does not point at a tag, and S19's re-parse would need a
  special case there. `wrap_blob` already writes a real tag, so slot 0 is
  a node like any other (S1).
- **Field-number paths.** Two occurrences of the same field number, and
  fields out of field-number order, both break the mapping; overrides
  already key positionally (S1a, test item 3).
- **A fifth array, or a bitset, for malformity.** Derivable by re-parsing
  at `raw_start`, because a node's range starts at the tag (S19).
- **A counting pass before the build.** The count is knowable only by
  parsing every tag, which is the walk; so it would double the cost to
  learn what the walk returns anyway. Over-reserve instead (S8, S24).
- **Truncating the arena at depth D.** A partial arena *is* the
  missing-slot failure this spec exists to remove; and `DEPTH` resets per
  entry point, so no arena bound alone can hold. Refuse the load (S9,
  N4).
- **Greedy as a thread-local.** It is a `Sink` predicate, checked where
  the 0097 cascade begins and const-folded away everywhere else — the one
  policy value this spec adds (S14, Q5).
- **Building level order directly, level by level.** Groups have no
  length prefix, so a level cannot be completed without walking the ones
  below it, and a nested-group chain costs order D x blob. It also needs
  a group's extent ahead of its body — a `scan_group_extent` helper that
  walks the group only to throw the walk away. Depth-first then sort,
  with the extent backpatched on return (S8, S20).
- **Relocating a group's finished subtree with two `memmove`s.** A
  subtree is contiguous in document order and *scattered across levels*
  in level order, so no block move converts one to the other — it is a
  scatter, and nested groups would repeat it (S8).
- **The 20n phase-2 ordering.** Rewriting the three permuted arrays in
  separate loops holds 20n instead of 34n, at the cost of two extra
  passes. Recorded as the fallback for pathological depth, not taken:
  the peak precedes rendering and speed is what matters there (S8).
- **Protolens refusing to override a node the walk left childless.** The
  narrow fix, defeated by the ancestor case: overriding any ancestor
  reaches the rejected payload through the unconditional schema-driven
  branch (S14).
- **A public `Sink`, or the traversal as a macro.** The first freezes six
  internal enums as API; the second moves ~300 lines of decode logic into
  a body type-checked only at expansion sites. Monomorphization already
  buys what both were for (N6, S26).
- **Building the arena serially at startup.** Proposed by an earlier
  draft on the strength of an unmeasured 4%. The walk depends on the blob
  alone, so it overlaps the descriptor load and inference and the cost
  disappears — a conclusion that survives being several-fold wrong
  (S10).
- **Spec 0215 step 3, the cursor-line cache.** Withdrawn. It existed to
  avoid a walk that a sibling-contiguous arena makes cheap, and its
  `structural_version` key would never change once the arena is frozen
  (S23). Steps 1 and 2 of 0215 stand.
- **Depth-first as the final *layout*.** It is the build order (S8 phase
  1) but not the layout: `first_child[i] + step` needs sibling
  contiguity, which document order does not give. Level order's cost is
  subtree locality; the judgment and its terms are Q6.
- **Four CLI names for the access flag.** `--no-mmap`, `--snapshot`,
  `--greedy-read`, `--blob-access <mmap|read>`; reasons at S29.

## Implementation order

Item numbers are drafting identities, not a reading or landing order;
the sections are the reading order and this is the landing order. Each
step is separately testable, and steps 1-4 are prototext-core only.

1. **S21** — make `gar` a borrow. **Done 2026-07-31**, along with
   deleting prototext-graph's copy of the tag primitives and
   `render_as_bytes`'s pass-through copy. Independent of everything else
   here; first only because it is cheap and touches the layer everything
   else sits on. Items 9 and 10.
2. **S14** — `Sink::unknown_len_is_message`, default `false`. One new
   branch at the head of `render_len_field`'s no-schema cascade, no
   behavior change for existing sinks. **Done 2026-07-31.** Items 2
   (first half) and 9.
3. **Phase 1** — S8's document-order walk, plus S2-S4, S9, S15, S18,
   S19, S24-S26: the arena sink, `parent`/`depth`/`raw_start`/`raw_end`
   in document order, depth refusal. Testable on its own against
   `maximal_walk.rs`'s counts before any reordering exists, plus items 5,
   6 (first half) and 12.

   **Done 2026-07-31**, in `render_text/arena.rs`. `ArenaSink` claims a
   slot at `begin_nested` and backpatches `raw_end` at `end_nested`, so
   groups need no special case (S20); it carries the `raw_base` push/pop
   that turns `render_message`'s per-payload local offsets into absolute
   ones, exactly as `IndexingTextSink` does; and it declines
   `tracks_level`, so the walk leaves the render's indentation state
   alone. `walk_document_order` sets `HIDE_UNKNOWN` and both expansion
   switches off rather than trusting whatever ran last, which also makes
   the two virtual-node hooks unreachable.

   The depth refusal reads the sink's own counter rather than needing a
   notification from the renderer. The arena's `depth` is zero-based
   while `DEPTH` counts `render_message` frames from 1, so a node at
   depth *d* is walked with `DEPTH == d + 1`, and `at_depth_cap` begins
   degrading at depth `MAX_WIRE_DEPTH - 1`. Refusing on *reaching* that
   depth means no degraded node is ever recorded — the check is the same
   one, moved one step earlier so it is a precondition rather than an
   after-the-fact detection.

   The one visible loose end is dead code: nothing calls
   `walk_document_order` until step 4 consumes it.
4. **Phase 2** — S8's counting sort into level order, the `first_child`
   derivation, and the permutation of phase 1's arrays. The four output
   arrays move out together (S25). Separately testable against phase 1's
   output as the reference: item 14, plus items 3 and 4 which only
   become meaningful once the layout is level order. S16 is satisfied
   here.

   **Done 2026-07-31**, in the same file. `sort_into_level_order` is the
   four passes S8 describes — count per depth, prefix-sum to level
   bases, scatter in document order, then rewrite `parent` through the
   permutation while permuting the byte ranges. `first_child` is derived
   afterwards in two sweeps: forward, the first slot claiming *p* is
   where *p*'s block starts (slot 0 is skipped, which is what stops a
   self-parenting root from being its own first child); backward, a
   childless node inherits the next node that has one, which is what
   makes the subtraction give a child count for *every* slot rather than
   only for parents.

   The `Arena` that moves out is **one** `Vec<u32>` of `4n + 1`, sliced
   into the four arrays by the accessors, per S8's memory table — not
   four separately owned `Vec`s. They are built together, have identical
   lifetimes and are read by disjoint passes, so one `malloc` and one
   first touch beat four. Phase 1's arrays are consumed by value and
   dropped the moment the sort returns, so the 34n peak is confined to
   this function.

   `build_arena` (phase 1 then phase 2) and `Arena` are the crate's
   public surface, re-exported from `lib.rs`; step 3's dead code is
   resolved by that.
5. **S28, S29** — the wrapped buffer, the two producers, the peek and
   the two flags. Independent of the arena but a **prerequisite for
   step 6**, since the arena indexes the wrapped bytes. Items 12, 13.

   **Done 2026-07-31.** `protolens/src/blob.rs` holds the whole of it:
   the headroom, `from_headroom`'s backwards-written prefix, the peek,
   both producers and the `MAP_FIXED` mapping. `Blob` derefs to the
   wrapped bytes, so the ~30 sites that read `self.blob[..]` are
   untouched; it is held as `Arc<Blob>` from `main` through `Decoded`
   into `App`, which turns the heat worker's whole-blob clone
   (`tui/mod.rs:2392`) into a refcount bump and is what lets a mapped
   blob cross a thread at all. `decode::wrap_blob` is deleted — it was
   the last per-call wrapper — and `decode`/`render_resolved` take the
   `Arc<Blob>` rather than a slice, reading `blob.payload()` where they
   want the file's own bytes.

   Two departures from S28/S29 as written:

   - The appending API landed in prototext-core as
     `encode_text_to_binary_into`, not as a flavor of `render_as_bytes`.
     That is the function `render_as_bytes` delegates to on the text
     branch, so routing through the outer one would only have re-derived
     the peek protolens has already made. The change needed was one
     parameter: every index the placeholder machinery records is already
     absolute (each is an `out.len()` at the time of writing), so only
     `compact`'s sweep assumed the encode owned the whole buffer.
   - `encoded_capacity` was a ratio guess (`len/6`) and is now an upper
     bound: the text length, plus [`BASE_OVERHEAD`] per `{` for the
     placeholders that live in the buffer until compaction removes them.
     Appending exists to save a copy, and a reallocation mid-encode
     would hand that copy straight back, so the reservation has to be a
     bound rather than a typical case. Item 12 pins it.

   Mapping also gained two fallbacks S29 did not name, both silent
   because both are ordinary ways to run protolens rather than mistakes
   to report: a file **under 1 MiB**, whose saved pages would not repay
   the mapping's setup and which is what the per-type instance blobs a
   reader opens by the hundred all are; and anything that is **not a
   regular file**, a fifo being the case `--eager-read` cannot see
   coming. A failing `mmap` falls back the same way.
6. **Adopt the arena** — S1, S1a, S12, S17, S27: path lookup by
   arithmetic, derived document order, per-interpretation overlay.
   Deletes `sibling_ordinal`, `doc_next`/`doc_prev` and `build_tree`'s
   sort. Items 3, 4.

   **Done 2026-07-31**, in three passes: build the arena at load and
   cross-check it against `build_tree`'s own structure, move the
   readers onto it, then delete the cross-check along with the links it
   compared. The structural accessors are gathered in
   `tui/structure.rs`, which is where the two halves — arena and
   overlay — actually meet.

   Two production defects the migration exposed, neither of them
   test-only breakage:

   - `splice_override` has to vacate `idx`'s **own** slot before
     `overlay_spans` writes the new interpretation. `overlay_spans`
     treats a second span landing on an already-rendered slot as one
     more row of a packed run and *adds* its lines, so leaving the old
     interpretation in place counted `idx`'s lines twice.
   - `absolute_start` never summed the roots preceding `cur`. A loaded
     document has one root and the term is zero, which is why the bug
     could exist at all; a fixture handing the arena an unwrapped blob
     of several top-level records has several, and every one of them
     reported line 0.

   The verification is `#[cfg(test)]`-gated throughout — the production
   binary carries no arena probe, env-var-driven or otherwise.
7. **Display rows** — S6, S7, S22: span-to-slot mapping keyed on
   `same_packed_record`, `LinePos` widened to `line_in_node`. Items 1,
   2, 8.

   **Done 2026-07-31**, with step 6; the two are one change seen from
   two sides, since a packed run collapsing onto one slot is what makes
   a node draw more than one row in the first place.

   `cursor_footer: bool` becomes `cursor_line_in_node: u32`, and the
   footer is `lines_total - 1` — equal to 1 only for a node with
   nothing between its braces, which is the trap every converted call
   site fell into once.

   `extract::message_payload_range` **loses its `packed_record_start`
   parameter**. It existed so a bare packed element, which carries no
   wire tag of its own, would not have a tag stripped off it; under S22
   the node's `raw_range` is the whole `WT_LEN` record, an ordinary
   tagged field, so the generic path is right for it. `packed_record_
   siblings` goes the same way, a run being a single slot.
8. **Delete the dynamic arena** — S11, S13, S23: `local_tree` and the
   structural splice go; specs 0203 and 0206 are superseded. Items 7,
   11.

   **Done 2026-07-31.** Falls out of step 6 rather than being separate
   work: with the arena immutable there is nothing to append, so
   `local_tree`, the compaction pass (`tui/compact.rs`) and spec 0202's
   memory guard are all deleted rather than rewritten. Specs 0202, 0203
   and 0206 are marked superseded.

   `absolute_start` is S23's sequential scan over the preceding
   siblings' contiguous run, plus one term for the roots below `cur`.
9. **S10** — move the walk off the main thread. Last, deliberately: it
   is pure scheduling, and everything must be correct serially first.
   Both phases move together; they are one unit of work.

## Test plan

1. **Superset property, by construction.** Over a corpus and a set of
   type assignments, assert every emitted `NodeSpan` corresponds to a
   slot already in the maximal tree, or is a display row under S22.
   Cheap to check, and the regression guard for S15's single
   decomposition (Q4).
2. **The probe-rejected payload.** Construct a payload the 0097 probe
   declines, inside a field an override can give a message type. Assert
   the greedy walk gave it children, and that overriding an *ancestor*
   still finds every slot. The concrete instance of item 1 that
   motivated S14.

   First half done 2026-07-31, in prototext-core:
   `greedy_recurses_where_the_probe_declines` renders field 1 LEN
   `"hello"` — five bytes that read as a varint followed by an unmatched
   `END_GROUP` — through one sink twice, and asserts 0 nested openings
   with the default and 1 with `unknown_len_is_message`. The second
   half, the ancestor override, needs the arena and belongs to step 6.
3. **Path lookup against a reference.** For every node of a corpus blob,
   assert S17's descent from its positional path returns its own slot,
   and `i - first_child[parent[i]]` equals the ordinal a naive chain
   walk computes. Include a message carrying the **same field number
   twice** and one whose fields are **not in field-number order** —
   the two cases where a field-number path and a positional path
   disagree (S1a).
4. **Document order without stored links.** Assert S27's derived
   `doc_next` reproduces today's sorted `doc_order` exactly, and that
   iterating from slot 0 visits every slot once.
5. **Malformity is derivable.** For every node, assert that re-parsing
   the tag at `raw_start` and comparing the extent against
   `raw_end - raw_start` agrees with what the walk decided. This is what
   makes S19's dropped flag safe.
6. **Depth refuses rather than truncates.** A blob nesting past S9's D
   is rejected at load with a depth diagnostic, and no partial arena is
   handed over. Conversely, for a blob that *does* load, override a node
   inside a deeply nested subtree and assert the subtree render — whose
   `DEPTH` counter restarts at 0 — finds every slot it descends into.
   This is the case the withdrawn S9 would have panicked on.

   Alongside, and on the same thread the walk will really run on: a blob
   at exactly D nests without overflowing the stack. The model is
   prototext-graph's `max_depth_walk_fits_in_a_default_thread_stack`,
   including its `#[cfg(not(debug_assertions))]` guard — debug frames are
   ~8x release — and its trick of asserting the node count too, so an
   early stop cannot make the stack assertion vacuous.

   First half done 2026-07-31, in `arena.rs`: `a_tree_at_the_recursion_
   cap_is_refused` gets `InputTooDeep` and no arena at all, and
   `a_tree_just_under_the_cap_walks` gets a full one — both release-only,
   like the neighboring `deeply_nested_len_*` tests. The subtree-override
   half needs the arena and belongs to step 6, and the stack assertion on
   the *real* thread belongs to step 9, which is what creates it.
7. **Path stability across overrides.** Applying, deactivating and
   reapplying an override leaves every path on the same slot index.
   Today this is exactly what fails.
8. **Cursor traversal over multi-line nodes.** A packed record of many
   elements, and an Any expansion, both fully traversable by repeated
   `move_down` with no barrier — the 2026-07-25 bug class, against S7's
   widened coordinate. Include a message whose header and footer sit
   many lines apart, since S7's `line_in_node` indexes the node's lines,
   not the screen.
9. **Existing render output is unchanged.** The full prototext suite
   passes with S14's `unknown_len_is_message` and S21's `gar` change in
   place. N5 as an executable claim. **Done 2026-07-31**: 804 tests, 0
   failures, clippy clean on all targets, `reuse lint` compliant — step
   4 consumes phase 1, so the dead-code warnings step 3 carried are
   gone. Step 5 rewrote every `decode(&blob, ..)` in the suite to
   `decode(wrapped(&blob), ..)` and every hand-built `App` fixture's
   `blob` field to an `Arc<Blob>`; the output assertions are unchanged,
   which is the point.
10. **Existing render performance is unchanged.** `bin/bench -p
    prototext-core --bench codec`, baseline against baseline first to
    establish the floor (-3.0%..+1.7% for this target), then against the
    change. S21 should show as an *improvement* on schemaless input.
11. **The sibling scan.** Re-run spec 0215's pty harness
    (`/tmp/scroll_asym.py`: `G`, then 150 PageUp at 30 Hz, with
    `PROTOLENS_TRACE` parsed) and compare the **`draw` and `render`
    lines, not `key`**. Baseline is the post-0215 state: `key` already
    356 µs, a frame ~1 292 µs at the top against ~3 000 µs deep. S23's
    claim as a number is that the *gap* closes. The 13 624 µs figure
    belongs to spec 0215 and is not this spec's baseline. Secondary: the
    post-burst repaint backlog (~25 s, 384 of 641 frames `draw heat`)
    shortens in proportion.
12. **Reserve and shrink behave as claimed.** S24 governs phase 1 only,
    so measure it there: peak RSS across a load shows phase 1's
    over-reservation is not resident, and `shrink_to_fit` returns the
    tail without a copy. Both are platform behavior and must be measured
    on it. Separately, assert the combined peak matches S8's formula —
    34n while both phases are live, 16n + 4 once phase 1 is dropped.

    The cheap half is done 2026-07-31:
    `the_over_reservation_is_returned` asserts the finished arrays hold
    capacity equal to the node count, not to the blob. That the shrink is
    an `mremap` tail-unmap rather than a copy, and that the reservation
    was never resident, are the platform claims and still need measuring.

    Alongside: assert S28 wraps once, that the blob
    is never copied on either producer path (binary and `#@ prototext`),
    and that the backwards-written prefix round-trips for every varint
    width — including the boundaries where `w` changes.

    **Done 2026-07-31** for the prefix and the copy:
    `the_wrapper_prefix_lands_flush_at_every_varint_width` wraps payloads
    of 0, 1, 127, 128, 16 383 and 16 384 bytes and parses the prefix
    back with `parse_wiretag`/`parse_varint`, asserting the recovered
    field number, wire type and length, and that the prefix ends exactly
    where the payload begins.

    That the text producer does not copy is
    `the_reservation_is_never_outgrown` (prototext-core): it reserves
    exactly `encoded_capacity` and asserts the capacity is unchanged
    after the encode, so a growth — which is the copy — fails the test.
    It runs over the 1 MB `descriptor_protoc.txt` fixture and over a
    synthetic message-dense input, the latter because the term the
    output bound does not cover is the transient placeholder per `{`,
    and `a {  #@ n` is about the shortest line that buys one. The binary
    producer's copy is structural rather than testable: with mapping
    there is no buffer to copy into, and `read_to_end` fills the
    headroom buffer directly.
13. **Producer selection.** S29's peek routes a binary blob to the map
    and a `#@ prototext` blob to the encoder, and both yield identical
    wrapped bytes. `--assume-binary` maps a file whose first 13 bytes
    are the magic; `--eager-read` reads, and is accepted silently as a
    no-op on text input. A fifo (process substitution) and a
    sub-threshold file each fall back to reading **without an error**.

    **Done 2026-07-31**, less the fifo:
    `the_magic_selects_the_text_producer_and_assume_binary_overrules_it`
    loads one file both ways and asserts the payload is the encoding in
    the first case and the text verbatim in the second;
    `declining_to_map_produces_the_same_blob` asserts an over-threshold
    file *is* mapped, that `--eager-read` declines, and that the wrapped
    bytes agree either way; `a_small_blob_is_read_without_complaint`
    covers the size fallback. The mapped assertion is deliberate even
    though a mapping failure is silent in production — without it the
    test would pass with the mapped producer never running.

    The fifo case is not covered: `Blob::load` needs a path, and a unit
    test would have to make a named pipe and feed it from a second
    thread to avoid blocking on `open`. The code path it would exercise
    is the same `is_file()` test the size fallback shares.
14. **The sort produces level order.** Phase 1's output is the reference
    and the assertions are on phase 2's, over a corpus and over crafted
    group nestings:
    - `depth` is non-decreasing across the array, and slot 0 is the root;
    - `parent[i] <= parent[i+1]` within a level, so each parent's
      children form one contiguous block — this is the property S17's
      `first_child[i] + step` arithmetic rests on, and it is the one
      thing document order does not give;
    - `first_child[i]` is the first slot whose parent is `i`, and equals
      the *next* node's `first_child` when `i` is childless, so the
      backward sweep is checked and not just the forward pass;
    - the permutation is a bijection: every phase-1 slot appears exactly
      once, and `raw_start`/`raw_end` travel with it;
    - stability, stated as an observable: reading the level-order array
      left to right within one level reproduces phase 1's document order
      for those same nodes.

    Include a blob whose *only* nesting is groups, since that is the
    case that has no length prefix to fall back on, and one at depth 1
    (a flat message) to exercise the D = 1 degenerate counting sort.

    **Done 2026-07-31.** A shared `assert_level_ordered` carries the
    structural half, and it states the block property directly rather
    than as `parent[i] <= parent[i+1]`: for every slot *i*, the range
    `first_child[i]..first_child[i+1]` holds exactly the slots naming
    *i* as parent, all one level deeper — and those ranges partition the
    non-roots, which is the bijection. Root-ness is asserted as an
    equivalence with depth 0, not as "slot 0", since a blob may have
    several top-level fields and so several roots.

    Around it: `the_sort_moves_document_order_into_level_order` is the
    smallest blob where the two orders differ (two messages with one
    child each; document order interleaves them, level order does not)
    and pins all four arrays by value; `nested_groups_sort_like_anything
    _else` is the group-only case; `a_flat_blob_is_already_level_ordered`
    the degenerate one; `an_empty_blob_gives_an_empty_arena` the zero
    case, where only the sentinel exists.
    `a_real_blob_sorts_into_level_order` runs both phases over the
    committed descriptor fixture and checks the sort against phase 1 as
    the reference — equal node counts and an equal multiset of byte
    ranges, so it is a permutation and nothing else — then reads
    stability off the result: within each parent's block, `raw_start`
    strictly increases.

## Open questions

- **Q1.** ~~The node-count multiplier of always-recurse.~~ **Answered:
  1.01-1.02x.**
- **Q2.** Whether the memory comparison holds once done properly. The
  right comparison is not node count against node count — at 1.02x
  nodes (Q1) that term is a rounding error — it is **structure against
  structure**. Today's `TreeNode` is 264 B, and the measured 3.93 GB
  peak on `googleapis.desc` is three terms: the arena including
  abandoned slots ~2.74 GB, `local_tree` 1.19 GB, and the render cache's
  span clone 432 MB (unaccounted in that sum — an open discrepancy in
  its own right). S18 replaces the first two with 4.58 M x 16 B =
  **73 MB**, once, never growing; S11 deletes `local_tree` outright.

  What is genuinely not yet stated is **S12's overlay width** — per-slot
  `rendered_as`, `lines_total`, `lines_visible` and resolved type — and
  the render products, which this spec does not by itself shrink. Those
  are the terms to price before implementation; the structural term is
  no longer in doubt.
- **Q3.** ~~Whether inference can read the arena instead of re-walking
  the bytes.~~ **Answered: no, and it must not.** S10 runs the two
  concurrently, forbidding a dependency either way — and there is
  nothing to read, since inference scores candidate type assignments
  over ranges, the one thing the arena deliberately knows nothing about.
  What the concurrency raises instead is **what the build actually
  costs** in the form it will take (both phases, writing S18's arrays).
  That number does not exist; S10 is written so it need not, but measure
  it alongside item 12.
- **Q4.** ~~The superset property is an unverified load-bearing claim.~~
  **Withdrawn.** It follows from S2 (the walk declines no payload) and
  S15 (one decomposition, so the renderer cannot draw a boundary the
  arena did not). Test item 1 stays as a regression guard, not a
  question. The one thing S2 does not imply is **unterminated-group
  recovery** — and that is settled by construction, not left open: under
  S15 there is one decomposition, so the arena inherits whatever
  prototext-core already does, whatever that is. The rule need not be
  chosen; it needs only to stay single. (`maximal_walk.rs` had to
  reimplement it by hand, which is why it reports both bracketing rules
  in Measured outcome — that ambiguity is the measurement tool's, not
  the design's.)
- **Q5.** ~~Whether greedy is a `Sink` capability or a thread-local.~~
  **Answered: neither.** S14 makes it a property of the driver.
- **Q6.** ~~Depth-first or level order?~~ **Answered: both, in
  sequence** (S8). They were never rivals for the same job: depth-first
  is the only shape that reads the bytes once, level order is the only
  shape that makes S17 arithmetic, and a linear counting sort converts
  one into the other. What the *finished layout* still gives up is
  document-order locality — a subtree is scattered, and rendering
  descends subtrees. The judgment is unchanged: this loses to S23, since
  a render window touches ~50 nodes and is already scattered by folds
  whereas the sibling sums are unbounded and are the measured problem.
  Item 11 should be read with it in mind.
- **Q7.** ~~What bounds the number of levels, and what does a level
  cost?~~ **Answered: S9, at D = 1000**, and a level costs nothing
  particular — phase 1 reads every byte once regardless of depth, and
  phase 2 is O(n + D). The level histogram is still worth recording, as
  an input to phase 2's write-stream count (3D streams, ideal at the
  measured 13).

## Measured outcome

Measured 2026-07-30 with `prototext-core/examples/maximal_walk.rs` on
`googleapis.desc` (25 660 332 bytes), whose rendered arena holds
4 501 014 nodes today. The walker recurses into every length-delimited
payload without probing, so the figures already price the greedy walk:
closing that hole costs no extra slots.

**The node counts are the load-bearing result; the timing is a floor and
must not be quoted as a cost.** The walker only counts: it writes no
arena, so it never pays for first-touching phase 1's ~64 MB or phase 2's
~73 MB (~34 000 faults between them), and it hand-rolls its tag check, so
it prices no result struct at all. Phase 2 is absent from it entirely.

What makes 70-73 ms a *good* floor is that the walker is depth-first,
which is phase 1's own shape: it is the traversal, missing only the
writes. Phase 2 goes on top, touching each of n slots a small constant
number of times with no parsing at all.

Two opposite recovery rules for an unterminated `START_GROUP`: *nested*,
in which the group consumes the rest of its enclosing payload, and
*flat*, in which the tag is a leaf and scanning continues at the next
byte.

| | nested | flat |
| --- | ---: | ---: |
| nodes | 4 583 680 | 4 535 032 |
| **vs today** | **1.02x** | **1.01x** |
| length-delimited | 2 136 296 | 2 136 296 |
| malformed | 1 168 488 | 1 137 395 |
| max depth | 13 | 10 |
| one pass | 73 ms | 70 ms |

**These are not a bracket.** `maximal_walk.rs` labels nested
"undercounts", reasoning that swallowed bytes yield no nodes — but
nested measures 1.1% *more*, and the direction is unexplained. Nothing
here establishes which side the answer is on. What it does establish is
all the design needs: **the recovery rule moves the count by about one
percent**, so no conclusion depends on choosing one. The example's
labels should be dropped.

Three conclusions.

- **The maximal tree is essentially the size of today's arena**,
  1.01-1.02x. The always-recurse inflation (about a quarter of nodes are
  malformed children of payloads that are really strings) is very nearly
  cancelled by S6 dropping packed elements. At 16 bytes a slot the
  frozen arena is ~73 MB, and the entire dynamic part is gone.
- **Depth is small on real data**, 13 against a render cap of 1000. That
  is a comfortable margin for choosing S9's D, but it is one corpus and
  it is not licence to skip the bound: the always-recurse walk's depth
  is driven by coincidences inside strings, not by the data's real
  nesting, so the tail of the distribution is what matters (Q7).
- **The walk's tag-parsing component is small**, ~70 ms against a
  startup budget where descriptor decoding alone is ~1.7 s. This is the
  weakest of the three and does not license a serial plan: see the
  caveats above and S10.

Density, for S24's fallback: 25 660 332 / 4 583 680 = **5.6 bytes per
node**.

Cross-checks: the walker independently reports 7 771 nodes at depth 0,
matching the known root fanout; and the malformed share is 25.5% here
against 20.6% aggregated over the 375 per-type instance blobs, so the
spurious-recursion rate is stable across data shapes rather than an
artifact of descriptor data.

Not measured: the memory comparison proper (Q2) and phase 2 — neither
its time nor the 34n peak, both of which need a real implementation
rather than a counting walker.

### After implementation, 2026-07-31

Steps 1-8 landed. Same corpus, same box, `--descriptor-set /tmp/pdb.desc`
over `googleapis.desc`, driven through a pty; the commit measured is
`:type-as-raw` **on line 0**, the root retype, which is the only one that
used to materialize a full-document `local_tree`. The 76 B column is spec
0213's recorded figure for the identical driver.

| | 0213 (76 B slot) | after 0216 |
|---|---|---|
| VmRSS at rest | 1 063 420 kB | **988 280 kB** |
| VmHWM, root retype | 2 188 064 kB | **1 740 056 kB** |
| VmRSS after commit | 1 378 684 kB | 1 434 080 kB |

So the peak falls **2.09 → 1.66 GiB (−20.5%)** and at rest **1.01 → 0.94
GiB**, against a starting point of 4.18 GiB before the slot-narrowing
specs: **−60% cumulative**. The at-rest saving is smaller than the slot
alone would suggest because the arena's own `4n + 1` `u32` array is new
memory — ~81 MB at 4 737 284 slots — paid to delete the seven links.

**The arena is 4 737 284 slots** against 4 501 014 nodes in the old
render-derived tree: 1.05x, close to the 1.02x the counting walker
projected. Of those, 2 909 311 are rendered raw and 2 831 045 under
`FileDescriptorSet` — the arena is roughly 1.6x either interpretation,
which is the price of describing all the structure the bytes admit rather
than one reading of it.

`arena_gap`'s three properties — coverage, agreement, all-or-nothing —
hold over the whole corpus in both interpretations
(`the_arena_covers_a_real_corpus`). This is the superset claim, and it is
now checked against real data rather than only against fixtures.

**Spec 0202's crash is gone rather than guarded.** Its reproduction —
`Down`, then three rounds of `t`, `Enter`, `o`, `d`, `Esc` — reported
2045 → 3889 → 5256 MiB → OOM kill originally, and a flat 3.9 GiB once
0203 compacted. It now runs at **995 MiB, flat to 0.2 MiB across all
three cycles**: a splice allocates no slots, so there is nothing for a
second cycle to add.
