// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Editions feature resolution, end to end (spec 0025): protoback's
//! `--dump-resolved-features` against the golden `editions_resolution.yaml`,
//! and features flowing into a nested message (spec 0424 S1).
//!
//! Ported from `test_feature_resolution.py`, its command-line tests; the
//! YAML compared as data, read with `serde_yaml` where it used PyYAML. Its
//! unit tests (T1 to T7) are in `protoback/src/phase2.rs`, on the functions
//! that port reproto's `build_edition_defaults` and `resolve_features`.

use crate::common::{fixture, protoback_command, tmp, tools, write};
use serde_yaml::Value;
use std::path::Path;

/// `proto` (under `dir`) compiled with its imports, then protoback's
/// resolved features of `name`, as data.
fn dump(dir: &Path, proto: &Path, name: &str) -> Value {
    let pb = proto.with_extension("pb");
    let o = std::process::Command::new("protoc")
        .arg(format!("--descriptor_set_out={}", pb.display()))
        .arg("--include_imports")
        .arg(format!("-I{}", dir.display()))
        .arg(proto)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "protoc failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let o = protoback_command([
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", dir.display()),
        "--dump-resolved-features".into(),
        name.to_string(),
        pb.display().to_string(),
    ])
    .output()
    .unwrap();
    assert!(
        o.status.success(),
        "protoback failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    serde_yaml::from_slice(&o.stdout).unwrap()
}

#[test]
fn test_editions_resolution_golden() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_editions_resolution_golden").join("orig");
    let proto = dir.join("editions_resolution.proto");
    write(
        &proto,
        &std::fs::read_to_string(fixture("editions_resolution.proto")).unwrap(),
    );
    let actual = dump(&dir, &proto, "editions_resolution.proto");
    let golden: Value = serde_yaml::from_str(
        &std::fs::read_to_string(fixture("editions_resolution.yaml")).unwrap(),
    )
    .unwrap();
    assert!(
        actual == golden,
        "YAML output differs from golden.\n--- golden ---\n{golden:?}\n--- actual ---\n{actual:?}"
    );
}

/// Features flow file, each enclosing message outermost first, then field
/// or enum (spec 0424 S1), as protoc and upb resolve them; `json_format` is
/// the one core feature a message can set.
#[test]
fn test_nested_message_inherits_the_enclosing_messages_features() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_nested_message_inherits_the_enclosing_messages_features");
    let proto = dir.join("nested_features.proto");
    write(
        &proto,
        "edition = \"2023\";\npackage nested_features;\nmessage Outer {\n  option features.json_format = LEGACY_BEST_EFFORT;\n  message Inner {\n    int32 x = 1;\n    enum E { E_ZERO = 0; }\n  }\n}\n",
    );
    let doc = dump(&dir, &proto, "nested_features.proto");
    assert!(doc.is_mapping(), "{doc:?}");
    let inner = &doc["messages"]["Outer"]["messages"]["Inner"];
    for v in [
        &inner["resolved"],
        &inner["fields"]["x"]["resolved"],
        &inner["enums"]["E"]["resolved"],
    ] {
        assert_eq!(
            v["json_format"].as_str(),
            Some("LEGACY_BEST_EFFORT"),
            "{inner:?}"
        );
    }
}
