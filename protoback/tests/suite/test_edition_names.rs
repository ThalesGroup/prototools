// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! An edition protoback can't name is a W6, the file not written, never
//! `edition = "UNKNOWN";` (spec 0467 S1).
//!
//! Ported from `test_edition_names.py`, its command-line test; skipped via
//! text, as it was: the text format cannot hold an edition the closed
//! Edition enum lacks. Its inputs, bare FileDescriptorProtos its code built
//! with Python's protobuf (`b.pb` with edition 1003 appended), are the
//! `#@prototext` fixtures `edition_names/`, made once by that code
//! (`protoback/tests/fixtures/gen/test_edition_names_and_instantiate.py`). Its unit test,
//! `test_an_older_protobuf_is_said` (W37, a Python protobuf older than
//! reproto's descriptor.proto), is reproto-only: protoback has no Python
//! protobuf.

use crate::common::{encode_fixture, protoback, tmp};

#[test]
fn test_an_unnamed_edition_is_a_w6() {
    if crate::via_text::on() {
        eprintln!("skipped via text: the text format cannot hold an unnamed edition");
        return;
    }
    let dir = tmp("test_an_unnamed_edition_is_a_w6");
    let inp = dir.join("in");
    encode_fixture(
        "edition_names/ok.pb",
        "890aeac3d3ef47d5bbceebee129468f92066bd932bd01d1f2b258d4b8885d841",
        &inp.join("ok.pb"),
    );
    encode_fixture(
        "edition_names/b.pb",
        "9713f3f7624e3d8c7d136775b9ce65cf634dcbd5c82e1965c3f6099b36c6f1da",
        &inp.join("b.pb"),
    );
    let out = dir.join("out");
    let r = protoback([
        format!("-I{}", inp.display()),
        "-O".into(),
        out.display().to_string(),
        "--detailed-warnings".into(),
        "ok.pb".into(),
        "b.pb".into(),
    ]);
    assert!(
        r.stderr.contains("[W6]") && r.stderr.contains("edition 1003"),
        "{}",
        r.stderr
    );
    assert!(!out.join("a/b.proto").exists());
    assert!(out.join("a/ok.proto").exists(), "the other file is written");
    let mut stack = vec![out.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "proto") {
                assert!(
                    !std::fs::read_to_string(&p).unwrap().contains("UNKNOWN"),
                    "{}",
                    p.display()
                );
            }
        }
    }
}
