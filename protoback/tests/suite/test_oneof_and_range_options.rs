// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Custom options on oneofs and extension ranges round trip (spec 0424 S1).
//!
//! Each test compiles a fixture with protoc, renders it with protoback,
//! compiles protoback's output again, and requires the same descriptor set:
//! a custom option protoback leaves out is missing from the second one.
//!
//! Ported from `test_oneof_and_range_options.py`.

use crate::common::tmp;
use crate::common::tools;
use crate::test_roundtrip::{run_roundtrip, TOOLS};

const HEADER: &str = r#"syntax = "proto2";

package opts;

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

#[test]
fn test_custom_option_on_a_oneof() {
    roundtrip(
        "test_custom_option_on_a_oneof",
        "oneof_option.proto",
        "
message Target {
  oneof choice {
    option (.opts.oneof_opt) = 7;

    int32 a = 1;
    string b = 2;
  }
}

extend google.protobuf.OneofOptions {
  optional int32 oneof_opt = 50001;
}
",
    );
}

#[test]
fn test_custom_option_on_an_extension_range() {
    roundtrip(
        "test_custom_option_on_an_extension_range",
        "range_option.proto",
        r#"
message Target {
  extensions 100 to 199 [(.opts.range_opt) = "r"];
}

extend google.protobuf.ExtensionRangeOptions {
  optional string range_opt = 50002;
}
"#,
    );
}
