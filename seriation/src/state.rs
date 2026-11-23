// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The sort's state, loading (phase 0), the build-time sort (phase 1), and
//! the edge graph of phase 3 with its search.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::time::Duration;

use crate::greedy::Search;
use crate::{
    kendall, Comparator, Containment, Contents, Error, FileId, Phase, PhaseReport, Snapshot,
    VersionId,
};

/// The most instances of one file: the interleaving score indexes a file's
/// subsequence with `u16` slots.
pub(crate) const MAX_INSTANCES: usize = u16::MAX as usize;

pub(crate) struct Snap {
    /// Index into the caller's slice.
    pub(crate) index: usize,
    pub(crate) build_time: i64,
    /// The hint, revealed or not: what tau is scored against.
    pub(crate) true_hint: Option<u64>,
    /// The hint the sort is shown (spec 0451 S4).
    pub(crate) hint: Option<u64>,
}

pub(crate) struct File {
    /// One instance per snapshot holding the file, in load order (`sid`).
    pub(crate) instances: Vec<Instance>,
}

#[derive(Clone, Copy)]
pub(crate) struct Instance {
    pub(crate) sid: u32,
    pub(crate) version: VersionId,
}

pub(crate) struct SortState {
    pub(crate) snaps: Vec<Snap>,
    /// The caller's `FileId` of each file, by internal index.
    file_ids: Vec<FileId>,
    pub(crate) files: Vec<File>,
    /// `sid_at[pos]`: the snapshot at a position.
    pub(crate) sid_at: Vec<u32>,
    /// `pos_of[sid]`: the position of a snapshot.
    pub(crate) pos_of: Vec<u32>,
    /// Phase 3's graph, once built: one edge per judgment, `(older, newer)`
    /// by `sid`, weight 1 (scores.md).
    edges: Option<Vec<(u32, u32)>>,
}

impl SortState {
    /// Phase 0: snapshots in identity order, files and instances indexed.
    pub(crate) fn load(snapshots: &[Snapshot], reveal_percent: u8) -> Result<Self, Error> {
        if snapshots.len() > u32::MAX as usize {
            return Err(Error::Capacity(format!(
                "{} snapshots: at most {} can be sorted",
                snapshots.len(),
                u32::MAX
            )));
        }
        let mut by_identity: Vec<usize> = (0..snapshots.len()).collect();
        by_identity.sort_by(|&a, &b| {
            snapshots[a]
                .identity
                .as_bytes()
                .cmp(snapshots[b].identity.as_bytes())
        });
        for pair in by_identity.windows(2) {
            if snapshots[pair[0]].identity == snapshots[pair[1]].identity {
                return Err(Error::DuplicateIdentity(
                    snapshots[pair[0]].identity.clone(),
                ));
            }
        }

        let mut snaps = Vec::with_capacity(snapshots.len());
        let mut file_index: HashMap<FileId, u32> = HashMap::new();
        let mut file_ids = Vec::new();
        let mut files: Vec<File> = Vec::new();
        for (sid, &index) in by_identity.iter().enumerate() {
            let snapshot = &snapshots[index];
            let mut seen = std::collections::HashSet::new();
            for &(file, version) in &snapshot.items {
                if !seen.insert(file) {
                    return Err(Error::DuplicateFile {
                        identity: snapshot.identity.clone(),
                        file,
                    });
                }
                let fid = *file_index.entry(file).or_insert_with(|| {
                    file_ids.push(file);
                    files.push(File {
                        instances: Vec::new(),
                    });
                    (files.len() - 1) as u32
                });
                let instances = &mut files[fid as usize].instances;
                if instances.len() == MAX_INSTANCES {
                    return Err(Error::Capacity(format!(
                        "file {file} is in more than {MAX_INSTANCES} snapshots"
                    )));
                }
                instances.push(Instance {
                    sid: sid as u32,
                    version,
                });
            }
            snaps.push(Snap {
                index,
                build_time: snapshot.build_time,
                true_hint: snapshot.order_hint,
                hint: snapshot
                    .order_hint
                    .filter(|_| revealed(&snapshot.identity, reveal_percent)),
            });
        }
        let n = snaps.len() as u32;
        Ok(SortState {
            snaps,
            file_ids,
            files,
            sid_at: (0..n).collect(),
            pos_of: (0..n).collect(),
            edges: None,
        })
    }

