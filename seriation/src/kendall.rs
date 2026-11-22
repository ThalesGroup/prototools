// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Kendall's tau against the true order hints (scores.md).

use crate::state::SortState;

/// The negated number of pairs of snapshots with a true hint that the
/// current order puts opposite to their hints. Ties in hint are no
/// inversion. `O(n log n)`: the hints' ranks, read in the current order,
/// counted with a Fenwick tree.
pub(crate) fn tau(state: &SortState) -> i64 {
    let mut hinted: Vec<(u32, u64)> = state
        .snaps
        .iter()
        .enumerate()
        .filter_map(|(sid, snap)| snap.true_hint.map(|h| (state.pos_of[sid], h)))
        .collect();
    hinted.sort_unstable_by_key(|&(pos, _)| pos);
    inversions(&hinted.iter().map(|&(_, h)| h).collect::<Vec<_>>())
}

/// The negated number of pairs `i < j` with `seq[i] > seq[j]`.
fn inversions(seq: &[u64]) -> i64 {
    if seq.len() < 2 {
        return 0;
    }
    let mut values = seq.to_vec();
    values.sort_unstable();
    values.dedup();
    let mut fenwick = vec![0i64; values.len() + 1];
    let mut count = 0i64;
    for (inserted, &v) in seq.iter().enumerate() {
        let rank = values.partition_point(|&x| x < v);
        // Values inserted so far that are at most v.
        let mut at_most = 0;
        let mut i = rank + 1;
        while i > 0 {
            at_most += fenwick[i];
            i &= i - 1;
        }
        count += inserted as i64 - at_most;
        let mut i = rank + 1;
        while i < fenwick.len() {
            fenwick[i] += 1;
            i += i & i.wrapping_neg();
        }
    }
    -count
}

#[cfg(test)]
mod tests {
    use super::inversions;

    fn brute(seq: &[u64]) -> i64 {
        let mut count = 0;
        for i in 0..seq.len() {
            for j in i + 1..seq.len() {
                count += i64::from(seq[i] > seq[j]);
            }
        }
        -count
    }

    #[test]
    fn inversions_match_a_brute_force_count() {
        let mut rng = crate::tests::Rng(7);
        for len in 0..40 {
            let seq: Vec<u64> = (0..len).map(|_| rng.below(8)).collect();
            assert_eq!(inversions(&seq), brute(&seq), "{seq:?}");
        }
        assert_eq!(inversions(&[3, 2, 1]), -3);
        assert_eq!(inversions(&[1, 1, 1]), 0, "ties are no inversion");
    }
}
