<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# protoc --decode behavior on anomalous input

**Date:** 2026-03-23
**Scope:** `protoc --decode` behavior when given non-canonical or malformed
binary protobuf input; implications for `prototext --no-annotations`.

---

## 1. Entry point and general failure mechanism

`protoc --decode` calls `CommandLineInterface::EncodeOrDecode()`
(`compiler/command_line_interface.cc` ~line 3192), which calls
`message->ParsePartialFromZeroCopyStream(&in)` on a `DynamicMessage`.

When parsing returns false, protoc prints:

```
Failed to parse input.
```

to stderr and exits with status 1.  The parsing flows through
`TcParser::ParseLoop` → `TcParser::ReflectionFallback` →
`WireFormat::_InternalParse` → `WireFormat::_InternalParseAndMergeField`
(`wire_format.cc`).  Any null return from the inner parse functions
propagates upward as a parse failure.

---

## 2. Behavior by anomaly category

### 2.1 Non-canonical varints (overhanging bytes)

**Outcome: silently accepted.  Decoded value rendered.  Exit 0.**

Neither the new TcParser path (`parse_context.cc` `VarintParseSlow32` /
`VarintParseSlow64`) nor the old `CodedInputStream` path
(`coded_stream.cc` `VarintParseSlow32`) performs any canonicality check.
A varint such as `0x80 0x80 0x00` (three bytes encoding value 0) is
decoded to 0 and rendered identically to the canonical `0x00`.

A varint of 11 or more bytes (exceeding the 10-byte maximum for a
64-bit value) causes the decoder to return null/false → parse failure →
exit 1.

### 2.2 Unknown field numbers

**Outcome: rendered by field number.  Exit 0.**

When `field == nullptr` (field number not in schema),
`WireFormat::_InternalParseAndMergeField` (`wire_format.cc` ~line 828)
calls `UnknownFieldParse`, storing the data in the message's
`UnknownFieldSet`.  `TextFormat::Print` then calls
`PrintUnknownFields` (`text_format.cc` ~line 2446) which renders them:

| Wire type | Output format |
|-----------|--------------|
| VARINT (0) | `N: <unsigned decimal>` |
| FIXED64 (1) | `N: 0x<16 lowercase hex digits>` |
| LENGTH_DELIMITED (2) | `N { ... }` if payload re-parses as message; else `N: "<CEscaped>"` |
| FIXED32 (5) | `N: 0x<8 lowercase hex digits>` |
| GROUP (3/4) | `N { <nested unknown fields> }` |

### 2.3 Unknown enum values

**Outcome: rendered.  Exit 0.  Details differ between proto2 and proto3.**

The decision point is `field->legacy_enum_field_treated_as_closed()`
(`generated_message_reflection.cc` ~line 1758).

**Proto3 / open enum:** the integer is stored directly in the field.
At print time (`text_format.cc` ~line 2807), `FindValueByNumber` returns
null and `PrintEnum(value, absl::StrCat(value), generator)` is called —
the integer is rendered in the field's named position:

```
my_field: 42
```

**Proto2 / closed enum:** the integer is moved to `MutableUnknownFields()`
as a VARINT unknown field and rendered by field number:

```
5: 42
```

### 2.4 Wire type mismatch

**Outcome: consumed as unknown field, rendered by field number.  Exit 0.**

`WireFormat::_InternalParseAndMergeField` (`wire_format.cc` ~line 834):
if the wire type in the tag does not match the schema's expected wire
type, and the field is not a packable-via-LEN case, the code calls
`UnknownFieldParse(tag, reflection->MutableUnknownFields(msg), ptr, ctx)`.
The bytes are consumed according to the *actual* wire type on the stream,
stored as an unknown field, and rendered as described in §2.2.

Example: schema declares `int32` (varint expected), wire carries FIXED32.
The four bytes are consumed as a FIXED32 unknown field:

```
5: 0x0000002a
```

### 2.5 Truncated LENGTH-DELIMITED fields

**Outcome: parse failure.  Exit 1.**

