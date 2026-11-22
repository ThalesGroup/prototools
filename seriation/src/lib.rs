// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Order undated snapshots of files by how the files in them evolve (spec
//! 0451).
//!
//! A snapshot is the set of files a source held at one point of an unknown
//! history. Given each snapshot's build time, its order hint when it has
//! one, the version of each of its files, a way to fetch a version's bytes
//! and a comparator, [`sort`] returns the order that best explains them.
//! The algorithm is described in `docs/seriation/`.
//!
//! The crate reads no repository and prints nothing: the caller owns where
//! snapshots and contents come from, and what is done with the order.

pub mod generate;
mod greedy;
mod interleaving;
mod kendall;
mod order_hints;
mod state;
mod text;

use std::fmt;
use std::time::Duration;

use greedy::Search;

pub use text::LinesChars;

/// A file, as the caller numbers it. The crate only compares these.
pub type FileId = u32;

/// A version of a file, as the caller numbers it: two instances of a file
/// are the same version iff their `VersionId`s are equal.
pub type VersionId = u32;

/// One snapshot to order.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Unique among the snapshots. Snapshots are loaded in its byte order,
    /// whatever order they are passed in.
    pub identity: String,
    /// Seconds since the epoch.
    pub build_time: i64,
    /// A monotonic revision number, when the source has one. `Some(0)` is a
    /// hint like any other.
    pub order_hint: Option<u64>,
    /// The files this snapshot holds, each once, with its version.
    pub items: Vec<(FileId, VersionId)>,
}

/// Where the sort gets a version's bytes, to compare two versions of a file.
pub trait Contents {
    /// The bytes of `version` of `file`, in the form the comparator compares
    /// (canonical, spec 0450 S2). Called file by file, each distinct version
    /// of a file once; nothing fetched is kept past that file.
    fn fetch(&mut self, file: FileId, version: VersionId) -> Result<Vec<u8>, BoxError>;
}

/// Which of two versions of a file contains the other.
pub trait Comparator {
    fn compare(&self, a: &[u8], b: &[u8]) -> Containment;
}

/// A comparator's answer. Containment means age: the contained version is
/// the older one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Containment {
    Equal,
    /// The first is inside the second, so older.
    FirstInSecond,
    /// The second is inside the first, so older.
    SecondInFirst,
    Incomparable,
}

/// An error from a [`Contents`] implementation.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// How to sort. `Default` gives the defaults documented on each field.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Phase 3 compares two instances of a file only when their build times
    /// are at most this far apart. 30 days.
    pub window: Duration,
    /// The greedy search tries this many candidates before taking the best
    /// improving move it found. 1.
    pub nom_tries: usize,
    /// It tries at most this many before declaring convergence. 20.
    pub max_tries: usize,
    /// Phase 2: place the snapshots that have an order hint in hint order,
    /// and keep them so in phases 3 and 4. Off.
    pub order_hints: bool,
    /// Phases to skip, for experiments.
    pub skip: Skip,
    /// The share, in percent, of the snapshots whose order hint the sort is
    /// shown (the self-test, spec 0451 S4). 100.
    pub reveal_percent: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            window: Duration::from_secs(30 * 86_400),
            nom_tries: 1,
            max_tries: 20,
            order_hints: false,
            skip: Skip::default(),
            reveal_percent: 100,
        }
    }
}

/// Phases to skip. Loading is never skipped; phase 2 runs only with
/// [`Settings::order_hints`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Skip {
    pub build_time: bool,
    pub containment: bool,
    pub interleaving: bool,
}

/// The phases, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Load,
    BuildTime,
    OrderHints,
    Containment,
    Interleaving,
}

/// What one phase did, and every score after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseReport {
    pub phase: Phase,
    pub skipped: bool,
    /// Snapshots moved: positions changed for phase 1, moves made for the
    /// others.
    pub moves: usize,
    /// Phase 3 only: pairs of different versions the comparator was asked
    /// about, and how many of them it ordered.
    pub comparisons: u64,
    pub signals: u64,
    /// The edge score, once phase 3 has built its graph.
    pub edge_score: Option<i64>,
    /// The interleaving score, once phase 3 or 4 has run.
    pub interleaving_score: Option<i64>,
    /// Kendall's tau against the true order hints, hidden ones included:
    /// the negated number of pairs in the opposite order to their hints.
    pub tau: i64,
    /// Phases 3 and 4: each move, in the order made.
    pub moved: Vec<Move>,
}

