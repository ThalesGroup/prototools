// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The interleaving score of phase 4 and its search (scores.md).
//!
//! Each file with at least two distinct versions keeps its subsequence (its
//! instances in the current order), the span of each version over it, and
//! a layer decomposition of the spans, all updated move by move.

use std::collections::HashMap;

use crate::greedy::Search;
use crate::state::SortState;

/// A slot whose instance is removed, between a probe's `begin` and `end`.
const ABSENT: u32 = u32::MAX;

/// One file's subsequence.
#[derive(Clone)]
struct FileSeq {
    /// The version code at each slot, or `ABSENT`.
    version_at: Vec<u32>,
    /// The snapshot at each slot.
    sid_at_slot: Vec<u32>,
    /// `(lo, hi)` per version code, in slot space. Real spans only
    /// (`lo < hi`), but for a point span `fetch_undo` restores until the
    /// next `rebuild_layers`.
    span_of: HashMap<u32, (u16, u16)>,
    /// Real spans whose open interiors don't overlap, per layer, each layer
    /// by `lo`.
    layers: Vec<Vec<u32>>,
}

impl FileSeq {
    fn new(version_at: Vec<u32>, sid_at_slot: Vec<u32>) -> Self {
        let mut span_of: HashMap<u32, (u16, u16)> = HashMap::new();
        for (j, &code) in version_at.iter().enumerate() {
            let span = span_of.entry(code).or_insert((j as u16, j as u16));
            span.1 = j as u16;
        }
        span_of.retain(|_, &mut (lo, hi)| lo < hi);
        let mut seq = FileSeq {
            version_at,
            sid_at_slot,
            span_of,
            layers: Vec::new(),
        };
        seq.rebuild_layers();
        seq
    }

    /// Greedy interval coloring by `lo`. No two spans share a `lo` (a slot
    /// holds one version), so the result does not depend on the map's order.
    fn rebuild_layers(&mut self) {
        let mut real: Vec<(u16, u16, u32)> = self
            .span_of
            .iter()
            .map(|(&code, &(lo, hi))| (lo, hi, code))
            .collect();
        real.sort_unstable_by_key(|&(lo, _, _)| lo);
        self.layers.clear();
        let mut layer_hi: Vec<u16> = Vec::new();
        for (lo, hi, code) in real {
            match layer_hi.iter().position(|&h| h <= lo) {
                Some(l) => {
                    self.layers[l].push(code);
                    layer_hi[l] = hi;
                }
                None => {
                    self.layers.push(vec![code]);
                    layer_hi.push(hi);
                }
            }
        }
    }

    /// The straddle count, from scratch.
    fn straddles(&self) -> i64 {
        let mut count = 0;
        for (j, &bj) in self.version_at.iter().enumerate() {
            if bj == ABSENT {
                continue;
            }
            for (&code, &(lo, hi)) in &self.span_of {
                if code != bj && (lo as usize) < j && j < hi as usize {
                    count += 1;
                }
            }
        }
        count
    }

    /// The change of the straddle count if slot `jx`'s instance were
    /// removed.
    fn fetch_dryrun(&self, jx: usize) -> i64 {
        let bj = self.version_at[jx];
        if bj == ABSENT {
            return 0;
        }
        let mut delta = 0;
        // The spans straddling jx lose it.
        for layer in &self.layers {
            let idx = layer.partition_point(|code| (self.span_of[code].0 as usize) < jx);
            if idx > 0 {
                let code = layer[idx - 1];
                if self.span_of[&code].1 as usize > jx && code != bj {
                    delta -= 1;
                }
            }
        }
        // bj's own span shrinks if jx is one of its ends.
        let Some(&(lo, hi)) = self.span_of.get(&bj) else {
            return delta;
        };
        let (lo, hi) = (lo as usize, hi as usize);
        if lo == hi {
            return delta;
        }
        if jx == lo {
            let mut k = lo + 1;
            while self.version_at[k] != bj {
                if self.version_at[k] != ABSENT {
                    delta -= 1;
                }
                k += 1;
            }
        } else if jx == hi {
            let mut k = hi - 1;
            loop {
                let v = self.version_at[k];
                if v == bj {
                    break;
                }
                if v != ABSENT {
                    delta -= 1;
                }
                if k == lo {
                    break;
                }
                k -= 1;
            }
        }
        delta
    }

