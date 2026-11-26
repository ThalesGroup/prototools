// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Custom options read from protoc's text of a descriptor set (spec 0426).
//!
//! `protoc --decode=google.protobuf.FileDescriptorSet descriptor.proto`
//! prints a custom option, which descriptor.proto does not declare, by
//! number. Each test compiles a fixture with protoc, prints its set so, and
//! requires protoback to render from the text what it renders from the
//! binary set, custom options included, with the warnings of spec 0426 S4
//! where the text leaves the framing open.
//!
//! Ported from `test_text_input_options.py`.

use crate::common::{protoback, tmp, tools, write};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const OPTS: &str = r#"syntax = "proto2";

package txt;

import "google/protobuf/descriptor.proto";

enum Level {
  LOW = 0;
  HIGH = 1;
}

message Info {
  optional string owner = 1;
  repeated int32 codes = 2;
  optional Info child = 3;
}

extend google.protobuf.FileOptions {
  optional string label = 50001;
}

extend google.protobuf.MessageOptions {
  optional Info info = 50002;
  optional Level level = 50003;
  optional fixed32 tag32 = 50004;
  optional double ratio = 50005;
}

extend google.protobuf.FieldOptions {
  repeated int64 marks = 50006;
  optional bool hidden = 50007;
  optional bytes blob = 50008;
}
"#;

const MAIN: &str = r#"syntax = "proto2";

package txt;

import "opts.proto";

option (txt.label) = "main";

message Holder {
  option (txt.info) = { owner: "me" codes: 1 codes: 2 child { owner: "kid" } };
  option (txt.level) = HIGH;
  option (txt.tag32) = 7;
  option (txt.ratio) = 0.5;

  optional int32 a = 1 [(txt.marks) = 3, (txt.marks) = -4, (txt.hidden) = true];
  optional string b = 2 [(txt.blob) = "\001\002"];
}
"#;

/// Options protoc prints as `N { … }`, declared in the file that uses them:
/// a value framed as a group, a message, and strings whose bytes happen to
/// read as a message, with a group inside, not empty ("shoppingProduct":
/// W2) or empty ("UrlList": exact, no warning).
const DELIMITED: &str = r#"edition = "2023";

package dlm;

import "google/protobuf/descriptor.proto";

message Holder {
  option (dlm.as_group) = { x: 1 inner { y: 2 } };
  option (dlm.as_message) = { x: 3 };
  option (dlm.as_string) = "shoppingProduct";
  option (dlm.as_string_exact) = "UrlList";
}

message Value {
  int32 x = 1;
  Inner inner = 2 [features.message_encoding = DELIMITED];

  message Inner {
    int32 y = 1;
  }
}

extend google.protobuf.MessageOptions {
  Value as_group = 50001 [features.message_encoding = DELIMITED];
  Value as_message = 50002;
  string as_string = 50003;
  string as_string_exact = 50004;
}
"#;

type Files = BTreeMap<String, String>;

/// `main` and its imports compiled: the binary set, and its text, printed
/// with descriptor.proto alone, or with `schema` and its imports, which
/// names the options it declares `[pkg.ext]`.
fn compile(
    src: &Path,
    protos: &[(&str, &str)],
    main: &str,
    schema: Option<&str>,
) -> (PathBuf, PathBuf) {
    for (name, body) in protos {
        write(&src.join(name), body);
    }
    let pb = src.join("set.pb");
    let o = std::process::Command::new("protoc")
        .arg("--include_imports")
        .arg(format!("--descriptor_set_out={}", pb.display()))
        .arg(format!("-I{}", src.display()))
        .arg(src.join(main))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = std::process::Command::new("protoc")
        .arg("--decode=google.protobuf.FileDescriptorSet")
        .arg(format!("-I{}", src.display()))
        .arg(schema.unwrap_or("google/protobuf/descriptor.proto"))
        .stdin(std::fs::File::open(&pb).unwrap())
        .output()
        .unwrap();
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let textpb = src.join("set.textpb");
    std::fs::write(&textpb, text.stdout).unwrap();
    (pb, textpb)
}

