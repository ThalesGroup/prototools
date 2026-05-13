<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Known prost / prost-reflect issues

## 1. `is_packed()` returns false when `options {}` is present but empty (proto3)

**Crate:** `prost-reflect`
**Observed version:** 0.16.3

### Background

In proto3, `repeated` scalar fields (including enums) are packed by default.
`prost-reflect` computes `is_packed` as:

```rust
field.options.as_ref().map_or(syntax == Syntax::Proto3, |o| o.value.packed())
```

When no field options are present (`options` absent in the FDS), `as_ref()`
returns `None` and the expression correctly falls back to `syntax == Proto3`.

However, when a repeated scalar field carries a custom option (e.g.
`google.api.field_behavior`), `protoc` emits `options { <extension bytes> }`
for that field.  prost-reflect deserializes `FieldOptions` knowing only the
standard fields; it does not recognize the extension.  The resulting
`FieldOptions` struct has no `packed` entry, so `as_ref()` returns `Some` and
`o.value.packed()` returns `false` (the default for an absent bool) —
**incorrect for proto3**.

### Observed impact

`DynamicMessage::encode_to_vec()` encodes the repeated field as individual
VARINTs (wire_type=0) instead of a packed LEN record (wire_type=2), even
though proto3 mandates packed encoding by default.

### Concrete example

`google.analytics.data.v1alpha.ReportTask.ReportDefinition`, field 9
(`repeated MetricAggregation metric_aggregations`), proto3 syntax, no explicit
`[packed=…]` annotation.  The FDS contains a non-empty `options` message for
this field (carrying extension bytes from custom annotations elsewhere in the
file).  `prost-reflect` returns `is_packed = false`; `protoc` encoding is
packed.

### Minimal reproducer

Two `.proto` files and a small Rust program.

**`google/api/field_behavior.proto`** (minimal stub):

```proto
syntax = "proto2";
package google.api;
import "google/protobuf/descriptor.proto";
extend google.protobuf.FieldOptions {
  repeated FieldBehavior field_behavior = 1052;
}
enum FieldBehavior {
  FIELD_BEHAVIOR_UNSPECIFIED = 0;
  REQUIRED = 2;
  OUTPUT_ONLY = 3;
}
```

**`example.proto`**:

```proto
syntax = "proto3";
package example;
import "google/api/field_behavior.proto";
enum Color { COLOR_UNSPECIFIED = 0; RED = 1; GREEN = 2; BLUE = 3; }
message Widget {
  string name = 1;
  // Repeated enum, no explicit [packed=…].
  // Custom option causes protoc to emit options { field_behavior: OUTPUT_ONLY }
  // for this field in the FDS.
  repeated Color tags = 2 [(google.api.field_behavior) = OUTPUT_ONLY];
}
```

Compile the descriptor:

```bash
protoc -I. --include_imports --descriptor_set_out=example.desc example.proto
```

**`src/main.rs`**:

```rust
use prost_reflect::{DescriptorPool, DynamicMessage, Value};
use prost_reflect::prost::Message;

fn main() {
    let desc_bytes = std::fs::read("example.desc").unwrap();
    let pool = DescriptorPool::decode(desc_bytes.as_slice()).unwrap();
    let msg_desc = pool.get_message_by_name("example.Widget").unwrap();
    let tags_field = msg_desc.get_field_by_name("tags").unwrap();

    println!("is_packed: {}", tags_field.is_packed());   // prints: false  (BUG)
    println!("expected : true");                         // proto3 default

    // Encoding confirms the bug: prost-reflect emits non-packed VARINTs.
    let mut msg = DynamicMessage::new(msg_desc.clone());
    msg.set_field(
        &tags_field,
        Value::List(vec![Value::EnumNumber(1), Value::EnumNumber(2)]),
    );
    let encoded = msg.encode_to_vec();
    // With the bug:   10 01  10 02  (two field-2 VARINTs, wire_type=0)
    // Correct proto3: 12 02 01 02  (field-2 LEN record, wire_type=2, packed)
    println!("encoded (hex): {}", hex::encode(&encoded));
}
```

**Why it triggers:** `protoc` stores the `field_behavior` extension in the
`FieldOptions` of `tags`, so the FDS has `options` present for that field.
prost-reflect deserializes `FieldOptions` using only the fields it knows about;
it does not know extension 1052.  Its `FieldOptions` struct therefore has no
`packed` entry — `o.value.packed()` returns `false`.  The proto3 default
(`true`) is never applied.

---

## 2. No support for `edition` syntax (proto editions)

**Crates:** `prost`, `prost-reflect`
**Observed version:** prost 0.13, prost-reflect 0.16.3

Proto editions (introduced in protobuf 3.21 / `edition = "2023"`) are not
supported by prost or prost-reflect.  Any `.proto` file using `edition =
"2023"` (or later) instead of `syntax = "proto2"` / `syntax = "proto3"` will
fail to compile or parse correctly.
