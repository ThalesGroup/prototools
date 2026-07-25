<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0170 — prototext-core: render budget truncation as `...`, not as a malformed field

Status: superseded by
      docs/specs/0174-preview-interior-truncation-and-node-budget-removal.md
App: prototext-core
Refs: docs/specs/0163-protolens-override-splice-node-budget.md
      (introduces `DecodeRenderOpts::node_budget`, whose marker this
      spec replaces), docs/specs/0169-protolens-budget-elision-run.md
      (suspended — this spec is its carved-out subset),
      docs/protolens/rendering-worklist.md (W22)

> **Superseded.** This spec reworks `MalformedKind::NodeBudgetExceeded`
> into an `...` elision so that a budgeted render round-trips. Spec 0174
> concluded instead that the budget does not belong in `prototext-core`
> at all: it deletes `node_budget` and the marker outright, and bounds
> the sole caller (protolens's override preview) by handing the renderer
> fewer bytes. With the marker gone there is nothing left for this spec
> to rework. The `...` idea survives, but as a protolens display
> artifact appended to the rendered lines (0174 §S4) rather than as a
> `prototext-core` production.

## Background

Spec 0163 added `DecodeRenderOpts::node_budget`. When it trips,
`render_message`
(`prototext-core/src/serialize/render_text/mod.rs:396-412`) does:

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

**What is already right, and this spec does not change.** The control
flow and the range semantics. `&buf[pos..]` is exactly "from the stop to
the end of the enclosing message", and returning `buflen` consumes it.
`NODE_COUNT` is a single counter for the whole render pass, so once
tripped it stays tripped: every enclosing level trips on its own next
loop iteration as the stack unwinds, emitting one marker per open level,
O(depth). No closing brace ever falsely implies "and that was all of it."
That is kept verbatim.

Two things are wrong with the *representation*.

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
ALL-CAPS non-textproto and is entirely legitimate: `#@` annotations exist
precisely to carry the bits the grammar cannot express, so that a
round-trip reproduces the original wire bytes. **`#@` is a data channel.**
An elision is not data, so it does not belong there either.

**2. The marker is lossy where it need not be.** `render_node_budget_exceeded`
reports `NOT_RENDERED: n`, a byte *count*, and discards the range. The
doc comment is right that the remainder must not be escaped and embedded
— that remainder is exactly the pathological slice the budget exists to
avoid materializing — but the *range* costs sixteen bytes and is the only
thing a consumer could act on.

### Why this spec is small

Spec 0169 proposed the full treatment: give the elided region a real
`NodeSpan` so protolens can navigate it, `extract` it, and expand it back
into the N sibling records it stands for. That is a coherent design and
its modeling argument is sound — but nothing needs it today, and it
carries a new `NodeSpan` field, a `splice_override` refactor, a new
expansion path, and a heat-cue key collision. 0169 is therefore
**suspended**, and this spec is its `prototext-core`-only subset.

**The requirement is only that an elided region renders as `...`.**

This spec is deliberately a strict subset: it introduces the same
`Sink::elided(range)` event carrying the same explicit range that 0169
specified, so reviving 0169 later means *adding*
`IndexingTextSink::elided` and its Part B, not redesigning this.

## Goals

- **G1**: Move budget truncation out of `MalformedKind` into its own
  `Sink` event carrying the elided range explicitly.
- **G2**: Render it as `...` — O(1), nothing escaped, nothing
  materialized, preserving `render_node_budget_exceeded`'s stated reason
  for existing.
- **G3**: State the coverage contract: a tripped budget produces one
  `...` line per open level, and the union of the emitted elision ranges
  plus every rendered node's range covers the input exactly, without
  overlap.

## Non-goals

- **N1**: Emitting a `NodeSpan` for the elided region.
  `IndexingTextSink::elided` delegates and pushes nothing, exactly as
  `IndexingTextSink::malformed` already does (`sink.rs:1367-1369`). An
  elided region is therefore unnavigable in protolens: no cursor target,
  no fold, no `extract.rs` slice, no heat cue, no override target. This
  is the same status `INVALID_VARINT`, `TRUNCATED_BYTES` and the rest
  already have — a consistent position, not a new gap. Suspended spec
  0169 is where the other position is argued.
- **N2**: Any `protolens` change beyond what compiles. No `expand_elision`,
  no `splice_decoded` refactor, no new `colorize.rs` role, no statusline
  byte count, no unfold gesture. Spec 0169 Part B in its entirety.
- **N3**: A `NodeSpan::is_elision` field.
- **N4**: Changing *when* the budget trips, its default, or
  `DecodeRenderOpts::node_budget`'s type. Spec 0163's policy stands.
  (Note that `protolens` currently passes `node_budget: None` in
  production — turning it on is worklist item W22's neighbor W26, not
  this spec.)
- **N5**: Byte-capping the preview by passing a truncated sub-range
  instead of a budget. Considered and rejected in 0169: a sub-range cut
  at an arbitrary offset produces genuine mid-record corruption, which
  the renderer would then correctly report as `INVALID_*` — a false claim
  about the data.
- **N6**: Any change to `expand_any` / `expand_message_set`.

## Specification

### S1. New `Sink` event

Add to the `Sink` trait (`serialize/render_text/sink.rs:95-208`):

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
/// budget (spec 0170).
///
/// `range` is carried even though no sink in this crate reads it: it
/// is the only lossless description of what was skipped, and a sink
/// that wants to make the region navigable needs exactly this and
/// nothing else (see suspended spec 0169).
fn elided(&mut self, range: Range<usize>);
```

Remove `MalformedKind::NodeBudgetExceeded` (`sink.rs:70-73`) and its
`TextSink` dispatch arm (`sink.rs:833-838`).

### S2. Call site

`render_message` (`mod.rs:400-412`) becomes:

```rust
if count > budget {
    sink.elided(pos..buflen);
    return (buflen, None);
}
```

Note this drops the `&buf[pos..]` slice in favor of the range — no caller
needs the bytes, and the range is what was missing.

### S3. `TextSink::elided`

Replaces `render_node_budget_exceeded` (`helpers/scalar.rs:170-196`,
which is deleted). Emits `push_indent`, then `...`, then `newline()`.

No field number, no quotes, no `#@` annotation — a bare marker line.
Byte count and range are deliberately **not** rendered: the count is an
editorial statement about the render, and the whole point of item 1 in
the Background is to keep such statements out of the byte stream.

### S4. `ProbeSink::elided`

`unreachable!()`, matching its sibling methods (`sink.rs:913-938`):
`ProbeSink` sets `treat_len_as_opaque() == true` so it never recurses
into a LEN payload.

**Verify this before relying on it.** The rendering review found that
`ProbeSink` shares the `NODE_COUNT` thread-local with the outer render
(see `docs/prototext/decode-flaws.md` C5, worklist item W28), which means
a probe *can* be running under a nonzero count. If W28 has not landed,
confirm by test that `elided` is genuinely unreachable from a probe
rather than assuming it; if it is reachable, the correct body is a no-op
that sets no malformity, not `unreachable!()` — a budget trip inside a
probe must not be mistaken for "this payload is not a message."

### S5. `IndexingTextSink::elided`

Pure delegation to `self.inner`. Pushes no `NodeSpan` (N1).

Document on the method *why* it pushes nothing and where the other
position is argued, so the omission reads as a decision rather than an
oversight:

```rust
/// Delegates only: no `NodeSpan` is pushed, so an elided region is not
/// navigable in protolens — the same status every `MalformedKind` line
/// already has. Making it navigable requires modeling it as a *run of
/// siblings* rather than a node; see suspended spec 0169, whose
/// `Sink::elided` range this method already receives unchanged.
```

### S6. Rendering contract

State in `DecodeRenderOpts::node_budget`'s doc comment
(`mod.rs:215-224`) that a tripped budget produces one `...` line per open
level, and that the union of the emitted elision ranges plus every
rendered node's range covers the input exactly.

## Open questions

**Q1. Does `...` collide with anything in the tree-sitter-textproto
grammar?** protolens's `colorize()` parses protolens's own rendered
output. A bare `...` line at message level is not valid textproto;
confirm it degrades to an error node that `hints_by_line` clips
harmlessly, rather than poisoning the highlight of the following lines.
This is the one question that can force a change to S3's output — if `...`
poisons the parse, the fallback is `# ...` (a textproto comment), which
is uglier but grammatical.

**Q2. `CBL_START`.** `render_node_budget_exceeded` sets `CBL_START` past
the end "to inhibit folding" (`scalar.rs:167` does the same for
`TRUNCATED_BYTES`). With N1 in force there is no fold affordance to
preserve, so copying the inhibit-folding behavior is the safe default —
but verify what `CBL_START` actually governs before copying it blindly.

**Q3. Indentation level.** S3 emits `push_indent` at the level in force
at the trip point, which is the enclosing message's *content* level — the
same level the elided fields would have had. Confirm against a nested
fixture that this aligns `...` with its siblings and not with the closing
brace.

## Test plan

All in `prototext-core`; this spec adds no `protolens` behavior to test.

- Update the existing budget tests for the `...` rendering:
  `node_budget_truncates_deep_nesting_with_a_visible_marker`
  (`mod.rs:730`) and the three `usize::MAX` tests (`mod.rs:760`, `:784`,
  `:805`).
- **New — coverage contract (G3).** With a budget that trips at a known
  depth, assert the emitted elision ranges plus every `NodeSpan::raw_range`
  cover `[0, len)` exactly, with no overlap. This is the assertion that
  makes the `range` argument load-bearing rather than decorative, and it
  is what a future revival of 0169 will build on.
- **New — one marker per open level.** On a fixture nested at least three
  deep, assert one `...` line is emitted per open level as the stack
  unwinds, at the correct indent for each (Q3).
- **New — no spans (N1).** `decode_and_render_indexed` with a tripped
  budget produces zero additional `NodeSpan`s for the elided regions, and
  the span count equals the count from an unbudgeted decode truncated at
  the same point.
- **New — probe isolation (S4).** Assert `ProbeSink::elided` is
  unreachable, or — if W28 has not landed — that a budget trip during a
  cascade probe does not cause the probe to report the payload as
  malformed.
- **Regression.** With `node_budget: None`, no `elided` call is ever
  made and every existing test is unaffected.
- **Highlighting (Q1).** In `protolens`, assert that a rendered document
  containing a `...` line highlights the *following* lines identically to
  the same document without it.
