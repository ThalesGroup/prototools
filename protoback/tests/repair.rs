// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0469: `--repair-skew` makes the output compile despite version skew:
//! a missing import added (W33), a field of no type commented out (W34),
//! an extension out of its extendee's ranges commented out (W35).

use std::path::{Path, PathBuf};
use std::process::Command;

/// `a/row.proto`: `Row`, and `Base` with extensions 100 to 199.
const ROW: &str = r#"name: "a/row.proto" package: "a" message_type { name: "Row" }
message_type { name: "Base" extension_range { start: 100 end: 200 } }"#;

/// `b/user.proto`, not importing `a/row.proto`: a field of `a.Row`, a map
/// whose value type is in no file, extensions of `a.Base` in and out of
/// its range.
const USER: &str = r#"name: "b/user.proto" package: "b"
message_type { name: "User"
  field { name: "row" number: 1 label: LABEL_OPTIONAL type: TYPE_MESSAGE type_name: ".a.Row"
    json_name: "row" }
  field { name: "m" number: 2 label: LABEL_REPEATED type: TYPE_MESSAGE
    type_name: ".b.User.MEntry" json_name: "m" }
  field { name: "ok" number: 3 label: LABEL_OPTIONAL type: TYPE_INT32 json_name: "ok" }
  nested_type { name: "MEntry" options { map_entry: true }
    field { name: "key" number: 1 label: LABEL_OPTIONAL type: TYPE_STRING json_name: "key" }
    field { name: "value" number: 2 label: LABEL_OPTIONAL type: TYPE_MESSAGE
      type_name: ".a.Gone" json_name: "value" } } }
extension { name: "far" number: 500 label: LABEL_OPTIONAL type: TYPE_INT32 extendee: ".a.Base" }
extension { name: "near" number: 150 label: LABEL_OPTIONAL type: TYPE_INT32 extendee: ".a.Base" }"#;

struct Run {
    out: PathBuf,
    stderr: String,
    report: serde_json::Value,
}

fn run(test: &str, files: &[(&str, &str)], args: &[&str]) -> Run {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("repair-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    for (name, text) in files {
        std::fs::write(dir.join("in").join(name), text).unwrap();
    }
    let out = dir.join("out");
    let report = dir.join("report.json");
    let o = Command::new(env!("CARGO_BIN_EXE_protoback"))
        .arg("-I")
        .arg(dir.join("in"))
        .arg("-O")
        .arg(&out)
        .arg("--report-out")
        .arg(&report)
        .args(args)
        .env_remove("REPROTO_VARIANT")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
    assert_eq!(o.status.code(), Some(0), "{stderr}");
    Run {
        out,
        stderr,
        report: serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap(),
    }
}

impl Run {
    fn text(&self, file: &str) -> String {
        std::fs::read_to_string(self.out.join(file)).unwrap()
    }

    fn codes(&self, code: &str) -> Vec<serde_json::Value> {
        self.report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w["code"] == code)
            .map(|w| w["fields"].clone())
            .collect()
    }

    /// protoc's verdict on `file`, or `None` without protoc.
    fn compiles(&self, file: &str) -> Option<Result<(), String>> {
        let o = Command::new("protoc")
            .current_dir(&self.out)
            .args(["-I.", "--descriptor_set_out=/dev/null", file])
            .output()
            .ok()?;
        Some(if o.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&o.stderr).into_owned())
        })
    }
}

#[test]
fn repair_skew_repairs_and_the_output_compiles() {
    let files = [("row.textpb", ROW), ("user.textpb", USER)];
    let r = run("all", &files, &["--repair-skew"]);
    let text = r.text("b/user.proto");
    for expected in [
        "// WARNING[repair-skew]: import added: defines a.Base, a.Row, used here; not imported in the source\nimport \"a/row.proto\";",
        "  optional a.Row row = 1;",
        "  // WARNING[repair-skew]: commented out: its map value .a.Gone is defined in no file\n",
        "///repeated MEntry m = 2;",
        "  optional int32 ok = 3;",
        "// WARNING[repair-skew]: commented out: 500 is in no extension range of a.Base\n///optional int32 far = 500;",
        "  optional int32 near = 150;",
    ] {
        assert!(text.contains(expected), "missing:\n{expected}\nin:\n{text}");
    }
    let w33 = r.codes("W33");
    assert_eq!(w33.len(), 1, "{}", r.stderr);
    assert_eq!(w33[0]["import"], "a/row.proto");
    assert_eq!(r.codes("W34").len(), 1, "{}", r.stderr);
    assert_eq!(r.codes("W35")[0]["number"], 500);
    if let Some(verdict) = r.compiles("b/user.proto") {
        verdict.unwrap();
    }

    // Without the option: as the descriptors say, and protoc refuses it.
    let r = run("none", &files, &[]);
    let text = r.text("b/user.proto");
    assert!(!text.contains("repair-skew"), "{text}");
    assert!(r.codes("W33").is_empty());
    if let Some(verdict) = r.compiles("b/user.proto") {
        assert!(verdict.is_err());
    }
}

