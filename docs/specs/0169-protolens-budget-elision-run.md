<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0169 — prototext-core/protolens: model budget truncation as an elision run, not a malformed field

Status: suspended (2026-07-25)
App: prototext-core, protolens
Refs: docs/specs/0170-prototext-core-render-budget-truncation-as-ellipsis.md
      (the carved-out subset that is being implemented instead)

## Suspension note (2026-07-25)

**Do not implement this spec as written.** It is retained for its
analysis, not as a work order.

The rendering review that examined spec 0163's `NODE_BUDGET_EXCEEDED`
marker produced two separable claims:

1. The marker is **filed under the wrong taxonomy** and is **needlessly
   lossy in its rendering** — it should be a bare `...` line, not a
   synthetic empty field carrying an editorial byte count in the `#@`
   data channel.
2. The elided region should additionally be a **navigable node**, with a
   `NodeSpan`, an `extract`able range, and an expand gesture.

Only (1) is a current requirement. (2) — Goals G2, G4, G5, G6, G7, spec
items A5, A6 and the whole of Part B — is **suspended**: it is a
plausible future capability, but nothing needs it today, and it carries
by far the larger share of this spec's cost (a new `NodeSpan` field, a
`splice_override` refactor, a new expansion path, and a heat-cue key
collision that would otherwise force spec 0151's key widening first).

Claim (1) has been carved out verbatim into
[0170](0170-prototext-core-render-budget-truncation-as-ellipsis.md),
which is `prototext-core`-only and is the spec to implement. **0170's
design is deliberately a strict subset of this one** — nothing in it
forecloses resuming (2) later. In particular 0170 introduces the same
`Sink::elided(range)` event carrying the same explicit range, so
reviving this spec means adding `IndexingTextSink::elided` and Part B,
not redesigning the event.

The sections below are unchanged from the original draft. Read them for
the modeling argument — in particular *The modeling problem*, which
establishes that an elided region is a **run of siblings** rather than a
node, and that protolens's existing packed-run machinery is the right
precedent. That analysis remains correct and is the reason (2) was
scoped out rather than done cheaply and wrongly.

## Background

Spec 0163 added `DecodeRenderOpts::node_budget`. When it trips,
`render_message` (`prototext-core/src/serialize/render_text/mod.rs:396-412`)
does:

```rust
if let Some(budget) = NODE_BUDGET.with(Cell::get) {
    let count = NODE_COUNT.with(Cell::get) + 1;
    NODE_COUNT.with(|c| c.set(count));
    if count > budget {
        sink.malformed(0, TagFacts::default(),
                       MalformedKind::NodeBudgetExceeded, &buf[pos..]);
        return (buflen, None);
    }
}
```

which renders (`helpers/scalar.rs:176-196`) as a synthetic empty field:

```
  17: ""  #@ NODE_BUDGET_EXCEEDED NOT_RENDERED: 3142204
```

**What is already right, and this spec does not change.** The range
semantics. `&buf[pos..]` is exactly "from the stop to the end of the
enclosing message", and returning `buflen` consumes it. `NODE_COUNT` is
a single counter for the whole render pass, so once tripped it stays
tripped: every enclosing level trips on its own next loop iteration as
the stack unwinds, emitting one marker per open level, O(depth). No
closing brace ever falsely implies "and that was all of it." That
control flow is kept verbatim.

Three things are wrong with the *representation*.

**1. It is filed under a taxonomy it does not belong to.** Every other
`MalformedKind` member (`sink.rs:60-74`) states a property of the data:
this varint is over-encoded, these bytes are missing, this tag's wire
type is invalid. `NodeBudgetExceeded` states a decision by the
*renderer*. The same bytes render differently under a different budget.
That breaks the property the whole override model rests on — a rendering
is a function of `(bytes, schema)` — and it does so in the one place a
reader will read it as a claim about the blob.

Note that "not expressible in textproto grammar" is *not* the
distinguishing feature. `TRUNCATED_BYTES` / `MISSING: n` is already
ALL-CAPS non-textproto and is entirely legitimate: `#@` annotations
exist precisely to carry the bits the grammar cannot express, so that a
round-trip reproduces the original wire bytes. `#@` is a data channel.
An elision is not data, so it does not belong there either.

