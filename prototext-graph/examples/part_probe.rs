// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway. `group_probe.rs` showed that scoring every group against
//! 0.2% of the blob costs 46 ms and ranks the groups at rho +0.82, with
//! the champion third — the first signal in six families that is not
//! noise. This spends that estimate on the partition and asks the only
//! question that matters: does the pull queue finish sooner?
//!
//! Three arms, all at 24 parts:
//!
//! - `roundrobin` — today's partition, groups dealt in largest-first
//!   order (spec 0290, and spec 0301 at `SPREAD = 1`).
//! - `lpt` — longest-processing-time-first on the estimate: each group
//!   goes to the part whose estimated total is least. No constant.
//! - `quota s` — spec 0301's geometric grading, but the quota is a number
//!   of estimated work units rather than a number of groups, and the
//!   groups are dealt most-expensive-first so the champion lands in part
//!   0 by construction.
//!
//! Both estimating arms emit parts in descending estimated cost, which is
//! the scheduling change: the shared cursor hands out in index order.

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

/// Byte ranges of the top-level records.
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

/// Where `which` sits when the groups are ranked by estimate, descending.
fn by_est_rank(est: &[f64], which: usize) -> usize {
    est.iter().filter(|&&e| e > est[which]).count()
}

/// Geometric quotas summing to `total`, first : last = `spread`.
fn quotas(k: usize, total: f64, spread: f64) -> Vec<f64> {
    let decay = spread.powf(-1.0 / (k - 1) as f64);
    let weights: Vec<f64> = (0..k).map(|i| decay.powi(i as i32)).collect();
    let scale = total / weights.iter().sum::<f64>();
    weights.iter().map(|w| w * scale).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let g = score_load::load_graph(std::path::Path::new(&args[1])).expect("load graph");
    let pb = std::fs::read(&args[2]).expect("read blob");
    let opts = ScoringOpts::default();
    let k = 24;

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

    // The probe: the first records covering 0.2% of the blob.
    let recs = records(&pb);
    let want = (pb.len() as f64 * 0.002) as usize;
    let mut sample = Vec::new();
    for &(s, e) in &recs {
        if sample.len() >= want {
            break;
        }
        sample.extend_from_slice(&pb[s..e]);
    }
    let t = Instant::now();
    let est: Vec<f64> = groups
        .iter()
        .map(|grp| work_of(&score_subset(&sample, &g, &opts, grp, None)) as f64)
        .collect();
    let probe_ms = t.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "probe: {} bytes of {}, {} groups, {probe_ms:.0} ms",
        sample.len(),
        pb.len(),
        groups.len()
    );

    // The true straggler, by name — knowing it costs no ground-truth run.
    let champ = groups
        .iter()
        .position(|grp| {
            g.roots[grp[0] as usize].fqdn.as_str() == "google.protobuf.FileDescriptorSet"
        })
        .expect("the champion");
    let champ_rank = by_est_rank(&est, champ);
    eprintln!(
        "champion is group {champ}, probe ranks it {champ_rank} of {}",
        groups.len()
    );
    let champ_root = groups[champ][0];

    // Arms. Each returns the parts in the order they would be emitted,
    // paired with the estimated cost the arm believed each part carried.
    let mut arms: Vec<(String, Vec<Vec<u32>>, Vec<f64>)> = Vec::new();

    {
        let mut by_size: Vec<usize> = (0..groups.len()).collect();
        by_size.sort_by_key(|&i| std::cmp::Reverse(groups[i].len()));
        let mut parts: Vec<Vec<u32>> = vec![Vec::new(); k];
        let mut load = vec![0.0f64; k];
        for (n, &gi) in by_size.iter().enumerate() {
            parts[n % k].extend(&groups[gi]);
            load[n % k] += est[gi];
        }
        arms.push(("roundrobin".to_string(), parts, load));
    }

    let mut by_est: Vec<usize> = (0..groups.len()).collect();
    by_est.sort_by(|&a, &b| est[b].partial_cmp(&est[a]).expect("no NaN"));

    // Today's partition, with one change: the `w` groups the probe likes
    // most are seeded onto distinct parts 0..w before anything else is
    // dealt, so each of them starts in the first wave. The estimate is
    // used only as an order — never as a quantity, which is the thing it
    // is bad at.
    for w in [8usize, 24] {
        let seeded: Vec<usize> = by_est.iter().take(w.min(k)).copied().collect();
        let mut parts: Vec<Vec<u32>> = vec![Vec::new(); k];
        let mut load = vec![0.0f64; k];
        for (i, &gi) in seeded.iter().enumerate() {
            parts[i].extend(&groups[gi]);
            load[i] += est[gi];
        }
        let mut rest: Vec<usize> = (0..groups.len()).filter(|i| !seeded.contains(i)).collect();
        rest.sort_by_key(|&i| std::cmp::Reverse(groups[i].len()));
        for (n, &gi) in rest.iter().enumerate() {
            let p = (n + seeded.len()) % k;
            parts[p].extend(&groups[gi]);
            load[p] += est[gi];
        }
        arms.push((format!("seed {w}"), parts, load));
    }

    {
        let mut load = vec![0.0f64; k];
        let mut parts: Vec<Vec<u32>> = vec![Vec::new(); k];
        for &gi in &by_est {
            let m = (0..k)
                .min_by(|&a, &b| load[a].partial_cmp(&load[b]).expect("no NaN"))
                .expect("a part");
            load[m] += est[gi];
            parts[m].extend(&groups[gi]);
        }
        let mut order: Vec<usize> = (0..k).collect();
        order.sort_by(|&a, &b| load[b].partial_cmp(&load[a]).expect("no NaN"));
        arms.push((
            "lpt".to_string(),
            order.iter().map(|&i| parts[i].clone()).collect(),
            order.iter().map(|&i| load[i]).collect(),
        ));
    }

    for spread in [2.0f64, 4.0, 8.0] {
        let quota = quotas(k, est.iter().sum::<f64>(), spread);
        let mut unfilled = quota.clone();
        let mut parts: Vec<Vec<u32>> = vec![Vec::new(); k];
        for &gi in &by_est {
            // The part whose quota is least filled, as a fraction.
            let mut pick = 0;
            for i in 1..k {
                if unfilled[i] / quota[i] > unfilled[pick] / quota[pick] {
                    pick = i;
                }
            }
            unfilled[pick] -= est[gi];
            parts[pick].extend(&groups[gi]);
        }
        let load: Vec<f64> = (0..k).map(|i| quota[i] - unfilled[i]).collect();
        arms.push((format!("quota {spread:.0}"), parts, load));
    }

    println!();
    println!("arm            sum_ms   max_ms   sum/8   makespan   ratio  +probe  champ_at");
    for (name, parts, load) in &arms {
        let at = parts
            .iter()
            .position(|p| p.contains(&champ_root))
            .expect("the champion is somewhere");
        let est_total: f64 = load.iter().sum();
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
        let span = makespan(&times, 8);
        println!(
            "{name:12} {sum:8.1} {max:8.1} {:7.1}   {span:8.1}   {:.2}x {:7.1}  {at:8}",
            sum / 8.0,
            span / (sum / 8.0).max(max),
            span + probe_ms
        );
        print!("      costs:");
        for t in &times {
            print!(" {t:.0}");
        }
        println!();
        // What the arm *believed*, on the same scale, so that a part whose
        // estimate is low but whose cost is high shows up as a mismatch.
        print!("      believed:");
        for l in load {
            print!(" {:.0}", l / est_total * sum);
        }
        println!();
    }
}
