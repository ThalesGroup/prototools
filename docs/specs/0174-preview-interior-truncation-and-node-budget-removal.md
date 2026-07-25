<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0174 — protolens/prototext-core: bound the override preview by truncating the interior, and drop `node_budget` from prototext-core

Status: implemented
Implemented in: 2026-07-25
App: protolens, prototext-core
Refs: docs/specs/0163-protolens-override-splice-node-budget.md
      (introduces `node_budget`; this spec removes it),
      docs/specs/0135-*.md (the wrapper-render splice this preserves),
      docs/specs/0169-protolens-budget-elision-run.md (suspended),
      docs/specs/0170-prototext-core-render-budget-truncation-as-ellipsis.md
      (superseded — it reworks a marker this spec deletes),
      docs/prototext/decode-flaws.md (C5, which dissolves here),
      docs/specs/0171-wire-format-bounds-arithmetic-and-recursion-depth-caps.md
      (N3 deferred exactly this work)

## Background

`prototext-core` makes two promises: any byte sequence renders, and the
render re-encodes to those same bytes. `DecodeRenderOpts::node_budget`
breaks the second one. When it trips, `render_message` emits
`MalformedKind::NodeBudgetExceeded` (`render_text/mod.rs:472-488`), and
`encode_text/fields.rs` has no arm for it — so a budgeted render is the
one production in the crate that cannot round-trip.

It cannot be fixed in place, either. The obvious repair is the one spec
0171 §S4 used for its depth cap: dump the remainder verbatim as
`INVALID_TAG_TYPE`, which re-encodes tagless and byte-for-byte. That is
precisely wrong here — `render_node_budget_exceeded` reports only a
*length* and discards the bytes on purpose (`helpers/scalar.rs:170-175`),
because the remainder is the pathologically large slice the budget exists
to avoid materializing. Losslessness and bounded output are in direct
opposition. The budget does not belong in this crate.

### Who actually uses it

One caller. `override_apply.rs:1482` passes
`node_budget: is_preview.then_some(self.override_splice_node_budget)`.
Every other construction in the workspace passes `None`, including
protolens's own production decode (`decode.rs`) and `lib.rs:117`.

Its job (spec 0163) is to stop a *live override preview* from decoding a
pathological blob. It must bound the **work**, not merely the output, so
rendering fully and truncating afterwards is not a substitute.

### Why the naive replacement fails

The obvious way to bound the work without core's help is to hand core
fewer bytes. Done naively it destroys the preview.

`field_bytes` is the node's *complete* `tag + length + payload` span
(`override_apply.rs:1437`, spec 0135 G1), rendered against a synthetic
one-field wrapper (`decode::register_wrapper`). Truncating that buffer
cuts inside the wrapper's single field, so its length prefix now overruns
the buffer, and `render_message` answers with one `TRUNCATED_BYTES` field
covering the whole remainder and returns (`render_text/mod.rs:616-629`).
The preview collapses to a single escaped-bytes line with no structure —
always, not just for deeply nested payloads.

### The fix: cut the interior, not the field

Truncate the *payload* and rewrite the length prefix to match. The
wrapper then receives a well-formed, merely shorter field, so the cut
lands inside the interior. Every interior field that fits renders in
full, correctly typed and arbitrarily nested; only the one field
straddling the cut degrades, and it is dropped from the output and
replaced by a `...` line.

Note what this costs `prototext-core`: nothing. Core gains no preview
concept, no new option and no new `MalformedKind` — it only *loses*
`node_budget`. The bounding is entirely a matter of which bytes
protolens chooses to hand it, and the `...` is a protolens display
artifact appended after rendering. The preview path also gets cheaper
than today's: core stops walking a large payload and then discarding
most of it, and instead never sees the bytes past the cut.

This keeps the entire spec-0135 splice intact — same header line, same
`register_wrapper`, same local-root handling — which matters because
`splice_override` is 400 lines of carefully sequenced tree surgery and
the preview path shares all of it.

A preview is display-only and has no round-trip ambition. Spec 0163
already keys the render cache on `is_preview`
(`override_apply.rs:1446-1450`) precisely so a truncated preview can
never be mistaken for a real render, and confirmed overrides are exempt
from bounding by design. That exemption is unchanged here.