    pub(crate) fn n(&self) -> usize {
        self.snaps.len()
    }

    /// The caller's indices, in the current order.
    pub(crate) fn order(&self) -> Vec<usize> {
        self.sid_at
            .iter()
            .map(|&sid| self.snaps[sid as usize].index)
            .collect()
    }

    /// Take `sid_at` as the order, and recompute `pos_of`.
    pub(crate) fn adopt_order(&mut self, sid_at: &[u32]) {
        self.sid_at.copy_from_slice(sid_at);
        for (pos, &sid) in self.sid_at.iter().enumerate() {
            self.pos_of[sid as usize] = pos as u32;
        }
    }

    /// Move the snapshot at `from` to `to`, shifting those between by one.
    pub(crate) fn move_snapshot(&mut self, from: u32, to: u32) {
        let (from, to) = (from as usize, to as usize);
        if from < to {
            self.sid_at[from..=to].rotate_left(1);
        } else if from > to {
            self.sid_at[to..=from].rotate_right(1);
        } else {
            return;
        }
        for pos in from.min(to)..=from.max(to) {
            self.pos_of[self.sid_at[pos] as usize] = pos as u32;
        }
    }

    /// Phase 1: a stable sort by build time. Returns the positions that
    /// changed.
    pub(crate) fn sort_by_build_time(&mut self) -> usize {
        let snaps = &self.snaps;
        self.sid_at
            .sort_by_key(|&sid| snaps[sid as usize].build_time);
        let order = self.sid_at.clone();
        self.adopt_order(&order);
        self.sid_at
            .iter()
            .enumerate()
            .filter(|&(pos, &sid)| pos as u32 != sid)
            .count()
    }

    /// Phase 3's graph (design.md): for each file, each pair of its
    /// instances within `window`, an edge each way for one version, else
    /// the comparator's verdict. Returns the pairs of different versions
    /// compared, and how many the comparator ordered.
    pub(crate) fn build_edges(
        &mut self,
        contents: &mut dyn Contents,
        comparator: &dyn Comparator,
        window: Duration,
    ) -> Result<(u64, u64), Error> {
        let window = window.as_secs();
        let mut edges = Vec::new();
        let (mut comparisons, mut signals) = (0u64, 0u64);
        for (fid, file) in self.files.iter().enumerate() {
            let instances = &file.instances;
            if instances.len() < 2 {
                continue;
            }
            // Each version of this file fetched when a comparison first
            // needs it, once, dropped before the next file: a version no
            // pair within the window names is never read (spec 0470 S3).
            let mut bytes: HashMap<VersionId, Vec<u8>> = HashMap::new();
            for (i, u) in instances.iter().enumerate() {
                for v in &instances[i + 1..] {
                    let (tu, tv) = (
                        self.snaps[u.sid as usize].build_time,
                        self.snaps[v.sid as usize].build_time,
                    );
                    if tu.abs_diff(tv) > window {
                        continue;
                    }
                    if u.version == v.version {
                        edges.push((u.sid, v.sid));
                        edges.push((v.sid, u.sid));
                        continue;
                    }
                    for version in [u.version, v.version] {
                        if let Entry::Vacant(slot) = bytes.entry(version) {
                            slot.insert(
                                contents
                                    .fetch(self.file_ids[fid], version)
                                    .map_err(Error::Contents)?,
                            );
                        }
                    }
                    comparisons += 1;
                    match comparator.compare(&bytes[&u.version], &bytes[&v.version]) {
                        Containment::FirstInSecond => {
                            edges.push((u.sid, v.sid));
                            signals += 1;
                        }
                        Containment::SecondInFirst => {
                            edges.push((v.sid, u.sid));
                            signals += 1;
                        }
                        Containment::Equal | Containment::Incomparable => {}
                    }
                }
            }
        }
        self.edges = Some(edges);
        Ok((comparisons, signals))
    }

