<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0007 — String and bytes field encoding policy

**Status:** implemented
**App:** prototext
**Implemented in:** 2026-03-11

## Problem

Proto text format uses C-style string escaping for both `string` and `bytes`
fields.  The two types have different semantics at the wire level:

- A `string` field contains valid UTF-8 text.
- A `bytes` field contains an arbitrary byte sequence.

Both are rendered as quoted string literals in the text format, but the correct
escaping strategy differs.  Additionally, `protoc --decode` makes specific
choices about how to render non-ASCII content that do not always match what a
human reader would prefer.

This spec documents the escaping policy for both field types and the deliberate
divergence from `protoc --decode`.

---

## Specification

### 1. Escaping rules in the decoder (wire → text)

#### 1.1 `bytes` fields

Every byte is escaped according to its numeric value, regardless of whether the
byte sequence forms valid UTF-8:

| Byte value | Emitted form |
|---|---|
| `\` (0x5C) | `\\` |
| `"` (0x22) | `\"` |
| `'` (0x27) | `\'` |
| `\n` (0x0A) | `\n` |
| `\r` (0x0D) | `\r` |
| `\t` (0x09) | `\t` |
| 0x20–0x7E (printable ASCII, excl. above) | literal byte |
| all others (0x00–0x1F, 0x7F–0xFF) | `\NNN` (3-digit octal) |

This matches `protoc --decode` exactly for bytes fields.

#### 1.2 `string` fields — deliberate divergence from `protoc --decode`

`protoc --decode` applies byte-level escaping to string fields too,
octal-escaping every byte ≥ 0x80.  For a field containing `"café"`
(UTF-8 `63 61 66 C3 A9`), protoc emits `"caf\303\251"`.

`prototext` **intentionally diverges**: multi-byte UTF-8 sequences are emitted
as raw UTF-8, not as octal escapes.  The same field is rendered as `"café"`.

The precise escaping policy for string fields is:

| Byte / sequence | Emitted form |
|---|---|
| `\` (0x5C) | `\\` |
| `"` (0x22) | `\"` |
| `\n` (0x0A) | `\n` |
| `\r` (0x0D) | `\r` |
| `\t` (0x09) | `\t` |
| 0x00–0x1F (other control chars) | `\NNN` (3-digit octal) |
| 0x7F (DEL) | `\NNN` (3-digit octal) |
| 0x20–0x7E (printable ASCII, excl. above) | literal byte |
| multi-byte UTF-8 sequence (0xC2–0xFF lead byte) | raw UTF-8 bytes |

Note: control characters (0x00–0x1F) and DEL (0x7F) are technically valid
UTF-8 single-byte code points, but they are unprintable and octal-escaped for
readability, matching protoc.  The divergence from protoc applies only to
multi-byte UTF-8 sequences (code points U+0080 and above).

Rationale:
- String fields are defined by the proto spec to contain valid UTF-8.
  Rendering multi-byte sequences as raw UTF-8 is lossless and human-readable.
- `protoc --encode` accepts raw UTF-8 in string fields, so the round-trip
  invariant `wire → text → wire'` is preserved.
- Octal-escaping every non-ASCII byte in a UTF-8 string produces output that
  is harder to read and diff, with no correctness benefit.

If the wire bytes of a `string` field are not valid UTF-8, `prototext` emits an
`INVALID_STRING` anomaly (matching protoc behaviour).

### 2. Unescaping rules in the encoder (text → wire)

The encoder receives the text produced by the decoder (or hand-written text in
the same format).  It must invert the escaping faithfully.

A quoted string literal is unescaped by interpreting escape sequences as raw
byte values:

| Escape sequence | Byte value |
|---|---|
| `\n` | 0x0A |
| `\r` | 0x0D |
| `\t` | 0x09 |
| `\"` | 0x22 |
| `\'` | 0x27 |
| `\\` | 0x5C |
| `\NNN` (1–3 octal digits) | value of the octal number (0–255) |
| `\xHH` (2 hex digits) | value of the hex number (0–255) |
| any other char `c` | the UTF-8 encoding of `c` |

The last rule handles raw UTF-8 multi-byte sequences that appear in string
field values (see §1.2): `é` (U+00E9, bytes `C3 A9`) in the text is passed
through as the two bytes `0xC3 0xA9`, which is the correct wire encoding.

The Rust encoder operates on raw bytes throughout, with no intermediate `str`
conversion, so byte values ≥ 0x80 are handled correctly by the generic
fall-through rule above.

### 3. Summary table

| Field type | Decoder (wire→text) | Encoder (text→wire) |
|---|---|---|
| `bytes` | byte-level octal escape for all non-printable-ASCII | `unescape_bytes` → raw bytes |
| `string` (printable ASCII) | literal bytes | raw bytes |
| `string` (control chars, DEL) | octal-escape (matches protoc) | octal unescaped to byte value |
| `string` (multi-byte UTF-8, U+0080+) | **raw UTF-8 bytes** (diverges from protoc) | raw bytes recovered from UTF-8 sequence |
| `string` (invalid UTF-8) | `INVALID_STRING` anomaly | N/A (anomaly path) |
| unknown `bytes` wire type | byte-level octal escape | `unescape_bytes` → raw bytes |

---

## References

- `prototext-core/src/serialize/common.rs` — `escape_bytes_into`,
  `escape_string_into`
- `prototext-core/src/serialize/render_text/mod.rs` — decoder, string/bytes
  branch at `render_len_delimited`
- `prototext-core/src/serialize/encode_text/mod.rs` — Rust encoder,
  `unescape_bytes`