## Goals

- **G1**: `prototext-core` carries no caller budget. `node_budget`,
  `NODE_BUDGET`, `NODE_COUNT`, `MalformedKind::NodeBudgetExceeded` and
  `render_node_budget_exceeded` are deleted, and the round-trip promise
  becomes unconditional: every production the renderer can emit has an
  `encode_text` arm.
- **G2**: The live preview stays bounded. Bounded input bytes bound the
  decode, the render, the span count and the line count together.
- **G3**: The preview renders complete, correctly-typed fields with full
  nesting up to the cut — the property naive field-level truncation
  loses.
- **G4**: No malformity marker is visible in a preview, but the
  truncation is not silent either: a truncated preview ends with a
  literal `...` line, so the user sees that there is more.
- **G5**: Confirmed (non-preview) overrides are byte-for-byte unchanged.

## Non-goals

- **N1**: Preserving the *node* granularity of the budget. It becomes a
  byte budget. Bytes are what protolens can actually control from
  outside, and a node costs at least one byte, so a byte cap is a node
  cap up to a constant.
- **N2**: Round-tripping a preview. It is display-only, and G4's `...`
  line is deliberately not valid prototext — it exists only in
  protolens' rendered-line buffer, never in anything handed back to
  `encode_text`.
- **N3**: Changing the confirmed-override path, `register_wrapper`, or
  anything else spec 0135 settled (G5).
- **N4**: Implementing spec 0170. It reworks `NODE_BUDGET_EXCEEDED` into
  an `...` elision; this spec deletes the marker outright, so 0170 is
  marked superseded rather than implemented.
- **N5**: Making the preview budget adapt to the pane height. A fixed
  budget is enough to bound the work; sizing it to the viewport is a
  separate ergonomic question.

## Specification

### S1. Delete the budget from `prototext-core`

Remove, in `serialize/render_text/`:

- `DecodeRenderOpts::node_budget` (`mod.rs:278-281`) and its
  `Default` arm (`mod.rs:294`); the destructuring at `mod.rs:323`/`:399`
  and the two `NODE_BUDGET`/`NODE_COUNT` seeds (`mod.rs:352-353`,
  `:420-421`).
- The `NODE_BUDGET`/`NODE_COUNT` thread-locals (`mod.rs:148-149`).
- The budget check in `render_message` (`mod.rs:472-488`).
- The budget check in `render_packed` (`packed.rs:355-361`) and its
  imports (`packed.rs:24`, `:26`).
- `MalformedKind::NodeBudgetExceeded` (`sink.rs:70-73`) and its
  `TextSink::malformed` arm (`sink.rs:833-838`).
- `IndexingTextSink::scalar_field`'s `NODE_COUNT` snapshot/consumed
  arithmetic (`sink.rs:1167-1168`, `:1204-1205`), which exists only to
  attribute packed elements against the budget.
- `render_node_budget_exceeded` (`helpers/scalar.rs:170-198`).
- Tests `node_budget_truncates_deep_nesting_with_a_visible_marker` and
  `node_budget_none_is_unaffected_by_a_never_tripped_budget`
  (`mod.rs:829-910`).

`lib.rs:117`'s `node_budget: None` goes with the field.

**Decode-flaws C5 dissolves.** That bug — `ProbeSink` shares `NODE_COUNT`
with the outer render, so a tripped budget silently demotes well-formed
nested messages to bytes — has no subject once the counter is gone. Spec
0171 N3 deferred it here; no separate fix is needed.

### S2. A byte budget in protolens

`App::override_splice_node_budget: usize` becomes
`override_preview_byte_budget: usize`, and
`OVERRIDE_SPLICE_NODE_BUDGET_DEFAULT: usize = 200` becomes
`OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT: usize = 4096`.

4096 is chosen to be generous in lines while still bounding the work:
the smallest interior field is two bytes, so it admits at most ~2 000
nodes, and a realistic mixed payload yields a few hundred lines — more
than any pane shows, which is the point (G2 bounds work, it does not size
the viewport, N5).