/// Render `inputs`: each output file's text, and protoback's stderr, its
/// whitespace collapsed (the console wraps long warnings).
fn render(out: &Path, inputs: &[&Path]) -> (Files, String) {
    std::fs::create_dir_all(out).unwrap();
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        format!("--proto-out={}", out.display()),
    ];
    args.extend(inputs.iter().map(|p| p.display().to_string()));
    let r = protoback(&args);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let mut files = Files::new();
    let mut stack = vec![out.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "proto") {
                files.insert(
                    p.strip_prefix(out).unwrap().display().to_string(),
                    std::fs::read_to_string(&p).unwrap(),
                );
            }
        }
    }
    (
        files,
        r.stderr.split_whitespace().collect::<Vec<_>>().join(" "),
    )
}

fn from_binary_and_text(dir: &Path, protos: &[(&str, &str)], main: &str) -> (Files, Files, String) {
    let (pb, textpb) = compile(&dir.join("src"), protos, main, None);
    let (from_binary, _) = render(&dir.join("binary"), &[&pb]);
    let (from_text, stderr) = render(&dir.join("text"), &[&textpb]);
    (from_binary, from_text, stderr)
}

/// `file`'s entry of a set's text, alone, written as `src/<as>`.
fn entry_alone(src: &Path, textpb: &Path, file: &str, name: &str) -> PathBuf {
    let text = std::fs::read_to_string(textpb).unwrap();
    let start = text.find(&format!("file {{\n  name: \"{file}\"")).unwrap();
    let alone = src.join(name);
    std::fs::write(&alone, &text[start..]).unwrap();
    alone
}

const BOTH: &[(&str, &str)] = &[("opts.proto", OPTS), ("main.proto", MAIN)];

/// Options of each kind, declared in an import: as from the binary set.
#[test]
fn test_custom_options_survive_the_text_form() {
    if !tools(&["protoc"]) {
        return;
    }
    let (b, t, stderr) = from_binary_and_text(
        &tmp("test_custom_options_survive_the_text_form"),
        BOTH,
        "main.proto",
    );
    assert!(b["main.proto"].contains("(txt.info)"));
    assert_eq!(t, b);
    assert!(!stderr.contains("declared nowhere"), "{stderr}");
    assert!(!stderr.contains("printed as a message"), "{stderr}");
}

/// Framings the text leaves open, settled by the file's declarations.
#[test]
fn test_a_group_option_declared_in_its_own_file() {
    if !tools(&["protoc"]) {
        return;
    }
    let (b, t, stderr) = from_binary_and_text(
        &tmp("test_a_group_option_declared_in_its_own_file"),
        &[("delimited.proto", DELIMITED)],
        "delimited.proto",
    );
    assert!(b["delimited.proto"].contains("shoppingProduct"));
    assert_eq!(t, b);
    assert_eq!(
        stderr.matches("printed as a message").count(),
        1,
        "{stderr}"
    );
    assert!(
        stderr.contains("field 50003 of google.protobuf.MessageOptions"),
        "{stderr}"
    );
}

/// The declaring file left out of the input: W1, and the file rendered.
#[test]
fn test_an_option_declared_nowhere_is_reported() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_an_option_declared_nowhere_is_reported");
    let src = dir.join("src");
    let (_, textpb) = compile(&src, BOTH, "main.proto", None);
    // main.proto's entry alone: opts.proto, which declares its options, is
    // missing from the input.
    let alone = entry_alone(&src, &textpb, "main.proto", "main.textpb");
    let (files, stderr) = render(&dir.join("out"), &[&alone]);
    assert!(files.contains_key("main.proto"));
    assert!(
        stderr.contains("field 50002 of google.protobuf.MessageOptions is declared nowhere"),
        "{stderr}"
    );
}

