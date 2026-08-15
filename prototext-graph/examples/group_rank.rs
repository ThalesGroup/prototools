// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway. Two build-time signals against per-group cost, because the
//! expensive tier has two halves and one signal cannot cover both:
//!
//! - **Encompassing.** A type nothing else contains is a top-level type,
//!   and a top-level type is what someone serializes — so it is a
//!   candidate for being the blob's *true* type, which is the group that
//!   survives to the end. Measured as in-degree in the state graph, which
//!   is the compiled form of the import/containment structure. Closure
//!   size was already measured against cost at -0.089 and is not it.
//!
//! - **Permissive.** A type that declares almost nothing cannot be
//!   contradicted, so it survives the whole blob without ever being
//!   right. 35 of the 36 groups over 100 ms have 2-8 states. Measured as
//!   the number of field numbers the root state declares.
//!
//! Reports rank correlation, and — the part that decides it — how much of
//! the true top 43's cost each signal's own top 43 actually captures.

use prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph;
use prototext_graph::score::{load as score_load, score_subset, ScoringOpts};
use std::collections::HashSet;
use std::time::Instant;

fn ranks(v: &[f64]) -> Vec<f64> {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by(|&a, &b| v[a].partial_cmp(&v[b]).expect("no NaN"));
    let mut r = vec![0.0; v.len()];
    let mut i = 0;
    while i < idx.len() {
        let mut j = i;
        while j + 1 < idx.len() && v[idx[j + 1]] == v[idx[i]] {
            j += 1;
        }
        let avg = ((i + j) as f64) / 2.0 + 1.0;
        for &k in &idx[i..=j] {
            r[k] = avg;
        }
        i = j + 1;
    }
    r
}

fn spearman(a: &[f64], b: &[f64]) -> f64 {
    let (ra, rb) = (ranks(a), ranks(b));
    let n = a.len() as f64;
    let ma = ra.iter().sum::<f64>() / n;
    let mb = rb.iter().sum::<f64>() / n;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..a.len() {
        num += (ra[i] - ma) * (rb[i] - mb);
        da += (ra[i] - ma).powi(2);
        db += (rb[i] - mb).powi(2);
    }
    num / (da.sqrt() * db.sqrt())
}

/// How many distinct states have a transition into each state. Zero means
/// nothing in the whole database contains this type.
fn in_degree(g: &ArchivedCompiledGraph) -> Vec<usize> {
    let mut parents: Vec<HashSet<u32>> = vec![HashSet::new(); g.nodes.len()];
    for (s, node) in g.nodes.iter().enumerate() {
        let off = node.trans_offset.to_native() as usize;
        let len = node.trans_len.to_native() as usize;
        for t in &g.transitions[off..off + len] {
            let c = t.child_state_id.to_native();
            if c != u32::MAX {
                parents[c as usize].insert(s as u32);
            }
        }
    }
    parents.into_iter().map(|p| p.len()).collect()
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

    let indeg = in_degree(&g);
    let fanout = |s: u32| g.nodes[s as usize].trans_len.to_native() as f64;

    let mut time = Vec::with_capacity(groups.len());
    for (_, grp) in &groups {
        let t0 = Instant::now();
        let r = score_subset(&pb, &g, &opts, grp, None);
        std::hint::black_box(r.len());
        time.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    // Encompassing: nothing contains it. Permissive: it declares little.
    // Both are stated so that a *larger* number means *more expensive*.
    let encompassing: Vec<f64> = groups
        .iter()
        .map(|(s, _)| 1.0 / (1.0 + indeg[*s as usize] as f64))
        .collect();
    let permissive: Vec<f64> = groups
        .iter()
        .map(|(s, _)| 1.0 / (1.0 + fanout(*s)))
        .collect();
    let both: Vec<f64> = (0..groups.len())
        .map(|i| encompassing[i].max(permissive[i]))
        .collect();

    println!(
        "spearman(alone, encompassing) = {:+.3}",
        spearman(&time, &encompassing)
    );
    println!(
        "spearman(alone, permissive)   = {:+.3}",
        spearman(&time, &permissive)
    );

    let desc = |key: &[f64]| {
        let mut o: Vec<usize> = (0..key.len()).collect();
        o.sort_by(|&a, &b| key[b].partial_cmp(&key[a]).expect("no NaN"));
        o
    };
    let truth = desc(&time);
    println!();
    println!("of the true top 43 (every group over 50 ms):");
    for (name, key) in [
        ("encompassing", &encompassing),
        ("permissive", &permissive),
        ("either", &both),
    ] {
        let order = desc(key);
        let best: f64 = truth.iter().take(43).map(|&i| time[i]).sum();
        for n in [43usize, 128, 512] {
            let got: f64 = order.iter().take(n).map(|&i| time[i]).sum();
            let hits = order
                .iter()
                .take(n)
                .filter(|i| truth[..43].contains(i))
                .count();
            println!(
                "  {name:12} top {n:4}: {hits:2}/43 of them, {:5.1}% of their cost",
                100.0 * got / best
            );
        }
    }

    // Does it find the champion at all? That is the single group whose
    // placement decides the makespan.
    let champ = truth[0];
    for (name, key) in [("encompassing", &encompassing), ("permissive", &permissive)] {
        let order = desc(key);
        let at = order
            .iter()
            .position(|&i| i == champ)
            .expect("the champion");
        println!(
            "{name:12} ranks {} at {at} of {}",
            g.roots[groups[champ].1[0] as usize].fqdn.as_str(),
            groups.len()
        );
    }
}
