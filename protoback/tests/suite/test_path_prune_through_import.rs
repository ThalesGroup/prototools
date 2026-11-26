// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A path: prune applies to a file reached through an import.
//!
//! A path pattern prunes "that physical file, never loaded". An import of
//! a.proto resolves to a.pb (or a.textpb, ...); the pattern was matched
//! against the import's name, a.proto, so `-p path:a.pb` let a.pb load as
//! soon as a file imported it. It is now matched against the file found,
//! and against the import's name as before.
//!
//! Ported from `test_path_prune_through_import.py`; its two parameters run
//! in turn, each named in a failure.

use crate::common::{protoback, protoc, tmp, tools, write};

const A: &str = "syntax = \"proto2\";\npackage a;\nmessage A { optional int32 x = 1; }\n";
const B: &str =
    "syntax = \"proto2\";\npackage b;\nimport \"a.proto\";\nmessage B { optional a.A a = 1; }\n";

#[test]
fn test_a_path_prune_holds_through_an_import() {
    if !tools(&["protoc"]) {
        return;
    }
    for (n, pattern) in ["path:a.pb", "path:a.*"].into_iter().enumerate() {
        let dir = tmp(&format!("test_a_path_prune_holds_through_an_import-{n}"));
        let (src, inp) = (dir.join("src"), dir.join("in"));
        for (name, text) in [("a.proto", A), ("b.proto", B)] {
            write(&src.join(name), text);
            std::fs::create_dir_all(&inp).unwrap();
            protoc([
                format!("-I{}", src.display()),
                format!(
                    "--descriptor_set_out={}",
                    inp.join(name.replace(".proto", ".pb")).display()
                ),
                name.to_string(),
            ]);
        }
        let run = protoback([
            "--use-variant".as_ref(),
            "descriptor".as_ref(),
            "-p".as_ref(),
            pattern.as_ref(),
            format!("-I{}", inp.display()).as_ref(),
            "-O".as_ref(),
            dir.join("out").as_os_str(),
        ]);
        assert_eq!(run.code, Some(0), "{pattern}: {}", run.stderr);
        assert!(
            !dir.join("out/a.proto").exists(),
            "{pattern}: {}",
            run.stderr
        );
        assert!(
            dir.join("out/b.proto").exists(),
            "{pattern}: {}",
            run.stderr
        );
        assert!(
            run.stderr.contains("missing dependency file:a.proto"),
            "{pattern}: {}",
            run.stderr
        );
    }
}
