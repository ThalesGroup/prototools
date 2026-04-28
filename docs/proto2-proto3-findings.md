<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# Proto2 / Proto3 / Editions — Empirical Findings

Authoritative findings for spec `0015-proto3-rendering.md`.

**Test environment:**
- protoc: libprotoc 32.1
- protobuf Python library: 6.33.1
- Python: 3.x (nix-shell)

Source files in `docs/mockup/` were compiled with `protoc` and inspected
with the Python protobuf library.  Every claim here is backed by a mockup
file and the corresponding `.pb` descriptor.

---

## Part I — `FileDescriptorProto.syntax` and `.edition` fields

**Mockup files:** `f01` – `f04`

| Source file | `fdp.syntax` | `fdp.edition` |
|-------------|-------------|---------------|
| `syntax = "proto2";` explicit | `""` (empty string) | not set |
| No `syntax` statement (legacy) | `""` (empty string) | not set |
| `syntax = "proto3";` | `"proto3"` | not set |
| `edition = "2023";` | `"editions"` | `1000` (EDITION_2023) |

**Key findings:**

1. **Proto2 always produces `syntax = ""`** — both with an explicit
   `syntax = "proto2";` statement and with no statement at all.  The string
   `"proto2"` never appears in a real descriptor's `syntax` field.

2. **Edition files set `syntax = "editions"`** and `edition = 1000`
   (EDITION_2023).  The `Edition` enum has `EDITION_PROTO2 = 998` and
   `EDITION_PROTO3 = 999` as internal synthetic values; these are never
   written to a descriptor by the compiler.

3. The `effective_syntax()` function must therefore map:

   | `fdp.syntax` | `fdp.edition` | Result |
   |--------------|---------------|--------|
   | `""` | — | `"proto2"` |
   | `"proto3"` | — | `"proto3"` |
   | `"editions"` | `1000` | `"editions"` (not yet supported) |
   | anything else | — | `"proto2"` + `cli_warning` |

   **The value `"proto2"` never needs to be matched** — it simply does not
   occur in real descriptors.

---

## Part II — Field labels

**Mockup files:** `f05` (proto2), `f06` (proto3)

### Proto2 labels (f05)

| Field | `label` | `proto3_optional` | `oneof_index` |
|-------|---------|-------------------|---------------|
| `opt_field` | `LABEL_OPTIONAL` | False | — |
| `req_field` | `LABEL_REQUIRED` | False | — |
| `rep_field` | `LABEL_REPEATED` | False | — |
| `oneof_a` | `LABEL_OPTIONAL` | False | 0 (`choice`) |
| `oneof_b` | `LABEL_OPTIONAL` | False | 0 (`choice`) |

### Proto3 labels (f06)

| Field | `label` | `proto3_optional` | `oneof_index` |
|-------|---------|-------------------|---------------|
| `implicit_scalar` | `LABEL_OPTIONAL` | False | — |
| `explicit_opt` | `LABEL_OPTIONAL` | **True** | 1 (`_explicit_opt`) |
| `rep_field` | `LABEL_REPEATED` | False | — |
| `msg_field` | `LABEL_OPTIONAL` | False | — |
| `opt_msg_field` | `LABEL_OPTIONAL` | **True** | 2 (`_opt_msg_field`) |
| `oneof_a` | `LABEL_OPTIONAL` | False | 0 (`real_choice`) |
| `oneof_b` | `LABEL_OPTIONAL` | False | 0 (`real_choice`) |

**Key findings:**

1. **Implicit proto3 fields and proto2 optional fields both have
   `label = LABEL_OPTIONAL`**.  The absence of a label keyword in proto3
   source is not distinguishable from `optional` by `label` alone —
   it is inferred from `syntax == "proto3"` AND `proto3_optional == False`.

2. **`proto3_optional = True`** is set for fields declared with an
   explicit `optional` keyword in proto3.  It is always `False` in proto2.

3. **Message fields in proto3** (`msg_field`) have `label = LABEL_OPTIONAL`
   and `proto3_optional = False`.  They always have explicit presence
   regardless of the `optional` keyword.

4. **`optional` on a message field in proto3** (`opt_msg_field`) also
   sets `proto3_optional = True`.  This is observable in the descriptor
   even though it makes no semantic difference (message fields always
   have explicit presence).

