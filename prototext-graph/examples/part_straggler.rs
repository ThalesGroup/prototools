// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway. The one question the spread grid left: a part costs 3x the
//! mean and no statistic of it is unusual — so is the cost *one group*?
//! Times the most expensive part's groups one at a time. If a handful
//! dominate, the straggler is a property of those groups and can be
//! placed; if the 695 costs are flat, it is a property of the set and
//! cannot.

use prototext_graph::score::{load as score_load, partition_roots, score_subset, ScoringOpts};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let g = score_load::load_graph(std::path::Path::new(&args[1])).expect("load graph");
    let pb = std::fs::read(&args[2]).expect("read blob");
    let opts = ScoringOpts::default();

    let parts = partition_roots(&g, 24);
    let mut times: Vec<f64> = Vec::new();
    for part in &parts {
        let t = Instant::now();
        let r = score_subset(&pb, &g, &opts, part, None);
        std::hint::black_box(r.len());
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let worst = (0..parts.len())
        .max_by(|&a, &b| times[a].partial_cmp(&times[b]).expect("no NaN"))
        .expect("a part");
    println!(
        "part {worst} is the straggler: {:.1} ms against a {:.1} ms mean",
        times[worst],
        times.iter().sum::<f64>() / times.len() as f64
    );

    // The part's roots, grouped by state.
    let part = &parts[worst];
    let mut by_state: Vec<(u32, u32)> = part
        .iter()
        .map(|&r| (g.roots[r as usize].state_id.to_native(), r))
        .collect();
    by_state.sort_unstable();
    let mut groups: Vec<Vec<u32>> = Vec::new();
    let mut i = 0;
    while i < by_state.len() {
        let state = by_state[i].0;
        let mut group = Vec::new();
        while i < by_state.len() && by_state[i].0 == state {
            group.push(by_state[i].1);
            i += 1;
        }
        groups.push(group);
    }
    println!("{} groups in it", groups.len());

    let mut cost: Vec<(f64, usize)> = Vec::with_capacity(groups.len());
    for (gi, group) in groups.iter().enumerate() {
        let t = Instant::now();
        let r = score_subset(&pb, &g, &opts, group, None);
        std::hint::black_box(r.len());
        cost.push((t.elapsed().as_secs_f64() * 1000.0, gi));
    }
    let alone: f64 = cost.iter().map(|c| c.0).sum();
    cost.sort_by(|a, b| b.0.partial_cmp(&a.0).expect("no NaN"));
    println!(
        "summed alone: {alone:.1} ms  ({:.1}x the part, because closures overlap)",
        alone / times[worst]
    );
    println!("top 12 groups, alone:");
    for &(ms, gi) in cost.iter().take(12) {
        println!(
            "  {ms:8.2} ms  {:4} roots  e.g. {}",
            groups[gi].len(),
            g.roots[groups[gi][0] as usize].fqdn.as_str()
        );
    }
    let median = cost[cost.len() / 2].0;
    println!("median group alone: {median:.2} ms");
    println!(
        "top 1 / top 8 / top 32 share of the summed cost: {:.0}% / {:.0}% / {:.0}%",
        100.0 * cost[0].0 / alone,
        100.0 * cost.iter().take(8).map(|c| c.0).sum::<f64>() / alone,
        100.0 * cost.iter().take(32).map(|c| c.0).sum::<f64>() / alone
    );

    // The decisive test: drop the top 8 groups and re-time the part. If
    // the cost is theirs, what is left is an ordinary part.
    let drop: std::collections::HashSet<usize> = cost.iter().take(8).map(|c| c.1).collect();
    let rest: Vec<u32> = groups
        .iter()
        .enumerate()
        .filter(|(gi, _)| !drop.contains(gi))
        .flat_map(|(_, grp)| grp.iter().copied())
        .collect();
    let t = Instant::now();
    let r = score_subset(&pb, &g, &opts, &rest, None);
    std::hint::black_box(r.len());
    println!(
        "part without its top 8 groups: {:.1} ms (was {:.1})",
        t.elapsed().as_secs_f64() * 1000.0,
        times[worst]
    );
}
