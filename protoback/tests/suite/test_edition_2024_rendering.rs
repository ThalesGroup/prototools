// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Edition 2024 elements written as protoc reads them back.
//!
//! - `local` / `export` before a message or an enum whose visibility is
//!   set, whatever the file's default;
//! - `import option "…";` for each option dependency, its options read as
//!   the extensions they are, not orphans;
//! - an empty `features` written `option features = {};`, on a file and on
//!   a oneof (whose options are built-in options like the others).
//!
//! In a file written as proto2, none of these exists: the keywords dropped,
//! an `import option` a plain import.
//!
//! Ported from `test_edition_2024_rendering.py`; its one-parameter test
//! runs its parameter.

use crate::common::{protoback, tmp, tools, write, Run};
use std::path::{Path, PathBuf};

const OPT: &str = r#"edition = "2024";
package o;
import "google/protobuf/descriptor.proto";
extend google.protobuf.MessageOptions { int32 tag = 50001; }
"#;
const VIS: &str = r#"edition = "2024";
package v;
import option "opt.proto";
local message Hidden {
  export enum Shown { SHOWN_UNSPECIFIED = 0; }
  option (o.tag) = 7;
}
export message Out {
  oneof choice {
    option features = {};
    int32 a = 1;
    string b = 2;
  }
}
option features = {};
"#;

fn render(test: &str, flags: &[&str]) -> (Run, PathBuf) {
    let dir = tmp(test);
    let (src, inp, out) = (dir.join("src"), dir.join("in"), dir.join("out"));
    write(&src.join("opt.proto"), OPT);
    write(&src.join("vis.proto"), VIS);
    std::fs::create_dir_all(&inp).unwrap();
    let o = std::process::Command::new("protoc")
        .arg(format!("-I{}", src.display()))
        .arg("--include_imports")
        .arg(format!(
            "--descriptor_set_out={}",
            inp.join("vis.pb").display()
        ))
        .arg("vis.proto")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let mut args: Vec<String> = vec!["--use-variant".into(), "descriptor".into()];
    args.extend(flags.iter().map(|f| f.to_string()));
    args.extend([
        format!("-I{}", inp.display()),
        "-O".into(),
        out.display().to_string(),
    ]);
    let r = protoback(&args);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    (r, out)
}

fn compiles(out: &Path) {
    let o = std::process::Command::new("protoc")
        .arg(format!("-I{}", out.display()))
        .arg("--descriptor_set_out=/dev/null")
        .arg("vis.proto")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

/// The file's lines but blank ones and comments.
fn code(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty() && !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn test_edition_2024_elements_roundtrip() {
    if !tools(&["protoc"]) {
        return;
    }
    let (r, out) = render("test_edition_2024_elements_roundtrip", &[]);
    let text = code(&out.join("vis.proto"));
    for s in [
        "local message Hidden {",
        "export enum Shown {",
        "export message Out {",
        r#"import option "opt.proto";"#,
        "option (.o.tag) = 7;",
    ] {
        assert!(text.contains(s), "{s}:\n{text}");
    }
    assert!(!r.stderr.contains("W22"), "{}", r.stderr);
    assert_eq!(text.matches("option features = {};").count(), 2, "{text}");
    assert!(!text.contains("OneofOptions"), "{text}");
    compiles(&out);
}

/// The flags writing a file as proto2, each a run of the test below: the
/// original's parameters (`@pytest.mark.parametrize("flag", …)`).
const PROTO2_FLAGS: &[&str] = &["--force-proto2-output"];

#[test]
fn test_edition_2024_elements_as_proto2() {
    if !tools(&["protoc"]) {
        return;
    }
    for flag in PROTO2_FLAGS {
        let (_, out) = render(
            &format!("test_edition_2024_elements_as_proto2-{flag}"),
            &[flag],
        );
        let text = code(&out.join("vis.proto"));
        assert!(
            !text.contains("local ") && !text.contains("export "),
            "{text}"
        );
        assert!(
            text.contains(r#"import "opt.proto";"#) && !text.contains("import option"),
            "{text}"
        );
        assert!(!text.contains("features"), "{text}");
        compiles(&out);
    }
}