If the declared length extends beyond the available buffer,
`ReadString` / `ReadSize` in the EpsCopyInputStream machinery returns
null (`parse_context.h` ~line 198), which propagates as a parse failure.
Same in the old `CodedInputStream` path (`coded_stream.cc` ~line 256).

### 2.6 Invalid packed records (truncated varint in packed payload)

**Outcome: parse failure.  Exit 1.**

`_InternalParseAndMergeField` handles packed fields via
`HANDLE_PACKED_TYPE` macros (~lines 841–864 in `wire_format.cc`).
These iterate reading varints until the length-delimited boundary.  If a
varint read returns null (truncated mid-byte), null propagates up as a
parse failure.

### 2.7 Invalid UTF-8 in a string field

**Outcome: depends on proto2 vs proto3.**

The decision is `field->requires_utf8_validation()`, which returns true
for proto3 `string` fields (`IsStrictUtf8`, `descriptor.cc` ~line 4253).

**Proto3 string:** `WireFormat::_InternalParseAndMergeField`
(`wire_format.cc` ~lines 997–1005) calls
`WireFormatLite::VerifyUtf8String`, which calls
`utf8_range::IsStructurallyValid`.  On failure it logs an
`ABSL_LOG(ERROR)` message and returns null → parse failure.  **Exit 1.**

**Proto2 string:** UTF-8 validation is skipped; the raw bytes are stored
and printed using `HardenedPrintString` (`text_format.cc` ~line 2309),
which detects invalid sequences and escapes them with `absl::CEscape`
(3-digit octal).  **Exit 0.**

### 2.8 Out-of-range field numbers

**Field number 0: parse failure.  Exit 1.**

`parse_context.h` ~line 1601 asserts `number != 0` and returns nullptr
on failure.  Old path: `wire_format.cc` ~line 71 returns false.

**Field number > 536870911: structurally impossible.**

A tag is read as a `uint32_t` varint.  `field_number = tag >> 3`.
The maximum `uint32_t` is 4 294 967 295; shifted right 3 bits = 536 870 911,
which is the maximum valid field number.  No separate validation is needed;
the constraint is enforced by the 32-bit tag representation.

### 2.9 Mismatched group end tags

**Outcome: parse failure.  Exit 1.**

New path: `ParseGroupInlined` (`parse_context.h` ~lines 1287–1301) calls
`ConsumeEndGroup(start_tag)`, which checks that the end tag's field
number matches the start tag's field number.  A mismatch returns false →
parse failure.

Old path: `wire_format.cc` ~lines 107–112 checks
`LastTagWas(MakeTag(GetTagFieldNumber(tag), WIRETYPE_END_GROUP))`.

### 2.10 Unclosed groups (EOF without matching end tag)

**Outcome: parse failure.  Exit 1.**

When EOF is reached inside a group, the inner parse loop exits with tag
= 0 (end-of-stream sentinel, `parse_context.h` ~line 304).
`ConsumeEndGroup(start_tag)` receives `last_tag_minus_1_ = 1`, which
never equals `start_tag` (minimum 3 for any valid field number) →
parse failure.

---

## 3. Summary table

| Anomaly | protoc behavior | Exit |
|---------|----------------|------|
| Non-canonical varint (overhanging bytes) | Decoded silently; value rendered | 0 |
| Unknown field number | Rendered by number (decimal/hex/string/nested) | 0 |
| Unknown enum value — proto3/open | Rendered as integer in named field position | 0 |
| Unknown enum value — proto2/closed | Rendered as unknown varint by field number | 0 |
| Wire type mismatch | Consumed as unknown field; rendered by number | 0 |
| Truncated LENGTH-DELIMITED field | `"Failed to parse input."` on stderr | 1 |
| Truncated varint in packed payload | `"Failed to parse input."` on stderr | 1 |
| Invalid UTF-8 — proto3 string | `"Failed to parse input."` + `ABSL_LOG(ERROR)` | 1 |
| Invalid UTF-8 — proto2 string | Rendered with `\NNN` octal escaping | 0 |
| Field number 0 | `"Failed to parse input."` on stderr | 1 |
| Field number > 536870911 | Structurally impossible from a 32-bit tag | N/A |
| Mismatched group end tag | `"Failed to parse input."` on stderr | 1 |
| Unclosed group | `"Failed to parse input."` on stderr | 1 |

