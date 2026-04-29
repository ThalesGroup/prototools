<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# Fixture coding-style guide

Rules that fixture `.proto` files must follow so that the `test_roundtrip`
proto-text comparison passes.  The comparison normalizes both sides with
`uncomment` (strips comments and blank lines) then `buf format`, so only
structural choices matter.

## 1. Definition order (protoc canonical order)

Protoc imposes a canonical order when serializing to a descriptor set.
Fixtures must follow this order so that reproto's output — which reconstructs
from the descriptor — matches the fixture after normalization.

Top-level order:

1. `syntax`, `package`, `import`, file-level `option` statements
2. Services
3. Enums
4. Messages
5. Top-level `extend` blocks

Within a message body:

1. `extend` blocks
2. Fields, oneofs, map fields, groups
3. Nested messages
4. Nested enums
5. `extensions` ranges
6. `reserved` statements

## 2. Custom option names — always fully-qualified

Always use the fully-qualified form with a leading dot:

```proto
option (.reproto.example_enum_option) = "custom";
TWO = 2 [(.reproto.test.example_value_option) = "custom value"];
```

Never use the short unqualified form `(example_enum_option)`.  Reproto always
emits full FQDNs for custom extensions (conservative, protoc-compatible
approach documented in `mappings.py`).

## 3. Reserved statements — one range or name per statement

```proto
// correct
reserved 300;
reserved 301;
reserved "OLD_VALUE";
reserved "OBSOLETE";

// wrong
reserved 300, 301;
reserved "OLD_VALUE", "OBSOLETE";
```

Reproto emits one value per `reserved` statement.

## 4. Bytes default values — `\xNN` not `\NNN`

Use hex escapes, not octal:

```proto
optional bytes b = 1 [default = "binary\x00data"];  // correct
optional bytes b = 1 [default = "binary\000data"];  // wrong
```

## 5. Multi-option field tags — always expanded

When a field carries two or more options, use the multi-line form:

```proto
// correct
repeated fixed32 packed_deprecated = 4 [
  packed = true,
  deprecated = true
];

// wrong
repeated fixed32 packed_deprecated = 4 [packed = true, deprecated = true];
```