**2. It produces no `NodeSpan`, so protolens has no node for it.**
`IndexingTextSink::malformed` (`sink.rs:1367-1369`) is pure delegation:

```rust
fn malformed(&mut self, field_number: u64, tag: TagFacts, kind: MalformedKind, raw: &[u8]) {
    self.inner.malformed(field_number, tag, kind, raw);
}
```

Nothing is pushed onto `self.spans`. In protolens the marker is
therefore a rendered line with no arena entry: no cursor target, no
fold, no `extract.rs` slice, no heat cue, and — decisively — **no
override target**. The single gesture an elision most obviously needs,
"show me it after all", is the one this form cannot express.

(The same delegation makes `INVALID_VARINT`, `TRUNCATED_BYTES` and the
rest equally unnavigable. That is a separate, broader question — see
[../protolens/rendering-flaws.md](../protolens/rendering-flaws.md) — and
is a non-goal here.)

**3. It is lossy, and needlessly so.** `render_node_budget_exceeded`
reports `NOT_RENDERED: n`, a byte *count*. The range itself is
discarded. Its doc comment is right that the remainder must not be
escaped and embedded — that remainder is exactly the pathological slice
the budget exists to avoid materializing — but keeping a `Range<usize>`
costs sixteen bytes and is what makes lossless reconstruction possible.
Round-trip through the byte range, not through re-parsing the text.

### The modeling problem

The elided region is **not a node**. A `NodeSpan` is a field;
`[stop..parent_end)` is N complete tag+value records — a *run of
siblings*. "Opening" it cannot mean re-rendering one node.

protolens already has this exact relation, and it is the **packed run**.
`splice_override`'s sibling merge (`protolens/src/tui/override_apply.rs:1352-1368`):

```rust
if is_packed {
    let siblings = self.packed_record_siblings(idx);
    let (raw_range, text_range) = self.packed_record_extent(&siblings);
    idx = siblings[0];
    old_span.raw_range = raw_range;
    old_span.text_range = text_range;
    ...
}
```

One arena index standing for a contiguous run of siblings, collapsed for
the splice and re-expanded afterwards. The decode direction already
emits N spans from a single sink event (`sink.rs:1214-1227` pushes one
`NodeSpan` per packed element, same `level`, one line each). The
machinery, helpers and tests exist.

And the decode side needs nothing new either: N concatenated tag+value
records **is** a message payload, which is precisely the byte shape
`decode::wrap_blob` + `decode::register_wrapper` were built for. The
elision fits the synthetic wrapper better than packed does — a packed
run's bytes are a single wire record (one tag, one length, elements
inside), whereas an elision run is already a message body.

## Goals

**G1.** Move budget truncation out of `MalformedKind` into its own
`Sink` event carrying the elided range explicitly.

**G2.** `IndexingTextSink` emits a real `NodeSpan` for it, with an
absolute `raw_range` covering every elided byte.

**G3.** Render it as `...` — O(1), nothing escaped, nothing
materialized, preserving `render_node_budget_exceeded`'s stated reason
for existing.

**G4.** protolens models it as a *run*: one arena index, expanding to N
siblings, reusing the packed-run shape rather than a parallel one.

**G5.** Expanding an elision re-decodes only the run's own byte range,
under the parent's type, and never the parent itself.

**G6.** Expansion is recursive by construction: the expansion may itself
elide, and that is the intended progressive-disclosure behavior.

**G7.** Lossless byte round-trip: `extract.rs` on an elision node yields
exactly the elided bytes.

## Non-goals

**N1.** Fixing the missing `NodeSpan` for the other `MalformedKind`
members. Related, broader, and independently decidable.

**N2.** Changing *when* the budget trips, its default, or
`DecodeRenderOpts::node_budget`'s type. Spec 0163's policy stands.

**N3.** Byte-capping the preview by passing a truncated sub-range
instead of a budget. Considered and rejected: a sub-range cut at an
arbitrary offset produces genuine mid-record corruption, which the
renderer would then correctly report as `INVALID_*` — a false claim
about the data, and exactly the judgment a preview must support.

**N4.** Any change to `expand_any` / `expand_message_set`.

