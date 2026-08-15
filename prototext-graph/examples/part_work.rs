// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway. Two questions, in order:
//!
//! 1. Is there a *deterministic* measure of a part's work that tracks its
//!    measured time? Candidate: the sum of the score counters the walk
//!    already returns — every increment is one token examined against one
//!    candidate, so it needs no instrumentation at all.
//! 2. If so, does that measure over a *sample* of the blob predict it over
//!    the whole blob — and how big must the sample be?

use prototext_graph::score::{load as score_load, partition_roots, score_subset, ScoringOpts};
use std::time::Instant;

/// Every counter the walk increments, summed over a part's entries. Each
/// increment corresponds to one token weighed against one candidate, which
/// is exactly the walk's inner loop.
fn work_of(scores: &[prototext_graph::score::EntryScore]) -> u64 {
    scores
        .iter()
        .map(|s| s.matches + s.unknowns + s.out_of_range + s.non_canonical + s.mismatches)
        .sum()
}

/// Byte ranges of the top-level records. A descriptor set is a repeated
/// LEN field, so the top level is tag/len/payload triples.
fn records(pb: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < pb.len() {
        let start = i;
        let mut shift = 0;
        let mut tag = 0u64;
        loop {
            if i >= pb.len() {
                return out;
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
            return out;
        }
        let mut shift = 0;
        let mut len = 0u64;
        loop {
            if i >= pb.len() {
                return out;
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
        if i > pb.len() {
            return out;
        }
        out.push((start, i));
    }
    out
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

/// Kendall tau-a. Used to ask whether two sample sizes induce the *same
/// order*, which is the only thing a scheduler consumes.
fn kendall(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len();
    let (mut con, mut dis) = (0i64, 0i64);
    for i in 0..n {
        for j in (i + 1)..n {
            let s = ((a[i] - a[j]) * (b[i] - b[j])).signum();
            if s > 0.0 {
                con += 1;
            } else if s < 0.0 {
                dis += 1;
            }
        }
    }
    (con - dis) as f64 / (n * (n - 1) / 2) as f64
}

/// Makespan of a pull queue over `costs` handed out in `order` to `w`
/// workers — exactly what the shared cursor does.
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

fn desc(key: &[f64]) -> Vec<usize> {
    let mut o: Vec<usize> = (0..key.len()).collect();
    o.sort_by(|&a, &b| key[b].partial_cmp(&key[a]).expect("no NaN"));
    o
}

/// The reverted spec 0300 partition, rebuilt locally: state groups ordered
/// by their smallest FQDN, cut into `n` contiguous blocks. Kept here rather
/// than in the library because it is under test, not shipped — and because
/// its *high cost variance* is what makes it the right subject for
/// validating an estimator. Round-robin's parts are near-uniform, so a
/// correlation measured on them would be deflated by restriction of range.
fn fqdn_partition(
    g: &prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph,
    n: usize,
) -> Vec<Vec<u32>> {
    let total = g.roots.len();
    let mut by_state: Vec<(u32, u32)> = (0..total as u32)
        .map(|i| (g.roots[i as usize].state_id.to_native(), i))
        .collect();
    by_state.sort_unstable();
    let mut groups: Vec<(&str, Vec<u32>)> = Vec::new();
    let mut i = 0;
    while i < by_state.len() {
        let state = by_state[i].0;
        let mut grp = Vec::new();
        let mut name = g.roots[by_state[i].1 as usize].fqdn.as_str();
        while i < by_state.len() && by_state[i].0 == state {
            let f = g.roots[by_state[i].1 as usize].fqdn.as_str();
            if f < name {
                name = f;
            }
            grp.push(by_state[i].1);
            i += 1;
        }
        groups.push((name, grp));
    }
    groups.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let k = n.min(groups.len());
    let (base, extra) = (groups.len() / k, groups.len() % k);
    let mut out = Vec::with_capacity(k);
    let mut rest = groups.into_iter();
    for p in 0..k {
        let take = base + usize::from(p < extra);
        let mut part = Vec::new();
        for (_, grp) in rest.by_ref().take(take) {
            part.extend(grp);
        }
        part.sort_unstable();
        out.push(part);
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let g = score_load::load_graph(std::path::Path::new(&args[1])).expect("load graph");
    let pb = std::fs::read(&args[2]).expect("read blob");
    let opts = ScoringOpts::default();
    let which = args.get(3).map(String::as_str).unwrap_or("fqdn");
    let parts = if which == "rr" {
        partition_roots(&g, 24)
    } else {
        fqdn_partition(&g, 24)
    };
    let recs = records(&pb);
    eprintln!(
        "{which} partition: {} parts, {} top-level records",
        parts.len(),
        recs.len()
    );

    // Ground truth: deterministic work, and measured time over two rounds.
    let mut work = vec![0.0f64; parts.len()];
    let mut times = vec![0.0f64; parts.len()];
    for round in 0..2 {
        for (i, part) in parts.iter().enumerate() {
            let t = Instant::now();
            let r = score_subset(&pb, &g, &opts, part, None);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            work[i] = work_of(&r) as f64;
            times[i] = if round == 0 { ms } else { times[i].min(ms) };
        }
    }

    println!("part   time_ms        work   work/ms");
    for i in 0..parts.len() {
        println!(
            "{i:4}  {:8.1}  {:10}  {:8.0}",
            times[i],
            work[i] as u64,
            work[i] / times[i]
        );
    }
    println!();
    println!(
        "Q1  spearman(time, work) = {:+.3}   kendall = {:+.3}",
        spearman(&times, &work),
        kendall(&times, &work)
    );

    // A non-saturated scheduling metric: how close to the lower bound does
    // an order get? Ratio 1.00 means "as good as the bound allows".
    let sum: f64 = times.iter().sum();
    let max = times.iter().fold(0.0f64, |a, &b| a.max(b));
    let bound = (sum / 8.0).max(max);
    let ident: Vec<usize> = (0..parts.len()).collect();
    println!();
    println!(
        "lower bound = {bound:.1} ms  (sum/8 = {:.1}, max = {max:.1})",
        sum / 8.0
    );
    println!(
        "  name order   {:7.1} ms  = {:.2}x bound",
        makespan(&times, &ident, 8),
        makespan(&times, &ident, 8) / bound
    );
    println!(
        "  by work desc {:7.1} ms  = {:.2}x bound",
        makespan(&times, &desc(&work), 8),
        makespan(&times, &desc(&work), 8) / bound
    );
    println!(
        "  by time desc {:7.1} ms  = {:.2}x bound   (oracle)",
        makespan(&times, &desc(&times), 8),
        makespan(&times, &desc(&times), 8) / bound
    );

    // Q2: does the work counter over a sample predict it over the whole?
    // Prefix vs uniform subsample, because this partition is sorted by
    // package name and the blob is clustered by package — a prefix is not
    // a sample.
    println!();
    println!("frac        prefix                    subsample");
    println!("        rho    tau  makespan      rho    tau  makespan");
    let mut prev_prefix: Option<Vec<f64>> = None;
    let mut prev_sub: Option<Vec<f64>> = None;
    for frac in [0.005f64, 0.01, 0.02, 0.05, 0.10, 0.20] {
        // Prefix: the first records covering `frac` of the bytes.
        let want = (pb.len() as f64 * frac) as usize;
        let mut pre = Vec::new();
        for &(s, e) in &recs {
            if pre.len() >= want {
                break;
            }
            pre.extend_from_slice(&pb[s..e]);
        }
        // Subsample: every record whose deterministic hash falls under
        // `frac`, so the selection is spread over the whole blob.
        let mut sub = Vec::new();
        for (n, &(s, e)) in recs.iter().enumerate() {
            let mut h = n as u64 ^ 0x9E37_79B9_7F4A_7C15;
            h ^= h >> 30;
            h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h ^= h >> 27;
            if ((h % 1_000_000) as f64 / 1_000_000.0) < frac {
                sub.extend_from_slice(&pb[s..e]);
            }
        }

        let est = |blob: &[u8]| -> Vec<f64> {
            parts
                .iter()
                .map(|p| work_of(&score_subset(blob, &g, &opts, p, None)) as f64)
                .collect()
        };
        let (ep, es) = (est(&pre), est(&sub));
        let mp = makespan(&times, &desc(&ep), 8) / bound;
        let msu = makespan(&times, &desc(&es), 8) / bound;
        let stab_p = prev_prefix
            .as_ref()
            .map(|v| kendall(v, &ep))
            .unwrap_or(f64::NAN);
        let stab_s = prev_sub
            .as_ref()
            .map(|v| kendall(v, &es))
            .unwrap_or(f64::NAN);
        println!(
            "{:5.1}%  {:+.2}  {:+.2}    {:.2}x    {:+.2}  {:+.2}    {:.2}x   [stability vs prev: pre {:+.2} sub {:+.2}]",
            frac * 100.0,
            spearman(&work, &ep),
            kendall(&work, &ep),
            mp,
            spearman(&work, &es),
            kendall(&work, &es),
            msu,
            stab_p,
            stab_s
        );
        prev_prefix = Some(ep);
        prev_sub = Some(es);
    }
}
