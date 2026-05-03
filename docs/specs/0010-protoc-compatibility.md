<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0010 — protoc --decode compatibility for canonical wire input

**Status:** implemented
**Implemented in:** 2026-03-24
**App:** prototext

---

## Problem

`prototext` aims to be a superset of `protoc --decode`: for canonical
wire-level input (no anomalies), its output should be byte-for-byte identical
to `protoc --decode` (modulo the annotation comment, which protoc does not
emit).

Two known divergences exist today:

### Divergence A — packed repeated fields

`protoc --decode` renders a packed repeated field as **N separate lines**, one
per element, identical to non-packed repeated fields:

```
int32Pk: 1
int32Pk: 2
int32Pk: 3
```

`prototext` renders packed fields using a bracket array syntax:

```
int32Pk: [1, 2, 3]  #@ repeated int32 [packed=true] = 85
```

This is a fundamental output format difference.

### Divergence B — non-canonical NaN values

`protoc --decode` renders all NaN values (canonical or not) as bare `nan`.

`prototext` currently renders non-canonical NaNs as `nan(0x…)`, which is not
valid protoc syntax and would be rejected by `protoc --encode`.

For **canonical** wire input, NaN values are always the canonical quiet NaN
(`0x7FC00000` for float, `0x7FF8000000000000` for double), which already
renders as bare `nan` — so divergence B does not affect canonical input.
It only matters for non-canonical NaN payloads, which are anomalies by
definition.

---

## Goals

- For canonical wire input, `prototext` output (without annotations) is
  byte-for-byte identical to `protoc --decode` output.
- Lossless round-trip is preserved for all wire input (canonical and
  non-canonical) when annotations are enabled.
- The annotation continues to carry all information needed for lossless
  re-encoding.

## Non-goals

- Matching `protoc --decode` output for non-canonical input (protoc cannot
  decode anomalies; this is out of scope by definition).
- Implementing `protoc --encode` compatibility on the encoding side (a
  separate concern).

---

## Proposal

### A. Packed field rendering

#### A.1 Decoder (wire → text)

Render each element of a packed field on its own line, using the field name,
exactly as protoc does:

```
int32Pk: 1
int32Pk: 2
int32Pk: 3
```

Every element line carries its own annotation with the full field declaration.
Each annotation also carries any **element-level** anomaly modifiers that
apply to that specific element (`ohb`, `neg`, `nan_bits`, etc.):

```
int32Pk: 1  #@ repeated int32 [packed=true] = 85
int32Pk: 2  #@ repeated int32 [packed=true] = 85; ohb: 3
int32Pk: 3  #@ repeated int32 [packed=true] = 85
```

#### A.2 Wire record boundaries and `pack_size`

A protobuf message may contain **multiple consecutive packed wire records**
for the same field number.  Each is a distinct `(tag, LEN, payload)` triplet
on the wire.  The per-line format is ambiguous about boundaries: three lines
for `int32Pk` could be one record of three elements, three records of one
element each, or any other split.

To preserve this information and enable lossless round-trip, the **first
element of each wire record** carries a `pack_size: N` modifier, where N is
the number of elements in that record:

```
int32Pk: 1  #@ repeated int32 [packed=true] = 85; pack_size: 3
int32Pk: 2  #@ repeated int32 [packed=true] = 85
int32Pk: 3  #@ repeated int32 [packed=true] = 85
int32Pk: 4  #@ repeated int32 [packed=true] = 85; pack_size: 2
int32Pk: 5  #@ repeated int32 [packed=true] = 85
```

This represents two wire records: `[1, 2, 3]` and `[4, 5]`.

**Record-level** anomaly modifiers (`tag_ohb`, `TAG_OOR`, `len_ohb`) apply
to the wire tag and LEN varint of a single record.  They are placed on the
**first element** of that record (alongside `pack_size`):

```
int32Pk: 1  #@ repeated int32 [packed=true] = 85; pack_size: 3; tag_ohb: 2; len_ohb: 1
int32Pk: 2  #@ repeated int32 [packed=true] = 85
int32Pk: 3  #@ repeated int32 [packed=true] = 85
```

#### A.3 Empty packed records

An empty packed wire record (`tag + len=0`) contains no elements, so there
is no value line to carry the annotation.  It is rendered as a
**comment-only annotation line**:

```
#@ repeated int32 [packed=true] = 85; pack_size: 0
```

Record-level anomaly modifiers go on this comment-only line as well:

```
#@ repeated int32 [packed=true] = 85; pack_size: 0; tag_ohb: 1
```

