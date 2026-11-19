// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0447 S3, S5: `--target portable`, protoback's alone. The vendor
//! descriptor is kept as an ordinary file; the options messages custom
//! options extend are mapped to protoc's, with protoc's descriptor
//! imported; what protoc's descriptor cannot represent is redacted (W28),
//! the binary keeping it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A vendor's descriptor.proto, small: `FieldOptions` has a field protoc's
/// lacks (`vendor_only`, 90) and its `CType` a value protoc's lacks
/// (`VENDOR_TYPE`, 7).
const VENDOR: &str = r#"name: "vendor/proto/descriptor.proto" package: "vendor"
message_type { name: "FileOptions"
  field { name: "java_package" number: 1 label: LABEL_OPTIONAL type: TYPE_STRING }
  extension_range { start: 1000 end: 536870912 } }
message_type { name: "FieldOptions"
  field { name: "ctype" number: 1 label: LABEL_OPTIONAL type: TYPE_ENUM
    type_name: ".vendor.FieldOptions.CType" }
  field { name: "deprecated" number: 3 label: LABEL_OPTIONAL type: TYPE_BOOL }
  field { name: "vendor_only" number: 90 label: LABEL_OPTIONAL type: TYPE_BOOL }
  enum_type { name: "CType" value { name: "STRING" number: 0 }
    value { name: "CORD" number: 1 } value { name: "STRING_PIECE" number: 2 }
    value { name: "VENDOR_TYPE" number: 7 } }
  extension_range { start: 1000 end: 536870912 } }"#;

/// A custom option on the vendor's FieldOptions, set; the vendor-only field
/// and value set; a stock field set. By number: the small descriptor above
/// is not one the text read resolves option names against.
const USER: &str = r#"name: "c.proto" package: "c" dependency: "vendor/proto/descriptor.proto"
extension { name: "tag" number: 50000 label: LABEL_OPTIONAL type: TYPE_INT32
  extendee: ".vendor.FieldOptions" }
message_type { name: "M"
  field { name: "f" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32
    options { 3: 1 50000: 7 } }
  field { name: "g" number: 2 label: LABEL_OPTIONAL type: TYPE_INT32
    options { 90: 1 } }
  field { name: "h" number: 3 label: LABEL_OPTIONAL type: TYPE_STRING
    options { 1: 7 } }
  field { name: "i" number: 4 label: LABEL_OPTIONAL type: TYPE_STRING
    options { 1: 1 } } }"#;

fn setup(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("portable-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    std::fs::write(dir.join("in/vdesc.textpb"), VENDOR).unwrap();
    std::fs::write(dir.join("in/c.textpb"), USER).unwrap();
    dir
}

struct Run {
    status: i32,
    stderr: String,
    out: PathBuf,
    db: PathBuf,
}

fn run(dir: &Path, args: &[&str]) -> Run {
    let out = dir.join("out");
    let db = dir.join("db.desc");
    let o = Command::new(env!("CARGO_BIN_EXE_protoback"))
        .arg("-I")
        .arg(dir.join("in"))
        .arg("-O")
        .arg(&out)
        .arg(format!("--schema-db-out={}", db.display()))
        .args(args)
        .env_remove("REPROTO_VARIANT")
        .output()
        .unwrap();
    Run {
        status: o.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        out,
        db,
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn portable_maps_the_extendees_and_imports_protocs_descriptor() {
    let dir = setup("map");
    let r = run(&dir, &["--target", "portable"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    let c = read(&r.out.join("c.proto"));
    assert!(c.contains("extend google.protobuf.FieldOptions {"), "{c}");
    assert!(
        c.contains("import \"google/protobuf/descriptor.proto\";"),
        "{c}"
    );
    assert!(
        c.contains("import \"vendor/proto/descriptor.proto\";"),
        "{c}"
    );
    assert!(c.contains("(c.tag) = 7"), "{c}");
    // The vendor descriptor kept, an ordinary file; protoc's not on disk.
    let v = read(&r.out.join("vendor/proto/descriptor.proto"));
    assert!(v.contains("package vendor;"), "{v}");
    assert!(!r.out.join("google/protobuf/descriptor.proto").exists());
}

#[test]
fn portable_redacts_what_protocs_descriptor_lacks() {
    let dir = setup("redact");
    let r = run(&dir, &["--target", "portable"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    for expected in [
        "'c.proto': option vendor_only: field 90 of FieldOptions is not in protoc's \
         descriptor: written as an orphan (--target portable)",
        "'c.proto': option ctype: value 7 of field 1 of FieldOptions is not in protoc's \
         descriptor: written as an orphan (--target portable)",
    ] {
        assert!(
            r.stderr.contains(expected),
            "missing: {expected}\n{}",
            r.stderr
        );
    }
    let c = read(&r.out.join("c.proto"));
    let line = |needle: &str| {
        c.lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no {needle}:\n{c}"))
            .trim()
            .to_string()
    };
    assert!(line("vendor_only").starts_with("//"), "{c}");
    assert!(line("VENDOR_TYPE").starts_with("//"), "{c}");
    // A value protoc's enum has: written.
    assert!(!line("CORD").starts_with("//"), "{c}");
    // The binary keeps the input's options: field 90 = 1.
    let db = std::fs::read(&r.db).unwrap();
    assert!(
        db.windows(3).any(|w| w == [0xd0, 0x05, 0x01]),
        "field 90 lost from the binary"
    );
}

#[test]
fn keep_redacts_nothing() {
    let dir = setup("keep");
    let r = run(&dir, &[]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert!(!r.stderr.contains("W28"), "{}", r.stderr);
    let c = read(&r.out.join("c.proto"));
    assert!(c.contains("extend vendor.FieldOptions {"), "{c}");
}

#[test]
fn portable_with_keep_descriptor_path_is_a_usage_error() {
    let dir = setup("exclusive");
    let r = run(&dir, &["--target", "portable", "--keep-descriptor-path"]);
    assert_eq!(r.status, 2, "{}", r.stderr);
    assert!(
        r.stderr
            .contains("--target portable and --keep-descriptor-path (--target keep) are exclusive"),
        "{}",
        r.stderr
    );
}
