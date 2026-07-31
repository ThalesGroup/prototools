<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0219 — a length-delimited record can be read as a packed run

Status: implemented
Implemented in: 2026-07-31
App: protolens
Refs: docs/specs/0135-protolens-type-as-override.md (the synthetic
        one-field wrapper every override renders against),
      docs/specs/0174-protolens-override-preview-byte-budget.md (the
        preview budget this spec has to extend),
      docs/specs/0184-protolens-packed-record-ordinals.md (a packed run
        is one wire record and one ordinal),
      docs/specs/0119-protolens-override-natural-type-fallback.md
        (`natural_type`, the fallback a deleted override lands on)

## Background

Two problems, one cause.

**The bug.** On `googleapis.desc`, overriding the packed run at
`/1/5/1/1` as `None` and then *deleting* that override leaves the run
stuck at

```
1: "\004\000\002\000"  #@ bytes; TYPE_MISMATCH
```

instead of returning to the run it was. It is not recoverable from the
UI: re-picking `int32` from the override pane reproduces the same line.

**The gap.** `[packed=true]` is not something the override pane can
express. The candidate list is message/enum FQDNs plus
`ALL_PRIMITIVE_KEYWORDS`, and `can_override` admits any `WT_LEN` node,
so `int32` *is* offered on an unknown length-delimited blob — it just
answers `TYPE_MISMATCH`. There is no way to ask "read these bytes as a
run of varints", which is one of the more common things one wants from
a wire inspector.

Both come from `decode::register_wrapper` declaring its synthetic
one-field wrapper `Label::Optional` unconditionally. `prototext-core`
renders a packed run only when the field schema is *repeated*, of a
packable kind, and `is_packed()`
(`render_text/helpers/len_field.rs::use_packed`). An `optional` numeric
field carrying a LEN record is a genuine wire-type mismatch, so that is
what gets rendered.

The deletion path reaches it through `resettle_node` → `natural_type
(idx)` → `"int32"` → `render_node_as`.

## Goals

- **G1.** Deleting or deactivating an override on a packed run restores
  the run, byte-for-byte as it rendered before.
- **G2.** Retyping *any* `WT_LEN` record to a packable primitive reads
  it as a packed run of that type. This is the whole of the new
  capability: no new candidate, no new syntax, no new key.
- **G3.** The preview stays bounded. Spec 0174's byte budget must apply
  to a packed target as it does to a message target.

## Non-goals

- **N1.** No `[packed=false]` control. A repeated non-packed scalar is
  N separate wire records, each individually overridable already; there
  is nothing a per-record override could add.
- **N2.** `export --descriptor` is not taught about packedness. It
  already drops `[packed=true]` for genuinely packed schema fields
  (`resolve_export_fields` derives `Label::Repeated` from sibling count
  or parent cardinality and never emits `FieldOptions`), and it rejects
  primitive-keyword overrides outright. Both are pre-existing and
  orthogonal; fixing them here would hide this change inside a larger
  one.
- **N3.** No cap on the *committed* render. Spec 0174 G5 says a
  confirmed override renders completely, and that stands — see
  "Residual risk".

## Specification

- **S1.** `decode::register_wrapper` takes `packed: bool`. It is ANDed
  with `is_packable(field_type)` — the fourteen types protobuf permits
  `[packed=true]` on, i.e. every numeric scalar plus `bool` and `enum`
  — so a caller may pass `true` freely and a `string`/`bytes`/message
  target is unaffected. When the result is true the synthetic field is
  built with `Label::Repeated` and `FieldOptions { packed: Some(true) }`
  rather than `Label::Optional`.

  `is_packable` must stay in step with `len_field.rs`'s
  `is_packable_kind`. They are two spellings of the same protobuf rule
  in two crates; a type in one and not the other means a wrapper that
  claims to be packed and renders as a mismatch anyway.

- **S2.** `synthetic_wrapper_name` appends `":packed"` to its hash key
  **only when packed is true**, so every wrapper name that exists today
  is byte-identical afterwards and no cached descriptor is invalidated.

- **S3.** `render_node_as` decides packedness as

  ```rust
  let packed = in_packed_run || len_framed;
  ```

  where `in_packed_run` is `span.packed_record_start != NO_PACKED_RECORD`
  and `len_framed` is `span.wire_type == WT_LEN`. Neither disjunct is
  sufficient alone:

  - `packed_record_start` holds only while the run is still *rendered*
    as a run. Overriding it to `None` replaces the node's span with one
    from `extract.rs`, where the field is `NO_PACKED_RECORD`. By the
    time the override is deleted the flag is gone — this is precisely
    G1's bug.
  - `wire_type` on a live run *member* is the element's (`WT_VARINT`),
    not the record's. Reading it alone breaks the explicit retype of a
    run that has not been demoted.