The CLI flag `--override-preview-node-budget` (`main.rs:96-100`) is
renamed `--override-preview-byte-budget`, with
`default_value_t = tui::App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT`.

### S3. Truncate the interior, rewrite the frame

New helper in `override_apply.rs`, called only when `is_preview`:

```rust
/// A copy of `field_bytes` whose *interior* is cut to at most `budget`
/// bytes, re-framed so the result is still a well-formed field.
///
/// Returns `None` when nothing was cut, so the caller can tell a
/// truncated preview from a complete one (S4).
fn truncate_interior(
    field_bytes: &[u8],
    budget: usize,
    shape: TruncShape,
) -> Option<(Vec<u8>, usize)>
```

The second return value is the offset shift of S3's *Offsets* paragraph.

**The header line is untouched by all of this.** Truncation rewrites only
the length varint and the payload tail, never the tag and never the
wrapper descriptor. So line 0 — the type name from `begin_nested` against
`target_desc`, and the field name via `patch_synthetic_field_name`
(`override_apply.rs:1523-1531`) — is byte-identical to what an
untruncated preview of the same candidate would render. Synthesizing the
header by hand instead was considered and rejected: it would re-derive
`register_wrapper`'s naming and `begin_nested`'s group-vs-message type
name choice, and it would additionally force a synthetic root `NodeSpan`,
because `decode::build_tree` requires the local span array's last entry
to be the container's own span (`override_apply.rs:1651`).