---

## Part III — Synthetic oneofs

**Mockup file:** `f10`

### Descriptor structure for `SyntheticOneof`

| Field | `proto3_optional` | `oneof_index` |
|-------|-------------------|---------------|
| `opt_scalar` | True | 1 |
| `opt_string` | True | 2 |
| `implicit` | False | — |
| `rep` | False | — |
| `choice_a` | False | 0 |
| `choice_b` | False | 0 |

Oneofs in `oneof_decl`:

| Index | Name | Fields |
|-------|------|--------|
| 0 | `real_choice` | `choice_a`, `choice_b` |
| 1 | `_opt_scalar` | `opt_scalar` |
| 2 | `_opt_string` | `opt_string` |

**Key findings:**

1. **Real oneofs come first** in `oneof_decl` (index 0), synthetic oneofs
   follow (indices 1, 2, …).

2. **Synthetic oneof detection rule** (all three conditions must hold):
   - `oneof.name` starts with `_`
   - the oneof contains exactly one field
   - that field has `proto3_optional == True`

3. **Rendering rule:**
   - Suppress the synthetic oneof entry entirely (do not emit
     `oneof _foo { ... }`).
   - Render the field with `optional` prefix outside any oneof block.

4. **Real oneofs must never be suppressed**, even if their name happened
   to start with `_` (unlikely but possible in user code).  The
   `proto3_optional` check on the contained field is the authoritative
   discriminator.

---

## Part IV — Packed repeated fields

**Mockup files:** `f07` (proto2), `f08` (proto3)

### Proto2 packed (f07)

| Field | `HasField("options")` | `HasField("packed")` | `packed` value |
|-------|-----------------------|----------------------|---------------|
| `default_int` | False | False | — |
| `explicit_true` | True | True | `True` |
| `explicit_false` | True | True | `False` |
| `strings` | False | False | — |
| `bytesf` | False | False | — |
| `enums_default` | False | False | — |
| `enums_true` | True | True | `True` |
| `enums_false` | True | True | `False` |

### Proto3 packed (f08)

| Field | `HasField("options")` | `HasField("packed")` | `packed` value |
|-------|-----------------------|----------------------|---------------|
| `default_int` | False | False | — |
| `explicit_true` | True | True | **`True`** |
| `explicit_false` | True | True | `False` |
| `strings` | False | False | — |
| `bytesf` | False | False | — |
| `enums_default` | False | False | — |
| `enums_true` | True | True | **`True`** |
| `enums_false` | True | True | `False` |
| `doubles_def` | False | False | — |
| `floats_true` | True | True | **`True`** |

**Key findings:**

1. **`[packed = true]` is preserved in proto3 descriptors** — protoc does
   NOT strip it back to unset.  A proto3 field with `[packed = true]` in
   source produces `HasField("packed") == True` and `packed == True` in the
   descriptor, identical to proto2.

2. **Default-packed fields in proto3 produce no `packed` option** —
   `default_int` and `enums_default` have `HasField("options") == False`
   (no options at all set), not `packed = True`.

3. **The rendering rule is therefore syntax-independent:**
   - `HasField("packed") == False` → emit nothing (in both proto2 and
     proto3, this means "use the syntax default").
   - `HasField("packed") == True` and `packed == True` → emit
     `[packed = true]`.
   - `HasField("packed") == True` and `packed == False` → emit
     `[packed = false]`.

4. **String, bytes fields never have `packed` set** regardless of syntax.
   Protoc silently ignores `[packed = true]` on non-packable types (or
   rejects it — not tested here).

### Wire-level verification (f24)

**Mockup file:** `f24_packed_wire_test.proto` (proto3), test scripts:
`test_packed_wire.py`, prototext CLI.

A proto3 message with `[1, 2, 3]` in each field was encoded by the Python
protobuf runtime and the raw wire bytes inspected.  The same bytes were then
decoded by prototext using `f24.pb` as the descriptor.

#### Python runtime: `is_packed` vs `GetOptions().packed`

| Field | `HasField("packed")` | `GetOptions().packed` | `is_packed` |
|-------|----------------------|-----------------------|-------------|
| `default_field` (no annotation) | False | False | **True** |
| `explicit_true` `[packed=true]` | True | True | True |
| `explicit_false` `[packed=false]` | True | False | False |

