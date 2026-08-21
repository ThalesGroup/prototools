<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0343 — the last one wins, and the others say so

Status: draft
App: prototext-core, protolens
Refs: docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md (the
        annotation vocabulary and its drift test, which the new keyword
        enters through),
      docs/specs/0249-a-large-document-answers-the-user-first.md (the
        row budget and `Status::Unbaked`),
      docs/specs/0114-protolens-range-type-override.md (its blast-radius
        note inventories the `NodeSpan { .. }` literals A4 must reach),
      docs/specs/0212-the-span-is-a-third-as-wide.md (the 32-byte
        `NodeSpan` assertion A4 must not move, and the three places that
        quote it),
      docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (the
        maximal walk, positional slot identity, and the immutability
        the whole of B4 rests on),
      docs/specs/0222-the-text-lives-in-the-nodes.md (the derived
        closing brace — the first chunk `DocCursor` manufactures, which
        B11's mark joins),
      docs/specs/0235-a-search-answers-while-it-is-still-being-typed.md
        (the resumable idle-arm sweep the shadow sweep is modelled on),
      docs/specs/0255-the-document-finishes-itself-while-nobody-waits.md
        (`bake_step` and the idle ladder the shadow sweep joins, one
        rung above it and one below; also `auto_folded`, which B6
        rejects as the gate and B9 borrows an idiom from),
      docs/specs/0257-the-first-pane-does-not-wait-for-the-last-line.md
        (the open path B6's first stage is forbidden to touch),
      docs/specs/0274-a-match-may-cross-a-line.md (`node_text` is an
        `Arc` shared with the segment scan, so writing it halts the
        scan — the reason the shadow mark is not written there),
      docs/specs/0285-a-document-token-says-what-it-is.md (a keyword's
        tier and its clause have one home),
      docs/specs/0302-a-short-tail-is-still-walked.md (the malformed
        arena slot, whose `raw_start` need not carry a readable tag —
        B4 has a rule for it),
      docs/specs/0318-a-preview-ends-where-a-record-ends.md
        (`window_text` is what the highlighter parses; display
        insertions are applied downstream of it, and every row that
        *is* parsed must be grammatical prototext),
      docs/specs/0322-a-leaf-can-be-wrong-too.md (the margin mark a
        `Tier::NonCanonical` row already earns),
      docs/specs/0328-the-node-you-are-in-has-an-edge.md (the preview's
        `...`, the precedent for a styled display insertion in
        `row_spans`)

## Background

Overriding a node as `google.maps.places.v1.SearchTextRequest` scored
`-8`: twelve fields matching at `+1` and one non-canonical encoding at
`-20`. Nothing on the screen said which encoding.

The charge was a duplicate `text_query`. `places_service.proto:111`
declares it `string text_query = 1` — singular — and the document
carries it twice, so `apply_cardinality_multi`
(`prototext-graph/src/score/walk.rs:1321`) charged
`non_canonical += count - 1`.

It is invisible for two independent reasons.

**The score box structurally cannot point at a byte.** `EntryScore` is
six `u64` counters plus a bool, and `ScoreBreakdown` inherits exactly
that. No offset is recorded anywhere upstream, for *any* term. There is
no lost offset to recover; one was never taken.

**The renderer never asks the question.** protolens's `NON_CANONICAL`
list (`protolens/src/annotation.rs:63`) is eleven encoding-*shape*
anomalies — `tag_ohb`, `val_ohb`, `nan_bits`, `neg` — every one of them
readable from a single record's own bytes, which is why the emitter can
attach one to a row as it writes it. Cardinality needs the schema and a
frame's history, and `prototext-core` keeps neither. It reads
cardinality in exactly three places, none of them stateful:
`helpers/annotations.rs:69` and `:200` for the `repeated `/`required `
label prefix, and `helpers/len_field.rs:178` for the packed decision.
`render_message` (`mod.rs:630`) is a streaming loop over tags with no
per-frame field bookkeeping.

### Two findings, not one

The obvious design — one keyword meaning "this value is dead" — does not
survive contact with merge semantics. Given

```
1: { 2: { 3: { 4: "toto" } } }
1: { 2: { 3: { 4: "titi" } } }
```

the same bytes read three different ways depending only on the
descriptor: if every field is singular, `"toto"` is dead; if field `4`
is repeated, nothing is dead; if field `2` is repeated, nothing is dead
and the two subtrees stay side by side. Protobuf merges repeated
instances of an embedded **message** field as if by `MergeFrom`, and the
recursion descends until it reaches the first non-merging node — whose
position is a property of the schema, not of the encoding.

So the duplicate at field `1` is *plumbing*: reporting it as a loss
would be false. But it is not nothing either. No `protoc`-generated
serializer emits a singular field twice, in any language; a singular
message field split across two occurrences cannot come out of
`SerializeToString`. The duplicate is itself the deviation from
canonical serialization — evidence of concatenation, an append pipeline,
a hand-built payload, or a probe for a parser differential. And it is
exactly what the scorer already charges for.

This spec therefore reports two things, in two places, at two costs:

- **the repeat** — a singular field observed more than once *in one
  physical frame*. Cheap, exact, needs no history beyond the frame,
  and matches `apply_cardinality_multi` unit for unit. This goes in
  `prototext-core`, as a `#@` keyword in the document:
  `repeated_singular`.
- **the loss** — a scalar whose value is actually dead, which can lie
  arbitrarily deep under a chain of merges. This cannot be decided as
  the bytes stream past. This goes in protolens, as a bit per arena
  slot: `shadowed_scalar`.

The two findings are named after what they say and not after how they
are computed, because *structural* and *circumstantial* are spoken for:
B3 uses them for the two **conditions** that together decide the
second finding, and neither of those is the first one.

They are not two unrelated computations, and Specification B is built
on what they share. Underneath both there is one schema-free reading of
the bytes: *take every field to be singular and merge accordingly*.
Applied to a single frame that reading is a repeated field number,
which is what `repeated_singular` reports. Applied to the whole
document it is a **merged tree**, in which every displaced value is
linked to the value that directly displaces it, under the ancestor
they have in common. Cardinality then does not *compute* anything — it
**filters**: a link survives iff the fields between the two values and
that ancestor are all singular. Structure says what the bytes permit;
the descriptor says which of it happens.

## Goals

- **G1.** Every unit `apply_cardinality_multi` charges for is visible on
  a row, in `prototext decode` output and in protolens alike, without
  reading the schema by hand. Of a rendered body: a row budget can defer
  a body, and a mark deferred with it arrives when the body does (A7).
- **G2.** The number of `repeated_singular` marks in a frame equals the
  units that frame is charged, so the score box's arithmetic can be
  checked by eye. Under two conditions, both of which the reader who
  is checking a score already satisfies: the frame is rendered rather
  than deferred (A7, as in G1), and the type the render resolved is
  the candidate the score was computed under (N1). They are two
  computations that agree, not one computation read twice, and the
  equality is a test (item 4) rather than an invariant in the code.
- **G3.** A scalar whose value is discarded says so on its own row in
  protolens, however deep the merge chain above it.
- **G4.** Neither mark introduces a display path. `repeated_singular` is
  a `#@` keyword and inherits its tier, color, hover clause and margin
  mark; `shadowed_scalar` inherits the margin mark through
  `own_status` and is drawn by the insertion mechanism `row_spans`
  already has.
- **G5.** **Time to first frame does not move.** The open path stays
  unaware that any of this exists — the answer is worth waiting for and
  the first screenful is not. Everything Specification B adds happens
  after the document is on screen, in the order B6 sets out.

## Non-goals

- **N1.** **The score box is not linked to the rows.** The `#@` clause
  comes from the emitter under the *resolved* type; `non_canonical`
  comes from the scoring walk under a *candidate*. They agree when those
  are the same type, which is the case that prompted this, but they
  remain two independent computations. Linking them means recording
  offsets in `EntryScore`, which this spec does not do.
- **N2.** **`prototext-graph` does not change.** Not its counters, not
  its weights, not `apply_cardinality_multi`. `shadowed_scalar` carries
  **no penalty**: every shadowing implies at least one singular field
  repeating in some frame, so the deviation is already priced, and
  pricing it twice would make the score depend on the depth of the merge
  chain.
- **N3.** **`shadowed_scalar` is not *stored* document text.** It does
  not reach `prototext decode`, is not parsed by `row_status`, and does
  not round-trip through `encode_text`. Only `repeated_singular` is
  document text.

  It *is* searchable, though (B11). Spec 0222 already separated the two
  sets: a bracketed node's closing brace is stored nowhere and is
  searched all the same, because `DocCursor` manufactures the haystack
  rather than owning it. The mark is the second member of that set, not
  a new kind of thing.
- **N4.** **`grammar.js` does not change.** The grammar gives `#@`
  annotations structure but stays vocabulary-blind: `neg` and `nan_bits`
  are `annotation_word` nodes like any other, and `repeated_singular`
  lexes the same way. Only the `#any-of?` list in `highlights.scm` §7
  enumerates the vocabulary.

  Which is also why A5 and A6 are *both* required and neither is
  redundant, and it is worth writing down what each one changes.

  **On the row the default is green, and a tier is an exception to
  it.** The whole `#@ …` run is a comment to the grammar, so the `#@`
  marker and every token no rule claims take `SyntaxRole::Comment` —
  `theme.rs`'s `comment`, `#6A9955` on the dark page and `#008000` on
  the light one. `highlights.scm` §7 then overrides two sets of words
  and no others: `#any-of? @annotation.non_canonical` for the amber and
  `@annotation.invalid` for the red, mapped through `colorize.rs`'s
  `RECOGNIZED_NAMES` to `AnnotationNonCanonical` / `AnnotationInvalid`.
  Green therefore does not mean *checked and clean*; it means *nothing
  claimed this word* — which is the same arm `pack_size` and
  `[packed=true]` land in.

  **In the margin the color comes from somewhere else entirely.**
  `annotation::tier_of` reads its own `NON_CANONICAL` / `INVALID`
  arrays, and `node_status.rs` turns the tier into a `Status` that
  `theme.rs`'s `status_color` colors. Two lists, two mechanisms, one
  vocabulary: a keyword added to only one of them is amber on the row
  and silent in the margin, or the reverse. A6 buys the row, A5 buys
  the margin, and `repeated_singular` needs both or it reads as an
  unclassified modifier.
- **N5.** **Unknown fields are never marked.** With no schema there is
  no cardinality, and the scorer charges nothing — the two stay in
  agreement by both saying nothing.
- **N6.** **`oneof` and map keys are out of scope.** Both destroy values
  without any field number repeating, so both are silent here — and both
  are silent in the scorer today. They are not oversights, and nothing
  in this design has to be undone for either of them. They are not
  equally cheap to add, though, and the annex says which is which: a map
  is a change to the filter and to nothing else, while a `oneof` is a
  schema fact and B4's pass has no schema — so it needs a second pass
  that does, not a new clause in this one.
- **N7.** **No new thread and no second pool.** The last attempt to add
  a second source of background work shipped and was reverted, at
  +17.6% on a real `-j 8`. The shadow sweep is a step in the existing
  idle ladder, on the main thread.

## Specification — A. prototext-core

- **A1.** **One keyword, `repeated_singular`, `Tier::NonCanonical`, on
  occurrences 2..n.** It says: *this field number has already appeared
  in this frame, and the schema declares it singular*. It is a statement
  about the repeat, not a claim about which value survives — which is
  why it can be decided the moment the tag is read, with no lookback.

  It applies to every kind alike: scalar, enum, message and group. The
  scorer makes no distinction and neither does this.

- **A2.** **The frame's state is two locals in `render_message`.** A
  `u64` bitmask over `field_number % 64` and a
  `SmallVec<[u32; 8]>` of the field numbers seen. A clear bit proves
  first occurrence, so the common path is one bit test and a push with
  no scan; a set bit sends it to a linear scan of the vec to separate a
  real repeat from a modulo collision.

  The recursion gives both frame scoping for free. Only schema-backed
  singular fields are registered — repeated and unknown fields skip
  both, which is what makes N5 structural rather than a special case.

  A linear scan rather than a hash because at eight to twenty `u32`s it
  is one cache line and hashing buys nothing. If profiling says
  otherwise, the shared-scratch-with-watermark idiom `packed_scratch`
  already uses is the next step, not a map.

- **A3.** **The `Sink` trait does not change** — neither for the keyword
  nor for A4.

  Not for the keyword, because the decision is made *before* the
  occurrence's line is written, so it joins the line's other annotations
  through `AnnWriter` exactly as `neg` does: no handle, no reservation,
  no post-hoc splice, no trailing-space strip, no second pass. This is
  the whole reason the structural finding was separated from the
  semantic one.

  Not for A4 either, and that is worth saying because A4 asks a sink to
  learn something new. `scalar_field` and `begin_nested`
  (`sink.rs:122-153`) already take `field_schema: Option<&FieldOrExt>`,
  and `FieldOrExt::cardinality()` (`render_text/mod.rs:53`) is in scope
  for `arena.rs`. `ArenaSink` derives the label from a parameter it is
  already handed.

- **A4.** **`NodeSpan` records the field's *label*.** Two bits, and
  *only* those two — the *kind* Part B dispatches on is already there.
  `NodeSpan::is_message` (`sink.rs:1394`) is documented as the
  structural shape discriminator consumers should use, true for a
  nested message or group and false for a scalar. Part B reads it
  as-is.

  **Four states, not three, because two bits hold four and
  `Cardinality` has three.** `Optional`, `Required`, `Repeated` and
  *no schema* is exactly the width available, so the label stores the
  descriptor's own answer unabridged rather than a collapsed one. B5
  is where `Optional` and `Required` become the same thing — both are
  singular, which is also how `apply_cardinality_multi` reads them
  (`walk.rs:1336-1355`: labels `0` and `1` both charge on
  `count > 1`, and only the `_` arm is exempt). Collapsing at the
  point of storage would save nothing, would throw away the one bit
  that distinguishes a `required ` prefix from an `optional` one, and
  would make test item 30's round-trip a lie.

  This is the one place the two halves of the spec touch, and it is
  worth being plain about why the label is recorded rather than
  recovered. The `#@` clause would need a second parser, and resolving
  the type in protolens would mean re-deriving what the render already
  decided — under the same overrides, with the same wrapper and
  synthetic-type rules — and would make the answer depend on a node
  having been rendered *and* on its provenance being resolvable. The
  emitter has the schema in hand at that exact moment: it is what
  decides the `repeated `/`required ` prefix and the packed question
  already. Two bits is strictly cheaper than recovering them, and it
  follows spec 0342's rule — what the emitter emits, the emitter owns.

  **The two bits must cost no bytes.** `NodeSpan` is pinned at 32 by
  `const _: () = assert!(...)` (`sink.rs:1402`), whose own comment says
  the number is quoted in protolens's override headroom guard, in
  `docs/protolens/design/arena-and-batch.md` and in spec 0212's measured
  outcome; `TreeNode` is pinned at 44 (`decode.rs:660`) and paid 4.74 M
  times on a large descriptor set. The fields sum to exactly 32 with no
  padding, so a new field is +4 on both — some 19 MB on
  `googleapis.desc` — and moves two assertions and three documents. Not
  for two bits.

  **They join `wire_type`'s byte.** A wire type is the low three bits of
  a tag and never exceeds 5, so five bits of that `u8` are idle.

  **The field is renamed, not hidden.** It becomes `wire_and_label:
  u8`, still public, read through `wire_type()` and `label()` and
  written through a public `const fn pack(wire_type: u8, label: Label)
  -> u8`. The rename is what makes the change safe: it is the compiler,
  not a reviewer, that names every one of the ~20 sites reading the old
  field, so none can silently read a masked value.

  Making it private would do that too, and was the first plan. It does
  not compile. `NodeSpan { .. }` literals are *built* outside
  `prototext-core` — `extract.rs:399`, `:435`, `:473`, `decode.rs:680`
  and five test sites under `protolens/src/tui/tests/`, which spec
  0114's blast-radius note already inventories. A private field cannot
  be named in a struct literal from another crate, so hiding it would
  turn a two-bit change into a public constructor taking all eight
  fields.

  Both assertions stay as they are and nothing that quotes them moves.

  `level: u16` is the other candidate — `MAX_WIRE_DEPTH` is 1000, so it
  has six idle bits too — and is rejected only because it is used in
  indent arithmetic rather than in comparisons.

  `TextSink` remains the one text writer in the crate and
  `IndexingTextSink` wraps it, which is why the `repeated_singular`
  change serves both `prototext decode` and protolens.

- **A5.** **`protolens/src/annotation.rs`:** `NON_CANONICAL` grows from
  11 to 12, and `clause()` gains `repeated_singular`. Per spec 0285 the
  tier and the clause are written together.

- **A6.** **`reproto/tree-sitter-textproto/highlights.scm` §7:** the
  `#any-of? @annotation.non_canonical` list gains `repeated_singular`.
  A query change needs a Nix rebuild and a fresh shell before it takes
  effect.

- **A7.** **Bounded renders are safe.** The state is a `render_message`
  local and the decision is forward-only, so a frame the row budget cut
  in half loses only marks it never reached. The failure mode is a
  missing mark, never a wrong one.

  The loss is also temporary, which is what keeps G1 honest. The budget
  is checked in `descend` (`mod.rs:256-263`) and nowhere else, so it
  cuts *between* frames, never inside one: a frame the render enters is
  scanned to its last tag, and a frame it declines to enter is rendered
  whole later. A body therefore never carries a partial set of marks —
  it carries all of them or it is not yet drawn.

## Specification — B. protolens: the shadow sweep

### The rule

- **B1.** **Two scalar values shadow one another iff they share the
  same *path tail*.** From some common ancestor *node*, both are
  reached by the same sequence of field numbers, and every field in
  that sequence is singular. The ancestor need not be the root — any
  node the two have in common will do. Of the values sharing one tail,
  the last in document order wins; every earlier one is
  `shadowed_scalar`.

  Two words in that sentence do the work. **Node**, not field: naming
  a node is what pins down *which occurrence* of everything above it
  the two values are inside, without the rule having to say so.
  **Singular all the way down**: that is what makes the merge reach
  the leaf, and it is the property a descriptor supplies and the bytes
  do not.

  The rule as stated names a pair and a schema in one breath, which is
  why it is not the algorithm. B3 splits it: the pair and its ancestor
  are found once, from the bytes alone, and the word *singular* is
  applied to them afterwards.

- **B2.** **A repeated leg is a barrier. This is a consequence of B1,
  not a second rule.** Two occurrences of a repeated field are two
  distinct nodes, so a tail that crosses one contains a non-singular
  field and B1 refuses it. Nothing inside an instance of a repeated
  field can shadow, or be shadowed by, anything outside it, and the
  nearest enclosing repeated instance is therefore the widest ancestor
  B1 ever needs.

  Note what this is and is not a statement about. It is a condition on
  the *fields between two values and their common ancestor*, and on
  nothing above that ancestor. It therefore cannot be read off either
  value alone, and B5 does not try to: it is a test applied to a pair.

  The annex's map case is the same rule read once more, not an
  exception to it: a map is a repeated field whose entries *do*
  collide — by key rather than by identity. It changes what makes two
  nodes "the same ancestor", and changes nothing else.

### The structure

- **B3.** **A value is shadowed iff two conditions hold, and they are
  computed apart: a *structural* one the bytes decide, and a
  *circumstantial* one the descriptor decides.**

  - **Structural.** Read the whole arena *as if every field were
    singular*. B1's tail rule then degenerates to a single key — the
    field-number path from the root — and the document collapses into
    one **merged tree**, in which each displaced value is linked to
    the one that directly displaces it, under the ancestor the two
    have in common. This is B4.
  - **Circumstantial.** A link is real iff **every field between that
    common ancestor and the two values themselves is singular, and
    neither end is a message**. Fields *above* the ancestor impose no
    condition — the two values are already inside the same instance of
    each of them, which is exactly B1's "the ancestor need not be the
    root". Both halves of the test are schema answers, which is why
    they sit together, and both are B5.

  The bit is set iff both hold. Neither condition is a weakened form
  of the other: the first asks whether the bytes *permit* the
  shadowing, the second whether the schema *realizes* it. And they are
  separated so that the schema enters at exactly one point — B5 —
  which is what makes an override cheap and what makes the three
  readings of the Background three filterings of one structure.

- **B4.** **The merged tree is a trie keyed by field number, and the
  structural pass is one walk that inserts the document into it.**

  Throw the descriptor away and take every field to be singular. Then
  merging is total: two occurrences of a field at the same place are
  the same field, and the document collapses onto the set of its
  distinct **field-number paths**. That collapsed object is the merged
  tree. Its nodes *are* the paths — `1.5.2` is one node however often
  the document walks it, and a repeated field with ten thousand
  instances contributes one node rather than ten thousand. **The
  merged tree is the size of the document's *shape*, not of the
  document.**

  A trie is exactly that shape. Each node holds, per field number, a
  cell with two independent halves:

  - a **child node**, present once a slot with children has been seen
    at that field — the single subtree that everything at that path
    merges into;
  - a **value**, the arena slot of the *last* occurrence seen at that
    field, whatever its kind, because with everything singular the
    last one wins.

  Insertion is the whole computation, and **a collision on insertion
  *is* a link**: writing a value half that is already occupied
  displaces the slot it held, so the two are linked and then the half
  is overwritten. Nothing is searched for and nothing else emits a
  link. **The trie is scaffolding**, dropped when the walk ends; the
  links are the output.

  **Every slot writes the value half; a slot with children *also*
  descends into the child half.** The halves are not alternatives, and
  a cell that had to choose between them would be a false negative on
  exactly the payload this spec exists to surface: a singular `bytes`
  field carrying `\x08\x01` in one occurrence and `hello` in the next
  gets children on the first and none on the second — a phantom parse,
  since the maximal walk descends into every LEN payload (spec 0216
  S2) — and the second value really does shadow the first.

  Not choosing costs nothing, because the arena is maximal. It is a
  superset of every interpretation and no later reading can produce a
  node it does not already hold, so which half a slot fills is a byte
  fact, fixed for the life of the document; filling both records two
  such facts rather than resolving a conflict. **The pass therefore
  links kind-blind, and B5 is where an end that renders as a message
  drops the link** — "this merges rather than shadows" is a schema
  answer, and B3 puts every schema answer in the filter.

  **The pass reads the arena and the bytes, and nothing a render
  wrote.** It needs two facts per slot and takes both without a schema
  and without a rendering:

  - `Arena::raw_start(slot)` is the first byte of the node's **tag**,
    so one varint decode there yields the field number. The arena
    stores no field number — identity there is positional, "**not** a
    sequence of protobuf field numbers" (spec 0216 S1a) — and
    re-deriving it from the bytes costs a few instructions, so nothing
    is added to the arena to hold it.
  - `Arena::first_child` says whether the slot has children, which
    *is* the maximal reading's own answer to "message or leaf", and is
    the reading this pass wants.

  **A malformed slot enters the trie iff a field number comes out of
  it.** Spec 0302's descent pushes arena nodes whose `raw_start` need
  not carry a readable tag, so the decode is attempted rather than
  assumed. If it yields a field number the slot is inserted like any
  other — *including* a field number of `0`, which is undeclarable and
  so will be dropped by B5's no-schema clause, but which is a perfectly
  good trie key in the meantime, and one that groups the malformed
  tails of one frame together where a reader can see them. If it yields
  nothing, the slot is not in the trie at all: it shadows nothing and
  nothing shadows it, which is the right answer for bytes that are not
  a field.

  Neither is `NodeSpan`. A span is what the *render* said —
  `build_tree` fills every slot with `TreeNode::vacant()` and then
  overlays the spans the render emitted, so an unrendered slot's
  `field_number` and `is_message` are placeholder zeros. Reading them
  here would make the structural pass wait for the bake and depend on
  the resolved type, which is precisely what B3 separates. **A4's
  label is read in B5 and nowhere else.**

  **The walk is one DFS carrying a pointer into the trie.** At each
  child slot: decode the tag; write that field's value half, linking
  first if it was occupied; and if the slot has children, descend into
  that field's child half as well, creating it if absent. An unknown
  field is not a special case here — it is a slot like any other, and
  N5 is enforced by the filter.

  **A DFS driven by an explicit stack, because the arena is not in
  document order.** `arena.rs`'s phase 2 sorts the nodes into **level
  order** — the same fact `refresh_status_subtree` already warns about,
  that "a subtree is a union of one range per level" — so
  `for idx in 0..arena.len()` visits siblings before children and is
  *not* this walk. It is also not merely slower: the clock below counts
  node *entries*, and its stamp argument is a statement about a
  depth-first entry sequence. A level-order sweep computes wrong
  ancestors silently, on documents whose links are all correct, and
  test item 10 is the only thing between that mistake and a shipped
  build.

  The stack is one frame per open node — the slot, how far through
  `first_child(i)..first_child(i + 1)` it has got, and its trie node —
  and it is bounded by `MAX_WIRE_DEPTH`, so it is a small `Vec` rather
  than a recursion. **That stack and the clock are stage 2's resume
  state**: a chunk boundary is a return from the middle of the DFS, and
  B6 gets to call the pass resumable only because a stack makes the
  middle of a DFS a thing one can name.

  **Each link carries its common ancestor, computed as the link is
  made.** Both ends share a field-number path, so they sit at the same
  depth, and their *nearest* common arena ancestor is the one to
  record: a higher ancestor only lengthens the sequence B1 requires to
  be singular, so if the nearest one fails, every higher one fails
  too.

  It is read off the trie rather than climbed for. Give the walk a
  clock that ticks on every arena node entered, and give each trie
  node two words: the arena slot currently **occupying** it, and the
  clock at which that occupant was entered. A value half records the
  clock at which it was written. Then for a link whose displaced end
  was written at clock *k*, the common ancestor is the occupant of the
  deepest trie ancestor whose stamp is `<= k` — the deepest node whose
  occupant has *not* changed since. Stamps only increase going down,
  since replacing an occupant re-enters everything beneath it, so the
  answer is a climb that stops at the first stamp `<= k`; the root's
  stamp is 0, so it always stops.

  **The links are held both ways.** A slot has at most one link into
  it and at most one out of it — the occurrences at one path form a
  chain and each link joins consecutive members — so two sparse maps
  answer both of the questions worth asking of a slot: *which value
  does this one displace, and under which ancestor*, and *which value
  displaces it*. Sparse, because only a displaced value has an entry:
  **a document with no duplicates emits no link, and the pass's whole
  output is empty.**

  A link is three arena slots and nothing else — the **shadowed** one,
  the **shadowing** one, and their common **ancestor** — so the forward
  map is `shadowed -> (shadowing, ancestor)` and the backward one is
  `shadowing -> shadowed`, the ancestor being recoverable from the
  first. Which containers hold them is left open: they are keyed by
  slot, are read only by B5 and by whatever later spec takes up *say
  which row shadows it*, and are sized by the duplicate count. What is
  not open is the content, because the ancestor is the field the naive
  implementation forgets to store and then cannot recompute after an
  override.

  Linking kind-blind does widen that output, and it is worth knowing by
  how much. The Background's four-deep duplicate emits four links —
  one per path, `1` through `1.2.3.4` — where a leaf-only rule would
  emit one, and B5 drops three of them as plumbing. The extra links are
  bounded by the number of duplicated fields rather than by the
  document, and they are the price of the schema entering at exactly
  one point.

  **The output is a function of the bytes, exactly as the arena is
  (spec 0216), and is computed once per document.** The maximal walk
  descends into every length-delimited payload and never judges, so
  the arena is a superset of every interpretation, and a retype writes
  its new rendering into *the slots the arena already has* — "no
  append, no coordinate translation, no pointer repair"
  (`override_apply.rs`, spec 0216 S12). Slot ids are therefore stable
  for the life of the document, and so are the links. **An override
  re-runs the filter and nothing else.**

### The filter

- **B5.** **The filter visits links and nothing else.** For each, walk
  the arena from the shadowing value up to the recorded ancestor and
  again from the shadowed one, reading A4's `label` on every node
  crossed, the ancestor excluded. All singular, with both ends
  rendering as leaves, sets the bit on the shadowed slot; **one
  repeated field, one no-schema field, one unrendered slot, or one end
  rendering as a message drops the link**. Depth is small and links are
  rare, so this is the cheap half despite being the half that needs the
  schema.

  Both chains are read, not one. They carry the same field numbers, but
  under an override the two ends can be rendered under different types
  and so can disagree about a label; the chains are a handful of nodes
  each and the honest reading of B1 is that every field in the sequence
  is singular.

  The four drop reasons are one clause each and between them they
  absorb every case this design would otherwise need a rule for. A
  repeated field is B2's barrier. A no-schema field is N5, and it is
  also what disposes of phantom substructure: fields invented inside a
  string are declared by nothing, so a link crossing one dies here. An
  unrendered slot (`lines_total == 0`) is a slot the current
  interpretation does not show, whose span must not be read at all —
  there is no row to mark and no label to read, so the link is simply
  not this interpretation's business.

  **An end that renders as a message is the fourth, and it is what
  B4's kind-blind linking hands down.** `NodeSpan::is_message` is the
  render's own answer under the resolved type, and a singular message
  merges rather than shadows, so a link with a message at either end
  is plumbing and not a loss — precisely what "Two findings, not one"
  refuses to report. Asking here rather than in B4 is what lets one
  link be a loss under one descriptor and plumbing under another,
  which is B3's whole point; it is also what keeps B10's and B11's
  "only a singular scalar or enum is ever marked" true.

  **A dropped link is never re-pointed, and that is why a filter is
  enough.** Occurrences at one path are grouped by which instance of
  each repeated ancestor they sit in, and every such group is a
  contiguous run of the document, because an instance is a contiguous
  range of bytes. Cutting the cross-group links therefore severs the
  chain exactly at the group boundaries, and every adjacency surviving
  inside a group was already the right one. There is no case in which
  deleting a link obliges an earlier value to be re-linked to a later
  survivor.

  **Worked, because it is the whole design in six lines.** For

  ```
  1: { 5: "a"  5: "b" }
  1: { 5: "c" }
  ```

  B4 links `"a" ← "b" ← "c"`, all three being path `1.5`. The first
  link's recorded ancestor is the first `1` frame and it crosses only
  field `5`; the second link's ancestor is the root and it crosses `1`
  and `5`. With every field singular both survive, so both `"a"` and
  `"b"` are marked. With field `1` repeated instead, the second link
  crosses `1` and is dropped while the first survives — `"a"`
  shadowed, `"b"` and `"c"` alive. Same links, different descriptor.

  A packed run — the one arena slot that owns many rows — is a
  repeated scalar, so any link touching it crosses a repeated field
  and dies here.

- **B6.** **Three stages, in this order: the first frame, then the
  trie, then the marks.** Each gate is a *position in the idle ladder*
  of `terminal.rs` and nothing else. No stage polls another and no new
  predicate is introduced.

  **Stage 1 — the first frame owes this spec nothing.** `App::new`,
  `build_tree` and the first `draw` are untouched: no trie, no bitset,
  no per-slot pass, nothing to initialize beyond an empty field. Spec
  0257 spent real effort getting a screenful of a large document up,
  and this spec must not spend any of it back. The trie is allocated on
  the first idle pass rather than at open, and so is the bitset (B7).

  Putting both passes in the idle ladder is what enforces this, rather
  than a flag someone has to remember to check: the loop draws at the
  head of every iteration and only then reaches its idle arm, so no
  trie work can precede the first frame.

  **Stage 2 — the structural pass runs *alongside* the bake, not after
  it.** Everything B4 reads is present the moment the arena is, so the
  pass does not wait for `BakeStep::Idle`: it sits **between
  `discard_step` and `bake_step`**, and starts on the first idle pass
  after the document is on screen. Like `SearchSweep` (spec 0235) it is
  resumable, doing a bounded chunk per turn.

  Below `discard_step`, because spec 0256 S2's reason still holds — the
  drain is what keeps peak memory where it was during a splice, and the
  trie is one more thing alive next to two documents if it goes first.
  And inside the `SweepStep::Idle` arm like everything else, so the
  pass stands aside for a segment scan in flight (spec 0274 S10) with
  no clause of its own.

  **It is the one rung that does not `continue`.** Every other rung
  restarts the ladder from the top when it makes progress, which is how
  strict priority is expressed there — and a rung above the bake doing
  that would starve the bake until the whole trie was built, so the
  reader would watch the document stop growing. This one falls through
  instead: **one idle pass does one trie chunk *and* one bake step.**
  The deviation is the point, and it is written down because the
  obvious edit is to make the rung look like its neighbors.

  **Falling through means skipping the deadline, so the chunk is what
  bounds it.** Every other rung returns to the top of the ladder, and
  the top is where the frame deadline is checked; this one hands its
  remainder straight to `bake_step` without passing it. A trie chunk is
  therefore sized so that a chunk *plus* a bake step still fits the
  budget — the chunk is the smaller of the two and its cost is a fixed
  number of slots, so this is a constant to pick and item 32 is what
  says whether it was picked well. Re-checking the deadline inside the
  rung would work too and is rejected only because it puts a second
  deadline test in a ladder that has exactly one.

  **Stage 3 — the filter runs when both of those are done.** It needs
  the structural pass *complete*, because a link can be emitted at any
  later offset in the document and a filter run over half the links
  would mark half the rows and never revisit them. It needs the bake
  *idle*, because a slot's label is A4's pair of bits on its span,
  written by the render under the resolved type, and an unbaked slot
  has no rendering and therefore no label to read.

  **Position is the gate, and the only gate.** `BakeStep::Idle` is what
  falls through to the rungs below, so a filter placed there cannot run
  while the bake owes a row; and it goes directly after the bake,
  *ahead* of read-ahead, because it is bounded by the link count rather
  than by the document, it owes a frame (B7), and deferring the marks
  behind a finite prefetch queue would keep them off screen for no
  reason. No second predicate — in particular not
  `auto_folded.is_empty()`, which is the splice's bit and what
  `bake_dot_style` reads, and which is therefore free to disagree.

  **Each stage knows it is owed by its own cursor, which is the same
  thing "position is the gate" says about the ladder.** Stage 2 has
  work while its DFS stack is non-empty, and it is complete when the
  stack empties — the stack B4 gives it is the only state either
  question needs. Stage 3 has work while a cursor into the link list is
  short of its end, and it advances that cursor by a bounded number of
  links per turn for the same reason stage 2 is chunked. Neither is a
  flag anyone has to remember to set, which is the property that makes
  the invalidation rule below one line long.

  **An override invalidates the bits, never the links.** The bitset is
  cleared and the filter's cursor is rewound to zero — that is the
  entire invalidation — so the filter re-runs and the structural pass
  does not, because
  by B4 its output is a function of the bytes. No incremental repair,
  no invalidation scope, no partial-state bookkeeping — the thing that
  would have needed them is the thing that no longer has to be redone.

  Waiting is still a deliberate simplification on the filter's side,
  and it is worth being explicit about what it buys. Following the bake
  rather than waiting for it would need a rule for subtrees the bake
  has sealed and a guard for the document-order trap, where a link
  whose far end is not yet rendered gets filtered on a label that is
  not there. Both disappear if every slot a link names is rendered when
  the filter runs.

  The price is latency, and stage 2 is what keeps it small. The two
  waits do not add up — the trie is built *during* the bake, so what
  the reader waits for is the bake alone, plus a filter that visits
  links and no rows. After an override the re-bake is fast and the trie
  is not rebuilt at all, so the marks return almost with the rows. The
  answer arrives shortly after the document stops moving, which is also
  when the reader starts looking.

  Until stage 3 finishes the document is provisional in the way it
  already is: `Status::Unbaked` and the activity dot are still up. No
  new indicator is added, and in particular the absence of a mark is
  never presented as an answer — a document mid-sweep says the same
  thing a clean one does, which is why stage 3 completing is what the
  reader is really waiting for.

### The verdict

- **B7.** **The verdict is a bitset over the arena, one bit per slot.**
  `shadowed`, cleared — not resized — whenever the filter re-runs.
  **Its length is the arena's and never changes**, because the arena is
  immutable: a retype rewrites the overlay under a slot and allocates
  none (spec 0216). There is no growth path to write.

  **It is allocated when the filter first runs, not at open.** The
  length is known at open, but B6's first stage forbids spending
  anything there, and this is not nothing: 4.74 M slots is 74 k words,
  some 593 kB to zero on a path spec 0257 measures in milliseconds. So
  the field is empty until stage 3 reaches it, and until then every
  slot's bit reads `false` — which is also exactly the right answer,
  since nothing has been filtered yet.

  **The reader must therefore tolerate an empty set, and the word-wise
  read is where it would not.** `FoldSet::word` is `self.words[w]`
  (`fold_set.rs:149`), an unguarded index, and B9's fast path reads the
  shadow set the same way. An unallocated shadow set answers `0` to
  `word()` and `false` to a single-bit read rather than panicking, and
  that is a clause on the accessor rather than a guard at each of its
  call sites — every rebuild before stage 3 finishes goes through it.

  Not written into `node_text`: that is an `Arc` shared with the segment
  scan, so `node_text_mut` halts the scan (spec 0274), and a background
  sweep that halted the reader's search on every write would be worse
  than the problem.

  **The bitset is the authority; drawing indexes it by slot; a write
  that lands on a row currently on screen asks for a redraw.** Those
  three clauses are the whole contract between the sweep and the
  display. The renderer never searches the bitset — it knows the slot
  and wants that slot's bit.

  The bitset is `Arc<Vec<AtomicU64>>`, and the atomics are there for one
  reason only: `spawn_segment_scan` clones the arena's `Arc`s into a
  worker thread, so if the search scan reads the bit, a plain `Vec`
  would be a data race in the strict sense. Sweep and renderer are the
  same thread and need no synchronization between them; `Relaxed` is
  sufficient throughout.

  The third clause is an obligation, not an optimization: a bit set
  behind the reader's back with no frame requested is a row that
  silently disagrees with the sweep until something unrelated repaints
  it. The existing frame-notify path carries it; no new one is added.

- **B8.** **Fold state is not an input.** The walk descends the arena,
  not the visible rows, so folding or unfolding changes no bit and never
  restarts the sweep. Stated as a requirement rather than left implicit,
  because `auto_folded` is the bake's queue and it would be easy to
  reach for the fold sets when writing the gate.

### The display

- **B9.** **`own_status` gains a rung: a slot whose bit is set is at
  least `Status::NonCanonical`.** That is all it takes — the roll-up,
  the margin color and spec 0322's `◆` on a leaf follow from
  `status_of` with no new display code. The `◆` on a leaf is always the
  right glyph here, because B5 drops any link with a message end and a
  marked slot is therefore always a leaf — the question of what a
  bracketed row does with a diamond does not arise, and a rule that
  later marks a message would have to answer it.

  Four things about *how* the rung is wired, because each has a way of
  going wrong quietly.

  **The bit arrives as a parameter, not as a lookup inside.**
  `own_status(idx, is_stop, shadowed)`, mirroring `is_stop` exactly —
  whose own doc gives the reason: it is "taken as a parameter rather
  than asked here so that `rebuild_status` can answer it from a bitset.
  There is still one statement of the status rule."

  **Both callers pass it, and they read it differently.**
  `own_status` is called from two places (`node_status.rs:96` and
  `:114`) and the second is the one a splice runs:
  `refresh_status_subtree` recurses a subtree and passes
  `self.auto_folded.contains(idx)`, so the shadow bit joins it as the
  matching single-slot read. Only `rebuild_status` sweeps the whole
  arena and so only it wants the word-at-a-time form below. Naming both
  because the sweep-wide one is the one this spec talks about, and the
  splice path is the one that would be found later, by an assertion.

  **`rebuild_status`'s fast path must test it.** That loop skips a slot
  when `node_text[idx].is_none() && !is_stop`, writing `Ok` outright,
  and the shadow bit joins that condition — read word-at-a-time, as
  `auto_folded.word(idx / 64)` already is, so the fast path keeps
  costing one sequential read. Left out, a shadowed slot with no text is
  written `Ok`; and because the skipped slots are *written* rather than
  passed over, the from-scratch oracle would agree with the mistake
  instead of catching it.

  **The sweep owes a refresh on every bit it sets.** `status_own` and
  `status_rolled` are derived from `own_status`, so the bit is not the
  display's input — the arrays are. B7 makes the sweep a *second* writer
  of state those arrays depend on, and nothing about setting a bit calls
  `refresh_status_ancestors`. It must, on the slot it marked, before
  asking for the frame B7's third clause owes.

  This is not a cosmetic lag if it is missed: `assert_status_is_exact`
  compares the incremental arrays against a from-scratch computation
  under `#[cfg(test)]`, so an unrefreshed bit is an assertion failure —
  and one that points at `node_status.rs` rather than at the sweep that
  caused it.

  A splice needs nothing new: it already rebuilds status over what it
  touched, and by B6 it has cleared the whole bitset anyway.

- **B10.** **The row says the word, as a display insertion.**
  `row_spans` appends it in `theme::status_color(NonCanonical)`,
  through the same `insertions` list that carries the `{ ... }`
  collapse summary and spec 0328's preview `...`. `row_text_of` mirrors
  it, as it mirrors the
  collapse summary, so that `row_content` and `row_spans` stay
  byte-identical and the caret can walk the suffix.

  The highlighter never sees it: `window_text` builds from
  `display_row_text`, not from `row_text_of`, so spec 0318's "every row
  is grammatical prototext" is untouched and the mark carries its own
  style rather than a `highlights.scm` capture. That is also why A6 adds
  only `repeated_singular` to the query.

  **The bytes are `; shadowed_scalar`, appended to the row's existing
  `#@` clause.** Not `  ; …` — `"; "` is the v2 format's *intra*-comment
  separator (`helpers/annotations.rs:91-94`: the first token carries the
  `"  # "` prefix, later ones are separated by `"; "` with no trailing
  `;`). The mark is a later token and takes the later token's form.

  **It therefore requires that clause to exist**, and this is a real
  dependency rather than a stylistic one: the mark is safe only because
  the `#@` comment runs to end of line and swallows it. Appended to a
  row with no clause it would leave a bare `;` outside any comment in
  `row_content` — which is what `max_visible_line_len` and the clipboard
  read. The dependency holds because a marked row is always a rendered,
  schema-backed singular scalar — B5 drops the link otherwise — and so
  always carries `#@ type = N`; the point of writing it down is that a
  future rule which marks anything else must check this first.

  **`row_text_of`'s doc invariant changes with it.** It currently
  promises that passing a wrong `owner` "changes only the fold glyph and
  hence the `{ ... }` collapse summary, never the underlying text". Once
  the mark is resolved from `owner` that is false, and the comment is
  part of this change.

  **The mark travels with a clipboard copy, deliberately.** `row_text`
  is what `selected_text` uses, so a copied row carries a token that is
  not in the file. Allowed rather than suppressed: it lands inside a `#`
  comment, so a paste still parses, and a reader who copies a row to ask
  someone about it is copying it *because* of the mark. N3 is about what
  `prototext decode` writes and what `encode_text` round-trips, neither
  of which a clipboard touches.

  **With annotations off the word is not drawn, and the amber diamond
  stays.** `row_text_of` already applies `code_part` when
  `!self.annotations` — which would strip the `#@` clause the mark
  attaches to, so suppressing it there is not a policy choice but the
  only coherent option. The margin needs no exception: B9's rung comes
  from the bitset, which never consulted the toggle.

- **B11.** **The search finds it, because `DocCursor` manufactures the
  haystack.** Spec 0274's cursor does not read a string; it hands
  `regex-cursor` one chunk at a time, and spec 0222's derived closing
  brace is already a chunk that is stored nowhere. The mark is the
  second member of that set, and it is built the same way.

  **The same way, and not a shortcut.** `bytes()` cannot decide to
  synthesize: it feeds `Cursor::chunk(&self)`, which returns a borrow,
  so there is no `&mut self` there to fill anything with. That is
  precisely why `close` is a `String` field rather than something
  `bytes()` builds, and why `reload_close(&mut self)` is called from
  four places — the constructor, the seek, `advance` and `backtrack`.
  **The mark's scratch is filled at those same four points, on the same
  bit test.** An implementation that tries to do it inside `bytes()`
  will not compile, and the redesign it then needs is this paragraph.

  **The scratch costs a copy per step onto a marked node**, since it
  holds the node's whole text and then the suffix — unlike
  `reload_close`, whose work is skipped by a bool on nearly every step.
  That is affordable for the same reason the feature exists at all: a
  mark is an anomaly, so marked nodes are rare, and a document with none
  never fills the scratch. It is affordable, not free, and the
  measurement that shows it must therefore be taken over a document that
  *has* marks — a document without them exercises nothing.

  **No new `Place` variant.** The mark is a suffix of the node's own
  chunk, not a chunk of its own, so `Place::next` and `Place::prev` are
  untouched — which matters, because `prev` is required to be the exact
  inverse of `next` and that is what lets `backtrack` undo `advance`
  step for step.

  Two things follow:

  - `chunk_len` — "a chunk's length in bytes, without building it", the
    basis of `base_of`'s one-add-per-node arithmetic — adds the mark's
    length on a bit test. This one really does touch no text.
  - `locate_in_chunk` maps a byte offset within a chunk back to a
    `(LinePos, Range<usize>)`. A match landing inside the synthetic
    suffix has no real byte to name, so it clamps to the row's end —
    and the clamp must precede the two slices of `node_text` that
    follow, which would otherwise index past it. This is the one place
    the synthesis is visible, and it has its own test.

  **The annotations toggle reaches the cursor too.** B10 suppresses the
  word when `!self.annotations`, because `code_part` strips the `#@`
  clause it attaches to — and the cursor has to ask the same question
  or the two disagree. The scratch is filled, and `chunk_len` adds the
  suffix's length, only when annotations are on; otherwise the haystack
  would carry a suffix no row displays and a search would land the
  caret on text that is not there. One condition, read where B10 reads
  it.

  A marked slot is always flat and always one chunk, because **only a
  singular scalar or enum is ever marked**. A message merges rather
  than shadows, so B5 drops any link with one at either end; the packed
  run — the one multi-row slot — is a repeated scalar and is never
  marked either. So the suffix has exactly one place to go.

## Alternatives considered

**One keyword for both findings.** Shorter, and false on message fields
in the one place the reader most needs to trust it: a merged singular
message loses nothing, and telling the reader it was shadowed sends them
looking for data that is still there.

**Report `merged` on the plumbing duplicates.** The first design. In
`1{2{3{4:"toto"}}} 1{2{3{4:"titi"}}}` the merge at `1`, `1.2` and
`1.2.3` is entirely lossless; a keyword on each is noise, and on the
outer one it is the misleading case above. `repeated_singular` says the
true thing about the same rows — that the serializer was not a normal
one — and says nothing about survival.

**Do the whole thing in `prototext-core`, with room reserved on every
singular line for a later `; shadowed` overwrite.** Reserving ten
spaces before `newline()` and overwriting them in place is sound —
overwriting shifts nothing, so no offset is invalidated and `line_count`
is untouched — but it needs a `Sink::Handle` associated type, a change
to `scalar_field`'s and `begin_nested`'s return types across five
impls, a per-frame handle map, and a trailing-space strip pass in
`into_inner` so that unclaimed reservation never reaches the output or
widens protolens's pan extent. All of that to compute, in a streaming
parser, a verdict that needs a tree. Rejected in favor of the split: the
part that streams stays in the emitter, the part that needs a tree waits
for the tree.

**Mark the repeat instead of the shadowed value** — for the semantic
finding. Deciding at occurrence *k* needs no lookback, and the count
still matches. Rejected because it points at the live value; the
reader's question is "which value is dead". Note this is exactly what
`repeated_singular` *does* do, and correctly, because it is not
answering that question.

**Maintain the state eagerly.** One persistent slot per singular field
per frame, everywhere, whether or not anything repeats — which is what
a byte-level single-pass detector would have to do, since it cannot see
a frame's children before descending into the first one. Superseded
rather than rejected: B4's structural pass *does* keep one entry per
field number per distinct path, and that is affordable precisely
because the merged tree collapses repetition — the cost is the
document's shape, which a byte-level detector, having no tree, cannot
collapse to.

**One schema-aware walk, keyed by the tail since the last repeated
ancestor.** The design B4 replaced. It marks the same bits, and it is
the obvious thing to write, but it interleaves the two conditions in a
single traversal — and the moment they are interleaved it becomes
tempting to decide from a frame's own children whether any state is
worth keeping, which is wrong for `1{5:"a" 5:"b"} 1{5:"c"}` and wrong
silently. Separating them removes the temptation rather than warning
against it: the structural pass has nothing to decide, because it
keeps everything, and the filter has nothing to keep, because it
decides one link at a time.

It also gives up the property that makes the split worth having. A
merged tree is O(the document's shape); a tail-keyed table threaded
through a schema-aware walk is not smaller, and it must be rebuilt
from nothing when the only thing that changed is which fields are
singular.

**Run the structural pass inside the arena build rather than as an idle
rung.** The maximal walk that fills `ArenaSink` reads the same tags in
the same order, so folding the trie insertion into it would save a
traversal. Not taken, on two counts that have nothing to do with
correctness: that walk is on the open path, where spec 0257 spent real
effort getting the first screenful up, and a pass that is resumable in
the idle ladder costs the reader nothing at all. Keeping it out also
keeps it *droppable* — a
document nobody asks a shadow question about pays for it only in idle
time. Worth revisiting if the scan ever proves too slow to finish in
idle turns, which links being rare makes unlikely.

**Put the structural pass below the bake, like every other rung.** It
would need no deviation from the ladder's convention: one rung, one
`continue`, strict priority, done. Rejected because it inverts the
staging. The bake is the long job on exactly the documents where the
trie is also long, so the trie would start only once the last row was
in place, and the reader would wait for the two serially having watched
the first finish. Sharing the ladder costs the bake a fraction of each
idle pass and starts the answer at the earliest moment it can be
started, which is the trade B6 wants. The alternative worth revisiting
is the reverse of it — a *smaller* trie chunk, if the bake is measured
to have slowed noticeably (item 32).

**Say *which* row shadows it.** B4 already knows: the link names the
shadowing slot and the ancestor, both ways, so the hover box could say
*shadowed by row N* instead of merely `shadowed_scalar`. Deferred, not
rejected — it is display work G3 does not ask for, and nothing in
either pass has to change to add it later. Recorded here because the
usual objection, that a link costs a `u32` per arena slot, is false:
links exist only for displaced values, so the maps are sparse over the
marked slots.

**Give the arena a field number per slot.** It would save B4 a varint
decode per slot. Rejected: spec 0216 S1a is explicit that arena
identity is positional and deliberately not a sequence of field
numbers, `raw_start` already points at the tag so the number is one
decode away, and the arena is 4.74 M slots on a large descriptor set —
a `u32` there is ~19 MB to avoid re-reading a byte that is in cache
because the walk just touched it.

**Let the filter find the common ancestor by climbing.** Both ends of
a link sit at the same depth, so a lockstep climb until the two paths
meet would find it, and B4 would not need the clock or the occupancy
words. Rejected for what it costs *elsewhere*: the ancestor is then
recomputed on every override rather than once per document, and the
link stops being a self-contained fact that the display can read. The
clock is two words per trie node, and the trie is the document's
shape.

**Use a general tree-merge algorithm.** Small-to-large, DSU-on-tree,
hash consing and persistent-trie union all merge two trees and are all
the wrong shape here: this pass merges *n* subtrees into one
accumulator, in document order, and needs the collisions rather than
the result. Trie insertion is the specialization that produces exactly
that, at one hash-free array probe per slot — the field numbers are
small integers, so a merged node is a short sorted vector scanned
linearly, the same trade A2 makes for the same reason.

**Filter each link as the bake seals the slots it names.** The marks
would appear with the rows instead of after them. Rejected on
complexity: it needs a rule for when a link's two ends are sealed at
different times and a guard for the document-order trap, where a link
filtered on a label that is not yet written reads as a barrier and is
dropped for good. Sitting below `BakeStep::Idle` deletes both, and the
wait is now only the filter's — the structural pass never had it.

**Re-filter only the enclosing repeated leg after a splice.** By B2
nothing outside the nearest enclosing repeated instance can change, so
re-filtering that leg alone would be exact and usually small. Rejected
for now in favor of re-running the whole filter, which needs no
invalidation scope — and which is a much smaller thing to re-run than
it was before B4's split, since it visits links rather than the
document. Worth revisiting **only if measurement says so** — the case
that would force it is a root-level override on a large document,
where the re-bake dominates anyway.

**Detect it in protolens by comparing sibling rows.** `node_status.rs`
already reads `#@` clauses, and two siblings with the same declaration
and no `repeated` prefix is the tell. Rejected three times over: it is a
second `#@` parser, siblings are not all materialized under a bounded
render, and it sees only the frame-local case — the merge chain is
invisible to it.

**Publish the offsets from `EntryScore`.** The general fix; it would
serve every term, not just cardinality, and it is why N1 says what it
says. Rejected for now as a much larger change to a hot structure whose
six counters are deliberately six `u64`s.

## Test plan

1. `a_repeated_singular_field_is_marked_on_every_repeat` — three
   occurrences of one singular `string`, two `repeated_singular` marks,
   on the second and third rows.
2. `a_repeated_singular_message_is_marked_too` — A1's "every kind
   alike", and the case where nothing is shadowed.
3. `a_repeated_field_is_never_marked` and
   `an_unknown_field_repeat_is_never_marked` — N5, and the agreement
   with `apply_cardinality_multi`'s `_ => {}` arm.
4. `the_marked_rows_equal_the_cardinality_charge` — a fixture whose only
   charged term is cardinality; the count of `repeated_singular` rows
   equals the `non_canonical` count in the score box. This is G2, and it
   is the only thing that ties the two computations together —
   deliberately a test rather than code, per N1.
5. `the_fixture_round_trips_byte_exact` — unchanged, must stay green.
6. `every_keyword_is_colored_by_its_tier` and
   `the_annotation_vocabulary_matches_the_grammars_captures` —
   unchanged tests; `repeated_singular` must reach `highlights.scm`
   through them rather than by being remembered.
7. `the_nested_merge_shadows_the_leaf` — the Background's example under
   an all-singular descriptor: exactly one `shadowed_scalar` bit, on the
   `"toto"` leaf, and `repeated_singular` on three rows above it. It
   asserts the link count too — **four** links, one per path, of which
   B5 drops three on their message ends. A leaf-only structural pass
   also sets exactly one bit here, so the bit alone does not
   distinguish the two designs and the count is what does.
8. `a_repeated_leg_shadows_nothing` — the same bytes with field `2`
   repeated, and again with field `4` repeated: no bit set either time.
   The Background's three readings of one input, and therefore the
   direct test of B3's split — one structural pass, three filterings.
   The links must be **identical** in all three runs; that is the half
   of the claim a bit-only assertion misses.
9. `a_value_in_occurrences_one_and_three_is_shadowed` — accumulation
   across n > 2 with a gap — and
   `an_in_frame_duplicate_still_shadows_across_the_merge`, which is
   B5's `1{5:"a" 5:"b"} 1{5:"c"}` under an all-singular descriptor:
   **two** bits, on `"a"` and on `"b"`. A walk that answers the
   in-frame duplicate by a path of its own sets only one, so this is
   the test that fails on the natural wrong design.
10. `the_link_names_the_nearest_common_ancestor` — B4's clock, over
    that same fixture: the first link's ancestor is the first `1`
    frame, the second's is the root. Asserted against an ancestor
    computed by a naive lockstep climb from both ends, which is the
    definition the clock is an optimization of. This is the one place
    in B4 where a wrong answer is silent — a *higher* ancestor still
    filters correctly on an all-singular document and only diverges
    when a repeated leg sits between the two.

    It is also what catches the level-order mistake, so the fixture
    must be **wide as well as deep**: the two orders coincide on a
    document whose every frame has one child, and a walk that visits
    slots in arena order would pass a chain-shaped test while getting
    every real document wrong.

    Paired with `a_chunked_structural_pass_equals_an_unchunked_one`,
    which is B4's stack read as B6's resume state: the same arena
    walked in chunks of one slot, of three, and in a single pass must
    give identical links, ancestors and clocks. A pass that keeps its
    position anywhere but the stack — a slot index, say — passes the
    single-pass case and fails here.
11. `a_document_without_duplicates_emits_no_link` — B4's claim that the
    common document pays the structural pass and nothing else: no link,
    so the filter is never entered.
12. `the_structural_pass_reads_no_span` — B4's separation, and the
    reason B6's second stage can start before the bake ends. A
    structural pass run over an arena whose `TreeNode`s are all
    `vacant()` must emit the same links as one run after a full bake.
13. `opening_a_document_allocates_no_trie_and_no_bitset` — B6's first
    stage, asserted where it can actually be seen: the fields are still
    empty when the first `draw` returns. Paired with the startup
    measurement of item 32, which is what catches a regression this
    assertion is too coarse for.
14. `a_trie_chunk_does_not_delay_a_bake_step` — B6's second stage, and
    the single test that fails on the obvious wrong edit. One pass of
    the idle ladder with trie work outstanding must still take a bake
    step; a rung written like its neighbors, with a `continue`, starves
    the bake and this is what says so.
15. `the_filter_does_not_start_before_the_bake_is_idle` — B6's third
    stage, driven through the pty harness on a fixture large enough to
    bake in more than one step. Paired with
    `the_filter_does_not_start_before_the_trie_is_complete`, the other
    half of the same gate: a filter run over half the links would mark
    half the rows and never revisit them, so a partial run is not a
    slow answer but a wrong one.
16. `an_override_re_filters_and_does_not_re_scan` — B6's invalidation
    rule: the links after the splice are the links before it, and the
    bits agree with a filter run from scratch over the spliced
    rendering, in the manner of `assert_status_is_exact`.
17. `a_link_into_an_unrendered_slot_is_dropped` — B5's third drop
    reason, and the one whose absence is a panic rather than a wrong
    mark: a phantom parse inside a `bytes` field, whose slots have
    placeholder spans that must not be read. With it, three more over
    the same awkward arenas:
    `a_link_with_a_message_end_is_dropped`, B5's fourth reason and the
    guarantee B10 and B11 rest on;
    `a_phantom_parse_does_not_hide_a_shadow`, which is the case B4's
    two-halved cell exists for — a singular `bytes` field carrying
    `\x08\x01` and then `hello`, where a cell that had to choose emits
    no link and this asserts one bit on the first occurrence; and
    `a_malformed_slot_without_a_tag_is_not_in_the_trie`, paired with
    `a_malformed_slot_with_a_field_number_is`, B4's two-way rule over
    a spec-0302 short tail.
18. `folding_changes_no_bit` — B8, stated as a test because it is a
    negative requirement and would otherwise be checked by nobody.
19. `assert_status_is_exact` after a sweep that marked something —
    B9's third clause. Already wired under `#[cfg(test)]`, so the test
    is only obliged to build the shape: a fixture with a shadowed scalar
    under an open ancestor, swept to completion. It fails if the sweep
    sets a bit without refreshing.
20. `a_shadowed_slot_with_no_text_is_not_written_ok` — B9's second
    clause. The fast path is the one place the from-scratch oracle
    cannot help, because it writes the skipped slots too, so the
    assertion has to name `status_own` directly. Paired with
    `a_rebuild_before_the_filter_runs_does_not_panic`, which is B7's
    empty-set clause: the same fast path, run while the bitset is
    still unallocated, reads `word()` on it and must get `0`.
21. `row_content_and_row_spans_agree_byte_for_byte` — the existing test,
    over a fixture that has a shadowed row. B10's mirroring is what it
    checks.
22. `the_annotations_toggle_hides_the_word_and_keeps_the_diamond` —
    B10's last paragraph, both halves in one assertion.
23. `every_marked_row_carries_an_annotation_clause` — B10's dependency,
    asserted over every marked row of every shadow fixture rather than
    on one row. It is what stands between the mark and a bare `;` in
    `row_content`, and B5 is free to grow a rule that breaks it.
24. `a_copied_marked_row_still_parses` — B10's clipboard decision, made
    once and checked: `selected_text` over a marked row, fed back
    through the parser.
25. `a_search_finds_the_shadow_mark` — B11, end to end: typing
    `shadowed_scalar` lands the caret on a marked row. Paired with
    `the_annotations_toggle_hides_the_mark_from_the_search`, the same
    document with annotations off: no hit, and — the half that would
    otherwise rot — the caret positions either side of the marked row
    are the ones a document without the mark would give.
26. `the_cursor_is_reversible_over_a_marked_row` — the existing
    `Place::prev`-undoes-`Place::next` property, over a segment
    containing a marked slot. B11 claims the chunk sequence is
    unchanged; this is what checks it.
27. `a_match_inside_the_mark_clamps_to_the_row_end` — B11's one visible
    seam. A pattern matching only inside the synthetic suffix must
    resolve to a real position and must not panic or index past the
    row.
28. `a_shadow_bit_on_a_visible_row_asks_for_a_frame` — B7's third
    clause, which is the one an implementation would silently omit.
29. The two existing size assertions — `size_of::<NodeSpan>() == 32`
    and `size_of::<TreeNode>() == 44` — must still hold, untouched.
    A4's whole claim is that the label is free, and these are what say
    so. They are already in the tree; the point is that this spec must
    not edit them.
30. `a_label_round_trips_through_the_packed_byte` — every `Cardinality`
    and the no-schema case, written through `pack()` and read back
    through `label()`, with `wire_type()` unchanged across all of them.
    A4's packing is the one place a silent wrong answer is possible,
    and the rename is what makes the compiler find the readers; only
    the packing itself needs a test.
31. `bin/profile` before and after. A2 adds a bit test and a push per
    schema-backed singular field to the render loop; the claim that this
    is noise against formatting a line is a measurement, not an
    argument. Separately, and reported apart because their gates differ:
    the structural pass's cost over `googleapis.desc`, which is paid on
    every document and is the one this spec adds to the common case, and
    the filter's, which is paid per override and visits links only.
32. **Time to first frame, and time to a fully baked document**, both
    over `googleapis.desc`, against the numbers spec 0257 and spec 0255
    recorded. The first must not move at all — that is G5, and it is
    the claim item 13 can only approximate. The second may move a
    little, and by how much is the measured price of B6's second stage
    sharing the ladder with the bake.
33. A search over a document that **has** marks, timed. B11's scratch
    copies a node's whole text on every cursor step that lands on a
    marked node, and a document with no marks never fills it — so the
    unmarked case, which must also not have regressed, is the control
    and not the measurement.

## Annex — future work: `oneof` and map keys

Not in scope (N6). Recorded because both are real, both are silent in
the scorer as well as here, and because what each would cost to add is
worth knowing now — the two answers differ, and only one of them is
cheap.

### What they are

**`oneof`.** A `oneof` declares a group of fields of which at most one
may be set. The wire format has no marker for the group — each member is
an ordinary field with its own number — so the parser enforces the rule
by clearing the rest of the group whenever it sets any member. Bytes
carrying member `a` and then member `b` destroy `a`, with each field
number appearing exactly once. Nothing in this spec fires, and
`apply_cardinality_multi` charges nothing.

**Map keys.** A map field is sugar: `map<string, Foo> m = 1` is encoded
as a *repeated message* field of entries `{ key = 1, value = 2 }`. Two
entries with the same key are two occurrences of a repeated field —
entirely legal cardinality — but the parser inserts them into a hash
map, so the later one replaces the earlier and its value is dead.

### How they interact with merging

They compose with it fully, which is why they belong in this spec's
world rather than a separate one:

1. **Merging applies *inside* a `oneof`.** The *same* member appearing
   twice, message-typed, merges as an ordinary singular message field
   would — the oneof case is already set to that member, so the parser
   mutates the existing message.
2. **Changing member is what replaces.** The earlier member's whole
   subtree dies. No merge, no partial survival.
3. **`MergeFrom` carries both rules.** Merging A into B applies the
   oneof rule and the map-overwrite rule at every level, so the
   wire-level "concatenation parses as merge" property brings them
   along.
4. **Hence they nest arbitrarily deep under a merge chain.** Two
   occurrences of a singular message field `1`, each carrying a
   *different* member of a `oneof` three levels down: the second kills
   the first, invisibly, far below the duplicate. Structurally the same
   case as the shadowed leaf.
5. **A map is the mirror image.** Its entries are replaced, not merged,
   even when the value is message-typed — and the two colliding entries
   can perfectly well live in two different occurrences of a merged
   singular message.

### What deferring costs, which is not the same for both

All four cases are the same rule asking what makes two values share a
slot:

| level is | what identifies the slot |
|---|---|
| singular field | the field number |
| repeated field | the instance — a fresh node, so nothing is shared |
| `oneof` member | its declaration index — all members share a slot |
| map entry | the entry's key value |

B1 and B2 implement the first two. The other two are **not** the same
size, and the difference falls on B3's seam.

**A map is a filter change and nothing else.** Its entries are two
occurrences of a repeated field, so B5's barrier drops the link today;
a later spec replaces that drop, for map fields only, with a
comparison of the two entries' decoded keys. B4 inserts exactly what
it already inserts, the clock and the links are unchanged, and the bit
means what it already means.

**A `oneof` is not, and it is worth being blunt about why.** Its
members share one slot, so it changes the **structural** key — and B4
has no schema to read that from. The pass throws the descriptor away
on purpose (B3), so "these two field numbers are one cell" is a fact
it cannot know. Nor is there a cheap rescue: a trie node that
remembered the last leaf of *any* field number would link every
consecutive pair of leaves in the document, making the links
O(values) and destroying the sparsity that is B4's whole economy.

So a `oneof` needs a pass that does have the schema — plausibly a
schema-aware second insertion into a trie of its own, run where the
filter runs. Which shape is right is a later spec's question. What
matters here is that it is a *new pass* rather than a new clause, and
that this is the one place in the annex where the split B3 makes costs
something instead of saving it.

### What a later spec would have to settle

- **Synthetic `oneof`s must be excluded.** proto3 `optional` fields are
  implemented as single-member `oneof`s. Left in, every optional field
  becomes a false positive.
- **Map keys need *value* comparison.** Everything in this design is
  bookkeeping over integers the parser has already produced; a map-key
  set needs the decoded key, hashed or sorted. It is also the one place
  a message-typed value is replaced rather than merged, so it cannot
  reuse the merge recursion at all.
- **The name would have to widen.** A `oneof` victim is a shadowed
  *message*, not a shadowed scalar.
- **G2 would break unless the scorer moves too.** Both cases change the
  key of the structural check while `apply_cardinality_multi` stays
  per-field-number, so marks would stop equalling charges. That, more
  than the implementation cost, is why they are not in this spec: N2
  keeps `prototext-graph` still, and these two cannot be added honestly
  without moving it.

## Measured outcome

Filled in at implementation.
