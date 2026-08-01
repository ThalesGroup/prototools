// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The inference sweep, divided among the cores (spec 0217).
//!
//! Every ranked candidate list protolens produces comes from here —
//! startup's root-type sweep, the heat worker's per-range sweep, and the
//! synchronous fallback for the no-worker configuration. They differ only
//! in the byte range they score and in how many threads they are allowed
//! to use.
//!
//! The division is safe because the inputs are read-only for the whole
//! session: the scoring graph is mapped once and never written, and every
//! scored range is a subrange of an immutable blob. Shards therefore share
//! `&[u8]` and `&ArchivedCompiledGraph` and touch nothing else — no lock
//! in the walk, no synchronization between shards. They meet only at the
//! merge.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::thread;

use prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph;
use prototext_graph::score::{partition_roots, score_subset, EntryScore, ScoringOpts};

use crate::decode::RankedCandidates;

/// Stack reservation for any thread that runs the scoring walk (spec 0180
/// S4, generalized by spec 0217 S5).
///
/// `stack = depth × frame`, and both terms are measured — see
/// `MAX_WIRE_DEPTH`'s doc comment in `prototext-core`, which is the source
/// of truth. Depth is bounded only by `MAX_WIRE_DEPTH = 1000`, and that
/// cap is the only reason a worst case is finite at all, since wire depth
/// otherwise scales with input length. `score_message_multi`'s frame is
/// ≈ 590 B in release (bisected `stack_size`), so a full-cap nest costs
/// ≈ 576 KiB — and ~8× that in a debug build, which is why the
/// deep-nesting tests abort under a debug `cargo test`.
///
/// Sharding moves neither term. Depth is a property of the blob, not of
/// the candidate set, so a shard carrying a tenth of the roots recurses
/// just as deep; and the frame is candidate-count-independent because the
/// active set lives on the heap. Every shard therefore needs the same
/// reservation as one whole sweep, and N of them cost N × this in
/// *address space* rather than resident pages — measured maximum depth on
/// googleapis is 13, so a few kilobytes per shard is what is actually
/// touched.
///
/// 16 MiB gives the ~28× margin every other (walker, thread) pair in the
/// workspace has, and comfortably exceeds the main thread's own default
/// (commonly 8 MiB, per `RLIMIT_STACK`) rather than merely matching it.
///
/// Declared here, in the module every walking thread is spawned from, so
/// that no walker can silently miss it.
pub(crate) const SCORING_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

/// How many parts the roots are cut into, independently of how many
/// threads will walk them (spec 0218 S1).
///
/// A part's cost is how deep into the blob its candidates stay alive,
/// which is neither its root count nor its group count nor knowable
/// before the walk. So the partition cannot be balanced; it can only be
/// made fine enough that the imbalance stops mattering, with threads
/// taking a new part when they finish one.
///
/// **24 is a fit, not a derivation.** Measured on two corpora, each part
/// timed alone and the 8-worker makespan simulated from those times:
///
/// | | googleapis | pdb |
/// |---|---|---|
/// | roots / state groups | 49 255 / 17 572 | 1 900 / 1 166 |
/// | best part count | **24** (0.957 s) | 32-48 (0.019 s) |
/// | this value costs | 0.957 s | 0.026 s |
///
/// googleapis chose 24 at every blob size from 3.2 to 25.7 MB and at 4,
/// 8 and 12 workers alike; pdb prefers finer, but its whole sweep is
/// 20 ms, so the 7 ms it gives up here is not a cost. The two differ 15×
/// in group count and 23× in blob size, yet the optimal *part count*
/// moves only 24 → 40 while the optimal *groups per part* moves 630 →
/// 29 — which is why this is a part count and not a work-proportional
/// target.
///
/// Why that should be so is not understood; see spec 0218's "What is not
/// understood". A third corpus may move this number, which is why it is
/// one constant in one place.
pub(crate) const SWEEP_PARTS: usize = 24;

/// The one ranking order (spec 0217 S2): highest score first, ties broken
/// by FQDN ascending.
///
/// Sharding makes this the single definition rather than merely the tidy
/// one: [`Merged`] assumes each shard sorted under exactly the relation
/// the merge compares with, which hand-copied closures cannot guarantee.
///
/// Root FQDNs are unique, so this is a *total* order on the candidates.
/// That is what makes a sharded result identical to a whole-sweep one
/// rather than merely equivalent up to tie order.
pub(crate) fn candidate_order(a: &(String, i64), b: &(String, i64)) -> Ordering {
    b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0))
}