`is_packed` is a **computed property** that applies the syntax default when
the `packed` option is absent.  `GetOptions().packed` returns the raw option
value and is `False` (the protobuf scalar default) when unset — it does NOT
encode the proto3 semantic default.

#### Wire bytes

Raw bytes: `0a030102031203010203180118021803`

| Field | Wire type | Encoding |
|-------|-----------|----------|
| `default_field` | 2 (length-delimited) | **PACKED** |
| `explicit_true` | 2 (length-delimited) | PACKED |
| `explicit_false` | 0 (varint, ×3) | UNPACKED |

#### prototext output

```
default_field: 1  #@ repeated int32 [packed=true] = 1; pack_size: 3
default_field: 2  #@ repeated int32 [packed=true] = 1
default_field: 3  #@ repeated int32 [packed=true] = 1
explicit_true: 1  #@ repeated int32 [packed=true] = 2; pack_size: 3
explicit_true: 2  #@ repeated int32 [packed=true] = 2
explicit_true: 3  #@ repeated int32 [packed=true] = 2
explicit_false: 1  #@ repeated int32 = 3
explicit_false: 2  #@ repeated int32 = 3
explicit_false: 3  #@ repeated int32 = 3
```

prototext annotates `default_field` as `[packed=true]` and correctly decodes
the length-delimited payload as packed records.

#### How prototext resolves `is_packed`

prototext itself has no syntax-awareness logic.  It delegates entirely to
`prost-reflect`, which computes `is_packed` at descriptor load time
(`prost-reflect-0.16.3/src/descriptor/build/resolve.rs:121`):

```rust
let is_packed = cardinality == Cardinality::Repeated
    && kind.is_some_and(|k| k.is_packable())
    && (field.options
        .as_ref()
        .map_or(syntax == Syntax::Proto3, |o| o.value.packed()));
```

The `map_or` applies the proto3 default explicitly: **if no `packed` option
is present, `is_packed` is `true` iff `syntax == Proto3`**.  When options are
present the raw `packed()` value is used directly.

#### Conclusion

**Key finding:** `HasField("packed") == False` in the descriptor is
ambiguous without knowing the file syntax:

| Syntax | `HasField("packed") == False` means |
|--------|-------------------------------------|
| proto2 | unpacked (proto2 default) |
| proto3 | **packed** (proto3 default) |

Any tool that reads a `.pb` descriptor and decodes actual wire-format messages
**must consult `fdp.syntax`** to correctly interpret repeated integer fields
when the `packed` option is absent.  `prost-reflect` does this correctly.
The `syntax` field in `FileDescriptorProto` is not merely source-reconstruction
metadata — it is **semantically required** for correct wire-level decoding.

---

## Part V — `json_name`

**Mockup file:** `f09`

| Field | `json_name` | auto camelCase | `HasField` | custom |
|-------|-------------|----------------|------------|--------|
| `field_name` | `"fieldName"` | `"fieldName"` | True | **False** |
| `already_camel` | `"alreadyCamel"` | `"alreadyCamel"` | True | **False** |
| `custom` | `"My"` | `"custom"` | True | **True** |
| `same_as_auto` | `"sameAsAuto"` | `"sameAsAuto"` | True | **False** |
| `under_score_heavy` | `"underScoreHeavy"` | `"underScoreHeavy"` | True | **False** |

**Key findings:**

1. **`HasField("json_name")` is always `True`** — it cannot distinguish
   auto-derived from custom overrides.

2. **A `[json_name = "sameAsAuto"]` that equals the auto camelCase**
   produces the same descriptor as having no `json_name` option at all
   (`HasField` is `True` in both cases, values are identical).

3. **Detection rule for custom override:** emit `[json_name = "..."]` only
   when `field.json_name != camel_case(field.name)`, where `camel_case`
   is: split on `_`, keep first component lowercase, capitalize each
   subsequent component.

4. **This rule is syntax-independent** (identical for proto2 and proto3).

---

## Part VI — Default values

**Mockup file:** `f11`

