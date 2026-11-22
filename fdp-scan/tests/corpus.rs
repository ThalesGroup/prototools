// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The public corpus as fdp-scan's regression suite (spec 0448 S9): each
//! layout `tests/corpus/generate.py` wrote, scored against its truth.

use std::path::Path;

/// A true record's fate, as the contributing user's harness classifies it.
#[derive(Debug, PartialEq, Eq)]
enum Fate {
    Exact,
    /// Found at its start, ending late only by bytes framing declared
    /// top-level fields: no scanner can tell those from the record's own
    /// (spec 0448 N2).
    Ambiguous,
    Short(usize),
    Long,
    Merged,
    Missed,
}

fn varint(data: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *data.get(i)?;
        i += 1;
        v |= u64::from(b & 0x7F) << shift;
        if b < 0x80 {
            return Some((v, i));
        }
    }
    None
}

/// Whether `extra` is a run of well-framed top-level fields the stock
/// `FileDescriptorProto` declares, with a wire type they can wear: varint
/// only for the three numeric fields (`public_dependency`,
/// `weak_dependency`, `edition`), length-delimited for any. Never field 1:
/// `name` is singular, so a second one always ends the record.
fn frames_declared_fields(extra: &[u8]) -> bool {
    const DECLARED: [u64; 14] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15];
    const NUMERIC: [u64; 3] = [10, 11, 14];
    let mut i = 0;
    while i < extra.len() {
        let Some((tag, next)) = varint(extra, i) else {
            return false;
        };
        let (number, wire) = (tag >> 3, tag & 7);
        if number == 1 || !DECLARED.contains(&number) {
            return false;
        }
        i = match wire {
            2 => match varint(extra, next) {
                Some((len, at)) if at + len as usize <= extra.len() => at + len as usize,
                _ => return false,
            },
            0 if NUMERIC.contains(&number) => match varint(extra, next) {
                Some((_, at)) => at,
                None => return false,
            },
            _ => return false,
        };
    }
    true
}

struct Scored {
    fates: Vec<(String, Fate)>,
    spurious: Vec<(usize, usize)>,
    explained: fdp_scan::Explained,
}

fn score(layout: &str) -> Scored {
    score_with(layout, &fdp_scan::ScanOptions::default())
}

fn score_with(layout: &str, options: &fdp_scan::ScanOptions) -> Scored {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let data = std::fs::read(dir.join(format!("{layout}.bin"))).expect("layout");
    let truth: Vec<(String, usize, usize)> = serde_json::from_slice(
        &std::fs::read(dir.join(format!("{layout}.truth.json"))).expect("truth"),
    )
    .expect("truth is [name, start, end] triples");
    let explained = fdp_scan::scan_explained_options(&data, options);
    let found = explained.found.clone();
    let end_at: std::collections::HashMap<usize, usize> = found.iter().copied().collect();
    let fates = truth
        .iter()
        .map(|(name, start, end)| {
            let fate = match end_at.get(start) {
                Some(&got) if got == *end => Fate::Exact,
                Some(&got) if got < *end => Fate::Short(got),
                Some(&got) if frames_declared_fields(&data[*end..got]) => Fate::Ambiguous,
                Some(_) => Fate::Long,
                None if found.iter().any(|&(s, e)| s < *start && *start < e) => Fate::Merged,
                None => Fate::Missed,
            };
            (name.clone(), fate)
        })
        .collect();
    let starts: std::collections::HashSet<usize> = truth.iter().map(|t| t.1).collect();
    let spurious = found
        .into_iter()
        .filter(|r| !starts.contains(&r.0))
        .collect();
    Scored {
        fates,
        spurious,
        explained,
    }
}

/// Where a `#top-level-171` record must end: before its last field, the
/// four bytes of `field(171, "x")` the generator appended (spec 0448 S2).
const TOP_LEVEL_171: usize = 4;

