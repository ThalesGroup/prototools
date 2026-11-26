// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0447 S3 and S4: the targets, and the import rule.
//!
//! On a vendor fixture (protoc's descriptor as package `vendor`, with a
//! FieldOptions field only it declares), --target protoc writes the vendor
//! descriptor as google/protobuf/descriptor.proto, renames its definitions
//! alone, and imports it where a file sets that field: stock protoc then
//! compiles each output file on its own. --target keep adds nothing, but
//! the import under --import-descriptor.
//!
//! Ported from `test_targets.py`. Its last test, that reproto refuses
//! `--target portable`, was skipped against protoback, which implements it
//! (`protoback/tests/portable.rs`): reproto-only, not ported.

use crate::common::{
    protoback, protoback_command, stock_include, tmp, tools, vendor_descriptor, write, Run,
};
use std::path::{Path, PathBuf};

const VENDOR_ONLY: &str = "  optional bool vendor_only = 90;\n";
/// A custom option, and a field of a vendor descriptor's type.
const CUSTOM: &str = r#"name: "c.proto" package: "c" dependency: "vendor/proto/descriptor.proto"
extension { name: "tag" number: 50000 label: LABEL_OPTIONAL type: TYPE_INT32
  extendee: ".vendor.FieldOptions" }
message_type { name: "M"
  field { name: "f" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32
    options { deprecated: true 50000: 7 } }
  field { name: "m" number: 2 label: LABEL_OPTIONAL type: TYPE_ENUM
    type_name: ".vendor.FileOptions.OptimizeMode" } }
"#;
/// The vendor-only built-in field, without importing the descriptor.
const VENDOR_FIELD: &str = r#"name: "d.proto" package: "d" message_type { name: "D"
  field { name: "g" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32
    options { vendor_only: true } } }
"#;
/// A stock built-in field only: no import needed.
const STOCK_FIELD: &str = r#"name: "e.proto" package: "e" message_type { name: "E"
  field { name: "h" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32
    options { deprecated: true } } }
"#;
/// Another file of package vendor, a name the descriptor's prefixes.
const SIBLING: &str = r#"name: "vendor/other.proto" package: "vendor"
message_type { name: "FileOptionsX" field { name: "k" number: 1
  label: LABEL_OPTIONAL type: TYPE_INT32 } }
"#;

const ALL: &[(&str, &str)] = &[
    ("c.textpb", CUSTOM),
    ("d.textpb", VENDOR_FIELD),
    ("e.textpb", STOCK_FIELD),
    ("other.textpb", SIBLING),
];

/// The vendor fixture and `files` in `dir/in`, protoc's own descriptor too
/// if `stock_too`; rendered with `flags`. None without protoc.
fn run(
    test: &str,
    flags: &[&str],
    files: &[(&str, &str)],
    stock_too: bool,
) -> Option<(PathBuf, Run)> {
    if !tools(&["protoc"]) {
        return None;
    }
    let dir = tmp(test);
    vendor_descriptor(&dir, VENDOR_ONLY, "");
    let inp = dir.join("in");
    for (name, text) in files {
        write(&inp.join(name), text);
    }
    if stock_too {
        let o = std::process::Command::new("protoc")
            .arg(format!("-I{}", stock_include().display()))
            .arg(format!(
                "--descriptor_set_out={}",
                inp.join("stock.pb").display()
            ))
            .arg("google/protobuf/descriptor.proto")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    let mut args: Vec<String> = flags.iter().map(|f| f.to_string()).collect();
    args.extend([
        format!("-I{}", inp.display()),
        "-O".into(),
        dir.join("out").display().to_string(),
    ]);
    let r = protoback(&args);
    Some((dir, r))
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join("out").join(name)).unwrap()
}

fn imports(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.starts_with("import ")).collect()
}

fn protos(out: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![out.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "proto") {
                files.push(p.strip_prefix(out).unwrap().display().to_string());
            }
        }
    }
    files.sort();
    files
}