- **S4.** `override_select::warm_visible_override_wrappers` computes the
  same predicate. It exists to pre-register the wrapper the splice will
  ask for; disagreeing means it registers one the splice never looks up,
  silently restoring the per-keystroke registration stall it was written
  to remove.

- **S5.** A packed run renders one line per element, each carrying the
  wrapper's `"_"` placeholder, so `render_node_as`'s name patch loops
  over every rendered line when the render is packed. It stays a
  single-line patch otherwise: a blanket pass could reach a nested field
  genuinely named `_`.

- **S6.** `TruncShape` gains `PackedVarint` and `PackedFixed(usize)`,
  and `trunc_shape_for` takes the `packed` flag:

  | target | shape |
  | --- | --- |
  | message, group, `bytes` | `Exact` (unchanged) |
  | `string` | `CharBoundary` (unchanged) |
  | `double`/`fixed64`/`sfixed64`, packed | `PackedFixed(8)` |
  | `float`/`fixed32`/`sfixed32`, packed | `PackedFixed(4)` |
  | any other packable, packed | `PackedVarint` |
  | anything, not packed | `Never` (unchanged) |

  `cut_at` implements `PackedFixed(n)` as `budget - budget % n` and
  `PackedVarint` by walking back from `budget` while the preceding byte
  has its continuation bit set — a position immediately after a byte
  with that bit clear is a varint boundary, so this is at most ten
  steps.

  This is not decoration. `decode_packed_elems` is **all-or-nothing**:
  one bad element aborts the whole record to a single
  `INVALID_PACKED_RECORDS` line. An unaligned cut would therefore make
  the preview claim the bytes are invalid while the commit renders a
  clean run — the exact preview/commit divergence spec 0185 G3 forbids.

  `insert_truncation_marker` needs no change: with no `TRUNCATED_BYTES`
  straddler and no closing brace, it appends the `...` marker after the
  last element line at that line's indent, which is right.

## Residual risk

A confirmed override renders completely (0174 G5), and a packed run
costs one node — and, inside `prototext-core`, one `PackedElem` with an
owned `String` — per *element*. Elements can be one byte. So retyping a
25 MB LEN blob as `int32` materializes on the order of 25 million
nodes.

This is not new in kind: a schema-declared packed field of that size
already does it at decode time. It is new in *reach* — one keystroke on
any large length-delimited node now gets there. Accepted here rather
than capped, because a cap would be the first place protolens refuses to
show the user what they asked for, and because the arena's per-node cost
is being attacked directly elsewhere (specs 0203, 0206, 0211–0213,
0216). Revisit if it is actually hit.

Note the preview is *not* exposed to this, which is the whole point of
S6: 4 KB of budget bounds a packed preview at ~4096 lines.

### `wire_type` means two different things

S3 is a disjunction because `NodeSpan.wire_type` is the **element's** on
a packed-run member and the **record's** everywhere else. Both disjuncts
are asking one question — is the record this node stands for LEN-framed?
— and the second spelling exists only because the first cannot answer it
for a run member, whose record's tag was consumed once for N nodes.

This is not a new input. Which record a node belongs to is spec 0184's
N:1 mapping, already load-bearing in `nth_child`, `sibling_position`,
`render_overrides_inner`'s child loop, `splice_override`'s redirect to
the run's leader, and `heat_cue::heat_scored_range` — and already read
by `render_node_as` itself, whose pre-existing `in_packed_run` branch
widens `raw_range`/`text_range` to `packed_record_extent`. That is what
makes the bug in "Background" a genuine inconsistency rather than a
missing feature: the function was already handing the renderer the whole
LEN record's bytes while declaring the wrapper `optional`. S3 makes the
declaration agree with the range.

Nor does the schema decide the framing. `packed_record_start` is a byte
offset and `packed_record_extent` re-parses the tag and length at it
against `self.blob`, exactly as `extract::message_payload_range` and
`heat_scored_range` do. Schema decides only *where the record's tag is*.

What is accepted here is the ambiguity itself: any future predicate that
reads `span.wire_type` as "the record's framing" is silently wrong on a
run member, which is how this spec's first rejected alternative came to
be built. The defusing change, if it is ever worth making, is one
accessor — `record_wire_type(idx)`, the tag at `packed_record_start`
when set and `span.wire_type` otherwise — after which S3 is a single
comparison and no call site has to know which meaning it holds.

