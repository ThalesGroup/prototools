// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! End-to-end CLI regression tests for `score-graph build-scoring-graph` and
//! `score-graph score`.
//!
//! Each test:
//!   1. Runs `score-graph build-scoring-graph` on the YAML fixture in
//!      tests/fixtures/scoring/ to produce a compiled graph.
//!   2. Passes the corresponding binary protobuf fixture from
//!      tests/fixtures/scoring/proto/ to `score-graph score`.
//!   3. Asserts on the stdout line.
//!
//! The schema under test is the Outer/Inner schema defined in
//! tests/fixtures/scoring/outer.proto (compiled to outer.pb, then to
//! outer.yaml via reproto --emit-scoring-graphs).

use std::path::{Path, PathBuf};
use std::process::Command;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scoring")
}

fn proto_fixture(name: &str) -> PathBuf {
    fixture_dir().join("proto").join(format!("{name}.pb"))
}

fn score_graph_bin() -> &'static str {
    env!("CARGO_BIN_EXE_score-graph")
}

/// Build the scoring graph from the YAML fixture into a temp file.
/// Returns the path of the compiled .bin graph (inside the tempdir).
/// The returned `tempfile::TempDir` must be kept alive for the duration of the test.
fn build_graph() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let graph_path = dir.path().join("graph.bin");

    let status = Command::new(score_graph_bin())
        .args(["build-scoring-graph", "-q", "-o"])
        .arg(&graph_path)
        .arg(fixture_dir())
        .status()
        .expect("failed to run score-graph build-scoring-graph");
    assert!(status.success(), "build-scoring-graph failed");

    (dir, graph_path)
}

/// Run `score-graph score <graph> <entry> <proto>` and return stdout (trimmed).
fn run_score(graph: &Path, entry: &str, proto: &Path) -> String {
    let out = Command::new(score_graph_bin())
        .args(["score"])
        .arg(graph)
        .arg(entry)
        .arg(proto)
        .output()
        .expect("failed to run score-graph score");
    assert!(
        out.status.success(),
        "score-graph score exited non-zero:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// E2E-01: Perfect match — all fields present, all known, no errors.
#[test]
fn e2e_01_perfect_match() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e01"));
    assert_eq!(out, "matches=6 unknowns=0 non_canonical=0 score=6");
}

/// E2E-02: All-unknown fields.
#[test]
fn e2e_02_all_unknown() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e02"));
    assert_eq!(out, "matches=0 unknowns=3 non_canonical=0 score=-30");
}

/// E2E-03: Wrong wire type on a known field → veto.
#[test]
fn e2e_03_wrong_wire_type_veto() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e03"));
    assert_eq!(out, "Vetoed");
}

/// E2E-04: Invalid UTF-8 on string field → veto.
#[test]
fn e2e_04_invalid_utf8_veto() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e04"));
    assert_eq!(out, "Vetoed");
}

/// E2E-05: Enum value out of range → veto.
#[test]
fn e2e_05_enum_out_of_range_veto() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e05"));
    assert_eq!(out, "Vetoed");
}

/// E2E-06: Mixed known and unknown fields.
#[test]
fn e2e_06_mixed_known_unknown() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e06"));
    assert_eq!(out, "matches=2 unknowns=2 non_canonical=0 score=-18");
}

/// E2E-07: Sub-message recursion — matches accumulate across both levels.
#[test]
fn e2e_07_submessage_recursion() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e07"));
    // id + child-field + inner.value = 3 matches
    assert_eq!(out, "matches=3 unknowns=0 non_canonical=0 score=3");
}

/// E2E-08: Repeated field — multiple occurrences all count.
#[test]
fn e2e_08_repeated_multiple() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e08"));
    assert_eq!(out, "matches=4 unknowns=0 non_canonical=0 score=4");
}

/// E2E-09: Truncated LEN payload → veto (structural parse error).
#[test]
fn e2e_09_truncated_len_veto() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e09"));
    assert_eq!(out, "Vetoed");
}

/// E2E-10: Empty message — no fields, not vetoed.
#[test]
fn e2e_10_empty_message() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e10"));
    assert_eq!(out, "matches=0 unknowns=0 non_canonical=0 score=0");
}

/// E2E-11: Tag varint overhang on known field → non_canonical.
#[test]
fn e2e_11_tag_overhang() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e11"));
    assert_eq!(out, "matches=1 unknowns=0 non_canonical=1 score=-19");
}

/// E2E-12: Value varint overhang on known field → non_canonical.
#[test]
fn e2e_12_value_overhang() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e12"));
    assert_eq!(out, "matches=1 unknowns=0 non_canonical=1 score=-19");
}

/// E2E-13: LEN length-prefix overhang on known field → non_canonical.
#[test]
fn e2e_13_len_prefix_overhang() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e13"));
    assert_eq!(out, "matches=1 unknowns=0 non_canonical=1 score=-19");
}

/// E2E-14: Out-of-range field number (0) → non_canonical + unknown.
#[test]
fn e2e_14_field_number_oor() {
    let (_dir, graph) = build_graph();
    let out = run_score(&graph, "Outer", &proto_fixture("e14"));
    assert_eq!(out, "matches=0 unknowns=1 non_canonical=1 score=-30");
}
