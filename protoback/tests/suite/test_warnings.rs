// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The closing hint of the warning summary (spec 0041 TC-W2): "Run with
//! --detailed-warnings" when a warning occurred more than once, not when
//! each occurred once.
//!
//! Ported from `test_warnings.py`, its two W2 tests, adapted: the original
//! flushed reproto's collector and read the hint off its log; protoback
//! prints the hint to the console alone, unrecorded, so a run is read.
//! `test_warning_codes`' inputs give both cases: W5 twice (`a.textpb`), and
//! two W22, once each (`b.pb`). Its other tests, on the collector's lines,
//! are in `protoback/src/events.rs`.

use crate::common::{encode_fixture, fixture, protoback, tmp, Run};
use std::path::Path;

fn run(dir: &Path, name: &str) -> Run {
    protoback([
        format!("-I{}", dir.join("in").display()),
        "-O".into(),
        dir.join("out").display().to_string(),
        "--use-variant".into(),
        "descriptor".into(),
        name.into(),
    ])
}

#[test]
fn test_w2_hint_appears_when_count_gt1() {
    let dir = tmp("test_w2_hint_appears_when_count_gt1");
    std::fs::create_dir_all(dir.join("in")).unwrap();
    std::fs::copy(fixture("warning_codes/a.textpb"), dir.join("in/a.textpb")).unwrap();
    let r = run(&dir, "a.textpb");
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(r.stderr.contains("(2 occurrences)"), "{}", r.stderr);
    assert!(
        r.stderr.contains("--detailed-warnings"),
        "Hint must appear when any warning has count > 1:\n{}",
        r.stderr
    );
}

#[test]
fn test_w2_hint_absent_when_all_count_1() {
    let dir = tmp("test_w2_hint_absent_when_all_count_1");
    let sha = "988a8898a4c4704dd9501740826382f8a9c1bb5a8119a8d408b9edf2211c9f16";
    encode_fixture("warning_codes/b.pb", sha, &dir.join("in/b.pb"));
    let r = run(&dir, "b.pb");
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(
        r.stderr.contains("(1 occurrence)") && !r.stderr.contains("occurrences)"),
        "{}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("--detailed-warnings"),
        "Hint must not appear when all counts are 1:\n{}",
        r.stderr
    );
}
