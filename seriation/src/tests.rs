// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Tests of the sort as a whole, and of each score's incremental operations
//! against a full computation (spec 0451, test plan).

use std::collections::HashMap;

use crate::greedy::Search;
use crate::interleaving::Untangle;
use crate::state::{EdgeSearch, SortState};
use crate::*;

/// SplitMix64, for deterministic test data.
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Contents from a map, `(file, version) -> text`.
struct Texts(HashMap<(FileId, VersionId), Vec<u8>>);

impl Contents for Texts {
    fn fetch(&mut self, file: FileId, version: VersionId) -> Result<Vec<u8>, BoxError> {
        Ok(self.0[&(file, version)].clone())
    }
}

/// A history of `files` files over `revisions` revisions, each file growing
/// by a line now and then, and `n` snapshots of it, each holding a random
/// subset of the files at a random revision. Version `r` of a file is its
/// text at revision `r`, deduplicated: the version is the revision at which
/// the file last changed.
fn history(seed: u64, n: usize, files: u32, revisions: u64) -> (Vec<Snapshot>, Texts) {
    let mut rng = Rng(seed);
    // last_change[f][r]: the revision of file f's text at revision r.
    let mut texts = HashMap::new();
    let mut last_change = vec![vec![0u64; revisions as usize]; files as usize];
    for f in 0..files {
        let mut text = String::new();
        let mut changed = 0;
        for r in 0..revisions {
            if r == 0 || rng.below(3) == 0 {
                text.push_str(&format!("line {r} of file {f}\n"));
                changed = r;
                texts.insert((f, r as u32), text.clone().into_bytes());
            }
            last_change[f as usize][r as usize] = changed;
        }
    }
    let snapshots = (0..n)
        .map(|i| {
            let r = rng.below(revisions);
            let items = (0..files)
                .filter(|_| rng.below(4) != 0)
                .map(|f| (f, last_change[f as usize][r as usize] as u32))
                .collect();
            Snapshot {
                identity: format!("snap-{i:03}"),
                // A day per revision, give or take two.
                build_time: (r as i64 + rng.below(5) as i64 - 2) * 86_400,
                order_hint: Some(r),
                items,
            }
        })
        .collect();
    (snapshots, Texts(texts))
}

fn loaded(snapshots: &[Snapshot], texts: &mut Texts) -> SortState {
    let mut state = SortState::load(snapshots, 100).unwrap();
    state.sort_by_build_time();
    state
        .build_edges(
            texts,
            &LinesChars,
            std::time::Duration::from_secs(30 * 86_400),
        )
        .unwrap();
    state
}

/// Random moves, each probed then made: the probe's delta equals the
/// change of the full score, and `begin` then `end` leave the state as it
/// was.
fn check_incremental(s: &mut dyn Search, rng: &mut Rng) {
    let n = s.n() as u64;
    for _ in 0..200 {
        let from = rng.below(n) as u32;
        let to = rng.below(n) as u32;
        if from == to {
            continue;
        }
        let (before, rank) = (s.score(), s.rank());
        s.begin(from, rank[from as usize]);
        let delta = s.probe(from, to);
        s.end(from);
        assert_eq!(s.score(), before, "begin then end changed the score");
        assert_eq!(s.rank(), rank, "begin then end changed the ranks");
        s.commit(from, to);
        assert_eq!(s.score(), before + delta, "move {from} -> {to}");
    }
}

#[test]
fn the_edge_score_probe_matches_a_full_computation() {
    let (snapshots, mut texts) = history(1, 40, 6, 30);
    let mut state = loaded(&snapshots, &mut texts);
    check_incremental(&mut EdgeSearch::new(&mut state), &mut Rng(2));
}

#[test]
fn the_interleaving_probe_matches_a_full_computation() {
    let (snapshots, mut texts) = history(3, 40, 6, 30);
    let state = loaded(&snapshots, &mut texts);
    check_incremental(&mut Untangle::new(&state), &mut Rng(4));
}

#[test]
fn the_edge_ranks_count_each_edge_at_both_ends() {
    let (snapshots, mut texts) = history(5, 20, 4, 15);
    let mut state = loaded(&snapshots, &mut texts);
    let edges = state.edge_count();
    let search = EdgeSearch::new(&mut state);
    // Every edge adds +1 at each end if it disagrees, −1 if it agrees.
    let disagreeing = -search.score();
    let agreeing = edges as i64 - disagreeing;
    let ranks: i64 = search.rank().iter().sum();
    assert_eq!(ranks, 2 * (disagreeing - agreeing));
    assert!(edges > 0);
}

#[test]
fn a_growing_history_comes_back_in_order() {
    let (snapshots, mut texts) = history(6, 60, 8, 40);
    let sorted = sort(&snapshots, &mut texts, &LinesChars, &Settings::default()).unwrap();
    let after_build_time = sorted.phases[1].tau;
    let last = sorted.phases.last().unwrap();
    assert!(last.tau >= after_build_time, "{:?}", sorted.phases);
    assert!(last.tau > after_build_time, "the content should help");
    assert!(sorted.phases[3].comparisons > 0);
}