**N5.** Reclaiming the orphaned arena entries an expansion leaves behind
(spec 0162's territory).

## Specification

### Part A — `prototext-core`

**A1. New `Sink` event.** Add to the `Sink` trait
(`serialize/render_text/sink.rs:95-208`):

```rust
/// The caller-set `node_budget` was reached (spec 0163) and the
/// remainder of the current message payload was not decoded. `range`
/// is local to the *active coordinate frame* — the same frame
/// `begin_nested`'s `raw_start` is expressed in — and runs from the
/// stop position to the end of the enclosing payload.
///
/// Distinct from `malformed` on purpose: every `MalformedKind` states
/// a property of the data, whereas this states a decision by the
/// renderer. The same bytes render differently under a different
/// budget (spec 0169).
fn elided(&mut self, range: Range<usize>);
```

Remove `MalformedKind::NodeBudgetExceeded` (`sink.rs:70-73`) and its
`TextSink` dispatch arm (`sink.rs:833-838`).

**A2. Call site.** `render_message` (`mod.rs:400-412`) becomes:

```rust
if count > budget {
    sink.elided(pos..buflen);
    return (buflen, None);
}
```

Note this drops the `&buf[pos..]` slice in favor of the range — no
caller needs the bytes, and the range is what was missing.

**A3. `TextSink::elided`.** Replaces `render_node_budget_exceeded`
(`helpers/scalar.rs:170-196`, which is deleted). Emits `push_indent`,
then `...`, then `newline()`. No field number, no quotes, no `#@`
annotation — a bare marker line. Byte count and range are *not*
rendered: protolens has the `raw_range` on the node and can display the
count in its own statusline, which keeps the editorial statement out of
the byte stream entirely.

**A4. `ProbeSink::elided`.** `unreachable!()`, matching its sibling
methods (`sink.rs:913-938`) — `ProbeSink` sets
`treat_len_as_opaque() == true` so it never recurses into a LEN payload,
and never runs under a budget.

**A5. `IndexingTextSink::elided`.** Delegates to `self.inner`, then
pushes a `NodeSpan`:

```rust
NodeSpan {
    field_number: 0,          // existing convention for a node with no
                              // field number of its own (Any's `value {}`)
    raw_range: (self.raw_base + range.start)..(self.raw_base + range.end),
    text_range: text_start..text_start + 1,
    level: LEVEL.with(|c| c.get()),
    type_fqdn: None,
    is_message: false,
    packed_record_start: None,
    wire_type: WT_LEN,
    natural_annotation: None,
    is_elision: true,         // new field, see A6
}
```

**A6. `NodeSpan::is_elision: bool`.** A new field, `false` everywhere
else. `packed_record_start` is deliberately *not* reused: it means "this
span is one element of the packed record starting at offset N", which is
a different claim.

Document on the field that an elision span is the **only** `NodeSpan`
whose `raw_range` covers bytes no decode pass validated. This matters:
`build_tree`'s `doc_next` threading is unaffected (the range is still
well-ordered by `start`), but anything reasoning "this range parsed
cleanly" must exclude it.

**A7. Rendering contract.** State in `DecodeRenderOpts::node_budget`'s
doc comment (`mod.rs:215-224`) that a tripped budget produces one `...`
line per open level, and that the union of the emitted elision ranges
plus every rendered node's range covers the input exactly.

### Part B — `protolens`

**B1. `TreeNode`/arena.** `is_elision` rides through on the span; no new
`TreeNode` field. The elision node is a leaf with `is_message: false`.

**B2. Display.** An elision node's label in the statusline and in
`manage_pane` reads its own `raw_range` for the byte count — e.g.
`… 3,142,204 bytes elided`. The main pane shows the bare `...` line as
`prototext_core` rendered it, styled via a new `colorize.rs` role
(elisions are not comments, not values, and must not read as either).

**B3. Expansion.** A new `App::expand_elision(idx)`:

1. `let range = self.tree[idx].span.raw_range.clone();`
2. `let parent_type = self.effective_type(self.tree[idx].parent)` — the
   descriptor the elided records belong to. `None` (raw) is valid and
   common.