/// How many CPUs this process may actually run on, cached.
///
/// `available_parallelism` respects the cgroup quota and the CPU affinity
/// mask, so under a container limit or a `taskset` it reports the real
/// ceiling rather than the machine's core count. Cached because the heat
/// worker asks once per request served, and a request can be a few
/// milliseconds of work.
pub(crate) fn available_cpus() -> usize {
    static CPUS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CPUS.get_or_init(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    })
}

/// The thread count a sweeper may actually use, given what it asked for.
///
/// `--jobs` is a **ceiling, not a target** (spec 0217 S4). A request is
/// clamped to what the process can really run on: spawning ten shards on
/// two CPUs does not divide the walk into ten, it divides it into two and
/// adds eight sets of setup, eight stack reservations, and five times the
/// convergence duplication described in [`partition_roots`]. Every extra
/// shard past the CPU count is pure loss, so the number the user supplies
/// bounds the fan-out from above and never drives it.
///
/// Always at least 1, so a sweep always happens.
pub(crate) fn effective_jobs(requested: usize) -> usize {
    requested.clamp(1, available_cpus())
}

/// How many parts to ask [`partition_roots`] for, given the number of
/// threads that will pull them (spec 0218 S1, S5).
///
/// Cut by a constant rather than by the thread count, because the point
/// of the cursor is that a thread takes another part when it finishes
/// one. `partition_roots` returns at most one part per state group, so a
/// small graph clamps itself and needs no lower bound here; the `max`
/// matters only where `--jobs` exceeds [`SWEEP_PARTS`].
///
/// One worker is the exception and takes one part. Cutting finer is
/// faster on googleapis (6.96 → 5.53 s of total work) and *slower* on pdb
/// (0.072 → 0.098 s), and the single-threaded path is the escape hatch
/// for a loaded machine — it must not be a place where a corpus can lose.
fn target_parts(workers: usize) -> usize {
    if workers <= 1 {
        1
    } else {
        workers.max(SWEEP_PARTS)
    }
}

/// Score `pb` against every root in `graph`, ranked, using up to `jobs`
/// threads (see [`effective_jobs`] — `jobs` is a ceiling).
///
/// `cancel` reaches every shard's `score_subset` unchanged: raising it
/// stops each of them at its next wire field, and the ranking returned is
/// then **partial and meaningless**. It is for a caller that has already
/// decided not to use the answer and only wants its thread back.
pub(crate) fn ranked(
    pb: &[u8],
    graph: &ArchivedCompiledGraph,
    jobs: usize,
    cancel: Option<&AtomicBool>,
) -> RankedCandidates {
    ranked_with(pb, graph, jobs, cancel, || ()).0
}

/// [`ranked`], with `meanwhile` run on the calling thread while the shards
/// walk.
///
/// Spec 0217 S6: startup's other unavoidable work — building the node
/// arena — depends only on the bytes (spec 0216) and not on the root type,
/// so it has no reason to wait for the sweep. Handing it in here is what
/// overlaps them, and doing it with a closure rather than by returning a
/// join handle is what keeps the shards borrowing `pb` and `graph` instead
/// of demanding `'static` copies of them.
///
/// `meanwhile` runs on this thread, so it need not be `Send`, and it runs
/// exactly once whether or not any shard was spawned.
pub(crate) fn ranked_with<T>(
    pb: &[u8],
    graph: &ArchivedCompiledGraph,
    jobs: usize,
    cancel: Option<&AtomicBool>,
    meanwhile: impl FnOnce() -> T,
) -> (RankedCandidates, T) {
    let opts = ScoringOpts::default();
    // Clamped here rather than only at the command line, so that every
    // path into the sweep is bounded by the machine whether or not its
    // caller remembered to ask.
    let workers = effective_jobs(jobs);

    let parts = partition_roots(graph, target_parts(workers));

    // One part — or none, on a graph with no roots — is the un-sharded
    // path, on this thread, with nothing spawned (spec 0217 G5).
    if parts.len() <= 1 {
        let run = match parts.first() {
            Some(part) => rank(score_subset(pb, graph, &opts, part, cancel)),
            None => Vec::new(),
        };
        return (run, meanwhile());
    }

    // Spec 0218 S2. `Relaxed` is sufficient: the counter's only job is to
    // hand each index to exactly one thread, which `fetch_add` guarantees
    // whatever the ordering. Everything else is either immutable and
    // published before the scope opens (`pb`, `graph`, `parts`) or
    // returned through `join`, which synchronizes already.
    let cursor = AtomicUsize::new(0);
    // Spec 0218 S3: threads, not parts, bound the spawn. Spawning one per
    // part would reinstate the fixed assignment the cursor exists to undo.
    let threads = workers.min(parts.len());

    let (runs, meanwhile_result) = thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let cursor = &cursor;
                let parts = &parts;
                let opts = &opts;
                thread::Builder::new()
                    .name("protolens-sweep".to_string())
                    .stack_size(SCORING_THREAD_STACK_SIZE)
                    .spawn_scoped(scope, move || {
                        // Spec 0218 S4: one run per part, kept separate.
                        // Concatenating a thread's parts would break the
                        // sortedness `Merged` relies on.
                        let mut runs = Vec::new();
                        loop {
                            let i = cursor.fetch_add(1, AtomicOrdering::Relaxed);
                            let Some(part) = parts.get(i) else { break };
                            runs.push(rank(score_subset(pb, graph, opts, part, cancel)));
                        }
                        runs
                    })
                    .expect("spawn sweep worker")
            })
            .collect();

        let meanwhile_result = meanwhile();

        let runs: Vec<RankedCandidates> = handles
            .into_iter()
            // A worker panicking is the walk panicking; re-raise it here
            // rather than letting a partial ranking look like an answer.
            .flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect();
        (runs, meanwhile_result)
    });

    (Merged::new(runs).collect(), meanwhile_result)
}

