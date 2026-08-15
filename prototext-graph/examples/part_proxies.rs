// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway: does any cheap static proxy rank the parts of the shipped
//! partition by their measured cost? Times each part one at a time,
//! single-threaded, and reports Spearman rank correlation against three
//! candidate proxies.

use prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph;
use prototext_graph::score::{load as score_load, partition_roots, score_subset, ScoringOpts};
use std::collections::HashSet;
use std::time::Instant;

/// Number of distinct states reachable from `roots`, following every
/// transition. This is what a part actually walks.
fn closure_size(graph: &ArchivedCompiledGraph, roots: &[u32]) -> usize {
    let mut seen: HashSet<u32> = HashSet::new();
    let mut stack: Vec<u32> = Vec::new();
    for &r in roots {
        let s = graph.roots[r as usize].state_id.to_native();
        if seen.insert(s) {
            stack.push(s);
        }
    }
    while let Some(s) = stack.pop() {
        let node = &graph.nodes[s as usize];
        let off = node.trans_offset.to_native() as usize;
        let len = node.trans_len.to_native() as usize;
        for t in &graph.transitions[off..off + len] {
            let c = t.child_state_id.to_native();
            if c != u32::MAX && seen.insert(c) {
                stack.push(c);
            }
        }
    }
    seen.len()
}

/// Number of transitions out of every state in the closure — a closer
/// stand-in for "edges examined" than the state count alone.
fn closure_edges(graph: &ArchivedCompiledGraph, roots: &[u32]) -> usize {
    let mut seen: HashSet<u32> = HashSet::new();
    let mut stack: Vec<u32> = Vec::new();
    for &r in roots {
        let s = graph.roots[r as usize].state_id.to_native();
        if seen.insert(s) {
            stack.push(s);
        }
    }
    let mut edges = 0usize;
    while let Some(s) = stack.pop() {
        let node = &graph.nodes[s as usize];
        let off = node.trans_offset.to_native() as usize;
        let len = node.trans_len.to_native() as usize;
        edges += len;
        for t in &graph.transitions[off..off + len] {
            let c = t.child_state_id.to_native();
            if c != u32::MAX && seen.insert(c) {
                stack.push(c);
            }
        }
    }
    edges
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
    let mut num = 0.0;
    let (mut da, mut db) = (0.0, 0.0);
    for i in 0..a.len() {
        num += (ra[i] - ma) * (rb[i] - mb);
        da += (ra[i] - ma).powi(2);
        db += (rb[i] - mb).powi(2);
    }
    num / (da.sqrt() * db.sqrt())
}

/// Makespan of a pull queue of `costs`, handed out in the given order to
/// `w` workers. Exactly what the shared cursor does.
fn makespan(costs: &[f64], order: &[usize], w: usize) -> f64 {
    let mut busy = vec![0.0f64; w];
    for &p in order {
        let m = busy
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).expect("no NaN"))
            .expect("a worker")
            .0;
        busy[m] += costs[p];
    }
    busy.into_iter().fold(0.0, f64::max)
}