## Alternatives considered

### Derive packedness from `packed_record_start` alone

Built first. Fixes an explicit retype of a live run and does **not**
fix G1's reported bug, for the reason in S3. It is the obvious fix and
it is wrong; recorded so it is not retried.

### Derive it from the parent's schema (`FieldDescriptor::is_packed`)

Also built, and it does fix G1 — `[packed=true]` is a property of the
parent's declaration, which survives any retype of the child. Correct
and strictly narrower than S3.

Rejected because it delivers G1 only. It cannot help an unknown blob,
which by definition has no parent schema field, and the unknown blob is
exactly where "show me this as varints" is worth having. It also leaves
protolens inconsistent about what `WT_LEN` means: `string` on a
submessage already succeeds silently, while `int32` on the same node
complained. S3 removes that asymmetry — under it, **no** override target
produces `TYPE_MISMATCH` on a LEN node, because every target has a
defined reading of length-delimited bytes.

### A separate `repeated int32 [packed=true]` candidate in the pane

Doubles the primitive section of the candidate list (fourteen extra
rows) to express something the wire framing already determines: on a
LEN record, packed is the only non-mismatching reading, and on a
varint/I32/I64 record it is impossible. A choice with one valid answer
is not a choice.

### Keeping `TYPE_MISMATCH` as a plausibility warning

The real cost of S3: an accidental retype of a submessage to `int32`
now yields a plausible-looking column of integers instead of a loud
complaint, and most byte sequences *are* valid varint streams.

Not enough to keep it. `decode_packed_elems` still rejects a great deal
— `bool` on anything but 0/1, `uint32`/`sint32` above 2^32, any
fixed-width type on a payload that is not a whole multiple of the
element size, any trailing truncated varint — so implausible readings
mostly still announce themselves as `INVALID_PACKED_RECORDS`. And
`can_override`'s own doc already states the governing posture: the test
is deliberately permissive, and the user is trusted to judge whether a
reinterpretation is meaningful.

## Test plan

1. `a_deleted_override_restores_a_packed_run_rather_than_a_type_mismatch`
   — G1 end to end: activate `None` on a packed run, delete the entry,
   assert `app.lines` is restored verbatim including the `pack_size: 3`
   annotation. Same test asserts an explicit `int32` retype of a live
   run renders the run, which is the half `packed_record_start` alone
   would have satisfied — so the test fails if S3 ever regresses to the
   first rejected alternative.
2. `an_unknown_length_delimited_blob_can_be_read_as_a_packed_run` — G2:
   a LEN node with no schema field behind it, retyped to `int32`,
   renders one line per element. Reached in two steps — retype a
   `bytes` field to a message with no declared fields, then retype the
   unknown LEN record that surfaces inside it — so the node has neither
   a parent field of any kind nor a `packed_record_start`. This is the
   case both rejected alternatives fail.
3. `a_packed_preview_is_cut_at_an_element_boundary` — G3/S6: a packed
   run longer than the byte budget previews as a whole number of
   elements plus the `...` marker, and specifically not as
   `INVALID_PACKED_RECORDS`.
4. Existing tests that used "retype a submessage as `int32`" purely as
   a device for producing a wide `TYPE_MISMATCH` line
   (`deactivating_override_reclamps_pan_offset_to_the_shrunk_content`,
   `a_deeply_nested_splice_moves_its_ancestors_footers_without_leaving_stale_entries`)
   move to `string`, which still collapses a subtree to one wide line.
   `overriding_a_varint_onto_an_incompatible_primitive_keeps_the_annotation`
   is untouched: its node is `WT_VARINT`, where `TYPE_MISMATCH` remains
   both reachable and correct.

## Measured outcome

Implemented 2026-07-31. The reported bug is gone: `None` on `/1/5/1/1`
of `googleapis.desc`, then delete, restores the run. Retyping any LEN
node to a packable primitive now reads it as a run, including a blob
with no schema behind it.

`protolens`'s suite went 534 → 536 tests, all green, workspace clean.
Two existing tests moved off `int32` onto `string` (test plan item 4);
no other test needed touching, and no wrapper name changed (S2), so no
cached descriptor was invalidated.

One flaw was found while specifying and is fixed here rather than
carried: `TruncShape::Never` justified itself in its own doc comment on
the premise that `register_wrapper`'s synthetic field is "always
`Label::Optional`". S1 falsifies that, and without S6 a packable target
on a LEN node would have returned `Never` and bypassed spec 0174's
4 KB preview budget entirely — an unbounded render per keystroke while
arrowing the candidate list.
