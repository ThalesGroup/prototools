// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Phase 2: the snapshots with an order hint placed in hint order
//! (design.md).

use std::collections::BTreeMap;

use crate::state::SortState;

/// Place the revealed snapshots in non-decreasing hint order. Returns the
/// number of moves.
pub(crate) fn place(state: &mut SortState) -> usize {
    let n = state.n() as u32;
    let mut groups: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    for pos in 0..n as usize {
        let sid = state.sid_at[pos];
        if let Some(hint) = state.snaps[sid as usize].hint {
            groups.entry(hint).or_default().push(sid);
        }
    }
    if groups.is_empty() {
        return 0;
    }
    let mut moves = 0;

    // Pack each group around its centroid, rounded down.
    for sids in groups.values() {
        if sids.len() < 2 {
            continue;
        }
        let len = sids.len() as u32;
        let sum: u64 = sids
            .iter()
            .map(|&s| u64::from(state.pos_of[s as usize]))
            .sum();
        let centroid = (sum / u64::from(len)) as u32;
        let block_start = centroid.saturating_sub((len - 1) / 2).min(n - len);
        let ordered = by_position(state, sids);
        moves += move_group(state, &ordered, block_start);
    }

    // The groups in their order after packing; the longest non-decreasing
    // run of their hints is settled.
    let mut seq: Vec<(u32, u64, Vec<u32>)> = groups
        .into_iter()
        .map(|(hint, sids)| {
            let ordered = by_position(state, &sids);
            (state.pos_of[ordered[0] as usize], hint, ordered)
        })
        .collect();
    seq.sort_by_key(|&(leftmost, _, _)| leftmost);
    let hints: Vec<u64> = seq.iter().map(|&(_, h, _)| h).collect();
    let mut settled = vec![false; seq.len()];
    for i in longest_nondecreasing(&hints) {
        settled[i] = true;
    }

    // The others, in hint order, between their settled neighbors.
    let mut others: Vec<usize> = (0..seq.len()).filter(|&i| !settled[i]).collect();
    others.sort_by_key(|&i| seq[i].1);
    for gi in others {
        let hint = seq[gi].1;
        let left = (0..seq.len())
            .filter(|&i| settled[i] && seq[i].1 < hint)
            .max_by_key(|&i| seq[i].1);
        let right = (0..seq.len())
            .filter(|&i| settled[i] && seq[i].1 > hint)
            .min_by_key(|&i| seq[i].1);
        let positions = |i: usize| {
            seq[i]
                .2
                .iter()
                .map(|&s| i64::from(state.pos_of[s as usize]))
        };
        let left_pos = left.map_or(-1, |i| positions(i).max().unwrap());
        let right_pos = right.map_or(i64::from(n), |i| positions(i).min().unwrap());

        let target_center = match (left, right) {
            // No settled group above: right after the one below.
            (_, None) => left_pos + 1,
            (None, Some(r)) => (right_pos as f64 * hint as f64 / seq[r].1 as f64).round() as i64,
            (Some(l), Some(r)) => {
                let (left_hint, right_hint) = (seq[l].1, seq[r].1);
                let fraction = (hint as i64 - left_hint as i64) as f64
                    / (right_hint as i64 - left_hint as i64) as f64;
                (left_pos as f64 + (right_pos - left_pos) as f64 * fraction).round() as i64
            }
        };
        let len = seq[gi].2.len() as i64;
        let (lo, hi) = (left_pos + 1, right_pos - len);
        let raw = target_center - (len - 1) / 2;
        let leftmost = positions(gi).min().unwrap();
        let block_start = if raw > leftmost {
            raw.max(lo).min(hi.max(0))
        } else {
            raw.max(lo).min(hi.max(lo))
        } as u32;
        let ordered = by_position(state, &seq[gi].2);
        moves += move_group(state, &ordered, block_start);
        settled[gi] = true;
    }
    moves
}

fn by_position(state: &SortState, sids: &[u32]) -> Vec<u32> {
    let mut ordered = sids.to_vec();
    ordered.sort_by_key(|&s| state.pos_of[s as usize]);
    ordered
}

/// Move `ordered` (by position) to consecutive positions from
/// `block_start`, in their order. Returns the moves made.
fn move_group(state: &mut SortState, ordered: &[u32], block_start: u32) -> usize {
    let last = state.n() as u32 - 1;
    let forward = block_start > state.pos_of[ordered[0] as usize];
    let mut moves = 0;
    let mut place = |i: usize| {
        let target = (block_start + i as u32).min(last);
        let from = state.pos_of[ordered[i] as usize];
        if from != target {
            state.move_snapshot(from, target);
            moves += 1;
        }
    };
    if forward {
        (0..ordered.len()).rev().for_each(&mut place);
    } else {
        (0..ordered.len()).for_each(&mut place);
    }
    moves
}

/// The indices of a longest non-decreasing subsequence of `seq`.
fn longest_nondecreasing(seq: &[u64]) -> Vec<usize> {
    if seq.is_empty() {
        return Vec::new();
    }
    let mut tails: Vec<u64> = Vec::new();
    let mut tail_idx: Vec<usize> = Vec::new();
    let mut predecessor: Vec<Option<usize>> = vec![None; seq.len()];
    for (i, &v) in seq.iter().enumerate() {
        let p = tails.partition_point(|&t| t <= v);
        if p == tails.len() {
            tails.push(v);
            tail_idx.push(i);
        } else {
            tails[p] = v;
            tail_idx[p] = i;
        }
        predecessor[i] = p.checked_sub(1).map(|q| tail_idx[q]);
    }
    let mut result = Vec::with_capacity(tails.len());
    let mut cur = tail_idx.last().copied();
    while let Some(i) = cur {
        result.push(i);
        cur = predecessor[i];
    }
    result.reverse();
    result
}

#[cfg(test)]
mod tests {
    use super::longest_nondecreasing;

    #[test]
    fn longest_nondecreasing_keeps_ties() {
        assert_eq!(longest_nondecreasing(&[]), Vec::<usize>::new());
        assert_eq!(longest_nondecreasing(&[3, 1, 2, 2, 5, 4]), vec![1, 2, 3, 5]);
        assert_eq!(longest_nondecreasing(&[5, 4, 3]).len(), 1);
    }
}
