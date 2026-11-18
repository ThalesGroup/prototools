// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0445: `move:` fixups. Each test writes its input as text FDPs, one
//! per `.textpb`, and runs protoback on them.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `p.T` and `p.U`, in a file of their own.
const OLD: &str = r#"name: "old.proto" package: "p"
  message_type { name: "T" } message_type { name: "U" }"#;
/// `p.T` only.
const NEW: &str = r#"name: "new.proto" package: "p" message_type { name: "T" }"#;

/// An importer of `dep`, using `p.T`, and `p.U` when `uses_u`.
fn importer(dep: &str, uses_u: bool) -> String {
    let u = if uses_u {
        r#"field { name: "u" number: 2 label: LABEL_OPTIONAL type: TYPE_MESSAGE type_name: ".p.U" }"#
    } else {
        ""
    };
    format!(
        r#"name: "b.proto" package: "b" dependency: "{dep}"
           message_type {{ name: "B"
             field {{ name: "t" number: 1 label: LABEL_OPTIONAL type: TYPE_MESSAGE type_name: ".p.T" }}
             {u} }}"#
    )
}

/// A fresh directory, `in/` holding `files` (name, text FDP).
fn setup(test: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("moves-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    for (name, text) in files {
        std::fs::write(dir.join("in").join(format!("{name}.textpb")), text).unwrap();
    }
    dir
}

struct Run {
    status: i32,
    stderr: String,
    out: PathBuf,
}

impl Run {
    fn proto(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.out.join(name)).ok()
    }
}

/// protoback on `dir/in`, with `config` as its `-C` file when given.
fn run(dir: &Path, config: Option<&str>, args: &[&str]) -> Run {
    let out = dir.join("out");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_protoback"));
    cmd.args(["--use-variant", "descriptor", "-I"])
        .arg(dir.join("in"))
        .arg("-O")
        .arg(&out)
        .args(args)
        .env_remove("REPROTO_VARIANT");
    if let Some(config) = config {
        let path = dir.join("filter.yaml");
        std::fs::write(&path, config).unwrap();
        cmd.arg("-C").arg(path);
    }
    let o = cmd.output().unwrap();
    Run {
        status: o.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        out,
    }
}

fn codes(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|l| l.strip_prefix("Warning ["))
        .filter_map(|l| l.split_once(']').map(|(c, _)| c.to_string()))
        .collect()
}

