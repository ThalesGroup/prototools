// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway. No build-time signal ranks groups (five families measured,
//! all dead), so the last candidate is a runtime probe: score every group
//! against a small sample of the blob and rank by what that costs.
//!
//! The part-level version of this failed — it schedules at 1.38x the
//! bound despite a +0.99 rank correlation — because it averaged 695
//! groups into one number. At group level the quantity is bimodal (median
//! 0.00 ms, 43 groups of 16 680 over 50 ms) and "expensive" means
//! "survives deep", which is exactly what a sample can see.
//!
//! Prefix *and* uniform subsample, because a descriptor set is clustered
//! by package and a prefix of it is not a sample.

use prototext_graph::score::{load as score_load, score_subset, EntryScore, ScoringOpts};
use std::time::Instant;

/// Every counter the walk increments, summed. One increment is one token
/// weighed against one candidate, which is the walk's inner loop.
fn work_of(scores: &[EntryScore]) -> u64 {
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
        let varint = |i: &mut usize| -> Option<u64> {
            let (mut shift, mut v) = (0u32, 0u64);
            loop {
                if *i >= pb.len() {
                    return None;
                }
                let b = pb[*i];
                *i += 1;
                v |= ((b & 0x7f) as u64) << shift;
                if b < 0x80 {
                    return Some(v);
                }
                shift += 7;
            }
        };
        let Some(tag) = varint(&mut i) else {
            return out;
        };
        if tag & 7 != 2 {
            return out;
        }
        let Some(len) = varint(&mut i) else {
            return out;
        };
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

fn desc(key: &[f64]) -> Vec<usize> {
    let mut o: Vec<usize> = (0..key.len()).collect();
    o.sort_by(|&a, &b| key[b].partial_cmp(&key[a]).expect("no NaN"));
    o
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
    let mut groups: Vec<Vec<u32>> = Vec::new();
    let mut i = 0;
    while i < by_state.len() {
        let state = by_state[i].0;
        let mut grp = Vec::new();
        while i < by_state.len() && by_state[i].0 == state {
            grp.push(by_state[i].1);
            i += 1;
        }
        groups.push(grp);
    }
    let recs = records(&pb);
    eprintln!("{} groups, {} top-level records", groups.len(), recs.len());

    // Ground truth.
    let mut truth_ms = Vec::with_capacity(groups.len());
    for grp in &groups {
        let t0 = Instant::now();
        let r = score_subset(&pb, &g, &opts, grp, None);
        std::hint::black_box(r.len());
        truth_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let truth = desc(&truth_ms);
    let hot: Vec<usize> = truth
        .iter()
        .copied()
        .filter(|&i| truth_ms[i] >= 50.0)
        .collect();
    let hot_cost: f64 = hot.iter().map(|&i| truth_ms[i]).sum();
    let all_cost: f64 = truth_ms.iter().sum();
    eprintln!(
        "{} groups over 50 ms, carrying {:.0} ms of {:.0} ms",
        hot.len(),
        hot_cost,
        all_cost
    );

    println!("mode      frac   probe_ms  alive   rho    champ   top43   top128   top512");
    for frac in [0.002f64, 0.005, 0.01, 0.02, 0.05] {
        for mode in ["prefix", "sample"] {
            let mut blob = Vec::new();
            if mode == "prefix" {
                let want = (pb.len() as f64 * frac) as usize;
                for &(s, e) in &recs {
                    if blob.len() >= want {
                        break;
                    }
                    blob.extend_from_slice(&pb[s..e]);
                }
            } else {
                for (n, &(s, e)) in recs.iter().enumerate() {
                    let mut h = n as u64 ^ 0x9E37_79B9_7F4A_7C15;
                    h ^= h >> 30;
                    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                    h ^= h >> 27;
                    if ((h % 1_000_000) as f64 / 1_000_000.0) < frac {
                        blob.extend_from_slice(&pb[s..e]);
                    }
                }
            }

            let t = Instant::now();
            let mut est = vec![0.0f64; groups.len()];
            let mut alive = 0usize;
            for (gi, grp) in groups.iter().enumerate() {
                let r = score_subset(&blob, &g, &opts, grp, None);
                est[gi] = work_of(&r) as f64;
                if r.iter().any(|e| !e.vetoed) {
                    alive += 1;
                }
            }
            let probe_ms = t.elapsed().as_secs_f64() * 1000.0;

            let order = desc(&est);
            let champ = order
                .iter()
                .position(|&i| i == truth[0])
                .expect("the champion");
            let cover = |n: usize| -> f64 {
                let got: f64 = order.iter().take(n).map(|&i| truth_ms[i]).sum();
                100.0 * got / hot_cost
            };
            println!(
                "{mode:8} {:5.1}%  {probe_ms:8.0}  {alive:5}  {:+.3}  {champ:6}  {:5.1}%  {:6.1}%  {:6.1}%",
                frac * 100.0,
                spearman(&truth_ms, &est),
                cover(43),
                cover(128),
                cover(512)
            );
        }
    }
}