| Field | `HasField("default_value")` | `default_value` (string) |
|-------|-----------------------------|--------------------------|
| `i32` | True | `'42'` |
| `i64` | True | `'-1000000000000'` |
| `u32` | True | `'4294967295'` |
| `u64` | True | `'18446744073709551615'` |
| `s32` | True | `'-1'` |
| `flt` | True | `'3.14'` |
| `flt_inf` | True | `'inf'` |
| `flt_ninf` | True | `'-inf'` |
| `dbl_nan` | True | `'nan'` |
| `b_true` | True | `'true'` |
| `b_false` | True | `'false'` |
| `s` | True | `'hello'` |
| `s_empty` | True | `''` (empty string) |
| `byt` | True | `'binary\\000data'` |
| `color` | True | `'GREEN'` |
| `no_def` | **False** | `''` (empty string, sentinel) |

**Key findings:**

1. **`HasField("default_value")` is the authoritative check** — when no
   default is set, `default_value` returns `''` (same as an explicit empty
   string default), so `HasField` is the only reliable discriminator.

2. **Numeric defaults are stored as decimal strings.**  Float specials
   (`inf`, `-inf`, `nan`) are stored as their string names.

3. **Enum defaults are stored as the value name** (e.g., `'GREEN'`), not
   as a number.

4. **Bytes defaults are stored with C-style octal escapes**
   (e.g., `'binary\\000data'`).  The existing `_render_default_value`
   already handles this correctly.

5. **Proto3 descriptors must never have `default_value` set** — this is a
   compile-time guarantee from `protoc`; the only source of a set
   `default_value` in a proto3 descriptor is a hand-crafted `.pb`.

---

## Part VII — Extensions and extension ranges

**Mockup file:** `f12`

- `Extendable.extension_range`: `[(100, 200)]`
  (note: the end stored is `start + 1` of the exclusive end — protoc stores
  200 as the stored end for `extensions 100 to 199`)
- File-level `extend` fields are in `FileDescriptorProto.extension[]`
- Message-level `extend` fields are in `DescriptorProto.extension[]`
- Both carry `extendee = ".mockup.Extendable"` and a field number

**Extension range options (f18):** `HasField("options") == False` for a
plain `extensions 100 to 199;` declaration.  Range options (`verification`,
`declaration`, `features`) are `RETENTION_SOURCE` — they do not survive
into the descriptor.  No rendering needed.

---

## Part VIII — Groups

**Mockup file:** `f13`

| Field | `type` | `label` | `type_name` |
|-------|--------|---------|-------------|
| `id` | `int32` | `LABEL_OPTIONAL` | `''` |
| `mygroup` | **`group`** | `LABEL_OPTIONAL` | `'.mockup.WithGroup.MyGroup'` |
| `repgroup` | **`group`** | `LABEL_REPEATED` | `'.mockup.WithGroup.RepGroup'` |

Nested types present: `MyGroup`, `RepGroup` (synthetic messages).

**Key findings:**

1. **Group fields have `type == TYPE_GROUP`** (value 10) and a `type_name`
   pointing to their synthetic nested message.

2. **The nested message is also present** in `message.nested_type[]` — but
   the group is rendered inline with the field, not as a separate message
   block.  Reproto's existing code already handles this.

3. **The field name is stored lowercase** (`mygroup`, `repgroup`) even
   though the source uses `MyGroup`, `RepGroup`.  The `type_name` uses the
   original capitalisation.

---

## Part IX — `import weak`

**Mockup files:** `f14`, `f14_dep`

- `fdp.dependency = ['f14_weak_import_proto2_dep.proto']`
- `fdp.weak_dependency = [0]`  (index 0 into `dependency[]`)

**Key findings:**

1. **`weak_dependency` is a list of indices into `dependency[]`**, not a
   list of file names.

2. **Reproto already supports `import weak` rendering** — `re_file.py`
   checks `index in self.weak_dependency` and emits `import weak`.

3. **Proto3 constraint:** `import weak` is not valid in proto3.  If a
   proto3 descriptor has `weak_dependency` entries, emit `cli_warning` and
   render as plain `import`.

---

## Part X — Enums

**Mockup files:** `f15` (proto2), `f15b` (proto3)

- No `options` field on proto2 or proto3 enums unless `allow_alias = true`
  is set.
