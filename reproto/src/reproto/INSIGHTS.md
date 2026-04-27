<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# Reproto Implementation Subtleties

This document describes non-obvious design choices and tricky implementation details in reproto.

## Table of Contents

1. [Element Ordering in Message Descriptors](#element-ordering-in-message-descriptors)
2. [Proto3/Editions → Proto2 Decompilation](#proto3editions--proto2-decompilation)

---

## Element Ordering in Message Descriptors

### Summary

**Key Finding**: Protoc ALWAYS groups descriptor elements by type, regardless of source file ordering.

**Proof**: We compiled two .proto files with identical content but different element ordering (nested messages/enums before vs. after fields). Both produced IDENTICAL descriptors (except filename).

**Implication for Reproto**: We use a **static rendering order** that matches protoc's descriptor structure. This guarantees perfect roundtripping for ANY input proto file, regardless of original source order. We do NOT need to parse `source_code_info` for element ordering.

### Investigation

**Test Setup**: Created two proto files with different element orderings:

**Version 1** (nested messages/enums BEFORE fields):
```protobuf
message OrderTest {
  message Nested1 { optional string value = 1; }
  enum Enum1 { UNKNOWN = 0; }

  optional int32 field1 = 1;
  optional Nested1 field2 = 2;
  optional Enum1 field3 = 3;

  message Nested2 { optional int32 count = 1; }
}
```

**Version 2** (fields BEFORE nested messages/enums):
```protobuf
message OrderTest {
  optional int32 field1 = 1;
  optional Nested1 field2 = 2;
  optional Enum1 field3 = 3;

  message Nested1 { optional string value = 1; }
  message Nested2 { optional int32 count = 1; }
  enum Enum1 { UNKNOWN = 0; }
}
```

**Result**: Both compile to IDENTICAL descriptors (except filename). The decoded text shows protoc ALWAYS outputs:

```
message_type {
  field { ... field1 ... }
  field { ... field2 ... }
  field { ... field3 ... }
  nested_type { ... Nested1 ... }
  nested_type { ... Nested2 ... }
  enum_type { ... Enum1 ... }
}
```

### Reproto's Design Choice

**Static Order** (hard-coded, does not depend on source_code_info):
1. Message-level comments (from `source_code_info`, if available - purely additive)
2. Extension blocks (`extend OtherMessage { ... }`)
3. Message options
4. Fields and oneofs (interleaved by field number)
5. Nested messages
6. Nested enums
7. Reserved ranges and names

**Rationale**:
1. **Protoc enforces grouping** - Since protoc ALWAYS groups elements by type in the descriptor (all fields together, all nested_type together, etc.), ANY source ordering compiles to the SAME descriptor structure
2. **Guarantees perfect roundtripping** - Our static order matches protoc's descriptor structure, so it will always roundtrip correctly regardless of original source order
3. **Simpler and more robust** - No need to parse `source_code_info`, sort by line numbers, or handle missing data
4. **source_code_info is optional** - It requires `protoc --include_source_info` flag, so reproto cannot depend on it for correctness

**What we DO preserve**:
- **Relative order within each type**: If `Nested1` appears before `Nested2` in `nested_type[]`, they'll be rendered in that order
- **Field numbers**: Fields are rendered in the order they appear in the descriptor's `field[]` array
- **Comments** (when available): Extracted from `source_code_info` and added to output

**What we do NOT attempt**:
- **Reconstructing arbitrary source order**: We don't try to interleave nested messages, enums, and fields based on original source positions
- **Depending on source_code_info**: The tool works correctly even when compiled without `--include_source_info`

### Special Cases

#### Map Entries

Map fields like `map<string, int32> my_map = 1;` are syntactic sugar for:
```protobuf
message MyMapEntry {
  optional string key = 1;
  optional int32 value = 2;
}
repeated MyMapEntry my_map = 1;
```

These synthetic `MyMapEntry` messages appear in `nested_type[]` and must be filtered out during rendering (they're rendered as `map<K,V>` syntax instead).

#### Groups

Proto2 groups like `repeated group MyGroup = 1 { ... }` create both:
- A field with `type = TYPE_GROUP`
- A nested message with the group's name

Groups are rendered inline with fields, not in the nested messages section.

#### Oneofs

Oneof fields must be kept together and rendered within `oneof { }` blocks. We track which oneofs have been rendered to avoid duplicating them.

---

## Proto3/Editions → Proto2 Decompilation

### Overview

Reproto currently reconstructs all protobuf descriptor sets as proto2 syntax, regardless of the original syntax used. This is a deliberate design choice for consistency and simplicity.

**Key Design Choice**: Proto2-only output ensures consistent, compilable results for all inputs without syntax-specific rendering logic.

### Known Tricky Issues

#### 1. ExtensionRangeOptions Cannot Be Set in Proto2

**The Trap**: You might think extension ranges should render with options like `[verification = DECLARATION]`, but this FAILS in proto2.

**Why**: ExtensionRangeOptions fields (`verification`, `declaration`, `features`) are defined with `retention = RETENTION_SOURCE` in descriptor.proto. They're internal to protoc and not meant for explicit use in user proto files.

**Attempting this causes**:
```protobuf
// This FAILS in proto2:
message TestMessage {
  extensions 100 to 199 [verification = DECLARATION];  // ERROR: unknown option
}

// This WORKS in proto2:
message TestMessage {
  extensions 100 to 199;  // Protoc handles options internally
}
```

**Resolution**: Reproto correctly renders only `extensions start to end;` without options. Protoc automatically generates internal options when compiling. Proto2 roundtrips work perfectly this way.

#### 2. Proto3 Optional Fields Create Synthetic Oneofs

**The Trap**: Proto3's `optional` keyword has different semantics than proto2's `optional`.

**Details**:
- Proto3 optional fields generate synthetic oneofs with `proto3_optional=true`
- The synthetic oneof names start with underscore (e.g., `_field_name`)
- When decompiled to proto2, these appear as explicit oneofs
- Recompiling adds `oneof_index` and `proto3_optional` fields to the descriptor

**Impact**: Proto3 files with optional fields will have slightly different descriptors when roundtripped through proto2 (but remain functionally equivalent).

#### 3. Edition Metadata Is Lost

**The Trap**: Protobuf Editions information (edition field, features) cannot be represented in proto2 syntax.

**Current Handling**: Edition information is preserved only as a comment in the output. The actual syntax is rendered as `syntax = "proto2";`.

### Future: Syntax Preservation (Low Priority)

Preserving original syntax (proto3, editions) when decompiling would require:
- Syntax-aware rendering throughout the codebase
- Handling many features that don't map cleanly between syntaxes
- Increased complexity

**Decision**: Keep proto2-only for now. The current approach provides consistent, compilable output for all inputs. May revisit if there's strong user demand.

---

## Related Documentation

- [globals.py](./globals.py) - Constants used throughout reproto (MESSAGE, FIELD, ENUM, FILE prefixes)
- [descriptor.proto specification](https://github.com/protocolbuffers/protobuf/blob/main/src/google/protobuf/descriptor.proto)