3. `wrap_blob(&blob[range])` + `register_wrapper(parent_type)`, then
   `decode_and_render_indexed` with a **fresh** budget
   (`OVERRIDE_SPLICE_NODE_BUDGET_DEFAULT` for a preview, unbounded for a
   commit — same rule `splice_override`'s `is_preview` already applies).
4. Splice the wrapper's *interior* spans in place of `idx`, as N
   siblings at `idx`'s level — the same "one index in, N siblings out"
   the packed path already performs.

**B4. Splice refactor.** `splice_override` (`override_apply.rs:1335`)
currently couples two things: choosing a target type, and replacing an
index's rendering with freshly decoded spans. B3 needs only the second —
its type is not a choice, it is the parent's. Factor the back half into
a private `splice_decoded(idx, decoded, patch_scope)`; `splice_override`
becomes "resolve target → decode → `splice_decoded`", and
`expand_elision` becomes "read parent type → decode → `splice_decoded`".

Do **not** synthesize an override entry whose target happens to equal
the parent's type. An expansion is not an override: it records no schema
knowledge, must not appear in the manage pane, and must not persist to
YAML.

**B5. Gesture.** The unfold key on an elision node calls
`expand_elision`. Semantically exact — an elision *is* a collapsed run —
and needs no new binding. Folding an expanded elision back is a plain
fold of the resulting sibling run; the elision node itself is not
restored.

**B6. Recursion.** An expansion that trips its own budget yields fresh
elision nodes inside it (G6). The invariant to assert and document is
therefore recursive: *an elision's range is fully accounted for by its
replacement, which may itself contain elisions.* Any "sum of children's
ranges equals the parent's range" reasoning must be written to tolerate
this.

**B7. Heat-cue key.** An elision node's `raw_range.start` is exactly the
offset where the next real field would begin, so expanding it produces a
real node with the **same** `start`. The heat cache is keyed on a bare
`usize` (`HeatCaches::by_range`) — this is a genuine collision, not a
theoretical one. Either land
[rendering-flaws.md](../protolens/rendering-flaws.md) A6 (key on the
whole `Range<usize>`) first, or make this spec's expansion invalidate
the key. A6 is the right fix; this is independent evidence for it.

## Open questions

**Q1. `CBL_START`.** `render_node_budget_exceeded` sets `CBL_START` past
the end "to inhibit folding" (`scalar.rs:167` does the same for
`TRUNCATED_BYTES`). An elision *wants* fold-like affordance (B5). Verify
what `CBL_START` actually governs before copying either behavior —
`TextSink::elided` may need neither treatment.

**Q2. Does `...` collide with anything in the tree-sitter-textproto
grammar?** `colorize()` parses protolens's own rendered output. A bare
`...` line at message level is not valid textproto; confirm it degrades
to an error node that `hints_by_line` clips harmlessly, rather than
poisoning the highlight of the following lines.

**Q3. Level of the elision node.** A5 uses `LEVEL` at the trip point,
which is the enclosing message's *content* level — the same level the
elided fields would have had. Confirm against a nested fixture that this
indents `...` to align with its siblings and not with the closing brace.

## Test plan

**prototext-core**

- `node_budget_truncates_deep_nesting_with_a_visible_marker`
  (`mod.rs:730`) and the two `usize::MAX` tests (`mod.rs:760`, `:784`,
  `:805`) updated for the `...` rendering.
- New: with a budget that trips at a known depth, assert the emitted
  elision `raw_range`s plus every `NodeSpan::raw_range` cover `[0, len)`
  exactly, with no overlap (A7's contract).
- New: assert one `...` line per open level as the stack unwinds, on a
  fixture nested at least three deep.
- New: `decode_and_render_indexed` with a tripped budget yields exactly
  one `is_elision` span per `...` line, and none otherwise.

**protolens**

- `expand_elision` on a synthetic blob with a forced-low budget produces
  the same spans as decoding the same range with no budget — the
  correctness anchor for B3/B4.
- `extract.rs` on an elision node yields byte-identical output to
  `blob[range]` (G7).
- Expanding an elision does **not** add an entry to the override
  collection, and the YAML round-trip is unchanged (B4).
- Nested case: a budget low enough that the expansion re-trips produces
  a fresh elision inside, and the recursive coverage invariant (B6)
  still holds.
- Regression: with `node_budget: None` (the default for non-preview
  renders), no `is_elision` span is ever produced and every existing
  test is unaffected.