#[test]
fn corpus_layouts_meet_their_targets() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let truth_of = |layout: &str| -> Vec<(String, usize, usize)> {
        serde_json::from_slice(&std::fs::read(dir.join(format!("{layout}.truth.json"))).unwrap())
            .unwrap()
    };
    let mut failures = Vec::new();
    let layouts = [
        "isolated",
        "consecutive",
        "fds",
        "crafted",
        "trailer",
        "noise-0",
        "noise-1",
        "noise-2",
        "decoys",
    ];
    for layout in layouts {
        let scored = score(layout);
        let truth = truth_of(layout);
        for ((name, fate), (_, _, end)) in scored.fates.iter().zip(&truth) {
            let ok = if name.ends_with("#top-level-171") {
                *fate == Fate::Short(end - TOP_LEVEL_171)
            } else if name.ends_with("#name-only")
                && (layout.starts_with("noise-") || layout == "trailer")
            {
                // Before random bytes or an undeclared field it does not
                // stand alone (S6).
                *fate == Fate::Missed
            } else if layout.starts_with("noise-") {
                matches!(fate, Fate::Exact | Fate::Ambiguous)
            } else {
                *fate == Fate::Exact
            };
            if !ok {
                failures.push(format!("{layout}: {name}: {fate:?}"));
            }
        }
        if !scored.spurious.is_empty() {
            failures.push(format!("{layout}: spurious {:?}", scored.spurious));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The committed corpus is what it says it is: every layout present, and
/// decoys without truth (spec 0448 S8).
#[test]
fn the_corpus_is_complete() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("corpus.json")).unwrap()).unwrap();
    assert_eq!(meta["source"], "public");
    assert!(
        score("isolated").fates.len() > 50,
        "the mutations are there"
    );
    assert!(score("decoys").fates.is_empty());
}

/// Spec 0449 S8. The options decoy is refused as a dangling dependency,
/// alone and between random bytes.
#[test]
fn the_options_decoy_is_a_dangling_dependency() {
    let refused: Vec<_> = score("decoys")
        .explained
        .rejected
        .into_iter()
        .filter(|r| r.name == "com.example.decoy.proto")
        .map(|r| r.reason)
        .collect();
    assert_eq!(refused, vec![fdp_scan::Reason::DanglingDependency; 2]);
}

/// Spec 0449 S9. The random-neighbor layouts with the order option: every
/// record it ends early is reported in `order_cuts`, and the fates stay
/// within the measured bounds.
#[test]
fn the_order_option_on_random_neighbors() {
    let on = fdp_scan::ScanOptions {
        increasing_field_numbers: true,
    };
    for layout in ["noise-0", "noise-1", "noise-2"] {
        let off = score(layout);
        let scored = score_with(layout, &on);
        let cut: std::collections::HashMap<usize, usize> = scored
            .explained
            .order_cuts
            .iter()
            .map(|c| (c.start, c.end))
            .collect();
        let before: std::collections::HashMap<usize, usize> =
            off.explained.found.iter().copied().collect();
        for &(start, end) in &scored.explained.found {
            if before.get(&start).is_some_and(|&was| end < was) {
                assert_eq!(
                    cut.get(&start),
                    Some(&end),
                    "{layout}: {start} cut unreported"
                );
            }
        }
        // Measured 2026-11-22: no record ambiguous, merged or spurious any
        // more; the name-only record missed, as without the option; and the
        // `file-option-5` mutations short. Those are the generator's: Python
        // re-serializes an unknown option after the known ones, where protoc,
        // which knows it, would write it in order. Each is reported above.
        for (name, fate) in &scored.fates {
            let ok = match fate {
                Fate::Exact => true,
                Fate::Short(_) => name.contains("file-option-5"),
                Fate::Missed => name.ends_with("#name-only"),
                Fate::Ambiguous | Fate::Long | Fate::Merged => false,
            };
            assert!(ok, "{layout}: {name}: {fate:?}");
        }
        assert!(
            scored.spurious.is_empty(),
            "{layout}: {:?}",
            scored.spurious
        );
    }
}
