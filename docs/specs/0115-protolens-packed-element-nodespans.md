<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0115 — `protolens`: one `NodeSpan` per packed-repeated element

Status: implemented
Implemented in: 2026-07-13
Refs: `docs/specs/0110-render-sink-unification.md`,
`docs/specs/0111-protolens-v1-decode-navigate-extract.md`,
`docs/specs/0114-protolens-range-type-override.md`
App: protolens

---

## Background

`protolens`'s main-pane cursor navigation (`tui.rs`'s `move_down`/`move_up`)
is node-granular: it walks `TreeNode`s via `doc_next`/`doc_prev`, one
selectable row per `NodeSpan`, and can only ever land a cursor on a node's
*first* displayed line (`text_range.start`).

For every field kind except one, this is invisible, because every other
field kind's `NodeSpan` already spans exactly one line (a scalar) or wraps
a nested subtree whose *children* are themselves separately selectable
nodes (a message/group). The exception is a **packed-repeated scalar
field**: `render_packed` (`prototext-core/src/serialize/render_text/
render_text/packed.rs`) writes one line per element directly into the
`Sink`'s output, entirely bypassing per-element `Sink::scalar_field` calls
— so `IndexingTextSink` (spec 0110 §3) observes only one top-level
`scalar_field` invocation for the whole record and pushes exactly one
`NodeSpan`, whose `text_range` spans every emitted element line.

This was discovered interactively (spec 0114 §5, overriding a root node to
a type whose payload is a large packed field — 226 elements in the
reported case): with a single `NodeSpan` covering all 226 lines, the
cursor can only ever select the field's first element; every other
element's line is permanently unreachable by any navigation key, so its
value cannot be read, searched, or targeted by any per-line operation.

This spec gives each packed-repeated element its own `NodeSpan`, matching
the granularity every other field kind already has.

---

## Goals

1. **One `NodeSpan` per packed element**, `text_range` covering exactly
   that element's own rendered line, `is_message: false`,
   `field_number` identical across all N sibling spans (same repeated
   field) — matching today's single-span convention of "one span per
   selectable row."
2. **Byte-accurate per-element `raw_range`**, computed independently for
   varint-packed and fixed-width-packed encodings (§1).
3. **No change to rendered text.** `TextSink`'s output for a packed field
   is reused as-is by `IndexingTextSink` — computing per-element spans
   must not require re-implementing or duplicating `render_packed`'s
   line-writing logic (§2).
