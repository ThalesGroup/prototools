// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Out-of-range scoring as a penalty, end to end (spec 0178).
//!
//! After spec 0176 the `Range` leaf reaches two kinds of field: `bool`,
//! range `(0, 1)`, and a closed enum. An out-of-range value on either used
//! to veto, eliminating the candidate; but neither is impossible on the
//! wire (`bool` is `value != 0` in every generated parser), and only the
//! impossible is vetoed. The charge is now `out_of_range`, weighted -15.
//! The closed-enum half is `test_open_enum_scoring`'s E3; this file covers
//! `bool` and the ranking consequence: the candidate stays in the ranking,
//! to be outscored, and does not win.
//!
//! The ranges are in the scoring graph protoback builds into the schema DB
//! (`hopcroft.rkyv`), read by the prototext binary (spec 0488 D4). Ported
//! from `test_out_of_range_scoring.py`, R1 to R3, names lower-cased. Its R4,
//! that prototext rejects `--relax-ranges` and `--no-strict-ranges` (spec
//! 0178 S3), tested prototext's command line alone: dropped (the
//! maintainer, 2026-11-26).

use crate::common::{tmp, tools};
use crate::test_open_enum_scoring::{build_db, run};
use std::path::{Path, PathBuf};

/// Field 1, varint 802: outside a bool's [0, 1], an ordinary int32.
const OUT_OF_RANGE_PAYLOAD: &[u8] = b"\x08\xa2\x06";

/// Two competing single-field schemas over the same tag, 802 out of range
/// under one and clean under the other (`tc77_12_bool_vs_int32_discrimination`
/// in prototext-graph, driven through the binary).
const BOOL_VS_INT32_PROTO: &str = r#"syntax = "proto3";
package oor;
message Flags { bool enabled = 1; }
message Count { int32 n = 1; }
"#;

fn setup(test: &str) -> Option<(PathBuf, PathBuf)> {
    if !tools(&["protoc", "prototext"]) {
        return None;
    }
    let dir = tmp(test);
    let db = build_db(&dir, "oor", BOOL_VS_INT32_PROTO);
    Some((db, payload(&dir)))
}

fn payload(dir: &Path) -> PathBuf {
    let path = dir.join("oor.bin");
    std::fs::write(&path, OUT_OF_RANGE_PAYLOAD).unwrap();
    path
}

/// A `bool` of 802 is charged `out_of_range`, not vetoed (spec 0178 S2);
/// `non_canonical: 0` too, the varint minimally encoded, the two counters
/// distinguishable.
#[test]
fn test_r1_bool_out_of_range_is_penalized_not_vetoed() {
    // Not the test's name: prototext prints the payload's path, which must
    // not hold the word asserted absent.
    let Some((db, payload)) = setup("test_r1_bool_out_of_range") else {
        return;
    };
    let out = run(&db, &payload, &["score", "--type", "oor.Flags"]);
    assert!(
        !out.contains("vetoed"),
        "802 parses as `true`; no veto:\n{out}"
    );
    assert!(out.contains("out_of_range: 1"), "{out}");
    assert!(
        out.contains("non_canonical: 0"),
        "the encoding is canonical:\n{out}"
    );
}

/// `oor.Flags` survives to be ranked, and `oor.Count` outranks it: survival
/// is what spec 0178 changed, losing proves the penalty still
/// discriminates.
#[test]
fn test_r2_penalized_candidate_stays_in_the_ranking_but_loses() {
    let Some((db, payload)) = setup("test_r2_penalized_candidate_stays_in_the_ranking_but_loses")
    else {
        return;
    };
    let out = run(&db, &payload, &["list-schemas", "--top", "5"]);
    let (flags, count) = (out.find("oor.Flags"), out.find("oor.Count"));
    assert!(
        flags.is_some(),
        "a penalized candidate is not eliminated:\n{out}"
    );
    assert!(count.is_some(), "{out}");
    assert!(
        count < flags,
        "list-schemas is score-descending; Count must come first:\n{out}"
    );
}

/// `list-schemas --detailed-score` surfaces the counter (spec 0178 S4).
#[test]
fn test_r3_detailed_score_reports_out_of_range() {
    let Some((db, payload)) = setup("test_r3_detailed_score_reports_out_of_range") else {
        return;
    };
    let out = run(
        &db,
        &payload,
        &["list-schemas", "--top", "5", "--detailed-score"],
    );
    assert!(out.contains("out_of_range: 1"), "{out}");
}
