<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0008 — NaN encoding for float and double fields

**Status:** implemented
**Implemented in:** 2026-03-24
**App:** prototext

---

## Problem

IEEE 754 NaN values are not a single value — they are a family of bit
patterns.  For a 32-bit float, any bit pattern where the exponent is
all-ones (bits 30–23 = 0xFF) and the mantissa is non-zero is a NaN.
For a 64-bit double, the exponent is bits 62–52 = 0x7FF.

The bits that vary across NaN patterns are:

| Field | f32 bits | f64 bits | Notes |
|---|---|---|---|
| sign | 31 (1 bit) | 63 (1 bit) | No standard meaning for NaN |
| quiet/signaling | 22 (1 bit) | 51 (1 bit) | 1 = quiet, 0 = signaling |
| payload | 21–0 (22 bits) | 50–0 (51 bits) | Arbitrary user payload |

The exponent field (all-ones) is fully determined and carries no
information.

Currently `prototext` collapses all NaN patterns to the single token
`nan` on output, and parses `nan` back as Rust's canonical quiet NaN
(f32: `0x7FC00000`, f64: `0x7FF8000000000000`).  This means that any
NaN with a non-canonical bit pattern does not survive a wire → text →
wire round-trip.

---

## Goals

- Preserve the full NaN bit pattern across a wire → text → wire
  round-trip.
- Keep the output human-readable.
- Keep bare `nan` valid as a shorthand for canonical quiet NaN.

## Non-goals

- Changing the encoding of normal finite values, ±Inf, or ±0.
- Interpreting the semantics of NaN payloads.
- Compatibility with `protoc --decode` / `protoc --encode` for NaN
  variants (protoc has the same lossy behaviour).

---

## Specification

### 1. Text representation of NaN values

#### 1.1 Canonical quiet NaN

The canonical quiet NaN for each type is defined as the bit pattern
produced by Rust's `f32::NAN` / `f64::NAN`:

- f32 canonical quiet NaN: `0x7FC00000`
- f64 canonical quiet NaN: `0x7FF8000000000000`

The canonical quiet NaN is rendered as the bare token `nan` (no
modifier).

#### 1.2 Non-canonical NaN

Any NaN whose bit pattern differs from the canonical quiet NaN is
rendered as:

```
nan(0xHHHHHHHH)        # float  — 8 hex digits, zero-padded
nan(0xHHHHHHHHHHHHHHHH)  # double — 16 hex digits, zero-padded
```

The hex value is the full 32-bit or 64-bit word as produced by
`f32::to_bits()` / `f64::to_bits()`, in lower-case hex with the `0x`
prefix.  The exponent bits are included in the word (they are always
all-ones for a NaN) for directness: the hex value can be passed
straight to `f32::from_bits()` / `f64::from_bits()` without
reconstruction.

Examples (f32):

| Bit pattern | Text |
|---|---|
| `0x7FC00000` | `nan` |
| `0xFFC00000` | `nan(0xffc00000)` |
| `0x7F800001` | `nan(0x7f800001)` |
| `0x7FC0CAFE` | `nan(0x7fc0cafe)` |

#### 1.3 Annotations

When annotations are enabled, a NaN with modifier is annotated with
its field name and type in the normal way, just like any other scalar.
The modifier is part of the value token, not the annotation.

### 2. Decoder changes (wire → text)

In `format_float_protoc` and `format_double_protoc` in
`prototext-core/src/serialize/common.rs`:

- If the value is NaN **and** equals the canonical quiet NaN
  (`v.to_bits() == f32::NAN.to_bits()` / `f64::NAN.to_bits()`),
  emit `nan`.
- If the value is NaN **and** does not equal the canonical quiet NaN,
  emit `nan(0x{:08x})` (f32) or `nan(0x{:016x})` (f64) using the
  full bit pattern.

The existing `v.is_nan()` → `"nan"` path is split into these two
cases.

### 3. Encoder changes (text → wire)

In `parse_num` in
`prototext-core/src/serialize/encode_text/mod.rs`:

- Bare `nan` continues to produce `f64::NAN` (canonical quiet NaN),
  as today.
- `nan(0xHH…)` is parsed as follows:
  1. Strip the `nan(` prefix and `)` suffix.
  2. Parse the hex literal (with `0x` prefix) as a `u64`.
  3. Validate that the bit pattern is actually a NaN (exponent
     all-ones, mantissa non-zero).  If not, return a parse error.
  4. For a `float` field: require the value fits in 32 bits (i.e.,
     the upper 32 bits are zero); call `f32::from_bits(value as u32)`
     and store as `Num::Float(v as f64)`.
  5. For a `double` field: call `f64::from_bits(value)` and store as
     `Num::Float(v)`.

The `parse_num` function currently returns `Option<Num>`; the new
`nan(…)` branch should return `None` on a malformed modifier (bad
hex, non-NaN bit pattern, wrong width) so the caller can emit a parse
error in the usual way.

### 4. Packed repeated fields

Packed float/double arrays use the same text tokens as scalars.
A packed array may mix bare `nan` and `nan(0x…)` elements:

```
colors: [1.0, nan, nan(0x7fc0cafe), -1.5]
```

The decoder emits `nan(0x…)` for any non-canonical NaN element.
The encoder parses each element with the same `parse_num` logic.

### 5. Schema-less (unknown field) rendering

When no schema is available and the wire type is FIXED32 (wire type 5)
or FIXED64 (wire type 1), `prototext` renders the value as a hex
literal today.  This path does not involve `format_float_protoc` /
`format_double_protoc` and is **not changed** by this spec.  The NaN
modifier syntax applies only when the schema identifies the field as
`float` or `double`.

---

## Examples

### f32 field, signaling NaN with payload 1

Wire bytes: `01 00 80 7F` (little-endian `0x7F800001`)

```
# annotations disabled
temperature: nan(0x7f800001)

# annotations enabled
temperature: nan(0x7f800001)  # float(0x7f800001)
```

Re-encoding produces `01 00 80 7F` exactly.

### f64 field, negative quiet NaN

Wire bytes: `00 00 00 00 00 00 F8 FF` (little-endian `0xFFF8000000000000`)

```
ratio: nan(0xfff8000000000000)
```

Re-encoding produces `00 00 00 00 00 00 F8 FF` exactly.

### f32 field, canonical quiet NaN

Wire bytes: `00 00 C0 7F` (little-endian `0x7FC00000`)

```
temperature: nan
```

Re-encoding produces `00 00 C0 7F` exactly.

---

## References

- `prototext-core/src/serialize/common.rs` — `format_float_protoc`,
  `format_double_protoc`
- `prototext-core/src/serialize/encode_text/mod.rs` — `parse_num`,
  `encode_num`
- `prototext-core/src/serialize/render_text/packed.rs` — packed
  float/double rendering
- IEEE 754-2019 §6.2 — NaN encodings