/// One move of the greedy search (phases 3 and 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    /// The snapshot moved: an index into the snapshots passed to [`sort`].
    pub snapshot: usize,
    /// Its position before and after the move, 0 the oldest.
    pub from: usize,
    pub to: usize,
    /// How much the move raised the phase's score.
    pub gain: i64,
    /// Its neighbors right after the move, older and newer: indices into
    /// the snapshots passed to [`sort`].
    pub after: Option<usize>,
    pub before: Option<usize>,
}

/// The steps of a search, each naming its snapshot, replayed from `order`,
/// the order (caller indices by position) the search started from.
fn named(mut order: Vec<usize>, steps: Vec<greedy::Step>) -> Vec<Move> {
    steps
        .into_iter()
        .map(|step| {
            let (from, to) = (step.from as usize, step.to as usize);
            let snapshot = order.remove(from);
            order.insert(to, snapshot);
            Move {
                snapshot,
                from,
                to,
                gain: step.gain,
                after: to.checked_sub(1).map(|i| order[i]),
                before: order.get(to + 1).copied(),
            }
        })
        .collect()
}

/// The result of [`sort`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sorted {
    /// Indices into the snapshots passed to [`sort`], oldest first.
    pub order: Vec<usize>,
    pub phases: Vec<PhaseReport>,
}

/// Why [`sort`] refused.
#[derive(Debug)]
pub enum Error {
    /// The settings contradict themselves.
    Settings(String),
    /// More snapshots or instances than the sort's indices can hold.
    Capacity(String),
    /// Two snapshots share an identity.
    DuplicateIdentity(String),
    /// A snapshot lists a file twice.
    DuplicateFile { identity: String, file: FileId },
    /// The [`Contents`] implementation failed.
    Contents(BoxError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Settings(m) | Error::Capacity(m) => f.write_str(m),
            Error::DuplicateIdentity(id) => write!(f, "two snapshots are named {id:?}"),
            Error::DuplicateFile { identity, file } => {
                write!(f, "snapshot {identity:?} lists file {file} twice")
            }
            Error::Contents(e) => write!(f, "fetching contents: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Order `snapshots` (spec 0451 S1).
pub fn sort(
    snapshots: &[Snapshot],
    contents: &mut dyn Contents,
    comparator: &dyn Comparator,
    settings: &Settings,
) -> Result<Sorted, Error> {
    check_settings(settings)?;
    let mut state = state::SortState::load(snapshots, settings.reveal_percent)?;
    let mut phases = vec![state.report(Phase::Load, false, 0)];

    // Phase 1.
    if settings.skip.build_time {
        phases.push(state.report(Phase::BuildTime, true, 0));
    } else {
        let moved = state.sort_by_build_time();
        phases.push(state.report(Phase::BuildTime, false, moved));
    }

    // Phase 2.
    if settings.order_hints {
        let moves = order_hints::place(&mut state);
        phases.push(state.report(Phase::OrderHints, false, moves));
    } else {
        phases.push(state.report(Phase::OrderHints, true, 0));
    }

    // Phase 3.
    if settings.skip.containment {
        phases.push(state.report(Phase::Containment, true, 0));
    } else {
        let (comparisons, signals) = state.build_edges(contents, comparator, settings.window)?;
        let start = state.order();
        let steps = greedy::search(&mut state::EdgeSearch::new(&mut state), settings);
        let moved = named(start, steps);
        let mut report = state.report(Phase::Containment, false, moved.len());
        report.moved = moved;
        report.comparisons = comparisons;
        report.signals = signals;
        report.interleaving_score = Some(interleaving::Untangle::new(&state).score());
        phases.push(report);
    }

    // Phase 4.
    if settings.skip.interleaving {
        phases.push(state.report(Phase::Interleaving, true, 0));
    } else {
        let start = state.order();
        let mut untangle = interleaving::Untangle::new(&state);
        let steps = greedy::search(&mut untangle, settings);
        let score = untangle.score();
        state.adopt_order(untangle.order());
        let moved = named(start, steps);
        let mut report = state.report(Phase::Interleaving, false, moved.len());
        report.moved = moved;
        report.interleaving_score = Some(score);
        phases.push(report);
    }

    Ok(Sorted {
        order: state.order(),
        phases,
    })
}

fn check_settings(settings: &Settings) -> Result<(), Error> {
    if settings.nom_tries == 0 {
        return Err(Error::Settings("nom_tries must be at least 1".into()));
    }
    if settings.nom_tries > settings.max_tries {
        return Err(Error::Settings(format!(
            "nom_tries ({}) must not exceed max_tries ({})",
            settings.nom_tries, settings.max_tries
        )));
    }
    if settings.reveal_percent > 100 {
        return Err(Error::Settings(format!(
            "reveal_percent ({}) is a percentage",
            settings.reveal_percent
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