- **Closed vs open** is purely semantic — there is no field in
  `EnumDescriptorProto` or `EnumOptions` that encodes `CLOSED` or `OPEN`
  for proto2/proto3 files.  It is entirely implied by `fdp.syntax`.
- **`allow_alias`** is identical in proto2 and proto3: stored as
  `EnumOptions.allow_alias = true` when set.
- **Reproto already renders `allow_alias`** via the generic `EnumOptions`
  path.  No change required.

---

## Part XI — FieldOptions: `ctype`, `jstype`, `deprecated`, `weak`

**Mockup files:** `f16` (proto2), `f16b` (proto3)

Both `ctype`, `jstype`, and `deprecated` compile and appear identically in
the descriptor for proto2 and proto3.

| Option | proto2 | proto3 |
|--------|--------|--------|
| `ctype = CORD` | `ctype=1` | `ctype=1` |
| `ctype = STRING_PIECE` | `ctype=2` | `ctype=2` |
| `jstype = JS_STRING` | `jstype=1` | `jstype=1` |
| `jstype = JS_NUMBER` | `jstype=2` | `jstype=2` |
| `deprecated = true` | `deprecated=True` | `deprecated=True` |
| `weak = true` (message field) | `weak=True` | `weak=True` |

**Key findings:**

1. **`ctype`, `jstype`, `deprecated` are syntax-neutral** — accepted and
   stored identically in proto2 and proto3.

2. **`weak = true` is accepted in both proto2 and proto3** (tested
   separately).  It is syntax-neutral.

3. **No rendering changes needed** for these options — they flow through
   the existing generic `FieldOptions` rendering path.

---

## Part XII — MessageOptions

**Mockup files:** `f17` (proto2), `f17b` (proto3)

| Option | proto2 | proto3 |
|--------|--------|--------|
| `message_set_wire_format = true` | accepted | **rejected by protoc** (`MessageSet is not supported in proto3`) |
| `no_standard_descriptor_accessor = true` | accepted | accepted |
| `deprecated = true` | accepted | accepted |

**Key findings:**

1. **`message_set_wire_format` is proto2-only** — protoc rejects it in
   proto3 at compile time (also requires `extensions` range which is
   itself proto2-only).

2. **`no_standard_descriptor_accessor` and `deprecated` are
   syntax-neutral** — accepted in both.

3. These options flow through the existing generic `MessageOptions`
   rendering path.  The only change is the inconsistency guard for
   `message_set_wire_format` in proto3.

---

## Part XIII — Edition 2023 features in the descriptor

**Mockup file:** `f19`

File-level options:
- `fdp.syntax = "editions"`, `fdp.edition = 1000`
- `fdp.options.features`: `enum_type=CLOSED(2)`, `utf8_validation=NONE(3)`

Enums:
- `OpenEnum`: `options.features.enum_type = OPEN(1)` (explicit override)
- `ClosedEnum`: no features (inherits file-level `CLOSED`)

Message `EditionMsg` fields:

| Field | `label` | `packed` | `features` in descriptor |
|-------|---------|----------|--------------------------|
| `implicit_field` | `LABEL_OPTIONAL` | unset | `field_presence=IMPLICIT(2)` |
| `explicit_field` | `LABEL_OPTIONAL` | unset | (none — default EXPLICIT) |
| `expanded` | `LABEL_REPEATED` | unset | `repeated_field_encoding=EXPANDED(2)` |
| `packed_field` | `LABEL_REPEATED` | unset | (none — default PACKED) |
| `delimited_field` | `LABEL_OPTIONAL` | unset | `message_encoding=DELIMITED(2)` |

**Key findings:**

1. **Features are stored as `FeatureSet` messages on the respective
   `*Options` messages** (`FileOptions.features`, `FieldOptions.features`,
   `EnumOptions.features`, etc.).

2. **Only explicit overrides are stored** — if a feature equals the
   edition default, no entry is written.  `explicit_field` and
   `packed_field` have no features despite having default behavior.

3. **`packed` option is NOT used in edition files** — `repeated_field_encoding`
   in `features` replaces it entirely.  Edition fields with explicit
   `EXPANDED` encoding have `features.repeated_field_encoding = EXPANDED`
   and `HasField("packed") == False`.

