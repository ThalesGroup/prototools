// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A file renders the same whatever renders before it (spec 0429 S14).
//!
//! ord_use.proto sets (ord.tag), declared in ord_ext.proto, its import of
//! ord_ext.proto dropped. Its options see only its own import closure: the
//! option is an orphan, with W22, whether ord_ext.proto renders before it or
//! after. It used to be written as (ord.tag) when ord_ext.proto came first.
//!
//! Ported from `test_render_order.py`. ord_ext.proto's set is compiled by
//! protoc, as there; ord_use.proto's, compiled then stripped of its import
//! with Python's protobuf, is those bytes, the `#@prototext` fixture
//! `render_order/ord_use.pb`. Its two parameters run in turn.

use crate::common::{encode_fixture, protoback, protoc, tmp, tools, write};

const EXT: &str =
    "syntax = \"proto2\";\npackage ord;\nimport \"google/protobuf/descriptor.proto\";\n\
                   extend google.protobuf.FileOptions { optional string tag = 50101; }\n";
const USE_SHA256: &str = "335ce0f64af0f32bdcda654dece785ccb5888a63901028eb89cb95a9cf916a17";

#[test]
fn test_an_option_outside_the_closure_is_an_orphan() {
    if !tools(&["protoc"]) {
        return;
    }
    for ext_first in [true, false] {
        let dir = tmp(&format!(
            "test_an_option_outside_the_closure_is_an_orphan-{ext_first}"
        ));
        let src = dir.join("src");
        write(&src.join("ord_ext.proto"), EXT);
        let (ext, use_) = (dir.join("ext.pb"), dir.join("use.pb"));
        protoc([
            format!("-I{}", src.display()),
            "--include_imports".into(),
            format!("--descriptor_set_out={}", ext.display()),
            "ord_ext.proto".into(),
        ]);
        encode_fixture("render_order/ord_use.pb", USE_SHA256, &use_);
        let (first, second) = if ext_first {
            (&ext, &use_)
        } else {
            (&use_, &ext)
        };
        let inp = dir.join("in");
        std::fs::create_dir_all(&inp).unwrap();
        let stem = |p: &std::path::Path| p.file_stem().unwrap().to_string_lossy().into_owned();
        std::fs::copy(first, inp.join(format!("1_{}.pb", stem(first)))).unwrap();
        std::fs::copy(second, inp.join(format!("2_{}.pb", stem(second)))).unwrap();
        let run = protoback([
            "--use-variant".as_ref(),
            "descriptor".as_ref(),
            format!("-I{}", inp.display()).as_ref(),
            "-O".as_ref(),
            dir.join("out").as_os_str(),
        ]);
        assert_eq!(run.code, Some(0), "ext_first={ext_first}: {}", run.stderr);
        let text = std::fs::read_to_string(dir.join("out/ord_use.proto")).unwrap();
        assert!(!text.contains("(ord.tag)"), "ext_first={ext_first}: {text}");
        assert!(
            text.contains(r#"option <50101> = "set";"#),
            "ext_first={ext_first}: {text}"
        );
        assert!(
            run.stderr.contains("[W22]"),
            "ext_first={ext_first}: {}",
            run.stderr
        );
    }
}
