<!-- SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis) -->
<!-- SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS -->
<!--
SPDX-License-Identifier: MIT
-->

# craft_a decode discrepancies

Each fixture in `ALL_FIXTURES` was run through `prototext -d` and the decoded
output was compared against the intent expressed in `craft_a.rs` (and its
Python reference `craft_a.py`).  Most fixtures decode as expected.  The
discrepancies and open questions are listed here.

---

## 1. `test_varint_packed` — `Tag!(field_num:1, wire_type:0)` with packed list

### Python source

```python
with Message(schema=SwissArmyKnife) as test_varint_packed:
    CustomField(b'\x82\x06\x00')
    Int64('int64Pk', [])
    Int64('int64Pk', [4])
    Int64('int64Pk', [Integer(1, ohb=3), 2, 3, 4])
    UInt64(Tag(1, type=0), [1, 2, 3, 4])   # ← packed payload, but tag wire_type forced to 0
    UInt64('uint64Pk', [1, 2, 3, 4])
    ...
```

### Rust fixture

```rust
fixture!(test_varint_packed, super::knife_descriptor();
    raw!(b"\x82\x06\x00"),
    packed_varints!("int64Pk", []),
    packed_varints!("int64Pk", [Integer!(unsigned: 4, ohb: 0)]),
    packed_varints!("int64Pk", [
        Integer!(unsigned: 1, ohb: 3),
        Integer!(unsigned: 2, ohb: 0),
        Integer!(unsigned: 3, ohb: 0),
        Integer!(unsigned: 4, ohb: 0),
    ]),
    // UInt64(Tag(1, type=0), [1, 2, 3, 4]) — packed payload, tag wire_type forced to 0
    packed_varints!(Tag!(field_num: 1, wire_type: 0), [
        Integer!(unsigned: 1),
        Integer!(unsigned: 2),
        Integer!(unsigned: 3),
        Integer!(unsigned: 4),
    ]),
    ...
);
```
### Decoded output (prototext -d, relevant lines)

```
1: 4  #@ varint
0: 0x02010405a2040302  #@ fixed64; TAG_OOR
0 {   #@ group; TAG_OOR; ETAG_OOR
}
```

### Analysis

Both Python and Rust emit a packed length-delimited blob but with the outer
tag's wire_type forced to 0 instead of 2.  The on-wire bytes are:
`\x08` (tag: field 1, wire_type 0) `\x04` (length 4) `\x01\x02\x03\x04`
(payload).

Prototext sees wire_type 0 on the tag and reads it as a varint field:
tag `\x08` → varint value `\x04` = 4.  Then the payload bytes
`\x01\x02\x03\x04` are read as subsequent fields → garbage.

This is the **intended** behavior of the fixture: it exercises what happens
when a packed payload is labelled with a non-LEN wire type.  The decoded
output is correct and both sides match.

---

## 2. `enum_collision_float_kind` — TYPE_MISMATCH for enum named `float`

### Python source

```python
# Enum named 'float' — exercises primitive-keyword collision path.
# Old encoder would mis-encode via the fixed32 arm; new encoder uses the
# (N) suffix to route unconditionally to varint.
with Message(schema=EnumCollision) as enum_collision_float_kind:
    Enum('kind', 1)  # FLOAT_ONE = 1
```

### Rust fixture

```rust
fixture!(enum_collision_float_kind, super::enum_collision_descriptor();
    enum_!("kind", 1_i32),
);
```

### Decoded output (prototext -d with `enum_collision.pb`, message `EnumCollision`)

Before fix:
```
1: 1  #@ varint; TYPE_MISMATCH
```

After fix:
```
kind: FLOAT_ONE  #@ float(1) = 1
```

### Resolution

The `TYPE_MISMATCH` was caused by `enum_collision.proto` declaring the field
as `optional float kind = 1`.  `protoc` resolves the unqualified identifier
`float` as the primitive keyword (TYPE_FLOAT / fixed32) before looking up
user-defined type names, so it wrote `type = TYPE_FLOAT` (2) in the compiled
descriptor with no `type_name` reference to the enum.  prost-reflect and
prototext were both correct; only the schema was wrong.

**Fix:** changed the field declaration to use a fully-qualified type name:

```proto
optional .float kind = 1;
```

With the leading dot, `protoc` cannot match a primitive keyword and correctly
resolves it to the user-defined enum, writing `type = TYPE_ENUM` (14) and
`type_name = ".float"` in the descriptor.

The `enum_named_float_not_mistaken_for_primitive` unit test in `schema.rs`
was already testing the correct behavior (it builds the descriptor
programmatically with `TYPE_ENUM` set), but the compiled schema was not
matching that intent until this fix.

---

## 3. Fixtures missing from `index.toml` (never decoded)

The following fixtures exist in `ALL_FIXTURES` and `craft_a.rs` but have no
entry in `fixtures/index.toml`.  They are skipped by all e2e tests.

| Fixture | Schema / message type |
|---|---|
| `test_fixed64_tag` | `SwissArmyKnife` |
| `test_string_wrong_len` | `SwissArmyKnife` |
| `test_packed_fixed64` | `SwissArmyKnife` |
| `test_packed_double` | `SwissArmyKnife` |
| `test_packed_fixed32` | `SwissArmyKnife` |
| `test_packed_sfixed32` | `SwissArmyKnife` |
| `test_packed_sfixed64` | `SwissArmyKnife` |

These have been added to `prototext/fixtures/index.toml`.