/// Longest prefix of `pb` that ends on a top-level record boundary and
/// covers at least `frac` of it. A descriptor set is a repeated LEN field,
/// so the top level is just tag/len/payload triples.
fn prefix_at_boundary(pb: &[u8], frac: f64) -> usize {
    let target = (pb.len() as f64 * frac) as usize;
    let mut i = 0usize;
    while i < pb.len() {
        if i >= target {
            return i;
        }
        // tag
        let mut shift = 0;
        let mut tag = 0u64;
        loop {
            if i >= pb.len() {
                return pb.len();
            }
            let b = pb[i];
            i += 1;
            tag |= ((b & 0x7f) as u64) << shift;
            if b < 0x80 {
                break;
            }
            shift += 7;
        }
        if tag & 7 != 2 {
            return pb.len();
        }
        // length
        let mut shift = 0;
        let mut len = 0u64;
        loop {
            if i >= pb.len() {
                return pb.len();
            }
            let b = pb[i];
            i += 1;
            len |= ((b & 0x7f) as u64) << shift;
            if b < 0x80 {
                break;
            }
            shift += 7;
        }
        i += len as usize;
    }
    pb.len()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let g = score_load::load_graph(std::path::Path::new(&args[1])).expect("load graph");
    let pb = std::fs::read(&args[2]).expect("read blob");
    let opts = ScoringOpts::default();

    let t = Instant::now();
    let parts = partition_roots(&g, 24);
    eprintln!("partition: {} parts in {:?}", parts.len(), t.elapsed());

    let t = Instant::now();
    let states: Vec<f64> = parts.iter().map(|p| closure_size(&g, p) as f64).collect();
    let edges: Vec<f64> = parts.iter().map(|p| closure_edges(&g, p) as f64).collect();
    eprintln!("both closures for all parts in {:?}", t.elapsed());

    let groups: Vec<f64> = parts
        .iter()
        .map(|p| {
            p.iter()
                .map(|&r| g.roots[r as usize].state_id.to_native())
                .collect::<HashSet<_>>()
                .len() as f64
        })
        .collect();
    let rootn: Vec<f64> = parts.iter().map(|p| p.len() as f64).collect();

    // Two interleaved rounds: this box drifts, so only compare inside a run.
    let mut times = vec![0.0f64; parts.len()];
    for round in 0..2 {
        for (i, part) in parts.iter().enumerate() {
            let t = Instant::now();
            let r = score_subset(&pb, &g, &opts, part, None);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(r.len());
            if round == 1 {
                times[i] = times[i].min(ms);
            } else {
                times[i] = ms;
            }
        }
    }

    println!("part  time_ms  states  edges  groups  roots");
    for i in 0..parts.len() {
        println!(
            "{i:4}  {:7.1}  {:6}  {:6}  {:6}  {:5}",
            times[i], states[i] as u64, edges[i] as u64, groups[i] as u64, rootn[i] as u64
        );
    }

    println!();
    println!("spearman(time, states) = {:+.3}", spearman(&times, &states));
    println!("spearman(time, edges)  = {:+.3}", spearman(&times, &edges));
    println!("spearman(time, groups) = {:+.3}", spearman(&times, &groups));
    println!("spearman(time, roots)  = {:+.3}", spearman(&times, &rootn));

    let sum: f64 = times.iter().sum();
    let max = times.iter().fold(0.0f64, |a, &b| a.max(b));
    println!();
    println!("sum={sum:.1} max={max:.1} sum/8={:.1}", sum / 8.0);

    let by = |key: &[f64]| {
        let mut o: Vec<usize> = (0..parts.len()).collect();
        o.sort_by(|&a, &b| key[b].partial_cmp(&key[a]).expect("no NaN"));
        o
    };
    let ident: Vec<usize> = (0..parts.len()).collect();
    println!(
        "makespan W=8, name order (today) = {:.1}",
        makespan(&times, &ident, 8)
    );
    println!(
        "makespan W=8, by states desc     = {:.1}",
        makespan(&times, &by(&states), 8)
    );
    println!(
        "makespan W=8, by edges desc      = {:.1}",
        makespan(&times, &by(&edges), 8)
    );
    println!(
        "makespan W=8, by groups desc     = {:.1}",
        makespan(&times, &by(&groups), 8)
    );
    println!(
        "makespan W=8, ORACLE (true cost) = {:.1}",
        makespan(&times, &by(&times), 8)
    );

    // Is name order unlucky, or is any arbitrary order this bad? A cheap
    // deterministic LCG over 2000 permutations answers it.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut ms: Vec<f64> = Vec::new();
    for _ in 0..2000 {
        let mut o: Vec<usize> = (0..parts.len()).collect();
        for i in (1..o.len()).rev() {
            let j = (rnd() % (i as u64 + 1)) as usize;
            o.swap(i, j);
        }
        ms.push(makespan(&times, &o, 8));
    }
    ms.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    println!(
        "makespan W=8, random order: p10={:.1} p50={:.1} mean={:.1} p90={:.1}",
        ms[200],
        ms[1000],
        ms.iter().sum::<f64>() / ms.len() as f64,
        ms[1800]
    );

    // The candidate that is not a static proxy: rank the parts by timing
    // them against a small prefix of the blob, then run them longest-first.
    println!();
    for frac in [0.01f64, 0.05] {
        let cut = prefix_at_boundary(&pb, frac);
        let sample = &pb[..cut];
        let t = Instant::now();
        let mut probe = vec![0.0f64; parts.len()];
        for (i, part) in parts.iter().enumerate() {
            let t0 = Instant::now();
            let r = score_subset(sample, &g, &opts, part, None);
            std::hint::black_box(r.len());
            probe[i] = t0.elapsed().as_secs_f64() * 1000.0;
        }
        let cost = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "probe {:.0}% ({} B): cost {:.1} ms, spearman(time, probe) = {:+.3}, \
             makespan by probe desc = {:.1}",
            frac * 100.0,
            cut,
            cost,
            spearman(&times, &probe),
            makespan(&times, &by(&probe), 8)
        );
    }
}
