// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0457: an edition named from the input's descriptor.proto, and no
//! file left unwritten without a warning. The inputs are binary: a text
//! input is parsed against the input's descriptor.proto, here the built-in
//! variant's, which predates edition 2026; its `Edition` enum is closed, so
//! the text can't hold an edition that enum lacks.

use std::path::{Path, PathBuf};
use std::process::Command;

use prototext_upb::descriptor_access::FileDescriptorSetMut;
use prototext_upb::{DynamicMessage, Pool};

fn varint(mut v: u64, out: &mut Vec<u8>) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn len_field(number: u64, bytes: &[u8], out: &mut Vec<u8>) {
    varint(number << 3 | 2, out);
    varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

fn varint_field(number: u64, v: u64, out: &mut Vec<u8>) {
    varint(number << 3, out);
    varint(v, out);
}

/// `name`, package `a`, `syntax: "editions"`, `edition`, and one message
/// `M<edition>` with one `int32 x = 1`.
fn editions_file(name: &str, edition: u64) -> Vec<u8> {
    let mut field = Vec::new();
    len_field(1, b"x", &mut field);
    varint_field(3, 1, &mut field); // number
    varint_field(4, 1, &mut field); // LABEL_OPTIONAL
    varint_field(5, 5, &mut field); // TYPE_INT32
    len_field(10, b"x", &mut field); // json_name
    let mut message = Vec::new();
    len_field(1, format!("M{edition}").as_bytes(), &mut message);
    len_field(2, &field, &mut message);
    let mut out = Vec::new();
    len_field(1, name.as_bytes(), &mut out);
    len_field(2, b"a", &mut out);
    len_field(4, &message, &mut out);
    len_field(12, b"editions", &mut out);
    varint_field(14, edition, &mut out);
    out
}

/// The vendored descriptor.proto, its `Edition` enum given `extra`.
fn descriptor_with(extra: (&str, i32)) -> Vec<u8> {
    let pool = Pool::vendored();
    let set = pool
        .get_message_by_name("google.protobuf.FileDescriptorSet")
        .unwrap();
    let mut msg = DynamicMessage::decode(&set, prototext_upb::DESCRIPTOR_SET).unwrap();
    {
        let mut s = FileDescriptorSetMut(msg.as_mut());
        let mut file = s.file_mut(0).unwrap();
        let index = file
            .as_ref()
            .enum_type()
            .iter()
            .position(|e| e.name() == b"Edition")
            .unwrap();
        let mut edition = file.enum_type_mut(index).unwrap();
        let mut value = edition.push_value().unwrap();
        value.set_name(extra.0.as_bytes()).unwrap();
        value.set_number(extra.1).unwrap();
    }
    // The set holds one file: field 1, its length, the file.
    let bytes = msg.encode().unwrap();
    let mut pos = 1;
    let mut len = 0usize;
    let mut shift = 0;
    loop {
        let b = bytes[pos];
        pos += 1;
        len |= usize::from(b & 0x7f) << shift;
        shift += 7;
        if b < 0x80 {
            break;
        }
    }
    assert_eq!(pos + len, bytes.len());
    bytes[pos..].to_vec()
}

/// A fresh directory, `in/` holding `files` (path, bytes).
fn setup(test: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("editions-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    for (name, bytes) in files {
        std::fs::write(dir.join("in").join(name), bytes).unwrap();
    }
    dir
}

struct Run {
    status: i32,
    stderr: String,
    out: PathBuf,
    report: serde_json::Value,
}

fn run(dir: &Path) -> Run {
    let out = dir.join("out");
    let report = dir.join("report.json");
    let o = Command::new(env!("CARGO_BIN_EXE_protoback"))
        .arg("-I")
        .arg(dir.join("in"))
        .arg("-O")
        .arg(&out)
        .arg("--report-out")
        .arg(&report)
        .env_remove("REPROTO_VARIANT")
        .output()
        .unwrap();
    Run {
        status: o.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        out,
        report: serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap(),
    }
}

impl Run {
    fn proto(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.out.join(name)).ok()
    }

    /// The report's warnings of `code`, as (file, reason).
    fn warnings(&self, code: &str) -> Vec<(String, String)> {
        self.report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w["code"] == code)
            .map(|w| {
                let f = &w["fields"];
                let s = |k: &str| f[k].as_str().unwrap_or("").to_string();
                (s("file"), s("reason"))
            })
            .collect()
    }
}

#[test]
fn edition_2026_renders() {
    let dir = setup("2026", &[("b.pb", editions_file("a/b.proto", 1002))]);
    let r = run(&dir);
    assert_eq!(r.status, 0, "{}", r.stderr);
    let text = r.proto("a/b.proto").expect("a/b.proto written");
    assert!(text.contains("edition = \"2026\";"), "{text}");
    assert!(text.contains("int32 x = 1;"), "{text}");
    assert!(r.warnings("W6").is_empty(), "{}", r.stderr);
}

#[test]
fn edition_unnamed_is_a_w6() {
    let dir = setup(
        "unnamed",
        &[
            ("b.pb", editions_file("a/b.proto", 1003)),
            ("c.pb", editions_file("a/c.proto", 1001)),
        ],
    );
    let r = run(&dir);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert!(r.proto("a/b.proto").is_none());
    assert_eq!(
        r.warnings("W6"),
        [(
            "a/b.proto".to_string(),
            "edition 1003 has no name in its descriptor.proto".to_string()
        )]
    );
    // Every summoned file is written or warned (S4).
    assert!(r.proto("a/c.proto").is_some());
}

#[test]
fn edition_named_by_the_inputs_descriptor() {
    let dir = setup(
        "input",
        &[
            ("b.pb", editions_file("a/b.proto", 1003)),
            ("descriptor.pb", descriptor_with(("EDITION_2027", 1003))),
        ],
    );
    let r = run(&dir);
    assert_eq!(r.status, 0, "{}", r.stderr);
    let text = r.proto("a/b.proto").expect("a/b.proto written");
    assert!(text.contains("edition = \"2027\";"), "{text}");
    assert!(r.warnings("W6").is_empty(), "{}", r.stderr);
}