#[test]
fn repair_skew_leaves_the_ambiguous() {
    // Two written files define `a.Row`, neither imported.
    let other = r#"name: "a/other.proto" package: "a" message_type { name: "Row" }"#;
    let user = r#"name: "b/user.proto" package: "b"
        message_type { name: "User" field { name: "row" number: 1 label: LABEL_OPTIONAL
          type: TYPE_MESSAGE type_name: ".a.Row" json_name: "row" } }"#;
    let r = run(
        "ambiguous",
        &[
            ("row.textpb", ROW),
            ("other.textpb", other),
            ("user.textpb", user),
        ],
        &["--repair-skew", "--keep-duplicates"],
    );
    let w36 = r.codes("W36");
    assert_eq!(w36.len(), 1, "{}", r.stderr);
    assert_eq!(w36[0]["symbol"], "a.Row");
    assert!(r.codes("W33").is_empty(), "{}", r.stderr);
    assert!(!r.text("b/user.proto").contains("import"));
}

#[test]
fn repair_skew_binary_follows_the_text() {
    let r = run(
        "binary",
        &[("row.textpb", ROW), ("user.textpb", USER)],
        &["--repair-skew", "--emit-binary"],
    );
    let bytes = std::fs::read(r.out.join("b/user.pb")).unwrap();
    let def = prototext_upb::Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let msg = prototext_upb::DynamicMessage::decode(&def, &bytes).unwrap();
    let f = prototext_upb::descriptor_access::FileDescriptorProto(msg.as_ref());
    assert_eq!(f.dependency(), [b"a/row.proto".as_slice()]);
}

/// `c/top.proto`, importing `b/user.proto`: refused through it unless the
/// repairs reach the pool (spec 0473).
const TOP: &str = r#"name: "c/top.proto" package: "c" dependency: "b/user.proto"
message_type { name: "Top" field { name: "u" number: 1 label: LABEL_OPTIONAL type: TYPE_MESSAGE
  type_name: ".b.User" json_name: "u" } }"#;

/// The file names a schema database (a `FileDescriptorSet`) holds.
fn schema_db_files(path: &Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap();
    let def = prototext_upb::Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorSet")
        .unwrap();
    let set = prototext_upb::DynamicMessage::decode(&def, &bytes).unwrap();
    let set = prototext_upb::descriptor_access::FileDescriptorSet(set.as_ref());
    set.file()
        .iter()
        .map(|f| String::from_utf8_lossy(f.name()).into_owned())
        .collect()
}

#[test]
fn a_repaired_file_is_not_refused() {
    let files = [
        ("row.textpb", ROW),
        ("user.textpb", USER),
        ("top.textpb", TOP),
    ];
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("repair-db.desc");
    let db = dir.to_str().unwrap();
    let r = run(
        "accepted",
        &files,
        &["--repair-skew", "--schema-db-out", db],
    );
    assert!(r.codes("W19").is_empty(), "{}", r.stderr);
    assert!(r.codes("W20").is_empty(), "{}", r.stderr);
    let names = schema_db_files(&dir);
    for f in ["b/user.proto", "c/top.proto"] {
        assert!(names.iter().any(|n| n == f), "{f} not in {names:?}");
    }

    // Without the option: both refused, as before; said once on stderr,
    // every occurrence in the report (spec 0474 S4).
    let r = run("refused", &files, &["--schema-db-out", db]);
    assert_eq!(r.codes("W19").len(), 2, "{}", r.stderr);
    assert_eq!(r.codes("W20").len(), 2, "{}", r.stderr);
    assert_eq!(r.stderr.matches("[W19]").count(), 1, "{}", r.stderr);
    assert!(!r.stderr.contains("[W20]"), "{}", r.stderr);
    assert!(
        r.stderr.contains("(and 1 file importing it)"),
        "{}",
        r.stderr
    );
}

#[test]
fn a_field_commented_out_leaves_the_pool() {
    let r = run(
        "left-out",
        &[("row.textpb", ROW), ("user.textpb", USER)],
        &["--repair-skew", "--emit-binary"],
    );
    let bytes = std::fs::read(r.out.join("b/user.pb")).unwrap();
    let def = prototext_upb::Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let msg = prototext_upb::DynamicMessage::decode(&def, &bytes).unwrap();
    let f = prototext_upb::descriptor_access::FileDescriptorProto(msg.as_ref());
    let names = |it: Vec<&[u8]>| -> Vec<String> {
        it.iter()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect()
    };
    let user = &f.message_type()[0];
    assert_eq!(
        names(user.field().iter().map(|x| x.name()).collect()),
        ["row", "ok"]
    );
    assert_eq!(
        names(f.extension().iter().map(|x| x.name()).collect()),
        ["near"]
    );
}

#[test]
fn still_refused_is_still_reported() {
    // A field naming a message as an enum: no repair makes it whole.
    let user = r#"name: "b/user.proto" package: "b" dependency: "a/row.proto"
        message_type { name: "User" field { name: "row" number: 1 label: LABEL_OPTIONAL
          type: TYPE_ENUM type_name: ".a.Row" json_name: "row" } }"#;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("repair-still-db.desc");
    let r = run(
        "still",
        &[("row.textpb", ROW), ("user.textpb", user)],
        &["--repair-skew", "--schema-db-out", dir.to_str().unwrap()],
    );
    assert_eq!(r.codes("W19").len(), 1, "{}", r.stderr);
    assert_eq!(r.codes("W20").len(), 1, "{}", r.stderr);
}
