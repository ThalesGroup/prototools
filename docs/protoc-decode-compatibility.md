<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# protoc --decode output format: reverse specification and compatibility assessment

**Date:** 2026-03-23
**Scope:** `protoc --decode` text output (canonical wire input, schema present)

This document reverse-engineers the exhaustive text output syntax of
`protoc --decode`, identifies every point where `prototext` currently
diverges, and classifies each divergence by severity.

---

## 1. Reference implementation

`protoc --decode` invokes `TextFormat::Print(*message, &out)` using a
**default-constructed `Printer`** (protobuf `text_format.cc`, entry point
`command_line_interface.cc` ~line 3192).  The default printer uses:

- `DebugStringFieldValuePrinter` — strings and bytes escaped with
  `absl::CEscape()`, not UTF-8-safe.
- `use_field_number_ = false` — known fields rendered by name, not number.
- `use_short_repeated_primitives_ = false` — repeated fields always one
  element per line, never bracket notation.
- Multi-line mode (default) — one field per line, 2-space indentation.

---

## 2. Exhaustive syntax rules

### 2.1 Header

`protoc --decode` emits **no header line** before the message body.

`prototext` emits `#@ prototext: protoc\n` as the first line.

> **Divergence D1 — prototext-specific header.**
> Acceptable: the header is a prototext extension.  `--no-annotations` does
> not suppress it (the header is always emitted).  Impact on protoc
> compatibility: the header line is present in prototext output but absent
> from protoc output.  Not a concern for the protoc-superset goal (which
> only requires that the body fields match), but worth documenting.

---

### 2.2 Field name vs field number

| Situation | protoc output | prototext output |
|-----------|--------------|-----------------|
| Known field | `field_name: value` | `field_name: value` ✓ |
| Extension field | `[pkg.ExtName]: value` | (unknown — see below) |
| Unknown field (no schema match) | `field_number: ...` | `field_number: ...` ✓ |

Unknown fields without annotations are **silently skipped** by prototext
(`render_len_field` returns early when `annotations=false`).

> **Divergence D2 — unknown fields suppressed without annotations.**
> `protoc --decode` always renders unknown fields (by field number).
> `prototext --no-annotations` silently drops them.
> This is a known, documented limitation.

---

### 2.3 Separator

Both use `: ` (colon + space) between field name/number and scalar value.
Message and group fields use ` {` (space + open brace, no colon) in both.

**No divergence.**

---

### 2.4 Indentation

Both use 2 spaces per nesting level.

**No divergence.**

---

### 2.5 Integer rendering

All integer wire types render as **signed or unsigned decimal**:

| Type | Sign | protoc | prototext |
|------|------|--------|-----------|
| `int32`, `int64` | signed | `-42` | `-42` ✓ |
| `uint32`, `uint64` | unsigned | `42` | `42` ✓ |
| `sint32`, `sint64` | signed (zigzag-decoded) | `-42` | `-42` ✓ |
| `fixed32`, `fixed64` | unsigned | `42` | `42` ✓ |
| `sfixed32`, `sfixed64` | signed | `-42` | `-42` ✓ |
| `bool` | — | `true` / `false` | `true` / `false` ✓ |

No hex notation for any integer type in known fields.

**No divergence.**

---

### 2.6 Unknown VARINT, FIXED32, FIXED64 rendering

| Wire type | protoc | prototext (annotations=true) |
|-----------|--------|------------------------------|
| VARINT (WT=0) | `N: <decimal>` | `N: <decimal>` ✓ |
| FIXED32 (WT=5) | `N: 0x<8 lowercase hex digits>` | `N: 0x<8 lowercase hex digits>` ✓ |
| FIXED64 (WT=1) | `N: 0x<16 lowercase hex digits>` | `N: 0x<16 lowercase hex digits>` ✓ |

protoc renders unknown VARINTs as **unsigned** decimal
(`absl::StrCat(field.varint())`).  Prototext renders them the same way
(`format_wire_varint_protoc`).

