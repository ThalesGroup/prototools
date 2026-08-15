// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway. Spec 0301's graded partition, rebuilt locally so that
//! `SPREAD` is an argument instead of a compile-time constant. Reports,
//! for each spread: the quota shape, the *total* single-threaded cost
//! (does grading add work?) and the pull-queue makespan (does grading fix
//! the tail?). The two questions have to be separated — a spread that
//! schedules beautifully and walks 30% more states is a loss.

use prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph;
use prototext_graph::score::{load as score_load, score_subset, ScoringOpts};
use std::time::Instant;

/// Spec 0301's `quotas`, verbatim but for the spread being a parameter.
fn quotas(k: usize, groups: usize, spread: f64) -> Vec<usize> {
    if k <= 1 {
        return vec![groups];
    }
    let decay = spread.powf(-1.0 / (k - 1) as f64);
    let weights: Vec<f64> = (0..k).map(|i| decay.powi(i as i32)).collect();
    let scale = groups as f64 / weights.iter().sum::<f64>();
    let mut quota: Vec<usize> = weights
        .iter()
        .map(|w| ((w * scale).round() as usize).max(1))
        .collect();
    let mut sum: usize = quota.iter().sum();
    while sum > groups {
        let big = (0..k).max_by_key(|&i| quota[i]).expect("k >= 1");
        quota[big] -= 1;
        sum -= 1;
    }
    quota[0] += groups - sum;
    quota
}

/// Spec 0301's `partition_roots`, likewise.
fn graded(g: &ArchivedCompiledGraph, n: usize, spread: f64) -> Vec<Vec<u32>> {
    let total = g.roots.len();
    let mut by_state: Vec<(u32, u32)> = (0..total as u32)
        .map(|i| (g.roots[i as usize].state_id.to_native(), i))
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
    groups.sort_unstable_by_key(|g| std::cmp::Reverse(g.len()));

    let k = n.min(groups.len());
    let quota = quotas(k, groups.len(), spread);
    let mut unfilled = quota.clone();
    let mut parts: Vec<Vec<u32>> = vec![Vec::new(); k];
    for group in groups {
        let mut pick = 0;
        for i in 1..k {
            if unfilled[i] * quota[pick] > unfilled[pick] * quota[i] {
                pick = i;
            }
        }
        unfilled[pick] -= 1;
        parts[pick].extend(group);
    }
    for part in &mut parts {
        part.sort_unstable();
    }
    parts
}

/// Makespan of a pull queue over `costs` handed out in index order to `w`
/// workers — exactly what the shared cursor does.
fn makespan(costs: &[f64], w: usize) -> f64 {
    let mut busy = vec![0.0f64; w];
    for &c in costs {
        let m = busy
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).expect("no NaN"))
            .expect("a worker")
            .0;
        busy[m] += c;
    }
    busy.into_iter().fold(0.0, f64::max)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let g = score_load::load_graph(std::path::Path::new(&args[1])).expect("load graph");
    let pb = std::fs::read(&args[2]).expect("read blob");
    let opts = ScoringOpts::default();
    let k = 24;

    println!("spread   q0    q23     sum_ms   max_ms   sum/8   makespan   ratio");
    for spread in [1.0f64, 2.0, 3.0, 4.0, 6.0, 8.0, 16.0] {
        let parts = graded(&g, k, spread);
        let groups: Vec<usize> = parts
            .iter()
            .map(|p| {
                p.iter()
                    .map(|&r| g.roots[r as usize].state_id.to_native())
                    .collect::<std::collections::HashSet<_>>()
                    .len()
            })
            .collect();
        let mut times = vec![0.0f64; parts.len()];
        for round in 0..2 {
            for (i, part) in parts.iter().enumerate() {
                let t = Instant::now();
                let r = score_subset(&pb, &g, &opts, part, None);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(r.len());
                times[i] = if round == 0 { ms } else { times[i].min(ms) };
            }
        }
        let sum: f64 = times.iter().sum();
        let max = times.iter().fold(0.0f64, |a, &b| a.max(b));
        let ms = makespan(&times, 8);
        println!(
            "{spread:5.0}  {:5} {:5}   {sum:8.1} {max:8.1} {:7.1}   {ms:8.1}   {:.2}x",
            groups[0],
            groups[k - 1],
            sum / 8.0,
            ms / (sum / 8.0).max(max)
        );
        print!("       costs:");
        for t in &times {
            print!(" {t:.0}");
        }
        println!();
    }
}
