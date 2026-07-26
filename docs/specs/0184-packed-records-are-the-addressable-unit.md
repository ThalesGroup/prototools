<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0184 — the packed record, not its elements, is the addressable unit

Status: implemented
Implemented in: 2026-07-26
App: protolens
Refs: docs/specs/0115-protolens-packed-element-nodespans.md,
      docs/specs/0135-protolens-override-raw-tag-rewrap.md (G1),
      docs/specs/0117-protolens-override-collection.md (§4),
      docs/specs/0123-protolens-batch-mode.md,
      docs/specs/0125-protolens-manage-pane-auto-manual-lifecycle.md (§G3),
      docs/specs/0183-prune-the-override-walk.md

## Background

A packed-repeated scalar field is **one** wire record — one tag, one
length prefix, one payload — holding N values. `prototext-core` renders
it as N lines, one per element, which is what `protoc` and `prototext
decode` do and is not up for discussion. To index those lines,
`IndexingTextSink` pushes **N separate `NodeSpan`s**
(`prototext-core/src/serialize/render_text/sink.rs:1142-1153`), each
carrying `packed_record_start: Some(tag_offset)` and a `raw_range`
carved out of the record's *payload*.

That expansion is conditional. It happens only when `field_schema` is
known and `decode_packed_elems` succeeds (`sink.rs:1128-1139`). The same
bytes resolving as `bytes`, or as an unresolved LEN field, produce
**one** span.

So the number of sibling nodes a single wire record contributes is a
function of the resolved type. And sibling count is what positional
paths are built from.

### Consequence 1: paths move when override state changes

`build_tree` (`protolens/src/decode.rs:352-357`) makes same-`level`
neighbors siblings, so every element consumes a sibling ordinal.
Meanwhile `splice_override` (`override_apply.rs:1564-1584`) *collapses*
a run when an override is applied to it:

```rust
if is_packed {
    let siblings = self.packed_record_siblings(idx);
    let (raw_range, text_range) = self.packed_record_extent(&siblings);
    idx = siblings[0];
    ...
    for &s in &siblings[1..] {
        packed_orphans.push(s);
        self.collect_descendants(s, &mut packed_orphans);
    }
}
```

N sibling nodes become 1. Every later sibling's ordinal drops by N−1.
Deactivate the override and natural rendering restores the run, and the
ordinals shift back.

Overrides are keyed by `OverrideOrigin::Path`, and
`render_overrides_inner` recomputes every node's path from live ordinals
on each pass. Nothing re-anchors an origin. So:

1. `P` holds a packed run of 5 elements at ordinals 3-7, then a message
   at ordinal 8.
2. The user overrides `/P/8`.
3. The user overrides the packed run — cursor anywhere in it, `t`.
4. The run collapses. The message is now ordinal 4. The stored origin
   `/P/8` designates something else, or nothing.

**Confirmed by the failing test written first** (2026-07-26). On a
fixture holding a packed run of 3 followed by a message, the message's
path was `/4`; after a raw override on the run it resolved to `None`
(the message had become `/2`).

### Consequence 2: the heat cue and the action disagree

`heat_cue_for` (`heat_cue.rs:216-228`) maps a line to a node, gates on
`can_override(idx)`, and resolves a cue. `can_override`
(`override_select.rs:33-35`) returns `true` **unconditionally** for a
packed element, short-circuiting the wire-type check — because
eligibility is judged against the record's reconstructed `WT_LEN`, not
the element's own varint/fixed wire type.

`heat_cue_resolve` then scores
`message_payload_range(&self.blob, &node.raw_range,
node.packed_record_start)` (`:238-241`), which for an element returns
**that element's own few bytes** (`extract.rs:96`).

So today every element line carries its own heat cue, computed over one
element's bytes, while `t` on that same line overrides the *whole
record*. The cue describes a different object than the action acts on.

This matters here because the path change **forces** the fix rather than
merely inviting it: `current_type_key` (`heat_cue.rs:181-190`) resolves
the active override by path. Once elements share the record's path, an
element's `current_type_key` becomes *the record's* override type while
its scored `range` stays *the element's* bytes — an actively mismatched
comparison. Leaving the cue alone is not an option.

### What is already true

The redirect means the record is *already* the unit of action; it is
simply reached indirectly. `splice_override`'s own doc comment
(`:1527-1532`) says every sibling is collapsed into `siblings[0]`
"regardless of which specific element the caller invoked the override
on." This spec does not introduce that semantics. It makes the naming
match it.

## Goals

- **G1** — a packed record occupies exactly one positional-path ordinal,
  whatever its element count and whatever its override state. Paths stop
  depending on how a field happens to be typed.
- **G2** — every element of a run resolves to the same path, and that
  path resolves back to the run's leader — the node the override system
  already redirects to.
- **G3** — heat cues on a run describe the record: one cue, over the
  record's payload, shown identically on the run's lines, sharing one
  cache entry instead of N.
- **G4** — the ordinal rule exists once, not three times.
- **G5** — the rendered text does not change by one byte. Elements keep
  their own lines, their own `display_range`, and their own spans.