4. **`field_presence = IMPLICIT(2)`** represents a proto3-like implicit
   field.  `field_presence = EXPLICIT(1)` is the edition 2023 default and
   is never stored.  `field_presence = LEGACY_REQUIRED(3)` replaces
   `LABEL_REQUIRED`.

5. **`message_encoding = DELIMITED(2)`** replaces proto2 groups in editions.
   The field still has `type = TYPE_MESSAGE` (not `TYPE_GROUP`), so
   the existing group detection by `TYPE_GROUP` does not apply.

6. **Per-element inheritance confirmed:** `ClosedEnum` inherits
   `enum_type = CLOSED` from the file without any stored feature; `OpenEnum`
   overrides it to `OPEN` with an explicit feature entry.

7. **For rendering purposes**, an edition file cannot be naively treated
   as proto2 or proto3 — the feature set must be resolved per-element.
   The proto2 fallback with a warning is the correct interim behavior.

---

## Part XIV — Rendering impact summary

Complete table of rendering decisions and their syntax dependency:

| Rendering decision | Proto2 | Proto3 | Edition 2023 |
|-------------------|--------|--------|--------------|
| `syntax = "...";` line | `"proto2"` (from `""`) | `"proto3"` | replaced by `edition = "2023";` |
| Field label (singular, non-oneof) | `optional`/`required` from `label` | none (implicit) or `optional` if `proto3_optional` | from `field_presence` feature |
| `[default = ...]` | emit if `HasField("default_value")` | never (inconsistency if set) | never |
| `[packed = ...]` | mirror `HasField("packed")` exactly | mirror `HasField("packed")` exactly | use `repeated_field_encoding` feature |
| `extend` blocks | emit | skip + warn | emit |
| `extensions N to M;` | emit | skip + warn | emit |
| Groups (`TYPE_GROUP`) | emit inline | skip + warn | replaced by `DELIMITED` feature |
| `import weak` | emit | skip + warn (degrade to plain import) | emit (`weak_dependency` index set — see Part XVI) |
| `message_set_wire_format` | emit | skip + warn | emit (accepted in editions — see Part XVII) |
| Synthetic oneof | N/A | suppress `_name` oneofs | depends on `field_presence` |
| `features { ... }` | N/A | N/A | emit per-element overrides |
| `edition = "...";` | N/A | N/A | emit instead of `syntax` |
| `ctype`, `jstype`, `weak`, `deprecated` | emit via generic path | emit via generic path (syntax-neutral) | emit via generic path |
| `no_standard_descriptor_accessor` | emit via generic path | emit via generic path (syntax-neutral) | emit via generic path |
| `json_name` | emit only if custom | emit only if custom (same rule) | same rule |
| Extension range options | not stored (RETENTION_SOURCE) | n/a | not stored |

---

## Part XV — (resolved; see Parts XVI–XIX)

All five previously open items have been resolved empirically.
See Parts XVI through XIX below.

---

## Part XVI — `import weak` in editions

**Mockup file:** `f20`

**Finding:** `import weak` is accepted by protoc in edition 2023 files.  The
`weak_dependency` index list is populated identically to proto2.

| Syntax | Accepted by protoc | `weak_dependency` populated |
|--------|-------------------|----------------------------|
| proto2 | yes | yes |
| proto3 | yes (with warning) | yes |
| editions | yes | yes |

**Descriptor output:**

```
fdp.dependency = ['f20_weak_import_editions_dep.proto']
fdp.weak_dependency = [0]   # index into dependency[]
```

**Rendering rule for editions:** same as proto2 — emit `import weak "..."`.
No special guard needed for editions.

---

## Part XVII — `message_set_wire_format` in editions

**Mockup file:** `f21`

**Finding:** `message_set_wire_format = true` is **accepted by protoc** in
edition 2023 files (protoc 32.1).  The option survives into the descriptor
exactly as in proto2.

```
options.HasField("message_set_wire_format") = True
options.message_set_wire_format = True
extension_range: start=4  end=2147483647  (MAX_INT32, same sentinel as proto2)
```

**Comparison:**