**Framing and cut position are independent.** Framing is read from the
node's own tag: LEN-framed emits `tag ++ varint(kept) ++ payload[..kept]`
(rewriting the length prefix is the whole mechanism — the field stays
well-formed, so the cut lands in the interior instead of overrunning the
wrapper's single field); group-framed has no length prefix, so it emits
`tag ++ payload[..kept]` and drops the closing `END_GROUP`.
`render_group_field` passes `close_facts: None` when the group reaches
end-of-buffer without a close tag (`helpers/len_field.rs:321-334`), so
`end_nested` emits a plain `}` with no annotation — nothing for G4 to
hide.

`kept` is chosen by the target's shape. Each rule is what it is because
the renderer's own failure mode differs, not for symmetry:

```rust
enum TruncShape {
    /// Message, group, `bytes`. Cut at exactly `budget`.
    Exact,
    /// `string`. Cut at the last UTF-8 character boundary <= `budget`.
    CharBoundary,
    /// Everything else. Never cut.
    Never,
}
```

- **`Exact` — message, group, `bytes`.** For a message or group the cut
  *should* land deep inside, so every field that fits renders in full,
  correctly typed and arbitrarily nested — that is G3. A boundary-aligned
  cut would be wrong here: if the interior's first field is a 1 MB
  submessage, aligning either swallows it whole or yields an empty
  preview. The one straddling child degrades and is dropped by S4. For
  `bytes` no alignment exists to respect: any byte sequence is a valid
  `bytes` value, so an exact cut produces a shorter valid value and no
  marker.
- **`CharBoundary` — `string`.** Walking back to a character boundary
  (at most three bytes) keeps the shortened payload valid UTF-8, so the
  renderer emits an ordinary string line rather than an `INVALID_STRING`
  marker. This is the rule that makes truncating strings safe at all.
- **`Never` — everything else.** Varint, I32 and I64 are bounded by
  construction at ten bytes. Return `None`.

Both cutting rules align on a boundary the *renderer* respects, so
neither can manufacture a malformity marker that the untruncated data did
not already have. That is the invariant they share, and the reason
`string` needs its own rule rather than falling under `Exact`.

**There is deliberately no packed-record rule**, though the question was
raised and is worth recording, because `decode_packed_elems` *is*
all-or-nothing — the fixed decoder rejects a payload whose length is not
a multiple of the element size (`packed.rs:70-73`), the varint decoder
rejects an incomplete trailing varint — so a misaligned cut would collapse
a whole record to one escaped-bytes line. Two facts make the rule
unreachable:

1. The previewed node can never *itself* be a packed record.
   `decode::register_wrapper` builds its synthetic field with
   `label: Some(Label::Optional as i32)` unconditionally
   (`decode.rs:489-496`), and `render_packed` fires only for a
   **repeated** packable schema field. Retyping a LEN node to a numeric
   keyword gives an optional scalar against a LEN wire type — a wire-type
   mismatch, not a packed record. A raw (`target: None`) preview has no
   schema at all.
2. A packed record nested *inside* the interior keeps its own untouched
   length prefix, because truncation rewrites only the **outer** field's
   length. So it either fits whole or overruns the shortened buffer and
   degrades to the ordinary `TRUNCATED_BYTES` straddler. It can never end
   up length-satisfied but misaligned, which is the only shape
   `decode_packed_elems` rejects.

By the same argument the deleted `render_packed` per-element budget check
was already unreachable: it was guarded by `NODE_BUDGET`, which only the
preview ever set.

Packed records with thousands of elements are a real cost, but in the
**main document**, which has never carried a budget and which this spec
does not change — nothing regresses. Bounding that is rendering-worklist
W33/W34. Making the packed rule live would require previewing a blob as a
repeated field, which is not a type override protolens offers.

Truncating `string`/`bytes` also bounds the rendered *line* length, which
a node budget never could: a 1 MB `bytes` retype currently escapes to
several MB of text on one line, which `colorize()` then scans. That is a
genuine preview cost, not just symmetry. It does **not** solve the same
problem for the main document, which stays open as rendering-worklist
W33/W34.

**Offsets.** Child `NodeSpan::raw_range`s come back as offsets into the
buffer that was passed in. The group framing is a pure prefix, so it
preserves every offset exactly (shift 0). For the LEN framings the
rewritten `varint(kept)` may be *shorter* than the original prefix,
shifting the whole interior by a constant
`shift = original_prefix_len - new_prefix_len`. The caller folds `shift`
into `splice_override`'s existing `byte_offset`
(`override_apply.rs:1649`) — one addition, no new traversal. The local
root's own `raw_range` needs no care: it is already force-overwritten
with `old_span.raw_range` (`override_apply.rs:1701`).

The alternative, padding `kept` to the original prefix width with
overlong varint bytes (which the wire format and `len_ohb` both
tolerate), is rejected: it keeps offsets identical but makes the header
line grow a `#@ len_ohb` modifier, which G4 then has to hide. A constant
integer shift is the smaller lie.

### S4. Replace the straddling field's line with `...`

Only `TruncShape::Exact` on a message or group can leave a straddling
field, and there it usually does: the interior's last field is incomplete
and renders as `TRUNCATED_BYTES`. `CharBoundary` aligns on a boundary the
renderer respects and `Exact` on a `bytes` target has none to violate, so
neither straddles — a truncated `string`/`bytes` is a shorter *valid*
value and its own line is the last line. Per G4 the preview shows no
malformity marker, so when
`truncate_interior` returns `Some`, `splice_override`, *before* handing
the lines to the splice:

1. drops trailing rendered lines carrying a `TRUNCATED_BYTES`
   annotation, together with their `line_styles` and any `NodeSpan`
   covering them;
2. appends a single line consisting of `...`, indented one level inside
   the header line for a message or group, and at the value line's own
   indent for a `string`/`bytes` target (which has no header line and no
   closing brace — its rendering is one line, so the `...` simply follows
   it). It gets an empty
   `line_styles` and no `NodeSpan` — it is not a node, so it is not
   selectable, not navigable, and not part of any span range.

   For a message or group the `...` goes *before* the closing `}`, as
   the last interior line, since that is where the elided content would
   have been.

Both steps operate on the rendered lines, not on core: core keeps no
knowledge of previews (G1) and `...` is not part of the prototext
grammar, so nothing in `render_text`/`encode_text` learns about it. A
preview is display-only (N2), so a synthetic trailing line costs
nothing.

The `...` is the whole user-facing signal. It says exactly what happened
— there is more below the cut — at the place the user is already
looking, without a `#@` modifier and without needing a footer note.

The dropped-line step is still required for `Exact` on a message or
group: without it the `...` would sit under a `TRUNCATED_BYTES` line,
which is the marker G4 excludes.

Note that step 1 may drop nothing — under the aligned rules by
construction, and under `Exact` when the cut happens to land exactly on a
field boundary — while `truncate_interior` still returned `Some`. The
`...` is emitted on `Some`, not on "a line was dropped": bytes were
removed either way, so there is more to see either way.

### S5. Call-site change

`override_apply.rs:1466-1486` drops `node_budget` from `DecodeRenderOpts`
and, when `is_preview`, feeds `truncate_interior`'s output to
`decode_and_render_indexed` in place of `field_bytes`, applying S3's
`delta` to the returned spans. When `is_preview` is false, `field_bytes`
is passed exactly as today (G5).

The render-cache key keeps `is_preview` (spec 0163) — now for a stronger
reason than before: the preview's bytes are literally not the confirmed
render's bytes.

## Test plan

**`prototext-core`**

- The two `node_budget` tests are deleted with the feature (S1).
- New: every `MalformedKind` the renderer can emit has an
  `encode_text/fields.rs` arm — the round-trip promise G1 restores. A
  table-driven test over the variants, so a future variant added without
  an encoder arm fails here rather than in the field.
- Regression: with `node_budget` gone, all remaining renders are
  unchanged (it defaulted to `None` everywhere else).

**`protolens`**

- `preview_on_a_pathological_candidate_is_bounded_by_the_byte_budget` —
  the spec 0163 fixture (`Holder` with `OVERRIDE_PREVIEW_BYTE_BUDGET_
  DEFAULT + 10_000` fields). Asserts a bounded tree/lines footprint,
  replacing `splice_override_on_a_pathological_candidate_is_bounded_by_
  the_node_budget`.
- `preview_renders_complete_nested_fields_up_to_the_cut` — the proving
  test for G3, and the one that fails against naive field-level
  truncation. A candidate whose interior holds several nested messages;
  asserts the first ones render as *messages* with their children, not
  as a single bytes line.
- `preview_shows_no_malformity_marker` — G4: no `TRUNCATED_BYTES` (nor
  any other `#@` malformity token) anywhere in a truncated preview's
  lines.
- `truncated_preview_ends_with_an_ellipsis_line` — G4/S4: the last line
  of a truncated preview is `...` at the interior indent, and it carries
  no `NodeSpan`.
- `untruncated_preview_has_no_ellipsis_line` — the converse: a candidate
  smaller than the budget renders with no `...`, so the marker means
  what it says.
- `preview_of_a_long_string_stays_valid_utf8` — S3's `CharBoundary`
  rule, with a multi-byte character deliberately straddling the budget:
  the rendered line must contain no `INVALID_STRING` and must end on a
  whole character.
- `preview_of_a_singular_varint_is_never_truncated` — S3's `Never` rule:
  `truncate_interior` returns `None`, so no `...` and byte-identical
  output to a non-preview render.
- `preview_child_spans_survive_the_length_prefix_shift` — G3/S3: a
  child's `raw_range` in a truncated preview equals the same child's
  `raw_range` in an untruncated render of the same node. This is what
  pins the `delta` correction; it fails if `delta` is dropped or
  double-applied.
- `confirmed_override_is_not_truncated` — G5, retained from spec 0163
  (`splice_override_on_a_confirmed_override_is_not_truncated_by_the_
  node_budget`), re-pointed at the byte budget.
- `preview_respects_a_custom_byte_budget` — retained from spec 0163,
  re-pointed.
- Regression: the spec 0135 splice tests
  (`splice_override_on_a_group_field_keeps_the_group_prefix`,
  `..._preserves_the_header_line_indentation`,
  `..._shows_the_root_field_number_in_the_header_line`, and the rest)
  are untouched — they all run `is_preview: false`.

## Resolved questions

- **Q1** (budget default). Settled: `4096`. Deriving it from viewport
  height stays out of scope (N5).
- **Q2** (CLI flag rename). Settled: rename outright to
  `--override-preview-byte-budget`. No hidden alias for the old name —
  the flag now means something different (bytes, not nodes), so
  silently accepting the old spelling would misreport rather than
  preserve behavior.
