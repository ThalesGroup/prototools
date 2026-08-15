// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway. Per-group cost is bimodal — a median group costs 0.00 ms
//! and a few hundred cost ~120 ms — and no graph statistic ranks it
//! (spearman -0.09 against closure size). This asks whether the split is
//! the obvious one: a root that the blob's *top-level tag* vetoes stops
//! immediately, and a root it does not must scan all 25 MB.
//!
//! The blob is a FileDescriptorSet, so its top level is field 1, LEN,
//! repeated. A root declaring field 1 with any other wire type mismatches
//! on the first tag and is vetoed; one declaring it LEN, or not declaring
//! it at all (an unknown field never vetoes), has to keep reading.

use prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph;
use prototext_graph::score::{load as score_load, score_subset, ScoringOpts};
use std::time::Instant;

/// What state `s` declares for field 1: `None` if it does not declare it,
/// otherwise the child wire type it expects and whether that field leads
/// to a child state — that is, whether matching it makes the walk
/// *descend into* the record rather than skip past it.
fn field_one(g: &ArchivedCompiledGraph, s: u32) -> Option<(u8, bool)> {
    let node = &g.nodes[s as usize];
    let off = node.trans_offset.to_native() as usize;
    let len = node.trans_len.to_native() as usize;
    g.transitions[off..off + len]
        .iter()
        .find(|t| t.field_number.to_native() == 1)
        .map(|t| (t.child_wire_type, t.child_state_id.to_native() != u32::MAX))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let g = score_load::load_graph(std::path::Path::new(&args[1])).expect("load graph");
    let pb = std::fs::read(&args[2]).expect("read blob");
    let opts = ScoringOpts::default();

    let total = g.roots.len();
    let mut by_state: Vec<(u32, u32)> = (0..total as u32)
        .map(|i| (g.roots[i as usize].state_id.to_native(), i))
        .collect();
    by_state.sort_unstable();
    let mut groups: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut i = 0;
    while i < by_state.len() {
        let state = by_state[i].0;
        let mut grp = Vec::new();
        while i < by_state.len() && by_state[i].0 == state {
            grp.push(by_state[i].1);
            i += 1;
        }
        groups.push((state, grp));
    }

    let mut time = Vec::with_capacity(groups.len());
    for (_, grp) in &groups {
        let t0 = Instant::now();
        let r = score_subset(&pb, &g, &opts, grp, None);
        std::hint::black_box(r.len());
        time.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    // Class each group by what it declares for field 1, and report what
    // that class costs.
    let mut classes: std::collections::BTreeMap<String, (usize, f64, f64)> =
        std::collections::BTreeMap::new();
    for (i, (state, _)) in groups.iter().enumerate() {
        let name = match field_one(&g, *state) {
            None => "1 undeclared".to_string(),
            Some((2, true)) => "1 is a LEN message".to_string(),
            Some((2, false)) => "1 is a LEN scalar".to_string(),
            Some((0, _)) => "1 is a varint".to_string(),
            Some((w, _)) => format!("1 is wire {w}"),
        };
        let e = classes.entry(name).or_insert((0, 0.0, 0.0));
        e.0 += 1;
        e.1 += time[i];
        e.2 = e.2.max(time[i]);
    }
    let sum: f64 = time.iter().sum();
    println!("class                 groups     total_ms   mean_ms    max_ms   share");
    for (name, (n, t, mx)) in &classes {
        println!(
            "{name:20}  {n:6}  {t:11.1}  {:8.2}  {mx:8.1}  {:5.1}%",
            t / *n as f64,
            100.0 * t / sum
        );
    }

    // The threshold view: how many groups are expensive at all?
    let mut sorted = time.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    let over = |ms: f64| time.iter().filter(|&&t| t >= ms).count();
    println!();
    println!(
        "cost is bimodal: {} groups under 1 ms, {} over 50 ms, {} over 100 ms",
        over(0.0) - over(1.0),
        over(50.0),
        over(100.0)
    );
    println!(
        "median {:.2} ms, p90 {:.2} ms, p99 {:.2} ms, max {:.1} ms",
        sorted[sorted.len() / 2],
        sorted[sorted.len() * 9 / 10],
        sorted[sorted.len() * 99 / 100],
        sorted[sorted.len() - 1]
    );
}