| Syntax | Accepted by protoc | Stored in descriptor |
|--------|-------------------|----------------------|
| proto2 | yes | yes |
| proto3 | **no** — compile error | n/a |
| editions | yes | yes |

**Rendering rule for editions:** emit `option message_set_wire_format = true;`
via the generic MessageOptions path, same as proto2.  No guard needed.

**Note:** The `extensions to max` sentinel for `message_set_wire_format`
messages stores `end = 2147483647` (MAX_INT32 = 2^31−1), distinct from the
normal proto max field number `536870912` (2^29).  See also Part XIX.

---

## Part XVIII — `deprecated_legacy_json_field_conflicts`

**Mockup files:** `f22` (proto2), `f22b` (proto3)

**Finding:** `deprecated_legacy_json_field_conflicts` is a boolean option on
`EnumOptions` and `MessageOptions`.  It survives into the descriptor in both
proto2 and proto3 when set to `true`.

```
# proto2 (f22)
enum  'LegacyJsonEnum':  HasField("deprecated_legacy_json_field_conflicts") = True, value = True
message 'LegacyJsonMsg': HasField("deprecated_legacy_json_field_conflicts") = True, value = True

# proto3 (f22b)
message 'LegacyJsonMsgP3': HasField("deprecated_legacy_json_field_conflicts") = True, value = True
```

**Proto3 note:** When a proto3 enum has case-conflicting names, protoc emits
a hard **error** (not just a warning).  `deprecated_legacy_json_field_conflicts`
on `EnumOptions` is therefore effectively proto2-only in practice, but it is
not syntactically rejected on the option itself — the enum body is the source
of the error.

**Rendering rule:** flow through the generic `EnumOptions` / `MessageOptions`
rendering path unchanged.  No syntax-specific guard is needed.

---

## Part XIX — Extension range `end` value semantics

**Mockup file:** `f23`

**Finding:** The descriptor stores `end` as **exclusive** (one past the last
field number in the range).

| Source | Stored `start` | Stored `end` | Note |
|--------|---------------|-------------|------|
| `extensions 100 to 199;` | 100 | 200 | 199 + 1 = 200 |
| `extensions 1000 to 1999;` | 1000 | 2000 | 1999 + 1 = 2000 |
| `extensions 2000 to max;` | 2000 | 536870912 | 536870911 + 1 = 536870912 |
| `extensions 4 to max;` (msg_set) | 4 | 2147483647 | MAX_INT32 sentinel |

**Key findings:**

1. **`end` is always `source_value + 1`** (exclusive upper bound).  When
   rendering, compute `source_end = stored_end - 1` and emit
   `extensions N to M;` where `M = stored_end - 1`.

2. **`to max` normal sentinel = `536870912`** (= 2^29 = proto maximum field
   number).  Render as `extensions N to max;` when `stored_end == 536870912`.

3. **`to max` message_set sentinel = `2147483647`** (= MAX_INT32 = 2^31−1).
   This is used when `message_set_wire_format = true` is set and the source
   says `extensions N to max;`.  Render as `extensions N to max;` when
   `stored_end == 2147483647`.

4. **Rendering rule:**
   ```python
   MAX_NORMAL    = 536870912    # 2^29
   MAX_MSG_SET   = 2147483647   # 2^31 - 1
   if end in (MAX_NORMAL, MAX_MSG_SET):
       render "to max"
   else:
       render f"to {end - 1}"
   ```

---

## Part XX — `DescriptorProto.visibility` / `EnumDescriptorProto.visibility`

**Finding:** The `visibility` field (field number 11, enum `Visibility`) is
present in protobuf 32.x but is **not set** by protoc for any existing syntax.

```
Visibility enum values:
  VISIBILITY_UNSET  = 0
  VISIBILITY_LOCAL  = 1
  VISIBILITY_EXPORT = 2
```

Observed values across proto2 (f01), proto3 (f03), and edition 2023 (f04):

```
All messages and enums: visibility = 0 (VISIBILITY_UNSET)
```

**Rendering rule:** This field is not set by protoc 32.1 in any syntax.
No rendering action needed.  The field appears to be reserved for future
use (possibly for module visibility in a future edition).  When `visibility`
becomes non-zero, it should be rendered as a message/enum option or keyword
depending on the final proto grammar — defer until the feature is stable.