/// Text printed with the options' schema, `[txt.info] { … }`: the names
/// resolved from the input's own declarations.
#[test]
fn test_options_printed_by_name_survive_the_text_form() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_options_printed_by_name_survive_the_text_form");
    let (pb, textpb) = compile(&dir.join("src"), BOTH, "main.proto", Some("opts.proto"));
    assert!(std::fs::read_to_string(&textpb)
        .unwrap()
        .contains("[txt.info]"));
    let (b, _) = render(&dir.join("binary"), &[&pb]);
    let (t, stderr) = render(&dir.join("text"), &[&textpb]);
    assert!(b["main.proto"].contains("(txt.info)"));
    assert_eq!(t, b);
    assert!(!stderr.contains("declared nowhere"), "{stderr}");
}

/// Text by name, the declaring file left out of the input: W3, and the
/// file rendered.
#[test]
fn test_an_option_by_name_declared_nowhere_is_reported() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_an_option_by_name_declared_nowhere_is_reported");
    let src = dir.join("src");
    let (_, textpb) = compile(&src, BOTH, "main.proto", Some("opts.proto"));
    let alone = entry_alone(&src, &textpb, "main.proto", "main.textpb");
    let (files, stderr) = render(&dir.join("out"), &[&alone]);
    assert!(files.contains_key("main.proto"));
    assert!(
        stderr
            .contains("extension [txt.info] of google.protobuf.MessageOptions is declared nowhere"),
        "{stderr}"
    );
}

/// Options declared in the file itself, whose import is missing from the
/// input: as from the binary set (the input's pool mends the file, spec
/// 0427 S2), with only the W2 of "shoppingProduct".
#[test]
fn test_a_file_whose_import_is_missing_keeps_its_own_options() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_a_file_whose_import_is_missing_keeps_its_own_options");
    let src = dir.join("src");
    let delimited = DELIMITED.replace(
        r#"import "google/protobuf/descriptor.proto";"#,
        "import \"google/protobuf/descriptor.proto\";\nimport \"gone.proto\";",
    );
    let (_, textpb) = compile(
        &src,
        &[
            (
                "gone.proto",
                "syntax = \"proto3\";\npackage gone;\nmessage G {}\n",
            ),
            ("delimited.proto", &delimited),
        ],
        "delimited.proto",
        None,
    );
    // delimited.proto alone, without gone.proto, in both forms.
    let alone_pb = src.join("alone.pb");
    let o = std::process::Command::new("protoc")
        .arg(format!("--descriptor_set_out={}", alone_pb.display()))
        .arg(format!("-I{}", src.display()))
        .arg(src.join("delimited.proto"))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let alone_text = entry_alone(&src, &textpb, "delimited.proto", "alone.textpb");
    let (b, _) = render(&dir.join("binary"), &[&alone_pb]);
    let (t, stderr) = render(&dir.join("text"), &[&alone_text]);
    assert!(b["delimited.proto"].contains("shoppingProduct"));
    assert_eq!(t, b);
    assert!(!stderr.contains("declared nowhere"), "{stderr}");
    assert_eq!(
        stderr.matches("printed as a message").count(),
        1,
        "{stderr}"
    );
}

/// One file of the input does not parse: said so, with its line and
/// column, and the others rendered.
#[test]
fn test_text_that_does_not_parse_is_reported_and_skipped() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_text_that_does_not_parse_is_reported_and_skipped");
    let src = dir.join("src");
    let (_, textpb) = compile(&src, BOTH, "main.proto", None);
    let text = std::fs::read_to_string(&textpb).unwrap();
    let start = text.find("file {\n  name: \"main.proto\"").unwrap();
    // main.proto's entry broken: a value missing.
    let broken = format!(
        "{}{}",
        &text[..start],
        text[start..].replacen(
            "name: \"main.proto\"",
            "name: \"main.proto\"\n  package:",
            1
        )
    );
    let bad = src.join("bad.textpb");
    std::fs::write(&bad, broken).unwrap();
    let (files, stderr) = render(&dir.join("out"), &[&bad]);
    assert!(files.contains_key("opts.proto"));
    assert!(!files.contains_key("main.proto"));
    assert!(stderr.contains("Cannot parse"), "{stderr}");
}
