// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Message-valued custom options round trip (spec 0424 S1).
//!
//! A custom option whose type is a message is printed as an aggregate,
//! `option (.pkg.opt) = { ... };`. Each test compiles a fixture with protoc,
//! renders it with protoback, compiles protoback's output again, and
//! requires the same descriptor set: the aggregate must say what the options
//! bytes say.
//!
//! Ported from `test_aggregate_options.py`.

use crate::common::tmp;
use crate::common::tools;
use crate::test_roundtrip::{run_roundtrip, TOOLS};

const HEADER: &str = r#"syntax = "proto2";

package agg;

import "google/protobuf/descriptor.proto";
"#;

fn roundtrip(test: &str, name: &str, body: &str) {
    if !tools(TOOLS) {
        return;
    }
    let dir = tmp(test);
    let content = format!("{HEADER}{body}");
    run_roundtrip(
        name,
        &content,
        &dir.join("orig"),
        &dir.join("new"),
        &[],
        true,
        false,
    );
}

/// A map field inside the value: one entry per key, `key` and `value`.
#[test]
fn test_aggregate_with_a_map_field() {
    roundtrip(
        "test_aggregate_with_a_map_field",
        "aggregate_map.proto",
        r#"
message Target {
  option (.agg.opt) = { counts { key: "b" value: 2 } counts { key: "a" value: 1 } counts { key: "c" value: 3 } };
}

message Agg {
  map<string, int32> counts = 1;
}

extend google.protobuf.MessageOptions {
  optional Agg opt = 50001;
}
"#,
    );
}

/// An extension inside the value: named `[pkg.ext]`.
#[test]
fn test_aggregate_with_an_extension() {
    roundtrip(
        "test_aggregate_with_an_extension",
        "aggregate_extension.proto",
        r#"
message Target {
  option (.agg.opt) = { n: 2 [agg.tag]: "t" };
}

message Agg {
  optional int32 n = 1;

  extensions 100 to 199;
}

extend Agg {
  optional string tag = 100;
}

extend google.protobuf.MessageOptions {
  optional Agg opt = 50001;
}
"#,
    );
}

/// A group inside the value: named by its type, its fields inside.
#[test]
fn test_aggregate_with_a_group() {
    roundtrip(
        "test_aggregate_with_a_group",
        "aggregate_group.proto",
        "
message Target {
  option (.agg.opt) = { G { x: 5 } };
}

message Agg {
  optional group G = 1 {
    optional int32 x = 2;
  }
}

extend google.protobuf.MessageOptions {
  optional Agg opt = 50001;
}
",
    );
}