The encoder recognises a `pack_size: 0` comment-only line and emits
`tag + len=0` with any accompanying anomaly modifiers.

**Special case: `INVALID_PACKED_RECORDS`.**  If the packed payload cannot be
decoded (truncated varint, etc.), the field is rendered as a single
`INVALID_PACKED_RECORDS` line as today — no change.

#### A.4 Encoder (text → wire)

The encoder uses `pack_size: N` on the first element line (or the
comment-only line for empty records) to know exactly how many element lines
to consume for each wire record.

For the example above:

```
int32Pk: 1  #@ repeated int32 [packed=true] = 85; pack_size: 3
int32Pk: 2  #@ repeated int32 [packed=true] = 85
int32Pk: 3  #@ repeated int32 [packed=true] = 85
int32Pk: 4  #@ repeated int32 [packed=true] = 85; pack_size: 2
int32Pk: 5  #@ repeated int32 [packed=true] = 85
```

→ two wire records: tag `(85<<3)|2` len=3 `[0x01,0x02,0x03]`, then
tag `(85<<3)|2` len=2 `[0x04,0x05]`.

The encoder does not need lookahead: `pack_size` tells it exactly how many
lines to buffer before flushing a record.  A comment-only line with
`pack_size: 0` flushes a zero-element record immediately.

**Ordering invariant:** All elements of a packed field are contiguous within
a wire record.  The encoder may assert this.

#### A.5 Impact on existing tests and fixtures

All existing packed fixture files must be regenerated to use the new per-line
format.  The `craft_a_matches_committed_fixtures` test (spec 0009) will catch
any divergence.

Existing round-trip tests that construct packed wire by hand
(`float_packed_noncanonical_nan_roundtrips`, etc.) continue to work — the
round-trip is wire→text→wire regardless of text format.

---

### B. Non-canonical NaN rendering

#### B.1 Current behaviour (recap)

Non-canonical NaN → value token `nan(0x7f800001)` (float) or
`nan(0xfff8000000000000)` (double).

Canonical NaN → value token `nan`.

#### B.2 Proposed change

Move the non-canonical NaN bit pattern from the **value token** into the
**annotation**:

```
floatOp: nan  #@ float = 22; nan_bits: 0x7f800001
```

For canonical NaN:

```
floatOp: nan  #@ float = 22
```

For a packed array with mixed NaNs:

```
floatPk: nan  #@ repeated float [packed=true] = 87
floatPk: nan  #@ repeated float [packed=true] = 87; nan_bits: 0x7f800001
floatPk: nan  #@ repeated float [packed=true] = 87; nan_bits: 0xffc00000
```

The value token is always bare `nan` — identical to protoc output.

The `nan_bits` modifier in the annotation carries the full 32-bit (float) or
64-bit (double) bit pattern, formatted as lower-case hex with `0x` prefix.

#### B.3 Encoder

The encoder parses `nan_bits: 0x…` from the annotation when the value is
`nan` and the field type is `float` or `double`, and uses the bit pattern
directly (with exponent forced to all-ones, as today).

When no `nan_bits` modifier is present, bare `nan` encodes as the canonical
quiet NaN (current behaviour).

#### B.4 Packed arrays

Per element: if a `nan_bits` modifier is present on the element's annotation
line, it applies to that element's encoding.

---

## Feasibility assessment

### A. Packed field rendering — feasibility

**Decoder side: moderate effort.**

The current `render_packed()` function produces a single line with `[...]`
syntax.  Replacing it with N individual scalar lines requires:
- Iterating over decoded elements one by one (already done internally).
- Calling `render_scalar()` for each element instead of accumulating into a
  bracket string.
- Emitting `pack_size: N` on the first element's annotation and record-level
  anomaly modifiers (`tag_ohb`, `len_ohb`) alongside it.
- Emitting element-level anomaly modifiers (`ohb`, `neg`, `nan_bits`) on
  each respective element's annotation.
- For empty records: emitting a comment-only annotation line with `pack_size: 0`.

The `decode_packed_to_str()` function currently returns a `String`.  It
would need to be restructured to return a `Vec` of `(value_str, modifiers)`
pairs, plus a record-level modifiers struct for the first element.  This is
a moderate refactor of ~100 lines.

**Encoder side: moderate effort** (reduced from "significant" by `pack_size`).

`pack_size` eliminates the need for lookahead or a two-pass approach.  The
encoder reads `pack_size: N` off the first element (or comment-only line),
buffers exactly N element lines, then flushes one wire record.  The main
loop structure is unchanged; the new logic is a small state machine that
activates when a `[packed=true]` annotation with `pack_size` is encountered.