## Non-goals

- **N1** — removing element node-hood. Collapsing the N spans into one
  would also fix G1, save 16 B on `NodeSpan` (`packed_record_start`) and
  cut node count on packed-heavy blobs — but it changes cursor
  granularity, is a `prototext-core` API change, and is not needed for
  correctness once this spec lands. It stands on its own merits, as its
  own spec, later.
- **N2** — changing which lines are rendered, or their content. G5 is
  absolute: `protoc`/`prototext decode` parity on packed rendering is
  the reason elements have lines at all.
- **N3** — preventing the cursor from resting on an element. It still
  can, and `t` still works there — it just names, scores, and acts on
  the record.
- **N4** — 0183's recursion-gate pruning. Independent, though this spec
  removes a caveat 0183 would otherwise have to carry.
- **N5** — the `siblings[0]` collapse in `splice_override` itself. It
  stays exactly as it is; only path computation and cue resolution
  change around it.

## Specification

### S1. The record-ordinal rule, and its one trap

Sibling ordinals count **records**, not nodes. Two adjacent siblings
belong to the same record iff:

```rust
fn same_packed_record(a: &NodeSpan, b: &NodeSpan) -> bool {
    match (a.packed_record_start, b.packed_record_start) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}
```

**The trap, and it is the single most likely way to get this wrong:**
the test must **not** be `a.packed_record_start == b.packed_record_start`.
Two adjacent ordinary scalars both hold `None`, and that comparison
would merge them into one ordinal — silently renumbering every path in
every document that has two consecutive scalar fields, which is nearly
all of them. `None` is never "the same record"; it means "no record".

This helper is the single definition required by G4. All three sites
below call it and none reimplements it.

### S2. `sibling_position` (`navigation.rs:441-449`)

Walk `prev_sibling` as today, incrementing only when crossing a record
boundary per S1. Every element of a run therefore yields the same
position, and the position of the sibling *after* a run is one more than
the run's — independent of N. Same O(k) cost.

### S3. `nth_child` (`command_line.rs:700-706`) and `resolve_path`

`nth_child(idx, pos)` returns the **first node of the `pos`-th record**
among `idx`'s children. `resolve_path` (`:714-725`) needs no change
beyond that, and neither does `origin_resolves` (`:733-`).

Note the reach: `resolve_path` is `pub(crate)` and is also used by
`main.rs`'s batch `extract` subcommand (spec 0123) to resolve its
`--path` argument. A path naming a packed run there now resolves to the
run's leader rather than to whichever element the old ordinal landed on.
That is the intended meaning, but it is a CLI-visible change and belongs
in the release notes.

### S4. `render_overrides_inner`'s ordinal counter
(`override_apply.rs:1249-1250`)

The child loop increments `ordinal` once per child. It must instead
increment only when the child begins a new record, per S1, comparing
against the previous child's span. `child_path` (`:1121-1127`) is
unchanged — it already takes the ordinal as a parameter.

### S5. One rule, one test

S2, S3 and S4 are three independent implementations of the same
traversal. The existing pair (`sibling_position` walking backward,
`child_path` counting forward) is already load-bearing and already
subtle — the doc comment at `:1148-1159` records that recomputing paths
per node "observed to make a single `render_overrides` pass take minutes
on a ~600k-node document," which is why the fast forward counter exists
at all.

Adding a rule to both raises the risk they diverge, and a divergence
produces mismatched paths that fail *silently*: an override is stored
under one path and looked up under another. So S1's helper is shared,
and a test asserts on a packed-heavy fixture that for every node,
`positional_path` agrees with the path `render_overrides_inner` would
build, and that `resolve_path` inverts it per S7.

### S6. Heat cues resolve to the record

In `heat_cue_resolve` (`heat_cue.rs:234-241`), a node with
`packed_record_start.is_some()` resolves to its run leader and scores
`packed_record_extent`'s reconstructed `raw_range` — the whole record's
payload — rather than the element's own bytes.

Three consequences, all wanted:

- the cue describes what `t` would act on;
- `current_type_key` and the scored range now refer to the same object,
  which is what makes the comparison meaningful at all;
- N element lines share one cache key instead of holding N, which is a
  direct reduction in the heat cache pressure recorded as P5/P6.

`can_override` (`override_select.rs:30-37`) keeps its packed early
return: acting from any element must keep working (N3).

### S7. The inverse invariant is restated, not preserved

`resolve_path` is documented as the inverse of `positional_path`, and
`tests/command_line.rs:444-461` pins it. That can no longer hold as an
identity — N element nodes share one path, so the map is many-to-one.

The invariant becomes:

> `resolve_path(positional_path(x))` is `x` for every node that is not a
> packed element, and the run leader for every node that is.

Equivalently: resolving a path always yields **the node that can be
acted upon**. That is a better property than the one it replaces, and
the test should be rewritten to state it rather than weakened to avoid
it.

### S8. Persisted paths change meaning, and that is fine

