// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A schema DB keeps editions files as editions (spec 0421).
//!
//! Before it, --schema-db-out converted every editions file to proto2 for
//! prost-reflect. The conversion is lossy: a delimited message field that
//! is not a group has no proto2 spelling, so prototext decoded it as a
//! TYPE_MISMATCH. These tests build a DB from an editions 2023 and an
//! editions 2024 file, check that it and the .proto files beside it keep
//! their editions, and decode a message of each through the prototext
//! binary (spec 0488 D4). --force-proto2-for-editions, and its alias
//! --prost-workaround, still give the proto2 form for older readers.
//!
//! Ported from `test_schema_db_editions.py`; its tests' names lower-cased.

use crate::common::{protoback, prototext, set_files, tmp, tools, write};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const ED2023_PROTO: &str = r#"edition = "2023";
package ed2023;
option features.repeated_field_encoding = EXPANDED;
message Inner { int32 a = 1; }
message Fields {
  repeated int32 expanded = 1;
  repeated int32 packed = 2 [features.repeated_field_encoding = PACKED];
  Inner delimited = 3 [features.message_encoding = DELIMITED];
  int32 implicit = 4 [features.field_presence = IMPLICIT];
}
"#;

const ED2024_PROTO: &str = r#"edition = "2024";
package ed2024;
import "ed2023.proto";
option features.message_encoding = DELIMITED;
message Holder {
  ed2023.Inner delimited = 1;
  repeated sint64 packed = 2;
}
"#;

/// Fields{ expanded: [1, 2], packed: [3, 4], delimited { a: 5 }, implicit:
/// 6 }, field 3 as a group (start tag 0x1b, end tag 0x1c).
const FIELDS_PAYLOAD: &[u8] = b"\x08\x01\x08\x02\x12\x02\x03\x04\x1b\x08\x05\x1c\x20\x06";
/// Holder{ delimited { a: 7 }, packed: [1, -1] }, field 1 as a group.
const HOLDER_PAYLOAD: &[u8] = b"\x0b\x08\x07\x0c\x12\x02\x02\x01";

const EDITION_UNKNOWN: i32 = 0;
const EDITION_2023: i32 = 1000;
const EDITION_2024: i32 = 1001;