    /// The edge score: the negated number of edges whose older end is not
    /// before their newer end. `None` before phase 3 builds the graph.
    pub(crate) fn edge_score(&self) -> Option<i64> {
        let edges = self.edges.as_ref()?;
        Some(
            -(edges
                .iter()
                .filter(|&&(u, v)| self.pos_of[u as usize] > self.pos_of[v as usize])
                .count() as i64),
        )
    }

    #[cfg(test)]
    pub(crate) fn edge_count(&self) -> usize {
        self.edges.as_ref().map_or(0, Vec::len)
    }

    pub(crate) fn report(&self, phase: Phase, skipped: bool, moves: usize) -> PhaseReport {
        PhaseReport {
            phase,
            skipped,
            moves,
            comparisons: 0,
            signals: 0,
            edge_score: self.edge_score(),
            interleaving_score: None,
            tau: kendall::tau(self),
            moved: Vec::new(),
        }
    }
}

/// Whether the self-test shows `identity`'s order hint to the sort (spec
/// 0451 S4): a hash of the identity, below the percentage. The hash is
/// multiply-then-add with FNV-1a's offset basis and prime, exactly as the
/// existing implementation computes it, so that both hide the same hints.
fn revealed(identity: &str, percent: u8) -> bool {
    match percent {
        100.. => true,
        0 => false,
        _ => {
            let h = identity.bytes().fold(0xcbf2_9ce4_8422_2325u64, |acc, b| {
                acc.wrapping_mul(0x0000_0100_0000_01b3)
                    .wrapping_add(u64::from(b))
            });
            h % 100 < u64::from(percent)
        }
    }
}

/// Phase 3's search over the edge score (greedy-search.md).
pub(crate) struct EdgeSearch<'a> {
    state: &'a mut SortState,
    /// `incident[sid]`: `(neighbor, out)` for each edge touching `sid`,
    /// `out` when `sid` is the edge's older end.
    incident: Vec<Vec<(u32, bool)>>,
}

impl<'a> EdgeSearch<'a> {
    pub(crate) fn new(state: &'a mut SortState) -> Self {
        let mut incident = vec![Vec::new(); state.n()];
        for &(u, v) in state.edges.as_deref().unwrap_or(&[]) {
            incident[u as usize].push((v, true));
            incident[v as usize].push((u, false));
        }
        EdgeSearch { state, incident }
    }
}

impl Search for EdgeSearch<'_> {
    fn n(&self) -> usize {
        self.state.n()
    }

    fn hint_at(&self, pos: u32) -> Option<u64> {
        self.state.snaps[self.state.sid_at[pos as usize] as usize].hint
    }

    fn score(&self) -> i64 {
        self.state.edge_score().unwrap_or(0)
    }

    /// Each edge counts at both its ends: +1 if it disagrees with the
    /// order, −1 if it agrees.
    fn rank(&self) -> Vec<i64> {
        let mut deltas = vec![0i64; self.state.n()];
        for &(u, v) in self.state.edges.as_deref().unwrap_or(&[]) {
            let (pu, pv) = (self.state.pos_of[u as usize], self.state.pos_of[v as usize]);
            let d = if pu < pv { -1 } else { 1 };
            deltas[pu as usize] += d;
            deltas[pv as usize] += d;
        }
        deltas
    }

    fn begin(&mut self, _from: u32, _rank: i64) {}

    fn probe(&self, from: u32, to: u32) -> i64 {
        if from == to {
            return 0;
        }
        let sid = self.state.sid_at[from as usize];
        let (fp, tp) = (from as usize, to as usize);
        let right = fp < tp;
        let mut delta = 0i64;
        for &(neighbor, out) in &self.incident[sid as usize] {
            let pv = self.state.pos_of[neighbor as usize] as usize;
            // A neighbor between the two positions shifts by one towards
            // `from`; the ±1 on the bound accounts for it.
            let (before, after) = if out {
                (fp < pv, tp < pv + usize::from(!right))
            } else {
                (pv < fp, pv < tp + usize::from(right))
            };
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    fn end(&mut self, _from: u32) {}

    fn commit(&mut self, from: u32, to: u32) {
        self.state.move_snapshot(from, to);
    }
}