#[test]
fn test_protoc_output_compiles_file_by_file_with_stock_protoc() {
    let Some((dir, r)) = run(
        "test_protoc_output_compiles_file_by_file_with_stock_protoc",
        &["--target", "protoc"],
        ALL,
        false,
    ) else {
        return;
    };
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let out = dir.join("out");
    let files = protos(&out);
    assert!(
        files
            .iter()
            .any(|f| f == "google/protobuf/descriptor.proto"),
        "{files:?}"
    );
    assert!(
        !files.iter().any(|f| f == "vendor/proto/descriptor.proto"),
        "{files:?}"
    );
    for f in &files {
        let o = std::process::Command::new("protoc")
            .arg(format!("-I{}", out.display()))
            .arg("--descriptor_set_out=/dev/null")
            .arg(f)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{f}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn test_protoc_renames_the_descriptor_and_its_references() {
    let Some((dir, r)) = run(
        "test_protoc_renames_the_descriptor_and_its_references",
        &["--target", "protoc"],
        ALL,
        false,
    ) else {
        return;
    };
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(read(&dir, "google/protobuf/descriptor.proto").contains("package google.protobuf;"));
    let c = read(&dir, "c.proto");
    for s in [
        r#"import "google/protobuf/descriptor.proto";"#,
        "extend google.protobuf.FieldOptions",
        "google.protobuf.FileOptions.OptimizeMode m = 2",
    ] {
        assert!(c.contains(s), "{s}:\n{c}");
    }
}

#[test]
fn test_protoc_renames_the_descriptor_definitions_alone() {
    let Some((dir, r)) = run(
        "test_protoc_renames_the_descriptor_definitions_alone",
        &["--target", "protoc"],
        ALL,
        false,
    ) else {
        return;
    };
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let other = read(&dir, "vendor/other.proto");
    assert!(
        other.contains("package vendor;") && other.contains("message FileOptionsX"),
        "{other}"
    );
}

#[test]
fn test_import_rule_only_where_needed() {
    let Some((dir, r)) = run(
        "test_import_rule_only_where_needed",
        &["--target", "protoc"],
        ALL,
        false,
    ) else {
        return;
    };
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert_eq!(
        imports(&read(&dir, "d.proto")),
        [r#"import "google/protobuf/descriptor.proto";"#]
    );
    assert!(imports(&read(&dir, "e.proto")).is_empty());
}

#[test]
fn test_keep_adds_nothing() {
    let Some((dir, r)) = run("test_keep_adds_nothing", &[], ALL, false) else {
        return;
    };
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(imports(&read(&dir, "d.proto")).is_empty());
    assert!(read(&dir, "vendor/proto/descriptor.proto").contains("package vendor;"));
    assert!(read(&dir, "c.proto").contains("extend vendor.FieldOptions"));
}

#[test]
fn test_keep_imports_the_descriptor_on_request() {
    let Some((dir, r)) = run(
        "test_keep_imports_the_descriptor_on_request",
        &["--import-descriptor"],
        ALL,
        false,
    ) else {
        return;
    };
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert_eq!(
        imports(&read(&dir, "d.proto")),
        [r#"import "vendor/proto/descriptor.proto";"#]
    );
    assert!(imports(&read(&dir, "e.proto")).is_empty());
}

#[test]
fn test_the_input_stock_descriptor_is_dropped_with_w27() {
    let Some((dir, r)) = run(
        "test_the_input_stock_descriptor_is_dropped_with_w27",
        &["--target", "protoc"],
        ALL,
        true,
    ) else {
        return;
    };
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(
        r.stderr.contains(
            "Warning [W27]: 'google/protobuf/descriptor.proto' dropped: 'vendor/proto/descriptor.proto', \
             the descriptor in use, is written as 'google/protobuf/descriptor.proto' (--target protoc)"
        ),
        "{}",
        r.stderr
    );
    let d = read(&dir, "google/protobuf/descriptor.proto");
    assert!(
        d.contains("package google.protobuf;") && d.contains("vendor_only"),
        "{d}"
    );
}

#[test]
fn test_protoc_with_keep_descriptor_path_is_a_usage_error() {
    let Some((_, r)) = run(
        "test_protoc_with_keep_descriptor_path_is_a_usage_error",
        &["--target", "protoc", "--keep-descriptor-path"],
        ALL,
        false,
    ) else {
        return;
    };
    assert_eq!(r.code, Some(2), "{}", r.stderr);
    assert!(
        r.stderr
            .contains("--target protoc and --keep-descriptor-path (--target keep) are exclusive"),
        "{}",
        r.stderr
    );
}

#[test]
fn test_proto_variant_is_a_usage_error() {
    let Some((_, r)) = run(
        "test_proto_variant_is_a_usage_error",
        &["--proto-variant", "any.yaml"],
        ALL,
        false,
    ) else {
        return;
    };
    assert_eq!(r.code, Some(2), "{}", r.stderr);
    assert!(
        r.stderr.contains("--proto-variant: variants are gone (spec 0447); use --profile, --descriptor and --target"),
        "{}",
        r.stderr
    );
}

#[test]
fn test_reproto_variant_env_is_ignored_with_a_notice() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_reproto_variant_env_is_ignored_with_a_notice");
    vendor_descriptor(&dir, VENDOR_ONLY, "");
    let inp = dir.join("in");
    write(&inp.join("e.textpb"), STOCK_FIELD);
    let o = protoback_command([
        format!("-I{}", inp.display()),
        "-O".into(),
        dir.join("out").display().to_string(),
    ])
    .env("REPROTO_VARIANT", dir.join("gone.yaml"))
    .output()
    .unwrap();
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(0), "{stderr}");
    assert!(
        stderr
            .contains("$REPROTO_VARIANT is ignored: variants are gone (spec 0447); use --profile"),
        "{stderr}"
    );
}

#[test]
fn test_keep_descriptor_path_is_deprecated() {
    let Some((_, r)) = run(
        "test_keep_descriptor_path_is_deprecated",
        &["--keep-descriptor-path"],
        ALL,
        false,
    ) else {
        return;
    };
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(
        r.stderr
            .contains("Warning [W18]: --keep-descriptor-path is deprecated; use --target keep"),
        "{}",
        r.stderr
    );
}