/// One shard's scores, filtered to the non-vetoed and sorted into
/// [`candidate_order`].
///
/// Vetoed entries are dropped before the sort rather than after it: the
/// result is the same list — a type the wire data already contradicts is
/// not a plausible candidate at any rank — over fewer elements.
fn rank(scores: Vec<EntryScore<'_>>) -> RankedCandidates {
    let mut out: RankedCandidates = scores
        .into_iter()
        .filter(|r| !r.vetoed)
        .map(|r| (r.fqdn.to_owned(), r.score()))
        .collect();
    out.sort_by(candidate_order);
    out
}

/// The best remaining candidate of one run, and which run it came from.
struct Head {
    score: i64,
    fqdn: String,
    run: usize,
}

impl Ord for Head {
    /// `BinaryHeap` is a max-heap and we want the *best* candidate on top,
    /// so this is [`candidate_order`] reversed: higher score is greater,
    /// and among equal scores the lexicographically smaller FQDN is.
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.fqdn.cmp(&self.fqdn))
    }
}

impl PartialOrd for Head {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Head {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Head {}

/// An N-way merge over runs that are already sorted into
/// [`candidate_order`] (spec 0217 S3).
///
/// Concatenating the runs and calling `sort_by` would also work — Rust's
/// stable sort detects natural runs — but it pays a linear scan to
/// rediscover boundaries the caller already knows. That scan is not the
/// reason this exists, though; it is microseconds against a multi-second
/// sweep. The reason is that a merge can stop early and a sort cannot, and
/// most callers do not want the whole ranking: the root-type winner is the
/// first element (plus one more for the tie check), `derive_stats` wants
/// the best score and how many share it, and `by_range`'s `top_n` is
/// capped at a screenful. Only the `complete` cache wants all of it.
///
/// So this is an `Iterator`, and `collect()` is the eager case rather than
/// the only one.
pub(crate) struct Merged {
    runs: Vec<std::vec::IntoIter<(String, i64)>>,
    heap: BinaryHeap<Head>,
    /// How many candidates are still to come. Known exactly and up front
    /// — the shards partition the roots, so the total is the sum of the
    /// run lengths — which is what makes this an [`ExactSizeIterator`]
    /// and lets a caller that only wants the top few still say how many
    /// there were.
    remaining: usize,
}

impl Merged {
    pub(crate) fn new(runs: Vec<RankedCandidates>) -> Self {
        let remaining = runs.iter().map(Vec::len).sum();
        let mut runs: Vec<std::vec::IntoIter<(String, i64)>> =
            runs.into_iter().map(Vec::into_iter).collect();
        let mut heap = BinaryHeap::with_capacity(runs.len());
        for (i, run) in runs.iter_mut().enumerate() {
            if let Some((fqdn, score)) = run.next() {
                heap.push(Head {
                    score,
                    fqdn,
                    run: i,
                });
            }
        }
        Merged {
            runs,
            heap,
            remaining,
        }
    }
}

impl Iterator for Merged {
    type Item = (String, i64);

