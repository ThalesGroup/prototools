// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A refusal said once on stderr (spec 0474): no W20 repeating its file's
//! W19; the files refused only through an import counted on their root's
//! line. --detailed-warnings keeps every occurrence, and --fail-on fails as
//! before.
//!
//! Ported from `test_refusal_said_once.py`.

use crate::common::{protoback, tmp, words, write, Run};
use std::path::Path;

const ROW: &str = r#"name: "a/row.proto" package: "a"
message_type { name: "Base" extension_range { start: 100 end: 200 } }
"#;

/// The root: extension 500 is in no extension range of a.Base.
const USER: &str = r#"name: "b/user.proto" package: "b" dependency: "a/row.proto"
message_type { name: "User" }
extension { name: "far" number: 500 label: LABEL_OPTIONAL type: TYPE_INT32
  extendee: ".a.Base" json_name: "far" }
"#;

const ROOT: &str = "'b/user.proto' : the descriptor pool refused it: field number 500 in \
                    extension b.far has no extension range in message a.Base";

fn importer(name: &str, deps: &[&str]) -> String {
    let package = name.split('/').next().unwrap();
    let imports: Vec<String> = deps
        .iter()
        .map(|d| format!(r#"dependency: "{d}""#))
        .collect();
    format!(
        r#"name: "{name}" package: "{package}" {} message_type {{ name: "M" }}"#,
        imports.join(" ")
    ) + "\n"
}

fn run(dir: &Path, args: &[&str]) -> Run {
    let inp = dir.join("in");
    for (name, text) in [
        ("row.textpb", ROW.to_string()),
        ("user.textpb", USER.to_string()),
        ("c1.textpb", importer("c1/c1.proto", &["b/user.proto"])),
        ("c2.textpb", importer("c2/c2.proto", &["b/user.proto"])),
        (
            "d.textpb",
            importer("d/d.proto", &["c1/c1.proto", "c2/c2.proto"]),
        ),
    ] {
        write(&inp.join(name), &text);
    }
    let mut all: Vec<std::ffi::OsString> = vec![
        format!("-I{}", inp.display()).into(),
        "-O".into(),
        dir.join("out").into_os_string(),
        format!("--schema-db-out={}", dir.join("db.desc").display()).into(),
    ];
    all.extend(args.iter().map(Into::into));
    protoback(all)
}

#[test]
fn test_a_refusal_is_said_once() {
    let dir = tmp("test_a_refusal_is_said_once");
    let result = run(&dir, &[]);
    assert_eq!(result.code, Some(0), "{}", result.stderr);
    let stderr = words(&result.stderr);
    assert!(
        stderr.contains(&format!(
            "[W19]: {ROOT} (and 3 files importing it) (1 occurrence)"
        )),
        "{stderr}"
    );
    assert_eq!(stderr.matches("[W19]").count(), 1, "{stderr}");
    assert!(!stderr.contains("[W20]"), "{stderr}");
    // Every occurrence, under --detailed-warnings (the report: protoback's
    // tests/repair.rs).
    let detailed = words(&run(&dir, &["--detailed-warnings"]).stderr);
    assert!(
        detailed.matches("[W19]").count() == 4 && detailed.matches("[W20]").count() == 4,
        "{detailed}"
    );
    assert!(!detailed.contains("(and 3 files"));
}

#[test]
fn test_fail_on_still_fails() {
    let dir = tmp("test_fail_on_still_fails");
    let result = run(&dir, &["--fail-on", "W20"]);
    assert_ne!(result.code, Some(0), "{}", result.stderr);
}