#[test]
fn move_repoints_importers() {
    let dir = setup(
        "repoint",
        &[
            ("old", OLD),
            ("new", NEW),
            ("b", &importer("old.proto", false)),
        ],
    );
    let r = run(&dir, Some("move:\n  - old.proto: new.proto\n"), &[]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    let b = r.proto("b.proto").expect("b.proto written");
    assert!(b.contains("import \"new.proto\";"), "{b}");
    assert!(!b.contains("old.proto"), "{b}");
    assert!(r.proto("old.proto").is_none(), "old.proto pruned");
    assert!(codes(&r.stderr).is_empty(), "{}", r.stderr);
}

#[test]
fn move_to_several_targets() {
    let n2 = r#"name: "n2.proto" package: "q" message_type { name: "V" }"#;
    let dir = setup(
        "several",
        &[
            ("old", OLD),
            ("new", NEW),
            ("n2", n2),
            ("b", &importer("old.proto", false)),
        ],
    );
    let r = run(
        &dir,
        Some("move:\n  - old.proto: [new.proto, n2.proto]\n"),
        &[],
    );
    assert_eq!(r.status, 0, "{}", r.stderr);
    let b = r.proto("b.proto").unwrap();
    assert!(
        b.contains("import \"new.proto\";") && b.contains("import \"n2.proto\";"),
        "{b}"
    );
}

#[test]
fn move_keeps_a_public_import_public() {
    let b = importer("old.proto", false).replace(
        r#"dependency: "old.proto""#,
        r#"dependency: "old.proto" public_dependency: 0"#,
    );
    let dir = setup("public", &[("old", OLD), ("new", NEW), ("b", &b)]);
    let r = run(&dir, Some("move:\n  - old.proto: new.proto\n"), &[]);
    let b = r.proto("b.proto").unwrap();
    assert!(b.contains("import public \"new.proto\";"), "{b}");
}

#[test]
fn move_prunes_a_walked_old_and_is_not_idle() {
    let dir = setup("walked", &[("old", OLD), ("new", NEW)]);
    let r = run(&dir, Some("move:\n  - old.proto: new.proto\n"), &[]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert!(r.proto("old.proto").is_none());
    assert!(
        !codes(&r.stderr).contains(&"W25".to_string()),
        "{}",
        r.stderr
    );
}

#[test]
fn lost_type_is_w24() {
    let dir = setup(
        "lost-type",
        &[
            ("old", OLD),
            ("new", NEW),
            ("b", &importer("old.proto", true)),
        ],
    );
    let r = run(&dir, None, &["--move", "old.proto=new.proto"]);
    assert!(
        r.stderr.contains(
            "Warning [W24]: 'b.proto': type '.p.U' is defined by no file it sees; \
             lost to move 'old.proto'"
        ),
        "{}",
        r.stderr
    );
    assert!(!r.stderr.contains("'.p.T'"), "{}", r.stderr);
}

#[test]
fn type_through_a_public_import_is_not_w24() {
    let u = r#"name: "u.proto" package: "p" message_type { name: "U" }"#;
    let new = r#"name: "new.proto" package: "p" dependency: "u.proto" public_dependency: 0
                 message_type { name: "T" }"#;
    let dir = setup(
        "public-type",
        &[
            ("old", OLD),
            ("new", new),
            ("u", u),
            ("b", &importer("old.proto", true)),
        ],
    );
    let r = run(&dir, None, &["--move", "old.proto=new.proto"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert!(
        !codes(&r.stderr).contains(&"W24".to_string()),
        "{}",
        r.stderr
    );
}

#[test]
fn lost_option_is_w24() {
    // old.proto declares option 50000 of FieldOptions; b sets it.
    let old = r#"name: "old.proto" package: "p" dependency: "google/protobuf/descriptor.proto"
                 message_type { name: "T" }
                 extension { name: "opt" number: 50000 label: LABEL_OPTIONAL type: TYPE_INT32
                             extendee: ".google.protobuf.FieldOptions" }"#;
    let b = r#"name: "b.proto" package: "b" dependency: "old.proto"
               message_type { name: "B" field { name: "t" number: 1 label: LABEL_OPTIONAL
                 type: TYPE_MESSAGE type_name: ".p.T" options { 50000: 7 } } }"#;
    let dir = setup("lost-option", &[("old", old), ("new", NEW), ("b", b)]);
    let r = run(&dir, None, &["--move", "old.proto=new.proto"]);
    assert!(
        r.stderr.contains(
            "Warning [W24]: 'b.proto': option 50000 of google.protobuf.FieldOptions is \
             declared by no file it sees; lost to move 'old.proto'"
        ),
        "{}",
        r.stderr
    );
}

#[test]
fn idle_move_is_w25() {
    let dir = setup("idle", &[("new", NEW)]);
    let r = run(
        &dir,
        Some("move:\n  - ghost.proto: new.proto\n"),
        &["--fail-on", "W25"],
    );
    assert!(
        r.stderr.contains(
            "Warning [W25]: move 'ghost.proto': nothing imports it, and no input holds it; \
             the directive is idle"
        ),
        "{}",
        r.stderr
    );
    assert_eq!(r.status, 3);
}

#[test]
fn broken_moves_are_w26() {
    let dir = setup(
        "broken",
        &[
            ("old", OLD),
            ("new", NEW),
            ("b", &importer("old.proto", false)),
        ],
    );
    let config = "\
prune:
  - file:pruned.proto
move:
  - old.proto: new.proto
  - old.proto: other.proto
  - a.proto: old.proto
  - c.proto: pruned.proto
variants:
  nosuchvariant:
    move:
      - x.proto: y.proto
";
    let r = run(&dir, Some(config), &[]);
    for expected in [
        "move 'old.proto': given twice; the second not applied",
        "move 'a.proto': its target 'old.proto' is itself moved; not applied",
        "move 'c.proto': its target 'pruned.proto' is pruned (file:pruned.proto); not applied",
    ] {
        assert!(
            r.stderr.contains(expected),
            "missing: {expected}\n{}",
            r.stderr
        );
    }
    // Another variant's section: expected in a shared file, not warned.
    assert!(!r.stderr.contains("nosuchvariant"), "{}", r.stderr);
    // The first old.proto move still applies.
    assert!(r
        .proto("b.proto")
        .unwrap()
        .contains("import \"new.proto\";"));
}

#[test]
fn a_missing_target_is_w26() {
    let dir = setup(
        "missing",
        &[("old", OLD), ("b", &importer("old.proto", false))],
    );
    let r = run(&dir, None, &["--move", "old.proto=absent.proto"]);
    assert!(
        r.stderr.contains(
            "Warning [W26]: move 'old.proto': its target 'absent.proto' is not in the input"
        ),
        "{}",
        r.stderr
    );
}

#[test]
fn variant_section_applies() {
    let dir = setup(
        "variant",
        &[
            ("old", OLD),
            ("new", NEW),
            ("b", &importer("old.proto", false)),
        ],
    );
    let config = "variants:\n  google.protobuf:\n    move:\n      - old.proto: new.proto\n";
    let r = run(&dir, Some(config), &[]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert!(r
        .proto("b.proto")
        .unwrap()
        .contains("import \"new.proto\";"));
}

#[test]
fn malformed_move_is_a_usage_error() {
    let dir = setup("malformed", &[("new", NEW)]);
    assert_eq!(run(&dir, None, &["--move", "old.proto"]).status, 2);
    assert_eq!(run(&dir, Some("move:\n  - old.proto: 3\n"), &[]).status, 2);
    assert_eq!(
        run(&dir, Some("variants:\n  x:\n    nope: []\n"), &[]).status,
        2
    );
}