    fn next(&mut self) -> Option<Self::Item> {
        let Head { score, fqdn, run } = self.heap.pop()?;
        if let Some((next_fqdn, next_score)) = self.runs[run].next() {
            self.heap.push(Head {
                score: next_score,
                fqdn: next_fqdn,
                run,
            });
        }
        self.remaining -= 1;
        Some((fqdn, score))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for Merged {}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(fqdn: &str, score: i64) -> (String, i64) {
        (fqdn.to_string(), score)
    }

    /// The merge reproduces what sorting the concatenation would give —
    /// the property spec 0217 G2 rests on, stated over the merge alone so
    /// a failure here is not confused with a scoring difference.
    #[test]
    fn merging_sorted_runs_equals_sorting_the_concatenation() {
        let runs = vec![
            vec![c("a.A", 90), c("a.D", 40), c("a.G", 10)],
            vec![c("a.B", 90), c("a.E", 30)],
            vec![],
            vec![c("a.C", 55), c("a.F", 30), c("a.H", -5)],
        ];

        let mut flat: RankedCandidates = runs.iter().flatten().cloned().collect();
        flat.sort_by(candidate_order);

        let merged: RankedCandidates = Merged::new(runs).collect();
        assert_eq!(merged, flat);
    }

    /// Equal scores are broken by FQDN *across* runs, not merely within
    /// one — the case a per-run sort cannot settle by itself, and the one
    /// that decides whether a sharded ranking is identical to a whole one
    /// or only equivalent.
    #[test]
    fn a_score_tie_is_broken_by_fqdn_across_runs() {
        let runs = vec![vec![c("z.Late", 7)], vec![c("a.Early", 7)]];
        let merged: RankedCandidates = Merged::new(runs).collect();
        assert_eq!(merged, vec![c("a.Early", 7), c("z.Late", 7)]);
    }

    /// Spec 0218 S1/S5: the part count comes from the constant, not from
    /// the thread count — except at one thread, which takes the
    /// un-sharded path.
    #[test]
    fn the_part_count_is_a_constant_not_the_thread_count() {
        assert_eq!(target_parts(0), 1, "no workers still means one part");
        assert_eq!(target_parts(1), 1, "one worker takes one part");
        for workers in 2..=SWEEP_PARTS {
            assert_eq!(
                target_parts(workers),
                SWEEP_PARTS,
                "{workers} workers should still cut {SWEEP_PARTS} parts"
            );
        }
        // Past the constant the thread count takes over, so a big machine
        // never has a thread with nothing to draw.
        assert_eq!(target_parts(SWEEP_PARTS + 1), SWEEP_PARTS + 1);
        assert_eq!(target_parts(96), 96);
    }

    /// Spec 0218 S4/G2: a thread that draws several parts contributes
    /// several runs, and which thread drew what — hence the order the runs
    /// arrive in — must not reach the ranking.
    #[test]
    fn the_merge_is_insensitive_to_run_order() {
        let runs = vec![
            vec![c("a.A", 90), c("a.D", 40)],
            vec![c("a.B", 90), c("a.E", 30)],
            vec![],
            vec![c("a.C", 55), c("a.F", 30), c("a.H", -5)],
        ];

        let expected: RankedCandidates = Merged::new(runs.clone()).collect();

        // Every rotation of the run list is a plausible completion order.
        for shift in 1..runs.len() {
            let mut rotated = runs.clone();
            rotated.rotate_left(shift);
            assert_eq!(
                Merged::new(rotated).collect::<RankedCandidates>(),
                expected,
                "rotating the runs by {shift} changed the ranking"
            );
        }

        let mut reversed = runs;
        reversed.reverse();
        assert_eq!(
            Merged::new(reversed).collect::<RankedCandidates>(),
            expected
        );
    }

    /// The merge is lazy: taking two elements must not drain the runs
    /// behind them. This is what buys the root-type winner and its tie
    /// check without materializing a 49 000-entry ranking.
    #[test]
    fn the_merge_yields_in_order_without_draining() {
        let runs = vec![
            vec![c("a.A", 90), c("a.D", 40)],
            vec![c("a.B", 80), c("a.E", 30)],
        ];
        let mut merged = Merged::new(runs);
        assert_eq!(merged.len(), 4);
        assert_eq!(merged.next(), Some(c("a.A", 90)));
        assert_eq!(merged.next(), Some(c("a.B", 80)));
        // Two of four consumed; the count reported is of what is left,
        // and the rest are still there and still ordered.
        assert_eq!(merged.len(), 2);
        let rest: RankedCandidates = merged.collect();
        assert_eq!(rest, vec![c("a.D", 40), c("a.E", 30)]);
    }
}