#[test]
fn shuffling_the_input_changes_nothing() {
    let (snapshots, mut texts) = history(8, 50, 6, 30);
    let settings = Settings::default();
    let sorted = sort(&snapshots, &mut texts, &LinesChars, &settings).unwrap();
    let identities = |s: &[Snapshot], order: &[usize]| -> Vec<String> {
        order.iter().map(|&i| s[i].identity.clone()).collect()
    };
    let mut shuffled = snapshots.clone();
    let mut rng = Rng(9);
    for i in (1..shuffled.len()).rev() {
        shuffled.swap(i, rng.below(i as u64 + 1) as usize);
    }
    let again = sort(&shuffled, &mut texts, &LinesChars, &settings).unwrap();
    assert_eq!(
        identities(&snapshots, &sorted.order),
        identities(&shuffled, &again.order)
    );
    // The same reports, a move's snapshots named by identity: their
    // indices are into the input, which the shuffle changes.
    type Named = Vec<(PhaseReport, Vec<[Option<String>; 3]>)>;
    let named = |s: &[Snapshot], phases: &[PhaseReport]| -> Named {
        let id = |i: Option<usize>| i.map(|i| s[i].identity.clone());
        phases
            .iter()
            .map(|p| {
                let ids = p
                    .moved
                    .iter()
                    .map(|m| [id(Some(m.snapshot)), id(m.after), id(m.before)])
                    .collect();
                let mut p = p.clone();
                for m in &mut p.moved {
                    (m.snapshot, m.after, m.before) = (0, None, None);
                }
                (p, ids)
            })
            .collect()
    };
    assert_eq!(
        named(&snapshots, &sorted.phases),
        named(&shuffled, &again.phases)
    );
}

#[test]
fn order_hints_put_the_hinted_snapshots_in_hint_order() {
    let (mut snapshots, mut texts) = history(10, 40, 6, 30);
    // Build times that say nothing.
    for s in &mut snapshots {
        s.build_time = 0;
    }
    let settings = Settings {
        order_hints: true,
        ..Settings::default()
    };
    let sorted = sort(&snapshots, &mut texts, &LinesChars, &settings).unwrap();
    assert_eq!(sorted.phases[2].tau, 0, "phase 2 places every hint");
    assert_eq!(sorted.phases.last().unwrap().tau, 0, "barriers hold");
    let hints: Vec<u64> = sorted
        .order
        .iter()
        .map(|&i| snapshots[i].order_hint.unwrap())
        .collect();
    assert!(hints.windows(2).all(|w| w[0] <= w[1]), "{hints:?}");
}

#[test]
fn hidden_hints_still_count_in_tau() {
    let (snapshots, mut texts) = history(11, 40, 6, 30);
    let settings = Settings {
        reveal_percent: 0,
        order_hints: true,
        ..Settings::default()
    };
    let sorted = sort(&snapshots, &mut texts, &LinesChars, &settings).unwrap();
    assert_eq!(sorted.phases[2].moves, 0, "no hint is shown");
    assert!(sorted.phases[1].tau < 0, "but tau still sees them");
}

#[test]
fn skipped_phases_are_reported() {
    let (snapshots, mut texts) = history(12, 10, 3, 10);
    let settings = Settings {
        skip: Skip {
            build_time: true,
            containment: true,
            interleaving: true,
        },
        ..Settings::default()
    };
    let sorted = sort(&snapshots, &mut texts, &LinesChars, &settings).unwrap();
    let skipped: Vec<bool> = sorted.phases.iter().map(|p| p.skipped).collect();
    assert_eq!(skipped, [false, true, true, true, true]);
    assert_eq!(sorted.order, (0..10).collect::<Vec<_>>(), "identity order");
}

#[test]
fn bad_inputs_are_refused() {
    let snap = |id: &str, items: Vec<(FileId, VersionId)>| Snapshot {
        identity: id.into(),
        build_time: 0,
        order_hint: None,
        items,
    };
    let mut none = Texts(HashMap::new());
    let refused = |snapshots: &[Snapshot], settings: &Settings, none: &mut Texts| {
        sort(snapshots, none, &LinesChars, settings).unwrap_err()
    };
    let ok = Settings::default();
    assert!(matches!(
        refused(&[snap("a", vec![]), snap("a", vec![])], &ok, &mut none),
        Error::DuplicateIdentity(id) if id == "a"
    ));
    assert!(matches!(
        refused(&[snap("a", vec![(1, 0), (1, 1)])], &ok, &mut none),
        Error::DuplicateFile { file: 1, .. }
    ));
    for bad in [
        Settings {
            nom_tries: 0,
            ..Settings::default()
        },
        Settings {
            nom_tries: 3,
            max_tries: 2,
            ..Settings::default()
        },
        Settings {
            reveal_percent: 101,
            ..Settings::default()
        },
    ] {
        assert!(matches!(refused(&[], &bad, &mut none), Error::Settings(_)));
    }
    let many: Vec<Snapshot> = (0..=u16::MAX as usize)
        .map(|i| snap(&format!("{i:05}"), vec![(0, 0)]))
        .collect();
    assert!(matches!(refused(&many, &ok, &mut none), Error::Capacity(_)));
}
