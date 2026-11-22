// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0458: `--export-used` writes `export` each message or enum local to
//! its file that another written file uses. The inputs are binary,
//! encoded against the vendored descriptor.proto, which knows edition
//! 2026; a text input would be parsed against the built-in variant's,
//! which doesn't.

use std::path::{Path, PathBuf};
use std::process::Command;

use prototext_upb::descriptor_access::{DescriptorProto, FileDescriptorProto};
use prototext_upb::text_format::parse_descriptor;
use prototext_upb::{DynamicMessage, Pool};

/// `a/outer.proto`, of `edition`: `Outer`, top-level, holding `Inner`
/// (its visibility `inner`, unset when empty), `Other` and the enum `Kind`;
/// `Top`, top-level. `Outer` uses `Inner` itself.
fn outer(edition: &str, inner: &str) -> String {
    let inner = if inner.is_empty() {
        String::new()
    } else {
        format!("visibility: {inner}")
    };
    format!(
        r#"name: "a/outer.proto" package: "a" syntax: "editions" edition: {edition}
        message_type {{ name: "Outer"
          nested_type {{ name: "Inner" {inner}
            field {{ name: "x" number: 1 type: TYPE_INT32 json_name: "x" }} }}
          nested_type {{ name: "Other" }}
          enum_type {{ name: "Kind" value {{ name: "KIND_UNSPECIFIED" number: 0 }} }}
          field {{ name: "i" number: 1 type: TYPE_MESSAGE type_name: ".a.Outer.Inner"
            json_name: "i" }} }}
        message_type {{ name: "Top" }}"#
    )
}

/// `b/user.proto`, edition 2024, using `uses` (full names).
fn user(uses: &[&str]) -> String {
    let fields: String = uses
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let kind = if t.ends_with("Kind") {
                "TYPE_ENUM"
            } else {
                "TYPE_MESSAGE"
            };
            format!(
                r#"field {{ name: "f{i}" number: {} type: {kind} type_name: "{t}"
                  json_name: "f{i}" }}"#,
                i + 1
            )
        })
        .collect();
    format!(
        r#"name: "b/user.proto" package: "b" dependency: "a/outer.proto" syntax: "editions"
        edition: EDITION_2024 message_type {{ name: "User" {fields} }}"#
    )
}

/// The vendored descriptor.proto (v36.2), which declares edition 2026 and
/// its `STRICT` default: an input of that edition carries one like it. A
/// serialized set of that one file: field 1, its length, the file.
fn vendored_descriptor() -> Vec<u8> {
    let set = prototext_upb::DESCRIPTOR_SET;
    let mut pos = 1;
    let (mut len, mut shift) = (0usize, 0);
    loop {
        let b = set[pos];
        pos += 1;
        len |= usize::from(b & 0x7f) << shift;
        shift += 7;
        if b < 0x80 {
            break;
        }
    }
    assert_eq!(pos + len, set.len());
    set[pos..].to_vec()
}

fn setup(test: &str, files: &[(&str, String)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("export-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    for (name, text) in files {
        let bytes = parse_descriptor(text, false).unwrap();
        std::fs::write(dir.join("in").join(name), bytes).unwrap();
    }
    dir
}

struct Run {
    stderr: String,
    out: PathBuf,
    report: serde_json::Value,
}

fn run(dir: &Path, args: &[&str]) -> Run {
    let out = dir.join("out");
    let _ = std::fs::remove_dir_all(&out);
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
        stderr,
        out,
        report: serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap(),
    }
}

impl Run {
    fn outer(&self) -> String {
        std::fs::read_to_string(self.out.join("a/outer.proto")).unwrap()
    }

    /// The report's warnings of `code`: (symbol, users).
    fn warnings(&self, code: &str) -> Vec<(String, Vec<String>)> {
        self.report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w["code"] == code)
            .map(|w| {
                let f = &w["fields"];
                let users = f["users"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|u| u.as_str().unwrap().to_string())
                    .collect();
                (f["symbol"].as_str().unwrap().to_string(), users)
            })
            .collect()
    }
}

const COMMENT: &str =
    "// WARNING[export-used]: exported: used by 'b/user.proto'; local in the source";

#[test]
fn export_used_exports_a_used_nested_symbol() {
    for inner in ["", "VISIBILITY_LOCAL"] {
        let dir = setup(
            "nested",
            &[
                ("outer.pb", outer("EDITION_2024", inner)),
                ("user.pb", user(&[".a.Outer.Inner"])),
            ],
        );
        let r = run(&dir, &["--export-used"]);
        let text = r.outer();
        assert!(
            text.contains(&format!("{COMMENT}\n  export message Inner {{")),
            "{text}"
        );
        assert!(text.contains("\n  message Other {}"), "{text}");
        assert!(text.contains("\n  enum Kind {"), "{text}");
        assert_eq!(
            r.warnings("W31"),
            [(
                "a.Outer.Inner".to_string(),
                vec!["b/user.proto".to_string()]
            )],
            "{}",
            r.stderr
        );

        // Without the option: as the source.
        let r = run(&dir, &[]);
        assert!(!r.outer().contains("export-used"), "{}", r.outer());
        assert!(r.warnings("W31").is_empty());
    }
}

#[test]
fn export_used_leaves_a_symbol_its_own_file_uses() {
    let dir = setup(
        "own",
        &[
            ("outer.pb", outer("EDITION_2024", "")),
            ("user.pb", user(&[".a.Top"])),
        ],
    );
    let r = run(&dir, &["--export-used"]);
    let text = r.outer();
    // Inner: used by Outer only. Top: exported already, top-level.
    assert!(!text.contains("export"), "{text}");
    assert!(r.warnings("W31").is_empty(), "{}", r.stderr);
}

#[test]
fn export_used_under_strict_reports() {
    let dir = setup(
        "strict",
        &[
            ("outer.pb", outer("EDITION_2026", "")),
            ("user.pb", user(&[".a.Outer.Inner", ".a.Top"])),
        ],
    );
    std::fs::write(dir.join("in/descriptor.pb"), vendored_descriptor()).unwrap();
    let r = run(&dir, &["--export-used"]);
    let text = r.outer();
    assert!(text.contains("\n  message Inner {"), "{text}");
    assert!(
        text.contains(&format!("{COMMENT}\nexport message Top {{}}")),
        "{text}"
    );
    let users = vec!["b/user.proto".to_string()];
    assert_eq!(r.warnings("W31"), [("a.Top".to_string(), users.clone())]);
    assert_eq!(r.warnings("W32"), [("a.Outer.Inner".to_string(), users)]);
}

#[test]
fn export_used_binary_follows_the_text() {
    let dir = setup(
        "binary",
        &[
            ("outer.pb", outer("EDITION_2024", "")),
            ("user.pb", user(&[".a.Outer.Kind"])),
        ],
    );
    let r = run(&dir, &["--export-used", "--emit-binary"]);
    let bytes = std::fs::read(r.out.join("a/outer.pb")).unwrap();
    let def = Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let msg = DynamicMessage::decode(&def, &bytes).unwrap();
    let f = FileDescriptorProto(msg.as_ref());
    let outer: DescriptorProto<'_> = f
        .message_type()
        .into_iter()
        .find(|m| m.name() == b"Outer")
        .unwrap();
    let kind = outer.enum_type().into_iter().next().unwrap();
    // VISIBILITY_EXPORT; the others as the input.
    assert_eq!(kind.visibility(), 2);
    let inner = outer.nested_type().into_iter().next().unwrap();
    assert!(!inner.has_visibility());
}
