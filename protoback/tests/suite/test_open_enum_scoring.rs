// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Open-enum range emission, end to end (spec 0176).
//!
//! An open enum (proto3, or editions with `features.enum_type = OPEN`)
//! accepts every 32-bit value: an unrecognized number is preserved on round
//! trip, and is what a newer sender emits to an older reader, so a `[min,
//! max]` range for one vetoed a blob its own true schema accepts. A closed
//! enum (proto2, or CLOSED) rejects unrecognized numbers, so its range must
//! survive. The proto2 case is the control that keeps the proto3 case from
//! passing for the wrong reason: both take the same non-vetoing path and
//! differ only in the `out_of_range` counter.
//!
//! The schema DB is protoback's; it is read as a user reads it, by the
//! prototext binary (spec 0488 D4). Ported from `test_open_enum_scoring.py`;
//! its tests' names lower-cased.

use crate::common::{protoback, prototext, tmp, tools, write};
use std::path::{Path, PathBuf};

/// Paint{ color: 99 }: field 1, varint, a number no declared value has.
const UNKNOWN_VALUE_PAYLOAD: &[u8] = b"\x08\x63";

const OPEN_ENUM_PROTO: &str = r#"syntax = "proto3";
package openenum;
enum Color {
  COLOR_UNSPECIFIED = 0;
  COLOR_RED = 1;
  COLOR_GREEN = 2;
}
message Paint {
  Color color = 1;
}
"#;

const CLOSED_ENUM_PROTO: &str = r#"syntax = "proto2";
package closedenum;
enum Color {
  COLOR_UNSPECIFIED = 0;
  COLOR_RED = 1;
  COLOR_GREEN = 2;
}
message Paint {
  optional Color color = 1;
}
"#;

/// `proto` compiled and made a schema DB (.desc and its sidecars); used
/// by `test_out_of_range_scoring` too.
pub fn build_db(dir: &Path, name: &str, proto: &str) -> PathBuf {
    let src = dir.join(name).join("src");
    write(&src.join(format!("{name}.proto")), proto);
    let pb = dir.join(name).join(format!("{name}.pb"));
    let o = std::process::Command::new("protoc")
        .arg(format!("-I{}", src.display()))
        .arg(format!("--descriptor_set_out={}", pb.display()))
        .arg(format!("{name}.proto"))
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "protoc failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let db = dir.join(name).join(format!("{name}.desc"));
    let r = protoback([
        "--use-variant".into(),
        "descriptor".into(),
        format!("--proto-out={}", dir.join(name).join("out").display()),
        format!("--schema-db-out={}", db.display()),
        pb.display().to_string(),
    ]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    db
}

fn payload(dir: &Path) -> PathBuf {
    let path = dir.join("paint.bin");
    std::fs::write(&path, UNKNOWN_VALUE_PAYLOAD).unwrap();
    path
}

/// prototext on `payload` against `db`, required to succeed; its output
/// (`_prototext`).
pub fn run(db: &Path, payload: &Path, args: &[&str]) -> String {
    let mut all: Vec<String> = vec!["--descriptor-set".into(), db.display().to_string()];
    all.extend(args.iter().map(|a| a.to_string()));
    all.extend(["--assume-binary".into(), payload.display().to_string()]);
    let o = prototext(&all);
    assert_eq!(o.code, Some(0), "prototext failed:\n{}", o.stderr);
    o.stdout
}

/// A proto3 enum value outside the declared set scores as a plain match
/// against its own true schema: not merely un-vetoed but un-penalized, an
/// open enum having no range to be outside of (spec 0176 S1).
#[test]
fn test_e1_open_enum_unknown_value_is_not_vetoed() {
    if !tools(&["protoc", "prototext"]) {
        return;
    }
    // Not the test's name: prototext prints the payload's path, which must
    // not hold the word asserted absent (pytest's directory, cut to 30
    // characters, never did).
    let dir = tmp("test_e1_open_enum");
    let db = build_db(&dir, "openenum", OPEN_ENUM_PROTO);
    let out = run(&db, &payload(&dir), &["score", "--type", "openenum.Paint"]);
    assert!(!out.contains("vetoed"), "open enum must not veto:\n{out}");
    assert!(out.contains("matches: 1"), "{out}");
    assert!(
        out.contains("out_of_range: 0"),
        "an open enum has no range:\n{out}"
    );
}

/// The same blob's true type is a surviving `list-schemas` candidate rather
/// than eliminated before ranking (spec 0176 S1).
#[test]
fn test_e2_open_enum_true_type_wins_the_ranking() {
    if !tools(&["protoc", "prototext"]) {
        return;
    }
    let dir = tmp("test_e2_open_enum_true_type_wins_the_ranking");
    let db = build_db(&dir, "openenum", OPEN_ENUM_PROTO);
    let out = run(&db, &payload(&dir), &["list-schemas", "--top", "5"]);
    assert!(out.contains("type: openenum.Paint"), "{out}");
}

/// Control (spec 0176 N1, spec 0178 S2): the same bytes against a proto2
/// closed enum are charged `out_of_range`, a penalty, not a veto: the
/// candidate survives ranking.
#[test]
fn test_e3_closed_enum_unknown_value_is_penalized() {
    if !tools(&["protoc", "prototext"]) {
        return;
    }
    let dir = tmp("test_e3_closed_enum_unknown_value_is_penalized");
    let db = build_db(&dir, "closedenum", CLOSED_ENUM_PROTO);
    let payload = payload(&dir);
    let out = run(&db, &payload, &["score", "--type", "closedenum.Paint"]);
    assert!(
        !out.contains("vetoed"),
        "out of range is not impossible:\n{out}"
    );
    assert!(
        out.contains("out_of_range: 1"),
        "closed enum keeps its range:\n{out}"
    );
    assert!(
        out.contains("non_canonical: 0"),
        "the encoding itself is fine:\n{out}"
    );
    let ranked = run(&db, &payload, &["list-schemas", "--top", "5"]);
    assert!(ranked.contains("closedenum.Paint"), "{ranked}");
}