/// protoback on the two files (compiled once per test directory); the DB's
/// directory.
fn build_db(dir: &Path, name: &str, flags: &[&str]) -> PathBuf {
    let pb = dir.join("in.pb");
    if !pb.exists() {
        write(&dir.join("src/ed2023.proto"), ED2023_PROTO);
        write(&dir.join("src/ed2024.proto"), ED2024_PROTO);
        let o = std::process::Command::new("protoc")
            .arg(format!("-I{}", dir.join("src").display()))
            .arg(format!("--descriptor_set_out={}", pb.display()))
            .args(["ed2023.proto", "ed2024.proto"])
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "protoc failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    let out = dir.join(name);
    let mut args: Vec<String> = vec!["--use-variant".into(), "descriptor".into()];
    args.extend(flags.iter().map(|f| f.to_string()));
    args.extend([
        format!("--proto-out={}", out.join("proto").display()),
        format!("--schema-db-out={}", out.join("db.desc").display()),
        pb.display().to_string(),
    ]);
    let r = protoback(&args);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    out
}

/// Each file's syntax (proto2 when unset) and edition.
fn syntaxes(out: &Path) -> BTreeMap<String, (String, i32)> {
    set_files(&out.join("db.desc"))
        .into_iter()
        .map(|f| {
            let syntax = if f.syntax.is_empty() {
                "proto2".to_string()
            } else {
                f.syntax
            };
            (f.name, (syntax, f.edition))
        })
        .collect()
}

fn first_line(proto: &Path) -> String {
    std::fs::read_to_string(proto)
        .unwrap()
        .lines()
        .find(|l| l.starts_with("edition") || l.starts_with("syntax"))
        .unwrap()
        .to_string()
}

fn decode(out: &Path, type_name: &str, payload: &[u8]) -> String {
    let blob = out.join(format!("{type_name}.bin"));
    std::fs::write(&blob, payload).unwrap();
    let o = prototext([
        "--descriptor-set".into(),
        out.join("db.desc").into_os_string(),
        "decode".into(),
        "--type".into(),
        type_name.into(),
        "--assume-binary".into(),
        blob.into_os_string(),
    ]);
    assert_eq!(o.code, Some(0), "prototext failed:\n{}", o.stderr);
    o.stdout
}

fn pair(syntax: &str, edition: i32) -> (String, i32) {
    (syntax.to_string(), edition)
}

/// The DB and the .proto files beside it keep each file's edition.
#[test]
fn test_s1_schema_db_keeps_editions() {
    if !tools(&["protoc", "prototext"]) {
        return;
    }
    let out = build_db(&tmp("test_s1_schema_db_keeps_editions"), "editions", &[]);
    let syntaxes = syntaxes(&out);
    assert_eq!(syntaxes["ed2023.proto"], pair("editions", EDITION_2023));
    assert_eq!(syntaxes["ed2024.proto"], pair("editions", EDITION_2024));
    assert_eq!(
        first_line(&out.join("proto/ed2023.proto")),
        r#"edition = "2023";"#
    );
    assert_eq!(
        first_line(&out.join("proto/ed2024.proto")),
        r#"edition = "2024";"#
    );
}

/// Delimited messages decode as their type, packed and expanded fields as
/// declared: nothing reads as a mismatch.
#[test]
fn test_s1_prototext_decodes_delimited_and_packed() {
    if !tools(&["protoc", "prototext"]) {
        return;
    }
    let out = build_db(
        &tmp("test_s1_prototext_decodes_delimited_and_packed"),
        "editions",
        &[],
    );
    let fields = decode(&out, "ed2023.Fields", FIELDS_PAYLOAD);
    assert!(!fields.contains("TYPE_MISMATCH"), "{fields}");
    for s in [
        "delimited {  #@ group; Inner = 3",
        "expanded: 2  #@ repeated int32 = 1",
        "packed: 3  #@ repeated int32 [packed=true] = 2",
        "implicit: 6",
    ] {
        assert!(fields.contains(s), "{s}:\n{fields}");
    }
    let holder = decode(&out, "ed2024.Holder", HOLDER_PAYLOAD);
    assert!(!holder.contains("TYPE_MISMATCH"), "{holder}");
    assert!(
        holder.contains("delimited {  #@ group; Inner = 1"),
        "{holder}"
    );
    assert!(
        holder.contains("packed: -1  #@ repeated sint64 [packed=true] = 2"),
        "{holder}"
    );
}

/// --force-proto2-for-editions applies to the DB, as to the rest of the
/// output; its alias --prost-workaround gives the same DB (S3).
#[test]
fn test_s2_flag_still_gives_proto2() {
    if !tools(&["protoc", "prototext"]) {
        return;
    }
    let dir = tmp("test_s2_flag_still_gives_proto2");
    let flagged = build_db(&dir, "flag", &["--force-proto2-for-editions"]);
    let expected: BTreeMap<String, (String, i32)> = [
        ("ed2023.proto".to_string(), pair("proto2", EDITION_UNKNOWN)),
        ("ed2024.proto".to_string(), pair("proto2", EDITION_UNKNOWN)),
    ]
    .into();
    assert_eq!(syntaxes(&flagged), expected);
    assert_eq!(
        first_line(&flagged.join("proto/ed2024.proto")),
        r#"syntax = "proto2";"#
    );
    let alias = build_db(&dir, "alias", &["--prost-workaround"]);
    assert!(
        std::fs::read(alias.join("db.desc")).unwrap()
            == std::fs::read(flagged.join("db.desc")).unwrap()
    );
}
