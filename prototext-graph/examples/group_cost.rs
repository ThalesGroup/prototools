// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway. Eight groups out of 1628 carry 70% of a part's cost, so the
//! quantity worth predicting is a *group's* cost, not a part's. Three
//! static proxies were measured against part cost and all read ~0 — but a
//! part's closure is the union of 695 of them and is ~3200 states however
//! it is assembled, which is exactly the restriction of range that would
//! hide the signal. Measured per group, over every group in the graph.

use prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph;
use prototext_graph::score::{load as score_load, score_subset, ScoringOpts};
use std::collections::HashSet;
use std::time::Instant;

fn closure(g: &ArchivedCompiledGraph, seed: u32) -> (usize, usize) {
    let mut seen: HashSet<u32> = HashSet::new();
    let mut stack = vec![seed];
    seen.insert(seed);
    let mut edges = 0usize;
    while let Some(s) = stack.pop() {
        let node = &g.nodes[s as usize];
        let off = node.trans_offset.to_native() as usize;
        let len = node.trans_len.to_native() as usize;
        edges += len;
        for t in &g.transitions[off..off + len] {
            let c = t.child_state_id.to_native();
            if c != u32::MAX && seen.insert(c) {
                stack.push(c);
            }
        }
    }
    (seen.len(), edges)
}

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
    eprintln!("{} groups", groups.len());

    let t = Instant::now();
    let stats: Vec<(usize, usize)> = groups.iter().map(|(s, _)| closure(&g, *s)).collect();
    eprintln!("closures for every group in {:?}", t.elapsed());

    let t = Instant::now();
    let mut time = Vec::with_capacity(groups.len());
    for (_, grp) in &groups {
        let t0 = Instant::now();
        let r = score_subset(&pb, &g, &opts, grp, None);
        std::hint::black_box(r.len());
        time.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    eprintln!("timed every group alone in {:?}", t.elapsed());

    let states: Vec<f64> = stats.iter().map(|s| s.0 as f64).collect();
    let edges: Vec<f64> = stats.iter().map(|s| s.1 as f64).collect();
    let roots: Vec<f64> = groups.iter().map(|(_, grp)| grp.len() as f64).collect();
    println!(
        "spearman(alone, closure states) = {:+.3}",
        spearman(&time, &states)
    );
    println!(
        "spearman(alone, closure edges)  = {:+.3}",
        spearman(&time, &edges)
    );
    println!(
        "spearman(alone, roots in group) = {:+.3}",
        spearman(&time, &roots)
    );

    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|&a, &b| time[b].partial_cmp(&time[a]).expect("no NaN"));
    let sum: f64 = time.iter().sum();
    println!();
    println!("summed alone: {sum:.0} ms");
    for n in [1usize, 4, 16, 64, 256] {
        let share: f64 = order.iter().take(n).map(|&i| time[i]).sum();
        println!("  top {n:4} groups = {:5.1}% of it", 100.0 * share / sum);
    }

    println!();
    println!("the 16 most expensive groups:");
    println!("     ms   states   edges  roots  fqdn");
    for &i in order.iter().take(16) {
        println!(
            "  {:6.1}  {:6}  {:6}  {:5}  {}",
            time[i],
            stats[i].0,
            stats[i].1,
            groups[i].1.len(),
            g.roots[groups[i].1[0] as usize].fqdn.as_str()
        );
    }

    // What a scheduler would actually consume: do the static proxies find
    // the expensive groups, whatever they do with the cheap ones?
    let mut by_states: Vec<usize> = (0..groups.len()).collect();
    by_states.sort_by(|&a, &b| states[b].partial_cmp(&states[a]).expect("no NaN"));
    for n in [16usize, 64, 256] {
        let want: HashSet<usize> = order.iter().take(n).copied().collect();
        let got = by_states
            .iter()
            .take(n)
            .filter(|i| want.contains(i))
            .count();
        let covered: f64 = by_states.iter().take(n).map(|&i| time[i]).sum::<f64>();
        let best: f64 = order.iter().take(n).map(|&i| time[i]).sum();
        println!(
            "top {n:4} by closure states: {got}/{n} of the true top {n}, \
             {:.0}% of their cost",
            100.0 * covered / best
        );
    }
}
