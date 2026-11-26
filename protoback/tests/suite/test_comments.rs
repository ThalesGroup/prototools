// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A comment comes back as written (spec 0376).
//!
//! protoc stores, for each comment line, exactly the text after `//`.
//! protoback renders it back as `//` plus that text: no stripping, and no
//! space of its own, so a decompiled comment reads as the original did.
//!
//! Ported from `test_comments.py`; protoc's set read with `descriptor_access`,
//! where the original parsed it with Python's protobuf.

use crate::common::{fixture, protoback, tmp, tools};
use prototext_upb::descriptor_access::FileDescriptorSet;
use prototext_upb::{DynamicMessage, Pool};
use std::path::Path;

const FIXTURE: &str = "comments.proto";

/// A Location's comments as read: path, leading, trailing, detached.
struct Loc {
    path: Vec<i32>,
    leading: String,
    trailing: String,
    detached: Vec<String>,
}

/// protoc with source info, the comments protoback renders coming from it;
/// the first file's Locations.
fn compile(proto_dir: &Path, name: &str, out: &Path) -> Vec<Loc> {
    let o = std::process::Command::new("protoc")
        .arg("--include_source_info")
        .arg(format!("-I{}", proto_dir.display()))
        .arg(format!("--descriptor_set_out={}", out.display()))
        .arg(proto_dir.join(name))
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "protoc failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let def = Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorSet")
        .unwrap();
    let msg = DynamicMessage::decode(&def, &std::fs::read(out).unwrap()).unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let file = FileDescriptorSet(msg.as_ref()).file().swap_remove(0);
    file.source_code_info()
        .map(|s| {
            s.location()
                .into_iter()
                .map(|l| Loc {
                    path: l.path(),
                    leading: text(l.leading_comments()),
                    trailing: text(l.trailing_comments()),
                    detached: l
                        .leading_detached_comments()
                        .into_iter()
                        .map(text)
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The fixture's Locations, and protoback's rendering of it.
fn decompile(dir: &Path) -> (Vec<Loc>, String) {
    let (orig, out) = (dir.join("orig"), dir.join("out"));
    std::fs::create_dir_all(&orig).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(fixture(FIXTURE), orig.join(FIXTURE)).unwrap();
    let pb = orig.join("comments.pb");
    let locs = compile(&orig, FIXTURE, &pb);
    let r = protoback([
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", orig.display()),
        format!("--proto-out={}", out.display()),
        pb.display().to_string(),
    ]);
    assert_eq!(r.code, Some(0), "protoback failed: {}", r.stderr);
    (locs, std::fs::read_to_string(out.join(FIXTURE)).unwrap())
}

/// The stored comments protoback renders: the file's (path [] or [12], the
/// syntax line) and each top-level message's (path [4, i]).
fn rendered_comments(locs: &[Loc]) -> Vec<String> {
    let mut out = Vec::new();
    for l in locs {
        if l.path.is_empty() || l.path == [12] || (l.path.len() == 2 && l.path[0] == 4) {
            out.push(l.leading.clone());
            out.push(l.trailing.clone());
            out.extend(l.detached.iter().cloned());
        }
    }
    out.into_iter().filter(|c| !c.is_empty()).collect()
}

/// A comment's lines, its final newline aside.
fn comment_lines(comment: &str) -> Vec<String> {
    comment
        .strip_suffix('\n')
        .unwrap_or(comment)
        .split('\n')
        .map(String::from)
        .collect()
}

#[test]
fn test_the_fixture_covers_every_case() {
    if !tools(&["protoc"]) {
        return;
    }
    let (locs, _) = decompile(&tmp("test_the_fixture_covers_every_case"));
    let lines: Vec<String> = rendered_comments(&locs)
        .iter()
        .flat_map(|c| comment_lines(c))
        .collect();
    assert!(lines.iter().any(|l| l.is_empty()), "an empty comment line");
    assert!(
        lines.iter().any(|l| !l.is_empty() && !l.starts_with(' ')),
        "//text"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("   ")),
        "spaces after //"
    );
    assert!(
        lines.iter().any(|l| l == " before the syntax line."),
        "a block comment"
    );
}

#[test]
fn test_comments_come_back_as_written() {
    if !tools(&["protoc"]) {
        return;
    }
    let (locs, rendered) = decompile(&tmp("test_comments_come_back_as_written"));
    // A line as rendered, without the indentation protoback gives it.
    let out: Vec<&str> = rendered
        .lines()
        .map(|l| l.trim_start_matches(' '))
        .collect();
    for comment in rendered_comments(&locs) {
        for line in comment_lines(&comment) {
            assert!(
                out.contains(&format!("//{line}").as_str()),
                "//{line:?} not rendered as written"
            );
        }
    }
    let trailing: Vec<&str> = rendered.lines().filter(|l| l.ends_with(' ')).collect();
    assert!(trailing.is_empty(), "lines ending in a space: {trailing:?}");
}

#[test]
fn test_the_file_header_survives_a_second_protoc() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_the_file_header_survives_a_second_protoc");
    let (locs, rendered) = decompile(&dir);
    let again_dir = dir.join("again");
    std::fs::create_dir_all(&again_dir).unwrap();
    std::fs::write(again_dir.join(FIXTURE), &rendered).unwrap();
    let again = compile(&again_dir, FIXTURE, &again_dir.join("again.pb"));
    let header = |locs: &[Loc]| -> Vec<String> {
        let syntax: Vec<&Loc> = locs.iter().filter(|l| l.path == [12]).collect();
        assert_eq!(syntax.len(), 1);
        syntax[0].detached.clone()
    };
    // protoback's own header comes first: the file's name.
    let mut expected = vec![format!(" {FIXTURE}\n")];
    expected.extend(header(&locs));
    assert_eq!(header(&again), expected);
}

#[test]
fn test_reprotos_own_header_keeps_its_space() {
    if !tools(&["protoc"]) {
        return;
    }
    let (_, rendered) = decompile(&tmp("test_reprotos_own_header_keeps_its_space"));
    assert_eq!(rendered.lines().next().unwrap(), format!("// {FIXTURE}"));
}
