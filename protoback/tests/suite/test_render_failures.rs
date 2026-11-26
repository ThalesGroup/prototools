// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A failed render's messages are fixed (spec 0429 S16).
//!
//! What a render failure writes, in the `.proto` comment and the W6
//! warning, comes from its cause, not from an error's text, which changes
//! with upb's version. The text is shown under `--debug` only.
//!
//! Ported from `test_render_failures.py`.

use crate::common::{fixture, protoback, tmp, tools, words, write, Run};
use std::path::{Path, PathBuf};

/// Why the pool refuses the extender (spec 0438 S8): upb's own message.
const REFUSED: &str = "the descriptor pool refused it: couldn't resolve name '.ext_test.Config'";

fn run(dir: &Path, pb: &Path, include: &Path, args: &[&str]) -> (Run, PathBuf) {
    let out = dir.join("out");
    let mut all: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", include.display()),
        format!("--proto-out={}", out.display()),
    ];
    all.extend(args.iter().map(|a| a.to_string()));
    all.push(pb.strip_prefix(include).unwrap().display().to_string());
    let r = protoback(&all);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    (r, out)
}

/// ext_no_options_extender.proto without the file declaring the message it
/// extends: the pool cannot build it.
fn extender_alone(dir: &Path) -> PathBuf {
    let root = dir.join("pb");
    std::fs::create_dir_all(&root).unwrap();
    let pb = root.join("extender.pb");
    let o = std::process::Command::new("protoc")
        .arg(format!("-I{}", fixture("").display()))
        .arg(format!("--descriptor_set_out={}", pb.display()))
        .arg("ext_no_options_extender.proto")
        .current_dir(fixture(""))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    pb
}

#[test]
fn test_a_file_the_pool_refuses_says_why() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_a_file_the_pool_refuses_says_why");
    let pb = extender_alone(&dir);
    let (r, out) = run(&dir, &pb, pb.parent().unwrap(), &[]);
    let text = std::fs::read_to_string(out.join("ext_no_options_extender.proto")).unwrap();
    let stderr = words(&r.stderr);
    // Said in the file, its options rendered all the same (spec 0475).
    assert!(
        text.contains(&format!("// WARNING[render]: {REFUSED}")),
        "{text}"
    );
    assert!(
        stderr.contains(&format!("'ext_no_options_extender.proto' : {REFUSED}")),
        "{stderr}"
    );
    // upb's message, not a wrapping of it, nor a type parsed out.
    assert!(!format!("{text}{stderr}").contains("Couldn't build proto file"));
    assert!(
        !stderr.contains("unresolvable type .ext_test.Config"),
        "{stderr}"
    );
    // The missing import is still reported, as before.
    assert!(
        stderr.contains("missing dependency file:ext_no_options_config.proto"),
        "{stderr}"
    );
}

#[test]
fn test_debug_shows_the_pools_own_reason() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_debug_shows_the_pools_own_reason");
    let pb = extender_alone(&dir);
    let (r, _) = run(&dir, &pb, pb.parent().unwrap(), &["--debug"]);
    assert!(
        words(&r.stderr).contains("couldn't resolve name '.ext_test.Config'"),
        "{}",
        r.stderr
    );
}

const M: &str = "syntax = \"proto2\";\npackage p;\nmessage M { optional int32 x = 1; }\n";
const U: &str =
    "syntax = \"proto2\";\npackage q;\nimport \"m2.proto\";\nmessage U { optional p.M m = 1; }\n";

/// m.proto and m2.proto, both defining p.M, and `u` importing m2.proto, each
/// compiled to its own set under `dir/pb`.
fn duplicates(dir: &Path, u: &str) -> PathBuf {
    let (src, root) = (dir.join("src"), dir.join("pb"));
    std::fs::create_dir_all(&root).unwrap();
    let m2 = M.replace("x = 1", "y = 1");
    for (name, body) in [("m.proto", M), ("m2.proto", m2.as_str()), ("u.proto", u)] {
        write(&src.join(name), body);
        let o = std::process::Command::new("protoc")
            .arg(format!("-I{}", src.display()))
            .arg(format!(
                "--descriptor_set_out={}",
                root.join(name.replace(".proto", ".pb")).display()
            ))
            .arg(name)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    root
}

/// Kept both (--keep-duplicates), the pool refuses m2.proto itself and
/// u.proto for its import: W19 and W20 say which, every occurrence under
/// --detailed-warnings (spec 0474 G2).
#[test]
fn test_a_file_refused_for_its_import_names_it() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_a_file_refused_for_its_import_names_it");
    let root = duplicates(&dir, U);
    let r = protoback([
        "--use-variant".into(),
        "descriptor".into(),
        "--keep-duplicates".into(),
        "--detailed-warnings".into(),
        format!("-I{}", root.display()),
        format!("--proto-out={}", dir.join("out").display()),
        format!("--schema-db-out={}", dir.join("db.desc").display()),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let stderr = words(&r.stderr);
    let itself = "the descriptor pool refused it: duplicate symbol 'p.M'";
    let imported = "the descriptor pool refused its import 'm2.proto'";
    for s in [
        format!("[W19]: 'm2.proto' : {itself}"),
        format!("[W19]: 'u.proto' : {imported}"),
        format!("[W20]: 'm2.proto' schema db: {itself}"),
        format!("[W20]: 'u.proto' schema db: {imported}"),
    ] {
        assert!(stderr.contains(&s), "{s}:\n{stderr}");
    }
}

/// u.proto with options on each kind of element, refused for its import.
const U_OPTIONS: &str = r#"syntax = "proto2";
package q;
import "m2.proto";
message U {
  option deprecated = true;
  optional p.M m = 1 [deprecated = true];
  enum E {
    option allow_alias = true;
    A = 0 [deprecated = true];
    B = 0;
  }
}
service V {
  option deprecated = true;
  rpc Go(U) returns (U) { option deprecated = true; }
}
"#;

/// Spec 0429 S15: the pool refuses u.proto (for its import), so knows none
/// of its elements; their options are read from their own descriptors, as a
/// field's are.
#[test]
fn test_a_refused_files_element_options_are_kept() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_a_refused_files_element_options_are_kept");
    let root = duplicates(&dir, U_OPTIONS);
    let r = protoback([
        "--builtin".into(),
        "descriptor".into(),
        "--keep-duplicates".into(),
        format!("-I{}", root.display()),
        format!("--proto-out={}", dir.join("out").display()),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(!r.stderr.contains("[W6]"), "{}", r.stderr);
    let text = std::fs::read_to_string(dir.join("out/u.proto")).unwrap();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    // message, service, method
    assert_eq!(
        lines
            .iter()
            .filter(|l| **l == "option deprecated = true;")
            .count(),
        3,
        "{text}"
    );
    for s in [
        "option allow_alias = true;",
        "A = 0 [deprecated = true];",
        "optional p.M m = 1 [deprecated = true];",
    ] {
        assert!(lines.contains(&s), "{s}:\n{text}");
    }
}
