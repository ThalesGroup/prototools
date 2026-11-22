// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The committed corpora (spec 0451 S5, S6): each `tests/corpora/NAME.args`
//! names the `seriation-gen` parameters of a corpus, regenerated here in
//! memory and sorted. Where `NAME.expected.txt` holds the order the
//! existing implementation gives on it, one identity per line, the sort
//! must give the same.

use std::collections::HashMap;
use std::path::Path;

use seriation::generate::{generate, Corpus, Params};
use seriation::{sort, BoxError, Contents, FileId, LinesChars, Settings, Snapshot, VersionId};

struct Texts(HashMap<(FileId, VersionId), Vec<u8>>);

impl Contents for Texts {
    fn fetch(&mut self, file: FileId, version: VersionId) -> Result<Vec<u8>, BoxError> {
        Ok(self.0[&(file, version)].clone())
    }
}

/// The corpus as the sort's input: a `FileId` per path, a `VersionId` per
/// distinct content of a file.
fn input(corpus: &Corpus) -> (Vec<Snapshot>, Texts) {
    let mut file_of: HashMap<&str, FileId> = HashMap::new();
    let mut version_of: HashMap<(FileId, &str), VersionId> = HashMap::new();
    let mut texts = HashMap::new();
    let snapshots = corpus
        .snapshots
        .iter()
        .map(|s| {
            let items = s
                .files
                .iter()
                .map(|(path, text)| {
                    let next = file_of.len() as FileId;
                    let file = *file_of.entry(path).or_insert(next);
                    let next = version_of.len() as VersionId;
                    let version = *version_of.entry((file, text)).or_insert(next);
                    texts.insert((file, version), text.as_bytes().to_vec());
                    (file, version)
                })
                .collect();
            Snapshot {
                identity: s.identity.clone(),
                build_time: s.build_time,
                order_hint: s.order_hint,
                items,
            }
        })
        .collect();
    (snapshots, Texts(texts))
}

/// Pairs the order puts opposite to the true revisions.
fn inversions(corpus: &Corpus, order: &[usize]) -> usize {
    let revisions: Vec<u32> = order
        .iter()
        .map(|&i| corpus.snapshots[i].revision)
        .collect();
    let mut count = 0;
    for i in 0..revisions.len() {
        for j in i + 1..revisions.len() {
            count += usize::from(revisions[i] > revisions[j]);
        }
    }
    count
}

#[test]
fn the_corpora_sort_closer_to_the_truth_than_their_build_times() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpora");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(".args").map(str::to_string)
        })
        .collect();
    names.sort();
    assert!(names.len() >= 3, "{names:?}");
    let mut moves_seen = 0;
    for name in names {
        let args = std::fs::read_to_string(dir.join(format!("{name}.args"))).unwrap();
        let params = Params::from_args(args.lines().map(str::to_string)).unwrap();
        let corpus = generate(&params);
        let (snapshots, mut texts) = input(&corpus);
        let sorted = sort(&snapshots, &mut texts, &LinesChars, &Settings::default()).unwrap();

        let mut by_build_time: Vec<usize> = (0..snapshots.len()).collect();
        by_build_time.sort_by_key(|&i| (snapshots[i].build_time, snapshots[i].identity.clone()));
        let (before, after) = (
            inversions(&corpus, &by_build_time),
            inversions(&corpus, &sorted.order),
        );
        eprintln!(
            "{name}: {} snapshots, inversions {before} by build time, {after} sorted; moves {:?}",
            snapshots.len(),
            sorted.phases.iter().map(|p| p.moves).collect::<Vec<_>>()
        );
        assert!(after <= before, "{name}: {after} > {before}");

        // The moves reported, replayed from the order phase 3 starts from
        // (phase 1's, with no order hints), give the order: each names the
        // snapshot at its `from`.
        let mut replayed = by_build_time.clone();
        for phase in &sorted.phases[3..] {
            assert_eq!(phase.moved.len(), phase.moves, "{name}: {:?}", phase.phase);
            for m in &phase.moved {
                assert_eq!(replayed[m.from], m.snapshot, "{name}: {m:?}");
                assert!(m.gain > 0, "{name}: {m:?}");
                let moved = replayed.remove(m.from);
                replayed.insert(m.to, moved);
                assert_eq!(m.after, m.to.checked_sub(1).map(|i| replayed[i]));
                assert_eq!(m.before, replayed.get(m.to + 1).copied());
            }
        }
        assert_eq!(replayed, sorted.order, "{name}: the moves replayed");
        moves_seen += sorted.phases[3].moved.len();

        let expected = dir.join(format!("{name}.expected.txt"));
        if let Ok(text) = std::fs::read_to_string(&expected) {
            let got: Vec<&str> = sorted
                .order
                .iter()
                .map(|&i| snapshots[i].identity.as_str())
                .collect();
            let want: Vec<&str> = text.lines().collect();
            assert_eq!(got, want, "{name}: the existing implementation's order");
        }
    }
    assert!(moves_seen > 0, "no corpus makes a phase 3 move");
}