    /// The change of the straddle count if version `bj` were inserted at
    /// `ins_pos`. A span ending at `ins_pos` straddles it: the move is a
    /// rotation, which shifts that end by one.
    fn insert_dryrun(&self, ins_pos: usize, bj: u32) -> i64 {
        if bj == ABSENT {
            return 0;
        }
        let mut delta = 0;
        for layer in &self.layers {
            let idx = layer.partition_point(|code| (self.span_of[code].0 as usize) < ins_pos);
            if idx > 0 {
                let code = layer[idx - 1];
                if self.span_of[&code].1 as usize >= ins_pos && code != bj {
                    delta += 1;
                }
            }
        }
        if let Some(&(lo, hi)) = self.span_of.get(&bj) {
            let (lo, hi) = (lo as usize, hi as usize);
            let widened = if ins_pos < lo {
                ins_pos..lo
            } else if ins_pos > hi + 1 {
                hi + 1..ins_pos
            } else {
                0..0
            };
            for k in widened {
                let v = self.version_at[k];
                if v != ABSENT && v != bj {
                    delta += 1;
                }
            }
        }
        delta
    }

    /// Remove slot `jx`'s instance; returns its version code.
    fn fetch(&mut self, jx: usize) -> u32 {
        let bj = self.version_at[jx];
        if bj == ABSENT {
            return ABSENT;
        }
        self.version_at[jx] = ABSENT;
        if let Some(&(lo, hi)) = self.span_of.get(&bj) {
            let (lo, hi) = (lo as usize, hi as usize);
            if lo == hi {
                self.span_of.remove(&bj);
                for layer in &mut self.layers {
                    if let Some(p) = layer.iter().position(|&c| c == bj) {
                        layer.remove(p);
                        break;
                    }
                }
            } else if jx == lo {
                let mut new_lo = lo + 1;
                while self.version_at[new_lo] != bj {
                    new_lo += 1;
                }
                self.span_of.get_mut(&bj).unwrap().0 = new_lo as u16;
            } else if jx == hi {
                let mut new_hi = hi - 1;
                while self.version_at[new_hi] != bj {
                    new_hi -= 1;
                }
                self.span_of.get_mut(&bj).unwrap().1 = new_hi as u16;
            }
        }
        bj
    }

    /// Put `bj` back at slot `jx`.
    fn fetch_undo(&mut self, jx: usize, bj: u32) {
        if bj == ABSENT {
            return;
        }
        self.version_at[jx] = bj;
        match self.span_of.get_mut(&bj) {
            Some(span) => {
                if (jx as u16) < span.0 {
                    span.0 = jx as u16;
                } else if (jx as u16) > span.1 {
                    span.1 = jx as u16;
                }
            }
            None => {
                self.span_of.insert(bj, (jx as u16, jx as u16));
            }
        }
    }

    /// Rotate the removed slot `src` to `dst`, spans first.
    fn apply_move(&mut self, src: usize, dst: usize) {
        if src < dst {
            self.update_spans_forward(src, dst);
            self.version_at[src..=dst].rotate_left(1);
            self.sid_at_slot[src..=dst].rotate_left(1);
        } else {
            self.update_spans_backward(src, dst);
            self.version_at[dst..=src].rotate_right(1);
            self.sid_at_slot[dst..=src].rotate_right(1);
        }
        self.rebuild_layers();
    }