**No divergence** for annotations=true.  With annotations=false, unknown
fields are suppressed (Divergence D2 above).

---

### 2.7 Unknown LENGTH-DELIMITED rendering

protoc attempts to parse the payload as a sub-message (recursion limit 10).
If successful, it renders as a nested `{ }` block with no colon.
If not, it renders as a `CEscape`d quoted string with a colon.

prototext (annotations=true) always renders unknown LEN payloads as a
quoted escaped bytes string:

```
N: "\001\002\003"  #@ bytes
```

It does **not** attempt sub-message parsing for unknown fields.

> **Divergence D3 — unknown LEN fields: no sub-message heuristic.**
> `protoc --decode` speculatively parses unknown LEN payloads as nested
> messages; prototext always renders them as raw bytes.
> Impact: for canonical input where an unknown LEN field happens to contain
> a valid sub-message, protoc renders nested structure while prototext
> renders opaque bytes.  This only affects truly unknown fields (no schema
> match), which are outside the primary use case.  Severity: low.

---

### 2.8 Float and double rendering

#### Special values

| Value | protoc | prototext (current) |
|-------|--------|---------------------|
| Canonical quiet NaN | `nan` | `nan` ✓ |
| Any non-canonical NaN | `nan` | `nan(0x...)` ✗ |
| +Infinity | `inf` | `inf` ✓ |
| -Infinity | `-inf` | `-inf` ✓ |

> **Divergence D4 — non-canonical NaN value token.**
> Already identified in spec 0010 (change B).  For canonical wire input this
> is not triggered (canonical NaN is always `0x7FC00000` / `0x7FF8000000000000`).
> Fix: move `nan(0x...)` to annotation modifier `nan_bits:`.

#### Precision

protoc uses:
- float: `%g` with 6 significant digits, retry at 9 if not round-trip exact.
- double: `%g` with 15 significant digits, retry at 17 if not round-trip exact.

prototext uses the same algorithm (`common.rs` `format_float_protoc` /
`format_double_protoc`).

**No divergence** in precision.

---

### 2.9 String and bytes escaping

protoc uses `absl::CEscape()` with `use_hex=false, utf8_safe=false`.

Named escapes produced: `\n`, `\r`, `\t`, `\"`, `\'`, `\\` only.
All other non-printable bytes (including `\x00`–`\x1F`, `\x7F`, `\x80`–`\xFF`)
use **3-digit octal** (`\NNN`).

Notable: `\x07` (BEL) → `\007` (not `\a`); `\x08` (BS) → `\010` (not `\b`);
`\x0B` (VT) → `\013` (not `\v`); `\x0C` (FF) → `\014` (not `\f`).
High bytes (`\x80`–`\xFF`) → 3-digit octal, never hex.
Multi-byte UTF-8 characters → each byte separately as 3-digit octal.

prototext (`common.rs` `escape_bytes_into` / `escape_string_into`) uses
the same named escapes (`\n`, `\r`, `\t`, `\"`, `\'`, `\\`) and 3-digit
octal for everything else.

The implementations must be verified to match exactly on the BEL/BS/VT/FF
boundary cases and on high bytes.  A quick review of `common.rs:31-96`
shows the escape table matches protoc's `CEscape` output.

**No divergence** (pending byte-level verification of edge cases — see
recommended test in §4).

---

### 2.10 Enum rendering

| Case | protoc | prototext |
|------|--------|-----------|
| Known value | Symbolic name (e.g. `GREEN`) | Symbolic name ✓ |
| Unknown value | Decimal integer (e.g. `10`) | Decimal integer ✓ |

**No divergence.**

---

### 2.11 Message field delimiters

protoc always uses `{` `}` (never `<` `>`).  Colon is omitted before `{`.

prototext uses `{` `}` with no colon.

**No divergence** in delimiter style.

#### Close-brace placement

protoc emits each `}` on its own line at the correct indentation level.

prototext emits each `}` on its own line.  The codebase contains a
`CBL_START` thread-local and comments describing a close-brace folding
mechanism, but the folding logic itself has been removed.  `write_close_brace`
(helpers.rs:117) simply writes `}\n`; no patching or folding occurs.
`CBL_START` is maintained as dead scaffolding.

