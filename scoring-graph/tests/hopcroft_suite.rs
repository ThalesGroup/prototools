// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Hopcroft minimizer regression suite (spec 0059).
//!
//! Each TC-xx test loads a fixture from tests/fixtures/hopcroft/<case>/input/
//! and asserts specific structural properties of the compiled graph.
//!
//! The full-corpus test (hopcroft_full_corpus) is gated on the environment
//! variable HOPCROFT_CORPUS_DIR.

use std::collections::HashMap;
use std::path::Path;

use score_graph_lib::build_scoring_graph::{build_compiled, serial::CompiledGraph};
use walkdir::WalkDir;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn load_fixture(case: &str) -> CompiledGraph {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hopcroft")
        .join(case)
        .join("input");
    load_dir(&dir)
}

fn load_dir(dir: &Path) -> CompiledGraph {
    let mut yamls: Vec<String> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
        .map(|e| std::fs::read_to_string(e.path()).expect("read yaml"))
        .collect();
    yamls.sort(); // deterministic merge order
    build_compiled(&yamls).expect("build_compiled")
}

/// Returns FQDN → state_id for all roots in the graph.
fn root_states(g: &CompiledGraph) -> HashMap<&str, u32> {
    g.roots
        .iter()
        .map(|r| (r.fqdn.as_str(), r.state_id))
        .collect()
}

/// Returns the child state reached from `from_state` via (field, label), or None.
fn child_state(g: &CompiledGraph, from_state: u32, field: u32, label: u8) -> Option<u32> {
    g.transitions
        .iter()
        .find(|t| t.state_id == from_state && t.field_number == field && t.label == label)
        .map(|t| t.child_state_id)
}

// ── TC-01: Two identical nodes collapse ──────────────────────────────────────

#[test]
fn tc01_identical_nodes_collapse() {
    let g = load_fixture("tc-01");
    let roots = root_states(&g);
    assert_eq!(
        roots["A"], roots["B"],
        "TC-01: A and B should collapse to the same state"
    );
}

// ── TC-02: Different field number → distinct ─────────────────────────────────

#[test]
fn tc02_different_field_number() {
    let g = load_fixture("tc-02");
    let roots = root_states(&g);
    assert_ne!(
        roots["A"], roots["B"],
        "TC-02: A (field 1) and B (field 2) should be distinct"
    );
}

// ── TC-03: Different field type → distinct ───────────────────────────────────

#[test]
fn tc03_different_field_type() {
    let g = load_fixture("tc-03");
    let roots = root_states(&g);
    assert_ne!(
        roots["A"], roots["B"],
        "TC-03: A (LEN_STRING) and B (VARINT) should be distinct"
    );
}

// ── TC-04: Different label → distinct ────────────────────────────────────────

#[test]
fn tc04_different_label() {
    let g = load_fixture("tc-04");
    let roots = root_states(&g);
    assert_ne!(
        roots["A"], roots["B"],
        "TC-04: A (optional) and B (repeated) should be distinct"
    );
}

// ── TC-05: LENDEL vs GROUP → distinct ────────────────────────────────────────

#[test]
fn tc05_lendel_vs_group() {
    let g = load_fixture("tc-05");
    let roots = root_states(&g);
    assert_ne!(
        roots["A"], roots["B"],
        "TC-05: A (LENDEL) and B (GROUP) should be distinct"
    );
}

// ── TC-06: AnnotatedMapEntry / MultiOptionMapEntry regression ────────────────

#[test]
fn tc06_map_entry_regression() {
    let g = load_fixture("tc-06");
    let roots = root_states(&g);

    assert_ne!(
        roots["AnnotatedMapEntry"], roots["MultiOptionMapEntry"],
        "TC-06: AnnotatedMapEntry and MultiOptionMapEntry must be in distinct states"
    );

    let map_state = roots["MapWithOptions"];
    let f1_child = child_state(&g, map_state, 1, 2 /*repeated*/)
        .expect("TC-06: MapWithOptions must have field 1 repeated transition");
    let f2_child = child_state(&g, map_state, 2, 2 /*repeated*/)
        .expect("TC-06: MapWithOptions must have field 2 repeated transition");

    assert_ne!(
        f1_child, f2_child,
        "TC-06: MapWithOptions field 1 and field 2 must lead to distinct child states"
    );
}

// ── TC-07: Bisimilar roots collapse ──────────────────────────────────────────

#[test]
fn tc07_bisimilar_roots_collapse() {
    let g = load_fixture("tc-07");
    let roots = root_states(&g);
    assert_eq!(
        roots["Root1"], roots["Root2"],
        "TC-07: Root1 and Root2 are bisimilar and should collapse to the same state"
    );
}

// ── Full-corpus regression test ───────────────────────────────────────────────

#[test]
fn hopcroft_full_corpus() {
    let corpus_dir = match std::env::var("HOPCROFT_CORPUS_DIR") {
        Ok(d) => d,
        Err(_) => {
            eprintln!("hopcroft_full_corpus: HOPCROFT_CORPUS_DIR not set, skipping");
            return;
        }
    };

    let dir = Path::new(&corpus_dir);
    assert!(
        dir.exists(),
        "HOPCROFT_CORPUS_DIR does not exist: {corpus_dir}"
    );

    // Run 8 times: the bug (spec 0062) fired ~75% of runs due to HashMap
    // non-determinism, so 8 independent compilations give high confidence.
    let mwo = "test.field.MapWithOptions";
    for run in 1..=8 {
        let g = load_dir(dir);
        let roots = root_states(&g);

        assert!(
            roots.contains_key(mwo),
            "full corpus run {run}: root '{mwo}' not found"
        );

        let map_state = roots[mwo];
        let f1_child = child_state(&g, map_state, 1, 2 /*repeated*/)
            .expect("full corpus: MapWithOptions must have field 1 repeated transition");
        let f2_child = child_state(&g, map_state, 2, 2 /*repeated*/)
            .expect("full corpus: MapWithOptions must have field 2 repeated transition");

        assert_ne!(
            f1_child, f2_child,
            "full corpus regression (spec 0062) run {run}: MapWithOptions field 1 and \
             field 2 must lead to distinct child states (AnnotatedMapEntry ≠ MultiOptionMapEntry)"
        );
    }
}