4. **Internal "no tag of its own" discriminator** on `NodeSpan`, so
   `protolens`'s tag/length-stripping logic (`extract::
   message_payload_range`, `tui::display_range`) can special-case packed
   elements rather than misinterpreting their first payload byte as a
   wire tag (§3).

## Non-goals

- **No status-line/UI surfacing of the new discriminator.** It is
  consumed internally by `protolens`'s own range logic only, not
  displayed, not exposed as a new visible field/annotation. (Deferred
  until a concrete need is identified — see Open Issues.)
- **No change to `render_packed`'s text output.** Purely an indexing-side
  change; a plain `TextSink`'s rendered bytes for a packed field are
  byte-for-byte unchanged.
- **No change to non-packed scalar or message/group `NodeSpan`
  granularity** — this spec is scoped to packed-repeated fields only.
- **No override-target changes.** Packed-element nodes are already
  excluded from `t`/`:type-as` targets today (`is_message: false`); this
  spec doesn't change that.
- **Implementation deferred.** This spec is written and reviewed now, but
  not implemented until after spec 0114 is fully complete (all of §1–§8).

---

## Specification

### §1 — Per-element byte ranges

`PackedElem` (`packed.rs`) gains a `byte_range: Range<usize>`: the
element's own byte span *within the packed payload* (`data`, the field's
LEN-delimited contents after tag+length) — not yet translated to absolute
document coordinates.

- **Fixed-width kinds** (`Double`/`Float`/`Fixed64`/`Sfixed64`/
  `Fixed32`/`Sfixed32`, `decode_packed_fixed_elems`): trivial —
  `i*elem_size .. (i+1)*elem_size` for the `i`-th element (`elem_size` is
  already computed per-kind: 8 or 4 bytes).
- **Varint-packed kinds** (`decode_packed_varint_elems`): the loop already
  tracks the element's start offset (`i`, before `parse_varint`) and its
  end offset (`vr.next_pos`, after) — `byte_range` is simply
  `start_i..vr.next_pos`, no new parsing needed.

`decode_packed_elems` (currently private to `packed.rs`) becomes
`pub(super)`, so `sink.rs` can call it independently of `render_packed`.

### §2 — Reusing `TextSink`'s output, no duplicated write logic

`render_packed` writes exactly one line per element, in element order,
with no other lines interleaved (confirmed: the only other line it can
emit is the single empty-record summary line, mutually exclusive with the
per-element loop — see `packed.rs`'s `pack_size == 0` branch).

`IndexingTextSink::scalar_field`'s `ScalarValue::Packed(data)` case:

1. Delegates to `self.inner.scalar_field(...)` exactly as it does today —
   `TextSink`'s rendered output stays byte-for-byte identical, no
   duplicated formatting/annotation logic in the indexing path.
2. Separately (before or after the delegated call — order doesn't matter,
   since this doesn't touch `self.inner` or `self.inner.out`), calls
   `decode_packed_elems(data, fs)` to obtain the same per-element list
   `render_packed` itself decoded, purely to read off each element's
   `byte_range` and the total count.
3. Zips the decoded elements 1:1 against the line range `text_start..
   text_end` measured around the delegated call (§2.1) — element `i`'s
   `text_range` is `text_start+i .. text_start+i+1` — and pushes one
   `NodeSpan` per element, `raw_range` translated to absolute coordinates
   the same way every other span already is (`base + payload_start +
   element.byte_range.start .. base + payload_start +
   element.byte_range.end`, where `payload_start = raw_range.end -
   data.len()`, matching `begin_nested`'s existing convention for
   locating a LEN field's payload start within its own `raw_range`).
4. **Empty record** (`pack_size == 0`, `decode_packed_elems` returns an
   empty `Vec`): falls back to today's single-`NodeSpan`-for-the-whole-
   field behavior (one span, covering the one summary line) — there are
   no elements to carry individual spans, and the field is still a
   legitimate, selectable row (matching every other zero/absent-value
   field today).
5. **Decode failure** (`decode_packed_elems` returns `Err`, i.e. the same
   condition under which `render_packed` itself falls back to
   `render_invalid`'s `INVALID_PACKED_RECORDS` rendering): also falls back
   to today's single-span behavior — `IndexingTextSink` has no reason to
   assume per-element structure exists when `TextSink` itself couldn't
   decode one.

This keeps `render_packed`'s formatting/annotation logic (record-level
`pack_size`/`tag_ohb`/`tag_oor`/`len_ohb` modifiers on the first element,
per-element `ohb`/`neg`/`nan_bits`/`ENUM_UNKNOWN` modifiers, `protoc`-style
value formatting) entirely untouched and un-duplicated — `sink.rs`'s new
code only ever reads `decode_packed_elems`'s output for byte-range
bookkeeping, never writes to `self.inner.out` itself.

### §3 — "No tag of its own" discriminator

Unlike every other `NodeSpan`-producing event, a packed element has no
wire tag (and no length prefix) of its own — its `raw_range` is a bare
value inside the record's shared LEN payload. `protolens`'s tag/length-
stripping helpers (`extract::message_payload_range`, consumed
unconditionally by `tui::display_range` as of spec 0114 §1.1's "payload-
only display" extension) assume every `raw_range` begins with a wire tag;
applied blindly to a packed-element span, the first payload byte would be
misparsed as a fake tag.

`NodeSpan` gains:

```rust
/// Absolute offset (same coordinate space as `raw_range`) of the
/// *enclosing packed record's own* tag, when this span is one element
/// of a packed-repeated field — `None` for every other node, including
/// non-packed scalars (which have their own tag) and the record's own
/// summary span in the empty/invalid fallback cases (§2.4/§2.5), which
/// again carry their own tag. Purely an internal discriminator (Non-
/// goals) — not surfaced in any user-visible rendering.
pub packed_record_start: Option<usize>,
```

Set to `Some(base + raw_range.start)` (the packed field's own tag start,
in the same absolute coordinates as every other `raw_range`) at every
per-element push (§2.3); `None` at every other `NodeSpan` push site,
including the empty/invalid packed-record fallback spans (§2.4/§2.5),
which — like any other scalar field — still start with the record's own
real tag.

`extract::message_payload_range`/`tui::display_range` consult it: `Some`
→ the span already *is* the payload (no tag/length to strip — return
`raw_range` unchanged); `None` → strip as today.

---

## Open Issues

1. **Status-line surfacing of `packed_record_start`** — e.g. showing "part
   of packed field at bytes[X..Y)" for a packed-element node — is
   plausible future UX but has no identified concrete need yet (Non-
   goals); revisit once one arises.
2. **Very large packed records** (thousands of elements): this spec adds
   one `TreeNode`/`NodeSpan` per element with no cap — consistent with
   every other field kind today (no existing size limit on sibling
   counts), but not yet load-tested against pathological cases.

---

## Files changed (anticipated)

| File | Change |
|---|---|
| `prototext-core/src/serialize/render_text/packed.rs` | `PackedElem` gains `byte_range: Range<usize>`, populated by both `decode_packed_fixed_elems` and `decode_packed_varint_elems`; `decode_packed_elems` becomes `pub(super)` |
| `prototext-core/src/serialize/render_text/sink.rs` | `NodeSpan` gains `packed_record_start: Option<usize>`; `IndexingTextSink::scalar_field`'s `ScalarValue::Packed` case pushes one `NodeSpan` per element (falling back to one span for the empty/invalid cases) instead of one span for the whole record |
| `protolens/src/extract.rs` | `message_payload_range` checks `packed_record_start`, returning the range unstripped when `Some` |
| `protolens/src/tui.rs` | `display_range` inherits the above via `message_payload_range`; no other change anticipated (packed-element nodes are already excluded from override targets via `is_message: false`) |