Steps:
1. Detect `pack_size: N` on a line with `[packed=true]`.
2. If N=0 (comment-only line): emit `tag + len=0` immediately.
3. If N>0: buffer N element lines, then emit the packed wire record.

**Risk:** moderate.  The encoder change is well-scoped thanks to `pack_size`.
Existing tests will catch regressions.

**Interaction with `INVALID_PACKED_RECORDS`:** the current single-line
fallback is unaffected — it stays as a single bytes-literal line.

**Empty packed fields:** rendered as a comment-only line with `pack_size: 0`.
The encoder emits `tag + len=0` when it encounters this line.

**Performance note:** At implementation time, carefully assess the impact on
the encoder's and decoder's performance properties:

- **Decoder:** the current packed decoder is single-pass and allocation-free
  for the output path.  Switching to per-line output requires buffering N
  `(value_str, modifiers)` pairs before emitting — this introduces a
  per-record allocation.  Evaluate whether the `Vec` can be replaced by an
  iterator that yields lines one at a time to preserve the single-pass,
  low-allocation character.

- **Encoder:** buffering N element lines before flushing a wire record
  requires holding N parsed values in memory simultaneously.  For large
  packed arrays this could be significant.  Evaluate whether the wire output
  (tag + length prefix + payload) can be written in two passes over the same
  input slice (first to compute the payload length, second to emit bytes)
  rather than accumulating a `Vec` of decoded values.

### B. Non-canonical NaN in annotation — feasibility

**Low effort.**

This is essentially a revert of spec 0008's value-token approach, replacing
it with an annotation modifier.

**Decoder side:** in `format_float_protoc` / `format_double_protoc`, change
the non-canonical NaN branch to always return `"nan"`, and pass the bit
pattern up to the annotation layer instead.  The `render_scalar` call site
gains an optional `nan_bits` argument.

**Encoder side:** in `parse_num`, bare `nan` always produces canonical NaN.
The annotation parser gains a `nan_bits` key whose value overrides the bit
pattern.

**Packed arrays:** the per-element annotation already carries modifiers
(see A.1 above).  A `nan_bits` modifier per element fits naturally.

**Key concern previously identified:** the annotation approach does not work
for packed arrays when they are rendered as a single `[…]` line — there is
no per-element annotation slot.  **This concern goes away** if A (per-line
packed rendering) is implemented first, since each element then has its own
annotation.

**Conclusion:** B depends on A.  B alone (without A) reintroduces the
packed NaN problem.  The two changes must be implemented together.

---

## Summary

| Change | Compatibility impact | Effort | Depends on |
|--------|---------------------|--------|------------|
| A. Per-line packed rendering | Restores protoc compatibility for packed fields | Moderate (decoder) + Moderate (encoder) | — |
| B. NaN bits in annotation | Restores protoc compatibility for canonical NaN; no impact on canonical input | Low | A |

**Recommended order:** implement A first, then B.  Neither is a blocker for
other work; both can be deferred until the test infrastructure from spec 0009
is in place, since that suite will make regressions visible immediately.

---

## Open questions

1. **Interaction with `--no-annotations` mode.**  Without annotations, packed
   fields become indistinguishable from non-packed repeated fields in the
   text output, and wire record boundaries are lost.  The encoder has no way
   to know whether to emit a packed or non-packed wire encoding, nor how to
   split elements across records.  This is an existing limitation (not
   introduced by this spec) and is acceptable: without annotations,
   lossless round-trip is only guaranteed for canonical input, and canonical
   packed fields re-encode correctly as a single packed record if the schema
   says `[packed=true]`.

---

## References

- `prototext-core/src/serialize/render_text/packed.rs` — current packed renderer
- `prototext-core/src/serialize/render_text/helpers.rs` — `render_scalar`,
  `AnnWriter`
- `prototext-core/src/serialize/encode_text/mod.rs` — encoder main loop,
  `encode_packed_array_line`
- `prototext-core/src/serialize/common.rs` — `format_float_protoc`,
  `format_double_protoc`
- `prototext/fixtures/cases/test_varint_packed.pb` — current packed fixture
- `prototext/fixtures/cases/canonical_repeated.pb` — protoc repeated format
  reference
- docs/specs/0008-nan-encoding.md — NaN encoding (to be revised by this spec)
- docs/specs/0009-protocraft-and-e2e-tests.md — test infrastructure
