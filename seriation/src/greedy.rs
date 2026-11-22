// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The greedy search phases 3 and 4 share (greedy-search.md).

use crate::Settings;

/// What the search needs from a score. Higher is better; every delta is a
/// change of the score.
pub(crate) trait Search {
    fn n(&self) -> usize;
    /// The order hint the sort is shown, of the snapshot at `pos`.
    fn hint_at(&self, pos: u32) -> Option<u64>;
    /// The score, computed from scratch.
    fn score(&self) -> i64;
    /// Per position, how promising moving its snapshot is: higher first.
    fn rank(&self) -> Vec<i64>;
    /// About to probe moves of the snapshot at `from`, whose rank is `rank`.
    fn begin(&mut self, from: u32, rank: i64);
    /// The change of the score if the snapshot at `from` moved to `to`.
    fn probe(&self, from: u32, to: u32) -> i64;
    /// Done probing `from`: undo whatever `begin` changed.
    fn end(&mut self, from: u32);
    /// Make the move.
    fn commit(&mut self, from: u32, to: u32);
}

/// Move snapshots one at a time while a move improves the score. Returns
/// the number of moves.
///
/// Which move is made decides the order, so every rule here is the
/// existing implementation's: candidates by rank, highest first, ties by
/// position; destinations leftwards from `from − 1`, then rightwards from
/// `from + 1`, each direction stopping at a barrier; the first improving
/// move kept unless a later one is strictly better; and the stopping rule
/// below.
pub(crate) fn search(s: &mut dyn Search, settings: &Settings) -> usize {
    let n = s.n() as u32;
    let mut score = s.score();
    let mut moves = 0;
    loop {
        debug_assert_eq!(score, s.score(), "the incremental score drifted");
        let rank = s.rank();
        let mut candidates: Vec<u32> = (0..n).collect();
        candidates.sort_by(|&a, &b| rank[b as usize].cmp(&rank[a as usize]).then(a.cmp(&b)));

        let mut best: Option<(u32, u32, i64)> = None;
        let mut found_at: Option<usize> = None;
        for (tried, &from) in candidates.iter().enumerate() {
            if tried >= settings.max_tries {
                break;
            }
            if tried >= settings.nom_tries && best.is_some() {
                break;
            }
            let hint = s.hint_at(from);
            s.begin(from, rank[from as usize]);
            let sides: [&mut dyn Iterator<Item = u32>; 2] =
                [&mut (0..from).rev(), &mut (from + 1..n)];
            for side in sides {
                for to in side {
                    if settings.order_hints && hint.is_some() {
                        let at = s.hint_at(to);
                        if at.is_some() && at != hint {
                            break;
                        }
                    }
                    let candidate = score + s.probe(from, to);
                    if candidate > score && best.is_none_or(|(_, _, b)| candidate > b) {
                        if best.is_none() {
                            found_at = Some(tried);
                        }
                        best = Some((from, to, candidate));
                    }
                }
            }
            s.end(from);
            if found_at == Some(tried) && tried + 1 >= settings.nom_tries {
                break;
            }
        }

        match best {
            None => return moves,
            Some((from, to, new_score)) => {
                s.commit(from, to);
                score = new_score;
                moves += 1;
            }
        }
    }
}