Override origins are persisted to YAML (spec 0125 §G3) and validated on
load by `origin_resolves` (spec 0117 §4). This spec changes what a
stored path denotes in any document containing a packed run, so an
existing saved file can retarget on next load.

**No migration, no format version.** The software is alpha; there is
also nothing sound to preserve, since today's meaning is already
unstable under the very override actions the file records (Consequence
1). A stored path in a packed-bearing document has no dependable
meaning to begin with, so migrating it would only propagate a
guaranteed-wrong prior meaning.

Worth knowing rather than acted upon: `origin_resolves` already rejects
paths that no longer resolve, so where the change is detectable at all
the failure mode is a dropped override rather than a misapplied one.

## Feasibility

Every site that produces or consumes a positional path was enumerated
before this spec was written. There are four, and all are in protolens:

| site | role | change |
|---|---|---|
| `sibling_position` (`navigation.rs:441`) | produces one segment | S2 |
| `positional_path` (`navigation.rs:462`) | joins segments | none |
| `child_path` + ordinal loop (`override_apply.rs:1121`, `:1250`) | produces, fast path | S4 |
| `nth_child` (`command_line.rs:700`) | consumes | S3 |

`positional_path` needs no change because it only concatenates what
`sibling_position` returns.

Nothing outside protolens computes positional paths — they are a
protolens concept (spec 0113 D25), not a `prototext-core` one — so this
is a single-crate change with no API surface.

The other `packed_record_start` readers were checked and are unaffected,
because they concern *display* rather than *addressing*:
`display_range` (`navigation.rs:495`) keeps showing an element's own
bytes, which is the point of rendering elements separately; the
wire-type reconstructions at `command_line.rs:233`/`:546` and
`override_apply.rs:553` already report the record's `WT_LEN`.

## Test plan

- **First, the failing test.** Reproduce Consequence 1 against the
  current code: build a fixture with a packed run followed by a
  message sibling, override the later sibling, override the run,
  and assert the first override still designates the same node. If it
  already passes, stop — the premise is wrong.
- **The S1 trap gets an explicit test:** a message with two consecutive
  non-packed scalar siblings must give them *distinct* ordinals. This is
  the regression that a plausible one-line implementation introduces,
  and it would otherwise be caught only by whichever unrelated test
  happens to use two scalars in a row.
- **Ordinal stability across override state:** for a fixture containing
  a packed run, assert the full path map is identical before an
  override on the run, while it is active, and after deactivating it.
- **The three producers agree** (S5), asserted node-by-node on a
  packed-heavy fixture.
- **S7's restated invariant**, replacing the current identity test.
- **Heat cue on a run** (S6): every line of a run yields the same cue,
  and it is the cue for the record's payload — with a test that the
  cache holds one entry for the run, not N.
- **`prototext decode` output is byte-identical** before and after, on
  every fixture. G5 is absolute and cheap to assert.
- `reuse lint` and `nix-build -A ci`.

## Open questions

- **Q1 — settled by test.** They coincide, as predicted, and the
  transition now has the explicit test the question asked for:
  `packed_run_ordinals_are_stable_across_the_override_lifecycle`
  asserts the path map across activate *and* deactivate, which is the
  direction that used to shift ordinals back.
- **Q2 — resolved as assumed: every line.** `heat_cue_for` stays keyed
  on `line_to_node` and every element line shows the record's cue, so
  the cue is visible wherever the cursor rests. It costs nothing: all N
  lines now resolve to one `heat_caches` entry, asserted by
  `a_packed_run_scores_one_cue_over_the_whole_record`.
- **Q3 — still open, and harmless.** `origin_resolves`'s `PathField`
  arm resolves the parent path and then walks children by field number,
  so it only depends on the parent's path, not on any ordinal inside
  the run. It is therefore at least as robust as the `Path` arm. Not
  investigated further, because S8 already declines to migrate.

## Implementation notes

Two things not foreseen by the spec, both recorded here rather than
retrofitted into the sections above.

**`repeated_scalar_fixture` was a source of distinct paths.** Four
manage-pane tests (spec 0134's ambiguity machinery, plus Shift-Down
activation) used that fixture only because its three packed elements
were the cheapest way to get three siblings sharing a parent type and
field number *and* deriving three different `Path` origins. G1 removes
exactly that property, so they now see one candidate where they expect
three. They were repointed at a new `repeated_message_fixture` —
`Outer { repeated Item items = 1; }`, three real submessages — which
supplies the precondition by construction. The assertions themselves
were not weakened.

**S6 is computed from `packed_record_start`, not by walking the run.**
The spec said the cue "resolves to its run leader"; doing that literally
would mean a `packed_record_siblings` walk on every visible line every
frame. The record's payload is recoverable in O(1) from the element's
own `packed_record_start` (parse the tag, parse the length), which is
what `heat_scored_range` does. The leader index is never needed:
`heat_caches` is keyed on the payload start, which all elements now
share, and `current_type_key` already resolves the record's override
because every element shares the record's path (S2).