    fn update_spans_forward(&mut self, src: usize, dst: usize) {
        let codes: Vec<u32> = self.span_of.keys().copied().collect();
        for code in codes {
            let (lo, hi) = self.span_of[&code];
            let (lo, hi) = (lo as usize, hi as usize);
            let (mut new_lo, mut new_hi) = (lo, hi);
            if (src..=dst).contains(&lo) {
                if lo == src {
                    let mut k = src;
                    while k < dst && self.version_at[k + 1] != code {
                        k += 1;
                    }
                    new_lo = k;
                } else {
                    new_lo = lo - 1;
                }
            }
            if (src..=dst).contains(&hi) {
                new_hi = if self.version_at[src] == code {
                    dst
                } else {
                    hi - 1
                };
            }
            self.span_of.insert(code, (new_lo as u16, new_hi as u16));
        }
    }

    fn update_spans_backward(&mut self, src: usize, dst: usize) {
        let codes: Vec<u32> = self.span_of.keys().copied().collect();
        for code in codes {
            let (lo, hi) = self.span_of[&code];
            let (lo, hi) = (lo as usize, hi as usize);
            let (mut new_lo, mut new_hi) = (lo, hi);
            if (dst..=src).contains(&hi) {
                if hi == src {
                    let mut k = src;
                    while k > dst && self.version_at[k - 1] != code {
                        k -= 1;
                    }
                    new_hi = k;
                } else {
                    new_hi = hi + 1;
                }
            }
            if (dst..=src).contains(&lo) {
                new_lo = if self.version_at[src] == code {
                    dst
                } else {
                    lo + 1
                };
            }
            self.span_of.insert(code, (new_lo as u16, new_hi as u16));
        }
    }
}

/// Phase 4's state and search.
pub(crate) struct Untangle {
    sid_at: Vec<u32>,
    pos_of: Vec<u32>,
    hints: Vec<Option<u64>>,
    /// `snapshot_files[sid]`: the retained files the snapshot holds.
    snapshot_files: Vec<Vec<u32>>,
    files: Vec<FileSeq>,
    /// Between `begin` and `end`: the removed snapshot's version codes, in
    /// `snapshot_files` order, and its rank.
    saved: Vec<u32>,
    removed_sid: u32,
    removal: i64,
}

impl Untangle {
    pub(crate) fn new(state: &SortState) -> Self {
        let n = state.n();
        let mut files = Vec::new();
        let mut snapshot_files = vec![Vec::new(); n];
        for file in &state.files {
            if file.instances.len() < 2 {
                continue;
            }
            // Codes by first occurrence in load order.
            let mut codes: HashMap<u32, u32> = HashMap::new();
            for instance in &file.instances {
                let next = codes.len() as u32;
                codes.entry(instance.version).or_insert(next);
            }
            if codes.len() < 2 {
                continue; // one version: never straddled
            }
            let code_of: HashMap<u32, u32> = file
                .instances
                .iter()
                .map(|i| (i.sid, codes[&i.version]))
                .collect();
            let (mut version_at, mut sid_at_slot) = (Vec::new(), Vec::new());
            for &sid in &state.sid_at {
                if let Some(&code) = code_of.get(&sid) {
                    version_at.push(code);
                    sid_at_slot.push(sid);
                }
            }
            let fid = files.len() as u32;
            for instance in &file.instances {
                snapshot_files[instance.sid as usize].push(fid);
            }
            files.push(FileSeq::new(version_at, sid_at_slot));
        }
        Untangle {
            sid_at: state.sid_at.clone(),
            pos_of: state.pos_of.clone(),
            hints: state.snaps.iter().map(|s| s.hint).collect(),
            snapshot_files,
            files,
            saved: Vec::new(),
            removed_sid: u32::MAX,
            removal: 0,
        }
    }

    pub(crate) fn order(&self) -> &[u32] {
        &self.sid_at
    }

    /// The snapshot's slot in file `fid`.
    fn slot_of(&self, fid: u32, sid: u32) -> usize {
        self.files[fid as usize]
            .sid_at_slot
            .iter()
            .position(|&s| s == sid)
            .expect("a snapshot is in the files it holds")
    }