---

## 4. Implications for `prototext --no-annotations`

### 4.1 Goal

`prototext --no-annotations` should produce output that is **byte-for-byte
identical to `protoc --decode`** for any input that protoc accepts
(exit 0).  For inputs that protoc rejects (exit 1), prototext should
match protoc's failure behavior: print an error to stderr and exit 1.

This gives `--no-annotations` a clear, testable contract and a
self-documenting name: the flag means "give me protoc output."

### 4.2 Consequences for the decoder

The exit-0 anomalies are already handled correctly by prototext (it
renders non-canonical varints, unknown fields, wire-type mismatches, and
unknown enums).  The only format change needed is matching the exact
protoc output format for those cases — which is the work already scoped
in `docs/specs/0010-protoc-compatibility.md` (per-line packed rendering,
NaN bits in annotation).

The exit-1 anomalies are currently rendered by prototext with `INVALID_*`
markers:

| prototext marker | protoc equivalent |
|-----------------|-------------------|
| `TRUNCATED_BYTES` | exit 1 |
| `INVALID_PACKED_RECORDS` | exit 1 |
| `INVALID_STRING` | exit 1 (proto3) / rendered (proto2) |
| `INVALID_TAG_TYPE` | exit 1 (field number 0 or reserved wire type) |
| `INVALID_GROUP_END` / open group | exit 1 |

In `--no-annotations` mode, when prototext encounters any of these
conditions it should **exit 1 with an error message** rather than
rendering an `INVALID_*` line.  This matches protoc's behavior exactly.

### 4.3 Consequences for the header and comment-only lines

Two prototext-specific output elements must be suppressed in
`--no-annotations` mode to match protoc:

- The `#@ prototext: protoc` header line — protoc emits no header.
- Comment-only annotation lines for empty packed records
  (`#@ repeated int32 [packed=true] = 85; pack_size: 0`) — protoc
  emits nothing for empty packed records.

### 4.4 Non-canonical input that protoc accepts silently

Protoc accepts non-canonical varints (overhanging bytes) and renders the
decoded value.  Prototext does the same in `--no-annotations` mode.
However, with annotations enabled, prototext records the `ohb` count in
the annotation for lossless round-trip.  In `--no-annotations` mode the
`ohb` information is simply discarded, matching protoc's output.

### 4.5 Implementation order

1. Implement spec 0010 change A (per-line packed rendering) — prerequisite
   for correct `--no-annotations` output on packed fields.
2. Implement spec 0010 change B (NaN bits in annotation) — prerequisite
   for correct `--no-annotations` output on NaN values.
3. Add `--no-annotations` mode changes:
   - Suppress the header line.
   - Suppress comment-only lines (empty packed records).
   - Exit 1 on `INVALID_*` conditions.

---

## 5. References

- `src/google/protobuf/compiler/command_line_interface.cc` — `--decode` entry point
- `src/google/protobuf/wire_format.cc` — `_InternalParseAndMergeField`
- `src/google/protobuf/parse_context.h` / `parse_context.cc` — TcParser path
- `src/google/protobuf/io/coded_stream.cc` — old CodedInputStream path
- `src/google/protobuf/text_format.cc` — `PrintUnknownFields`, `PrintEnum`
- `src/google/protobuf/generated_message_reflection.cc` — enum handling
- `src/google/protobuf/descriptor.cc` — `IsStrictUtf8`, `requires_utf8_validation`
- `abseil-cpp/absl/strings/escaping.cc` — `CEscape` for string rendering
- `docs/protoc-decode-compatibility.md` — known divergences between prototext and protoc
- `docs/specs/0010-protoc-compatibility.md` — planned fixes
