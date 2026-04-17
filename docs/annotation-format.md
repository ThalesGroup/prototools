<!--
SPDX-FileCopyrightText: 2025 - 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025 - 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# Annotation Format

This document is the definitive reference for the text representation emitted
by `prototext -d` (decode) and consumed by `prototext -e` (encode).

The format is a superset of the
[protobuf text format](https://protobuf.dev/reference/protobuf/textformat-spec/)
as produced by `protoc --decode`.  For canonical wire input, `prototext -d`
output (ignoring the `#@` annotation comment) is byte-for-byte identical to
`protoc --decode` output.  Every field line carries an inline annotation
comment (`#@`) that encodes enough information to reconstruct the exact binary
bytes on re-encoding, including all non-canonical or anomalous aspects.

---

## Line structure

The file starts with a header line:

```
#@ prototext: protoc
```

Every field occupies one line for scalar values, or an opening brace line plus
content lines plus a closing brace line for nested messages and groups:

```
{indent}{field_name}: {value}  #@ {annotation}
{indent}{field_name} {  #@ {annotation}
{indent}  {child fields…}
{indent}}
```

Two spaces before `#@` separate the value from the annotation.  The annotation
runs to end of line.

**Field name key rules:**
- Known fields use the field name from the schema (e.g. `int32Op`).
- Unknown fields (no schema match) use the raw field number (e.g. `999`).
- Invalid fields always use the raw field number, even when schema context is
  available.
- Schema-mismatched fields (wire type conflicts with declared type) use the
  raw field number and carry `TYPE_MISMATCH`.
- Extension fields use a bracketed fully-qualified name (e.g. `[acme.blade_count]`).

---

## Annotation structure

```
#@ [wire_type ";"] [field_decl] [";" modifier]*
```

All parts are optional.  Tokens are separated by `"; "`.  No trailing `";"`.

### Part 1 — wire type

A keyword naming the binary wire type.  Emitted only for:
- Unknown fields (no schema match) — wire type is the only type information available.
- Invalid fields — structural decode failure.
- Group fields — always emitted (`group`) because groups are structurally
  distinct from messages and must be preserved for re-encoding.

**Omitted for known, well-typed non-group fields** (the wire type is
unambiguously implied by the proto type).

Valid wire types (lower case):

| Token | Wire encoding |
|---|---|
| `varint` | VARINT (wire type 0) |
| `fixed64` | FIXED64 (wire type 1) |
| `bytes` | LEN (wire type 2) |
| `fixed32` | FIXED32 (wire type 5) |
| `group` | SGROUP/EGROUP (wire types 3/4); also emitted for known group fields |

Invalid wire types (ALL CAPS) — indicate a structural decode failure:

| Token | Meaning |
|---|---|
| `INVALID_TAG_TYPE` | Tag carries an unrecognised wire type (3-bit field not in {0,1,2,3,4,5}) |
| `INVALID_VARINT` | Varint value field is malformed |
| `INVALID_FIXED64` | FIXED64 payload is truncated |
| `INVALID_FIXED32` | FIXED32 payload is truncated |
| `INVALID_LEN` | LEN length prefix is malformed |
| `TRUNCATED_BYTES` | LEN length prefix is valid but the declared bytes are not all present |
| `INVALID_PACKED_RECORDS` | LEN payload is present but cannot be decoded as packed records |
| `INVALID_STRING` | LEN payload is present but the bytes are not valid UTF-8 (for `string` fields) |
| `INVALID_GROUP_END` | Group END tag varint is malformed |

For invalid fields, no field name or field declaration is emitted — only the
raw field number key and the `INVALID_*` wire type name.  Exception:
`INVALID_TAG_TYPE` uses field number `0` as the key (no valid number exists).

### Part 2 — field declaration

Format: `[label " "] type [" [packed=true]"] " = " field_number`

- **label**: `repeated` or `required`; `optional` is omitted (it is the default).
- **type**: proto scalar type name (e.g. `int32`, `double`, `string`), message or
  group type short name (e.g. `SwissArmyKnife`, `GroupOp`), or enum type short
  name.  For enum fields the type takes the form `EnumTypeName(N)` where `N`
  is the raw wire integer value.  For packed enum fields the form is
  `EnumTypeName([n1, n2, …])`.  Packed non-enum varint types (int32, int64,
  bool, sint32, sint64, uint32, uint64) also use the scalar proto type name
  (e.g. `repeated int32 [packed=true]`).
- **packed**: `[packed=true]` appended when the field uses packed wire encoding.
- **field_number**: the field's tag number in the `.proto` file.

The field declaration is omitted for:
- Unknown fields (no schema match).
- Invalid fields (structural decode failure).
- Schema-mismatched fields (wire type conflicts with declared proto type) —
  these carry `TYPE_MISMATCH` but no field declaration.

For group fields, the wire type token `group` precedes the field declaration:

```
GroupOp {  #@ group; GroupOp = 30
```

### Part 3 — modifiers

Zero or more `name: value` pairs (or bare flag names) describing non-canonical
or anomalous aspects of the binary encoding.  On packed field lines, order is:

1. Record-level modifiers (`pack_size`, `tag_ohb`, `TAG_OOR`, `len_ohb`)
2. Element-level modifiers (`ohb`, `neg`, `nan_bits`, `ENUM_UNKNOWN`, etc.)

---

## Modifier reference

### Non-canonical modifiers (lower case)

Non-canonical encodings are losslessly recoverable — they round-trip exactly.

| Name | Value type | Meaning |
|---|---|---|
| `tag_ohb: N` | integer | Tag varint uses N redundant continuation bytes |
| `val_ohb: N` | integer | Value varint (scalar field) uses N redundant continuation bytes |
| `len_ohb: N` | integer | Length-prefix varint uses N redundant continuation bytes |
| `etag_ohb: N` | integer | END_GROUP tag varint uses N redundant continuation bytes |
| `truncated_neg` | flag | Negative int32/enum encoded as 5-byte truncated varint instead of canonical 10-byte sign-extended form |
| `nan_bits: 0xHH…` | hex integer | Non-canonical NaN bit pattern for a `float` (8 hex digits) or `double` (16 hex digits) field |
| `pack_size: N` | integer | Number of elements in this packed wire record (on the first element line of each record) |
| `ohb: N` | integer | Per-element varint overhang bytes (packed varint fields, on each element line) |
| `neg` | flag | Per-element truncated-negative int32/enum (packed fields, on each element line) |

### Invalid modifiers (ALL CAPS)

Invalid encodings indicate data integrity issues.

| Name | Value type | Meaning |
|---|---|---|
| `TAG_OOR` | flag | Tag field number is 0 or >= 2^29 (out of valid range) |
| `ETAG_OOR` | flag | END_GROUP tag field number is out of valid range |
| `MISSING: N` | integer | N bytes are missing from a truncated field (used with `TRUNCATED_BYTES`) |
| `END_MISMATCH: N` | integer | END_GROUP tag carries field number N instead of the opening tag's number |
| `OPEN_GROUP` | flag | GROUP field has no END_GROUP tag before end of buffer |
| `TYPE_MISMATCH` | flag | Wire type conflicts with the declared proto type for the field |

### Informational modifiers

| Name | Value type | Meaning |
|---|---|---|
| `ENUM_UNKNOWN` | flag | Enum value is not in the schema's value table (integer emitted as field value) |

---

## Packed field encoding

Packed repeated fields are rendered as **one line per element**, identical to
non-packed repeated fields (matching `protoc --decode` output).  Each element
line carries its own annotation.

The **first element** of each wire record carries a `pack_size: N` modifier
indicating how many elements belong to that record.  Record-level anomaly
modifiers (`tag_ohb`, `TAG_OOR`, `len_ohb`) also appear on the first element
line.  Element-level anomaly modifiers (`ohb`, `neg`, `nan_bits`) appear on
each respective element's line.

```
int32Pk: 1  #@ repeated int32 [packed=true] = 85; pack_size: 3
int32Pk: 2  #@ repeated int32 [packed=true] = 85
int32Pk: 3  #@ repeated int32 [packed=true] = 85
```

Multiple consecutive wire records for the same field number each begin with
their own `pack_size`:

```
int64Pk: 1  #@ repeated int64 [packed=true] = 83; pack_size: 4; ohb: 3
int64Pk: 2  #@ repeated int64 [packed=true] = 83
int64Pk: 3  #@ repeated int64 [packed=true] = 83
int64Pk: 4  #@ repeated int64 [packed=true] = 83
```

An **empty packed wire record** (tag + len=0) has no value line.  It is
rendered as a comment-only annotation line with no leading spaces before `#@`:

```
#@ repeated int64 [packed=true] = 83; pack_size: 0
```

If the packed payload cannot be decoded, the field is rendered as a single
`INVALID_PACKED_RECORDS` line with the raw bytes:

```
85: "\200\200\200\200\020\002\003\004"  #@ INVALID_PACKED_RECORDS
```

---

## NaN encoding

`float` and `double` values are rendered as `nan` (bare token) in all cases,
matching `protoc --decode`.

For a **non-canonical NaN** (bit pattern differing from Rust's canonical quiet
NaN: `0x7FC00000` for float, `0x7FF8000000000000` for double), the full bit
pattern is recorded in a `nan_bits` annotation modifier:

```
floatOp: nan  #@ float = 22; nan_bits: 0x7f800001
doubleOp: nan  #@ double = 21; nan_bits: 0xfff8000000000000
```

For a **canonical NaN**, no `nan_bits` modifier is emitted:

```
floatOp: nan  #@ float = 22
```

In packed arrays, `nan_bits` appears on the element line of the non-canonical
NaN element:

```
floatPk: nan  #@ repeated float [packed=true] = 87; pack_size: 3
floatPk: nan  #@ repeated float [packed=true] = 87; nan_bits: 0x7f800001
floatPk: nan  #@ repeated float [packed=true] = 87; nan_bits: 0xffc00000
```

---

## Float and double formatting

Finite float and double values are formatted to match `protoc --decode` output:

- `double`: shortest representation using 15 significant digits, falling back to
  17 if needed for exact round-trip.
- `float`: shortest representation using 6 significant digits, falling back to 9.

Scientific notation is used when the exponent is >= 15 (double) or outside
the `[1e-4, 1e15)` range.  Example: `3.1415926535897931`, `1.23e-10`,
`1.7976931348623157e+308`.

Without a schema, `float` and `double` fields are rendered as raw hex
(FIXED32 / FIXED64): `0x40490fdb`, `0x4005bf0a8b145769`.

---

## String escaping

### `bytes` fields

Every byte is escaped by numeric value:

| Byte value | Emitted form |
|---|---|
| `\` (0x5C) | `\\` |
| `"` (0x22) | `\"` |
| `'` (0x27) | `\'` |
| `\n` (0x0A) | `\n` |
| `\r` (0x0D) | `\r` |
| `\t` (0x09) | `\t` |
| 0x20–0x7E (printable ASCII, excl. above) | literal byte |
| all others | `\NNN` (3-digit octal) |

This matches `protoc --decode` exactly for `bytes` fields.

### `string` fields — deliberate divergence from `protoc --decode`

`protoc --decode` octal-escapes every byte >= 0x80 in string fields.
`prototext` intentionally diverges: multi-byte UTF-8 sequences are emitted as
raw UTF-8.  For a field containing `"café"`, protoc emits `"caf\303\251"`;
`prototext` emits `"café"`.

Control characters (0x00–0x1F) and DEL (0x7F) are octal-escaped in both tools.

If the wire bytes of a `string` field are not valid UTF-8, `prototext` emits
`INVALID_STRING`.

---

## Formal grammar

```
-- Top level
message    := header NEWLINE field*
header     := "#@ prototext: protoc"

-- Field lines
field      := scalar_field | message_field
scalar_field  := field_key ": " value "  #@ " annotation NEWLINE
             |  "#@ " annotation NEWLINE              -- comment-only: empty packed record
message_field := field_key " {  #@ " annotation NEWLINE field* "}" NEWLINE

field_key  := IDENTIFIER | NUMBER | "[" IDENTIFIER ("." IDENTIFIER)* "]"

value      := STRING | NUMBER | BOOL | IDENTIFIER

-- Annotation
annotation := unknown_field_ann | known_field_ann

unknown_field_ann := wire_type [";" modifier]*
known_field_ann   := ["group" ";"] field_decl [";" modifier]*

-- Field declaration (optional is omitted as default label)
field_decl := [label " "] type [" [packed=true]"] " = " NUMBER
label      := "repeated" | "required"
type       := proto_scalar_type
           |  IDENTIFIER                                    -- message or group type name
           |  IDENTIFIER "(" NUMBER ")"                     -- enum: scalar numeric value
           |  IDENTIFIER "([" NUMBER ("," NUMBER)* "])"     -- enum: packed numeric values

proto_scalar_type := "double" | "float" | "int64" | "uint64" | "int32"
                  |  "fixed64" | "fixed32" | "bool" | "string" | "bytes"
                  |  "uint32" | "sfixed32" | "sfixed64" | "sint32" | "sint64"

-- Wire types
wire_type         := valid_wire_type | invalid_wire_type
valid_wire_type   := "varint" | "fixed64" | "bytes" | "fixed32" | "group"
invalid_wire_type := "INVALID_TAG_TYPE" | "INVALID_VARINT" | "INVALID_FIXED64"
                  |  "INVALID_FIXED32"  | "INVALID_LEN"   | "TRUNCATED_BYTES"
                  |  "INVALID_PACKED_RECORDS" | "INVALID_STRING" | "INVALID_GROUP_END"

-- Modifiers
modifier := noncanon_valued | noncanon_flag | invalid_valued | invalid_flag | info_flag

noncanon_valued := ("tag_ohb" | "val_ohb" | "len_ohb" | "etag_ohb" | "ohb" | "pack_size") ":" SP INTEGER
               |  "nan_bits: 0x" HEX+
noncanon_flag   := "truncated_neg" | "neg"

invalid_valued  := ("MISSING" | "END_MISMATCH") ":" SP INTEGER
invalid_flag    := "TAG_OOR" | "ETAG_OOR" | "OPEN_GROUP" | "TYPE_MISMATCH"

info_flag       := "ENUM_UNKNOWN"

-- Tokens
IDENTIFIER := /[a-zA-Z_][a-zA-Z0-9_]*/
NUMBER     := /-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?/ | "0x" HEX+ | "inf" | "-inf" | "nan"
INTEGER    := /[0-9]+/
HEX        := /[0-9a-f]/
STRING     := /"([^"\\]|\\.)*"/
BOOL       := "true" | "false"
SP         := " "
NEWLINE    := "\n"
```

**Notes:**
- Two spaces before `#@` are required as the value/annotation separator.
- For comment-only lines (empty packed records), the annotation begins with
  `#@ ` with no leading spaces.
- Groups are distinguished from messages by the `group` prefix in the annotation.
- `group` (lower case) is the wire type token; the group type name in the field
  declaration follows after `"; "` (e.g. `#@ group; GroupOp = 30`).

---

## Examples

The examples below are taken from actual `prototext -d` output against the
`SwissArmyKnife` and `EnumCollision` test schemas.

### Canonical scalars (schema-aware)

```
#@ prototext: protoc
doubleOp: 2.7182818284590451  #@ double = 21
floatOp: 3.14159274  #@ float = 22
int64Op: -123456789  #@ int64 = 23
uint64Op: 18446744073709551615  #@ uint64 = 24
int32Op: 42  #@ int32 = 25
fixed64Op: 987654321  #@ fixed64 = 26
fixed32Op: 123456  #@ fixed32 = 27
boolOp: true  #@ bool = 28
uint32Op: 999  #@ uint32 = 33
sfixed32Op: -999  #@ sfixed32 = 35
sfixed64Op: -123456789  #@ sfixed64 = 36
sint32Op: -42  #@ sint32 = 37
sint64Op: 123456789  #@ sint64 = 38
```

`optional` is omitted (default label).  Wire types are omitted for all
known fields (implied by proto type).

### Canonical scalars (no schema)

Without a schema, all fields render by wire type.  Float/double fields appear
as raw hex:

```
#@ prototext: protoc
21: 0x4005bf0a8b145769  #@ fixed64
22: 0x40490fdb  #@ fixed32
23: 18446744073586094827  #@ varint
25: 42  #@ varint
26: 0x000000003ade68b1  #@ fixed64
27: 0x0001e240  #@ fixed32
28: 1  #@ varint
```

### Repeated and nested fields

```
#@ prototext: protoc
int32Op: 100  #@ int32 = 25
messageOp {  #@ SwissArmyKnife = 31
 int32Op: 200  #@ int32 = 25
 stringOp: "nested"  #@ string = 29
}
messageRp {  #@ repeated SwissArmyKnife = 51
 stringOp: "first nested"  #@ string = 29
 uint32Op: 1  #@ uint32 = 33
}
messageRp {  #@ repeated SwissArmyKnife = 51
 stringOp: "second nested"  #@ string = 29
 uint32Op: 2  #@ uint32 = 33
}
```

### Group fields

```
#@ prototext: protoc
int32Op: 42  #@ int32 = 25
GroupOp {  #@ group; GroupOp = 30
 uint64Op: 111  #@ uint64 = 130
}
GroupRp {  #@ group; repeated GroupRp = 50
 uint64Op: 10  #@ uint64 = 150
}
GroupRp {  #@ group; repeated GroupRp = 50
 uint64Op: 20  #@ uint64 = 150
}
```

### Unknown fields mixed with known fields

```
#@ prototext: protoc
int32Op: 42  #@ int32 = 25
uint32Op: 100  #@ uint32 = 33
999: 123456  #@ varint
1000: "binary\000\377\376 data"  #@ bytes
```

### String and bytes escaping

```
#@ prototext: protoc
stringOp: "tab:\there\nnewline\\backslash\"quote"  #@ string = 29
bytesOp: "\000\001\002\003\004"  #@ bytes = 32
```

### Non-canonical varint (overhang bytes)

```
#@ prototext: protoc
1: 42  #@ varint; val_ohb: 3
```

Value `42` encoded with 3 extra continuation bytes.  Rounds-trip byte-exact.

### Non-canonical tag encoding

```
#@ prototext: protoc
GroupOp {  #@ group; GroupOp = 30; tag_ohb: 1
 uint64Op: 0  #@ uint64 = 130
}
GroupOp {  #@ group; GroupOp = 30; tag_ohb: 1; etag_ohb: 1
 uint64Op: 0  #@ uint64 = 130
}
GroupOp {  #@ group; GroupOp = 30; etag_ohb: 1
 uint64Op: 0  #@ uint64 = 130
}
```

### Truncated negative int32

```
#@ prototext: protoc
int32Rp: -2147483648  #@ repeated int32 = 45; truncated_neg
int32Rp: -2147483648  #@ repeated int32 = 45
int32Rp: -1  #@ repeated int32 = 45; truncated_neg
int32Rp: -1  #@ repeated int32 = 45
```

### Packed fields (varint)

```
#@ prototext: protoc
int32Pk: 1  #@ repeated int32 [packed=true] = 85; pack_size: 4
int32Pk: 2  #@ repeated int32 [packed=true] = 85
int32Pk: 3  #@ repeated int32 [packed=true] = 85
int32Pk: 4  #@ repeated int32 [packed=true] = 85
```

### Packed fields (with element-level overhang)

```
#@ prototext: protoc
int32Pk: 23  #@ repeated int32 [packed=true] = 85; pack_size: 3; ohb: 2
int32Pk: 24  #@ repeated int32 [packed=true] = 85
int32Pk: 35  #@ repeated int32 [packed=true] = 85; ohb: 3
```

### Packed fields (with truncated-negative elements)

```
#@ prototext: protoc
int32Pk: 1  #@ repeated int32 [packed=true] = 85; pack_size: 5
int32Pk: -1  #@ repeated int32 [packed=true] = 85; neg
int32Pk: -2147483648  #@ repeated int32 [packed=true] = 85; neg
int32Pk: -1  #@ repeated int32 [packed=true] = 85
int32Pk: 2  #@ repeated int32 [packed=true] = 85
```

### Packed fields (fixed-width)

```
#@ prototext: protoc
doublePk: 0  #@ repeated double [packed=true] = 81; pack_size: 3
doublePk: 3.1415926535897931  #@ repeated double [packed=true] = 81
doublePk: 1.7976931348623157e+308  #@ repeated double [packed=true] = 81
```

### Empty packed record

```
#@ prototext: protoc
#@ repeated int64 [packed=true] = 83; pack_size: 0
int64Pk: 4  #@ repeated int64 [packed=true] = 83; pack_size: 1
```

### Invalid packed records

```
#@ prototext: protoc
85: "\200\200\200\200\020\002\003\004"  #@ INVALID_PACKED_RECORDS
```

### Enum fields — known value

```
#@ prototext: protoc
color: GREEN  #@ Color(1) = 2
```

### Enum fields — unknown value

```
#@ prototext: protoc
unknown_color: 99  #@ Color(99) = 3; ENUM_UNKNOWN
```

### Packed enum fields

```
#@ prototext: protoc
colors_pk: RED  #@ repeated Color(0) [packed=true] = 5; pack_size: 3
colors_pk: GREEN  #@ repeated Color(1) [packed=true] = 5
colors_pk: BLUE  #@ repeated Color(2) [packed=true] = 5
```

### Packed enum with unknown value

```
#@ prototext: protoc
colors_pk: RED  #@ repeated Color(0) [packed=true] = 5; pack_size: 3
colors_pk: 99  #@ repeated Color(99) [packed=true] = 5; ENUM_UNKNOWN
colors_pk: BLUE  #@ repeated Color(2) [packed=true] = 5
```

### Type mismatch (wire type conflicts with schema)

```
#@ prototext: protoc
48: 2  #@ varint; TYPE_MISMATCH
```

Field 48 is declared `bool` (valid range 0–1) but wire value is 2.  No field
declaration emitted; field number used as key.

### Open-ended group

```
#@ prototext: protoc
GroupOp {  #@ group; GroupOp = 30; OPEN_GROUP
 uint64Op: 0  #@ uint64 = 130
}
```

### Mismatched group end

```
#@ prototext: protoc
4 {  #@ group; END_MISMATCH: 44
 11: 0  #@ varint
}
```

### Out-of-range tag

```
#@ prototext: protoc
0: 0x02010405a2040302  #@ fixed64; TAG_OOR
0 {  #@ group; TAG_OOR; ETAG_OOR
}
```

### INVALID_TAG_TYPE

```
#@ prototext: protoc
0: "\364\201\200"  #@ INVALID_TAG_TYPE
```

Field number `0` used as key (no valid field number available).

### INVALID_GROUP_END

```
#@ prototext: protoc
0: "\212\003\032Bogus END_GROUP just above"  #@ INVALID_GROUP_END; TAG_OOR
```

### TRUNCATED_BYTES / MISSING

```
#@ prototext: protoc
99: "\001\002"  #@ TRUNCATED_BYTES; MISSING: 5
```

Length prefix declared 7 bytes; only 2 available.

### INVALID_FIXED32 / INVALID_FIXED64

```
#@ prototext: protoc
floatRp: 3.14159274  #@ repeated float = 42
42: "\333\017"  #@ INVALID_FIXED32
```

```
#@ prototext: protoc
doublePk: 3.1415926535897931  #@ repeated double [packed=true] = 81
81: "\030-DT\373!\t"  #@ INVALID_FIXED64
```

### Extension field

```
#@ prototext: protoc
[acme.blade_count]: 42  #@ int32 = 1000
```