    /// Where a move from `from` to `to` inserts into file `fid`: moving
    /// right, after the slots at or before `to`; left, before those at `to`.
    fn insertion_of(&self, fid: u32, from: u32, to: u32) -> usize {
        let slots = &self.files[fid as usize].sid_at_slot;
        if to > from {
            slots
                .iter()
                .filter(|&&s| self.pos_of[s as usize] <= to)
                .count()
        } else {
            slots
                .iter()
                .filter(|&&s| self.pos_of[s as usize] < to)
                .count()
        }
    }

    fn remove(&mut self, pos: u32) {
        let sid = self.sid_at[pos as usize];
        self.removed_sid = sid;
        self.saved.clear();
        for i in 0..self.snapshot_files[sid as usize].len() {
            let fid = self.snapshot_files[sid as usize][i];
            let jx = self.slot_of(fid, sid);
            let code = self.files[fid as usize].fetch(jx);
            self.saved.push(code);
        }
    }

    fn undo(&mut self, pos: u32) {
        let sid = self.sid_at[pos as usize];
        debug_assert_eq!(sid, self.removed_sid);
        for i in 0..self.snapshot_files[sid as usize].len() {
            let fid = self.snapshot_files[sid as usize][i];
            let jx = self.slot_of(fid, sid);
            self.files[fid as usize].fetch_undo(jx, self.saved[i]);
        }
    }
}

impl Search for Untangle {
    fn n(&self) -> usize {
        self.sid_at.len()
    }

    fn hint_at(&self, pos: u32) -> Option<u64> {
        self.hints[self.sid_at[pos as usize] as usize]
    }

    /// The negated straddle count.
    fn score(&self) -> i64 {
        -self.files.iter().map(FileSeq::straddles).sum::<i64>()
    }

    /// Per position, the score change of removing its snapshot.
    fn rank(&self) -> Vec<i64> {
        (0..self.sid_at.len())
            .map(|pos| {
                let sid = self.sid_at[pos];
                -self.snapshot_files[sid as usize]
                    .iter()
                    .map(|&fid| self.files[fid as usize].fetch_dryrun(self.slot_of(fid, sid)))
                    .sum::<i64>()
            })
            .collect()
    }

    fn begin(&mut self, from: u32, rank: i64) {
        self.remove(from);
        self.removal = rank;
    }

    fn probe(&self, from: u32, to: u32) -> i64 {
        if from == to {
            return self.removal;
        }
        let sid = self.removed_sid;
        let mut delta = self.removal;
        for (i, &fid) in self.snapshot_files[sid as usize].iter().enumerate() {
            let ins = self.insertion_of(fid, from, to);
            delta -= self.files[fid as usize].insert_dryrun(ins, self.saved[i]);
        }
        delta
    }

    fn end(&mut self, from: u32) {
        self.undo(from);
    }

    fn commit(&mut self, from: u32, to: u32) {
        self.remove(from);
        let sid = self.removed_sid;
        let fids = self.snapshot_files[sid as usize].clone();
        let work: Vec<(usize, usize)> = fids
            .iter()
            .map(|&fid| {
                let src = self.slot_of(fid, sid);
                let ins = self.insertion_of(fid, from, to);
                (src, if to > from { ins - 1 } else { ins })
            })
            .collect();
        let (fp, tp) = (from as usize, to as usize);
        if fp < tp {
            self.sid_at[fp..=tp].rotate_left(1);
        } else {
            self.sid_at[tp..=fp].rotate_right(1);
        }
        for (pos, &s) in self.sid_at.iter().enumerate() {
            self.pos_of[s as usize] = pos as u32;
        }
        for (i, &fid) in fids.iter().enumerate() {
            let (src, dst) = work[i];
            let seq = &mut self.files[fid as usize];
            if src != dst {
                seq.apply_move(src, dst);
            }
            seq.fetch_undo(dst, self.saved[i]);
            seq.rebuild_layers();
        }
    }
}
