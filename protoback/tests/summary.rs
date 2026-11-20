// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The `protos:` summary line: where the `.proto` files were written, and
//! how many. After the database's summary with `--schema-db-out`, at the
//! end of "Rendering proto files" without; none in a dry run or under
//! `--quiet`.

use std::path::{Path, PathBuf};
use std::process::Command;

const A: &str = r#"name: "a.proto" package: "p" message_type { name: "A" }"#;
const B: &str = r#"name: "b.proto" package: "p" dependency: "a.proto"
  message_type { name: "B"
    field { name: "a" number: 1 label: LABEL_OPTIONAL type: TYPE_MESSAGE type_name: ".p.A" } }"#;

/// A fresh directory, `in/` holding `a.textpb` and `b.textpb`.
fn setup(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("summary-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    std::fs::write(dir.join("in/a.textpb"), A).unwrap();
    std::fs::write(dir.join("in/b.textpb"), B).unwrap();
    dir
}

/// protoback on `dir/in` with `args`: its exit status and stderr.
fn run(dir: &Path, args: &[&str]) -> (i32, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_protoback"))
        .current_dir(dir)
        .args(["-I", "in"])
        .args(args)
        .env_remove("REPROTO_VARIANT")
        .output()
        .unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn proto_out_alone_reports_after_rendering() {
    let dir = setup("proto-out");
    let (status, stderr) = run(&dir, &["-O", "./out/"]);
    assert_eq!(status, 0, "{stderr}");
    // The directory as Python's Path prints it, as reproto does.
    let line = "  protos:     out (2 files)";
    let lines: Vec<&str> = stderr.lines().collect();
    let at = lines.iter().position(|l| *l == line).expect(&stderr);
    assert_eq!(lines[at - 1], "Rendering proto files", "{stderr}");
}

#[test]
fn schema_db_reports_after_its_summary() {
    let dir = setup("schema-db");
    let (status, stderr) = run(&dir, &["--schema-db-out", "db.desc"]);
    assert_eq!(status, 0, "{stderr}");
    let lines: Vec<&str> = stderr.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == "  protos:     db/proto (2 files)")
        .expect(&stderr);
    assert!(lines[at - 1].starts_with("  index:      "), "{stderr}");
}

#[test]
fn nothing_written_nothing_reported() {
    for args in [
        &["-O", "out", "--dry-run"][..],
        &["-O", "out", "--quiet"][..],
        &["--schema-db-out", "db.desc", "--no-proto-out"][..],
    ] {
        let dir = setup("none");
        let (status, stderr) = run(&dir, args);
        assert_eq!(status, 0, "{args:?}: {stderr}");
        assert!(!stderr.contains("protos:"), "{args:?}: {stderr}");
    }
}
