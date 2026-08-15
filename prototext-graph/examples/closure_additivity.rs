// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway. How far is a part's cost from the sum of its groups' costs?
//! If closures were disjoint the two would be equal and a part could be
//! packed like a knapsack. They are not, so a group has no cost of its own.

use prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph;
use prototext_graph::score::{load as score_load, partition_roots};
use std::collections::HashSet;

fn closure(g: &ArchivedCompiledGraph, seeds: &[u32], out: &mut HashSet<u32>) {
    let mut stack: Vec<u32> = Vec::new();
    for &s in seeds {
        if out.insert(s) {
            stack.push(s);
        }
    }
    while let Some(s) = stack.pop() {
        let node = &g.nodes[s as usize];
        let off = node.trans_offset.to_native() as usize;
        let len = node.trans_len.to_native() as usize;
        for t in &g.transitions[off..off + len] {
            let c = t.child_state_id.to_native();
            if c != u32::MAX && out.insert(c) {
                stack.push(c);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let g = score_load::load_graph(std::path::Path::new(&args[1])).expect("load graph");

    let mut all = HashSet::new();
    closure(
        &g,
        &(0..g.roots.len())
            .map(|i| g.roots[i].state_id.to_native())
            .collect::<Vec<_>>(),
        &mut all,
    );
    println!("states in graph        : {}", g.nodes.len());
    println!("closure(all roots)     : {}", all.len());
    println!();

    let parts = partition_roots(&g, 24);
    println!("part  groups  |closure(part)|  sum|closure(group)|  ratio");
    let mut tot_union = 0usize;
    let mut tot_sum = 0usize;
    for (i, part) in parts.iter().enumerate().take(6) {
        let states: Vec<u32> = part
            .iter()
            .map(|&r| g.roots[r as usize].state_id.to_native())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let mut u = HashSet::new();
        closure(&g, &states, &mut u);
        let per_group: usize = states
            .iter()
            .map(|&s| {
                let mut c = HashSet::new();
                closure(&g, &[s], &mut c);
                c.len()
            })
            .sum();
        tot_union += u.len();
        tot_sum += per_group;
        println!(
            "{i:4}  {:6}  {:15}  {:18}  {:5.1}x",
            states.len(),
            u.len(),
            per_group,
            per_group as f64 / u.len() as f64
        );
    }
    println!();
    println!(
        "first 6 parts: union {tot_union}, summed {tot_sum}, \
         so per-group costs overcount by {:.1}x",
        tot_sum as f64 / tot_union as f64
    );

    // The marginal question: what does moving one group actually change?
    let part = &parts[0];
    let states: Vec<u32> = part
        .iter()
        .map(|&r| g.roots[r as usize].state_id.to_native())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut base = HashSet::new();
    closure(&g, &states, &mut base);
    let mut zero = 0usize;
    let mut marginals: Vec<usize> = Vec::new();
    for &s in &states {
        let others: Vec<u32> = states.iter().copied().filter(|&x| x != s).collect();
        let mut without = HashSet::new();
        closure(&g, &others, &mut without);
        let m = base.len() - without.len();
        if m == 0 {
            zero += 1;
        }
        marginals.push(m);
    }
    marginals.sort_unstable();
    println!();
    println!(
        "part 0: removing one group changes |closure| by 0 for {zero}/{} groups; \
         max {} ; median {}",
        states.len(),
        marginals.last().expect("non-empty"),
        marginals[marginals.len() / 2]
    );
}