**No divergence.**

---

### 2.12 Group rendering

protoc uses the **capitalized message type name** (not the field name) as
the identifier, e.g. `OptionalGroup {`.

prototext uses the same convention (verified in `render_group_field`).

**No divergence.**

---

### 2.13 Repeated fields (non-packed)

Both render one element per line with the field name repeated:

```
int32Rp: 1
int32Rp: 2
int32Rp: 3
```

**No divergence.**

---

### 2.14 Packed repeated fields

protoc renders packed repeated fields identically to non-packed repeated
fields — one element per line, field name repeated.  The bracket notation
(`[1, 2, 3]`) is **not** produced by `protoc --decode`.

prototext currently renders packed fields using bracket notation:

```
int32Pk: [1, 2, 3]  #@ repeated int32 [packed=true] = 85
```

> **Divergence D5 — packed field bracket syntax.**
> Already identified in spec 0010 (change A).  This is the most visible
> format divergence.  Fix: per-line rendering with `pack_size` boundary
> markers.

---

### 2.15 Extension fields

protoc renders extension fields with their bracketed fully-qualified name:

```
[com.example.MyExtension]: value
```

prototext has no extension field support.  Extension fields would appear
as unknown fields.

> **Divergence D6 — extension fields.**
> Out of scope for the current compatibility work.

---

## 3. Summary of divergences

| ID | Description | Canonical input affected? | Severity | Tracked in |
|----|-------------|--------------------------|----------|------------|
| D1 | prototext header line | Yes (always present) | Cosmetic | — |
| D2 | Unknown fields suppressed without annotations | Yes (unknown fields) | Known limitation | — |
| D3 | Unknown LEN: no sub-message heuristic | No (requires unknown field) | Low | — |
| D4 | Non-canonical NaN value token `nan(0x...)` | No (canonical NaN is always `nan`) | Low | spec 0010 change B |
| D5 | Packed field bracket syntax | Yes | High | spec 0010 change A |
| D6 | Extension fields not supported | Yes (if extensions present) | Out of scope | — |

For **canonical wire input with a complete schema**, only **D1** (header)
and **D5** (packed syntax) affect the output.  D1 is cosmetic and
intentional.  D5 is the primary fix target (spec 0010).

---

## 4. Recommended additional tests

The following edge cases are not currently covered by the test suite and
should be added (separately from or as part of spec 0009's e2e suite):

1. **String escaping edge cases:** BEL (`\x07`), BS (`\x08`), VT (`\x0B`),
   FF (`\x0C`), DEL (`\x7F`), null (`\x00`), high bytes (`\x80`–`\xFF`),
   multi-byte UTF-8 sequences — verify byte-for-byte match with protoc output.

2. **Unknown VARINT large values:** verify `uint64` max
   (`18446744073709551615`) renders as unsigned decimal, not signed.

3. **`-inf` and `inf` for float and double:** confirm rendering matches
   protoc exactly (lowercase, no leading `+`).

4. **Packed empty record:** verify the empty-packed case is handled when
   spec 0010 change A is implemented.

5. **Repeated non-packed vs packed canonical:** verify that with annotations
   disabled, a canonical packed field renders identically to a non-packed
   repeated field (after spec 0010 change A).

---

## 5. References

- `src/google/protobuf/text_format.cc` — protoc text printer
- `src/google/protobuf/io/strtod.cc` — `SimpleFtoa` / `SimpleDtoa`
- `absl/strings/escaping.cc` — `absl::CEscape`
- `src/google/protobuf/compiler/command_line_interface.cc` — `--decode` entry
- `prototext-core/src/serialize/render_text/mod.rs` — prototext top-level renderer
- `prototext-core/src/serialize/render_text/helpers.rs` — field rendering helpers
- `prototext-core/src/serialize/common.rs` — float/string formatting
- `docs/specs/0010-protoc-compatibility.md` — planned fixes for D4 and D5
