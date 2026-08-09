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
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering as AtomicOrdering,
};
use std::thread;
use std::time::Instant;

use prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph;
use prototext_graph::score::{partition_roots, score_subset, EntryScore, ScoringOpts};

use crate::affinity::Seat;
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
    ranked_with(pb, graph, jobs, cancel, |_| ()).0
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
///
/// It is handed **the number of threads that will actually walk parts**,
/// which is not `jobs` and cannot be derived from it: `jobs` is a
/// ceiling, the seating below may lower it, and the calling thread may
/// or may not join. That count exists only here, so this is where it is
/// reported from — `main`'s startup line is the caller that wants it.
pub(crate) fn ranked_with<T>(
    pb: &[u8],
    graph: &ArchivedCompiledGraph,
    jobs: usize,
    cancel: Option<&AtomicBool>,
    meanwhile: impl FnOnce(usize) -> T,
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
        return (run, meanwhile(1));
    }

    // Spec 0218 S2. `Relaxed` is sufficient: the counter's only job is to
    // hand each index to exactly one thread, which `fetch_add` guarantees
    // whatever the ordering. Everything else is either immutable and
    // published before the scope opens (`pb`, `graph`, `parts`) or
    // returned through `join`, which synchronizes already.
    let cursor = AtomicUsize::new(0);
    // Spec 0269 S1/S2: where the kernel published a seating plan, one
    // worker per *physical core* — two threads on the two hyperthreads
    // of one core deliver 1.04 cores of throughput for 1.92x the latency
    // on each, so the second one buys nothing and lengthens the part it
    // is walking. Elsewhere, spec 0218 S3 unchanged: threads, not parts,
    // bound the spawn, since spawning one per part would reinstate the
    // fixed assignment the cursor exists to undo.
    //
    // `workers` still bounds it from above: `--jobs` is a ceiling and not
    // a target (spec 0217 S4), and seating a member per core is a reason
    // to spawn *fewer* threads, never a licence to overrun the number the
    // caller allowed.
    let threads = Crew::seats()
        .unwrap_or(workers)
        .min(workers)
        .min(parts.len());

    // Spec 0269 S3: the main thread joins the pull loop after
    // `meanwhile`, so it gets a chair of its own — the last one. Read
    // now rather than at the join below, so that the count reported to
    // `meanwhile` and the threads that turn up are the same decision.
    let drawing = crate::affinity::drawing_seat();
    let crew = Crew::new(threads + 1);
    let pull = Pull {
        pb,
        graph,
        opts: &opts,
        parts: &parts,
        cursor: &cursor,
        cancel,
        crew: &crew,
    };

    let (runs, meanwhile_result) = thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let pull = &pull;
                thread::Builder::new()
                    .name("protolens-sweep".to_string())
                    .stack_size(SCORING_THREAD_STACK_SIZE)
                    .spawn_scoped(scope, move || {
                        if pull.crew.seated(i) {
                            // Spec 0269 S2.
                            pull.crew.member(i).sit();
                        } else {
                            // Spec 0264 S7: a sweep wants the whole
                            // machine, not the fast cluster the main
                            // thread may have narrowed itself to and
                            // which this thread inherited across
                            // `clone(2)`.
                            crate::affinity::widen();
                        }
                        pull.run(i)
                    })
                    .expect("spawn sweep worker")
            })
            .collect();

        let meanwhile_result = meanwhile(threads + usize::from(drawing.is_some()));

        // Spec 0269 S3: `meanwhile` is the reason spec 0265 reserved a
        // core for this thread, and it is done. The machine's best core
        // now joins the sweep for the part of it that decides the
        // makespan, rather than standing idle inside `join`.
        //
        // This thread keeps its whole-core mask: it is about to block,
        // so the seat below is a CPU it can give away, not one it holds
        // against anybody.
        let mut runs = match drawing {
            Some(seat) => {
                crew.member(threads).lend(seat);
                pull.run(threads)
            }
            None => Vec::new(),
        };

        runs.extend(
            handles
                .into_iter()
                // A worker panicking is the walk panicking; re-raise it
                // here rather than letting a partial ranking look like
                // an answer.
                .flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))),
        );
        (runs, meanwhile_result)
    });

    (Merged::new(runs).collect(), meanwhile_result)
}

/// A CPU number no seat can have, so one word says both "which CPU" and
/// "nobody's".
const VACANT: usize = usize::MAX;

/// One crew member's published state (spec 0269 S4-S6).
///
/// Published rather than private because a donation is something one
/// thread does *to* another: the donor reads every chair, picks a
/// victim, and re-pins that victim's thread. The victim is inside
/// `score_subset` throughout and never checks anything.
struct Chair {
    /// The member's kernel thread id, which is what `affinity::pin`
    /// takes. Written once, before the member draws its first part.
    thread: AtomicI32,
    /// The CPU this member holds, or [`VACANT`] before it sits and
    /// after it donates.
    cpu: AtomicUsize,
    /// Whether that CPU is one the kernel calls fast. Written by the
    /// donor, after it wins the claim, so a rescued member stops looking
    /// like a rescue target.
    fast: AtomicBool,
    /// Nanoseconds since the sweep opened at which the member started
    /// the part it is walking, or 0 when it is between parts.
    since: AtomicU64,
}

/// One chair as the chooser sees it (spec 0269 S5).
///
/// A plain snapshot, so that [`rescue`] is a pure function of the crew's
/// state and can be tested on a machine that seats nobody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Busy {
    cpu: usize,
    fast: bool,
    since: u64,
}

/// Which member a donor on a `donor_fast` seat should rescue, if any
/// (spec 0269 S5).
///
/// On a slower seat than the donor's, and among those the one that has
/// been walking its part the longest. `detect_fast` is binary, so
/// "slower" is exactly "the donor is fast and this one is not"; and
/// longest-running is the only signal there is, since a part's remaining
/// work is unknowable before the walk and the parts still running at the
/// end are the expensive ones.
///
/// Ties break on the lower index, which matters only for the test — two
/// members cannot start a part in the same nanosecond in practice.
fn rescue(donor_fast: bool, crew: &[Busy]) -> Option<usize> {
    if !donor_fast {
        return None;
    }
    crew.iter()
        .enumerate()
        .filter(|(_, m)| !m.fast && m.cpu != VACANT && m.since != 0)
        .min_by_key(|(i, m)| (m.since, *i))
        .map(|(i, _)| i)
}

/// Everyone walking parts of one job, and the seats between them (spec
/// 0270 S1).
///
/// Two callers drive this: the startup sweep below, whose members are
/// threads it spawns and joins, and the heat pool, whose members are
/// threads that outlive any one query. They share the chairs, the
/// chooser and the migration, and nothing else — the loop around it is
/// a slice cursor on one side and a condvar queue on the other.
pub(crate) struct Crew {
    chairs: Vec<Chair>,
    seats: Option<&'static [Seat]>,
    /// What `since` is measured from. An `Instant` is not storable in an
    /// atomic, and members only ever compare start times with each
    /// other, so one origin per crew is enough.
    epoch: Instant,
}

impl Crew {
    /// How many members the kernel is willing to seat, or `None` where
    /// it seats nobody — which is the whole of spec 0270 G2.
    ///
    /// Separate from [`new`](Self::new) because the startup sweep sizes
    /// its crew from this and then has to pass the size back in.
    pub(crate) fn seats() -> Option<usize> {
        crate::affinity::seats().map(<[Seat]>::len)
    }

    pub(crate) fn new(members: usize) -> Self {
        Self::with_seats(crate::affinity::seats(), members)
    }

    /// [`new`](Self::new) with the seating supplied rather than asked
    /// for, so that a test can seat a crew on a machine whose kernel
    /// seats nobody.
    pub(crate) fn with_seats(seats: Option<&'static [Seat]>, members: usize) -> Self {
        Crew {
            chairs: (0..members).map(|_| Chair::vacant()).collect(),
            seats,
            epoch: Instant::now(),
        }
    }

    /// Is member `i` owed a seat (spec 0270 S2)?
    ///
    /// A member's seat is its index: static, deterministic, and needing
    /// neither a claim protocol nor any notion of when a job begins. The
    /// one place the answer is derived, so that the pool's hand-out
    /// filter and this module's spawn count cannot drift apart.
    pub(crate) fn seated(&self, i: usize) -> bool {
        self.seats.is_some_and(|seats| i < seats.len())
    }

    pub(crate) fn member(&self, i: usize) -> Member<'_> {
        Member { crew: self, i }
    }

    /// Test-only: is member `i` holding a seat right now (spec 0270
    /// S6)? A chair is private state; this is the one window onto it,
    /// so that the pool's tests can say "the query drained and left
    /// nothing behind" without reaching into `Chair`.
    #[cfg(test)]
    pub(crate) fn occupied(&self, i: usize) -> bool {
        self.chairs[i].cpu.load(AtomicOrdering::Acquire) != VACANT
    }
}

/// One member's handle on its own chair — an index and a borrow.
pub(crate) struct Member<'a> {
    crew: &'a Crew,
    i: usize,
}

impl Member<'_> {
    /// Is this member kept off the work the seats exist for (spec 0270
    /// S3)?
    ///
    /// Only where the crew seats somebody. Where the kernel declares no
    /// fast cores there are no seats to be off, so nobody is barred and
    /// every member behaves exactly as it did before spec 0269 — which
    /// is the whole of G2, and the reason this is not simply
    /// `!seated(i)`.
    pub(crate) fn barred(&self) -> bool {
        self.crew.seats.is_some() && !self.crew.seated(self.i)
    }

    /// Take this member's own seat and move onto it (spec 0269 S2).
    ///
    /// Two no-ops, so that no caller needs a branch: a crew that seats
    /// nobody, and a chair that already holds a CPU — the latter being a
    /// member that was rescued onto a faster one and must keep it rather
    /// than march back to the seat it was given at the start.
    pub(crate) fn sit(&self) {
        let chair = &self.crew.chairs[self.i];
        if chair.cpu.load(AtomicOrdering::Acquire) != VACANT {
            return;
        }
        let Some(seat) = self.crew.seats.and_then(|seats| seats.get(self.i)) else {
            return;
        };
        chair.sit(*seat);
    }

    /// Take `seat` without narrowing this thread to it (spec 0269 S3).
    ///
    /// The startup sweep's main thread is the only caller: it owns the
    /// whole physical core the seat is on and is about to block in
    /// `join`, so the seat is a CPU it hands out rather than one it
    /// holds. Deliberately not [`sit`](Self::sit) — its index may well
    /// be a seated one, and `seats()[i]` is not the seat it must take.
    pub(crate) fn lend(&self, seat: Seat) {
        self.crew.chairs[self.i].sit_without_pinning(seat);
    }

    /// Mark this member as walking a part, until the guard drops.
    ///
    /// A guard rather than a pair of calls because the pool's walk arm
    /// leaves by four paths — a deposited run, a shutdown `break`, an
    /// abandoned part, a superseded epoch — and a mark left standing on
    /// an idle member makes it look like the oldest straggler on the
    /// machine, so every donation goes to it.
    #[must_use = "the mark lasts as long as the guard, so dropping it here marks nothing"]
    pub(crate) fn walking(&self) -> Walking<'_> {
        let chair = &self.crew.chairs[self.i];
        // Non-zero even in the first nanosecond of the job, so that 0
        // keeps its one meaning of "between parts".
        let since = self.crew.epoch.elapsed().as_nanos() as u64 + 1;
        chair.since.store(since, AtomicOrdering::Relaxed);
        Walking { chair }
    }

    /// Give this chair's seat to the straggler and vacate it (spec 0269
    /// S4-S7). `true` if there was a seat to give.
    ///
    /// The answer is what tells the pool whether it owes an
    /// `affinity::widen()`, and it is `false` on every call after the
    /// first, so a member parking repeatedly does not re-widen.
    pub(crate) fn leave(&self) -> bool {
        self.crew.chairs[self.i].donate(&self.crew.chairs)
    }
}

/// The mark that says this member is inside a part, for as long as it
/// lives. See [`Member::walking`].
pub(crate) struct Walking<'a> {
    chair: &'a Chair,
}

impl Drop for Walking<'_> {
    fn drop(&mut self) {
        self.chair.since.store(0, AtomicOrdering::Relaxed);
    }
}

impl Chair {
    fn vacant() -> Self {
        Chair {
            thread: AtomicI32::new(0),
            cpu: AtomicUsize::new(VACANT),
            fast: AtomicBool::new(false),
            since: AtomicU64::new(0),
        }
    }

    /// Take `seat` and move onto it (spec 0269 S2).
    fn sit(&self, seat: Seat) {
        crate::affinity::pin(crate::affinity::this_thread(), seat.cpu);
        self.sit_without_pinning(seat);
    }

    /// Take `seat` without narrowing this thread to it — what the main
    /// thread does, since it owns the whole core the seat is on.
    fn sit_without_pinning(&self, seat: Seat) {
        self.thread
            .store(crate::affinity::this_thread(), AtomicOrdering::Relaxed);
        self.fast.store(seat.fast, AtomicOrdering::Relaxed);
        self.cpu.store(seat.cpu, AtomicOrdering::Release);
    }

    fn snapshot(&self) -> Busy {
        Busy {
            cpu: self.cpu.load(AtomicOrdering::Acquire),
            fast: self.fast.load(AtomicOrdering::Relaxed),
            since: self.since.load(AtomicOrdering::Relaxed),
        }
    }

    /// Give this chair's seat to the straggler, and leave (spec 0269
    /// S4). `true` if there was a seat to give.
    ///
    /// A member finding no more work *is* the event that a core has gone
    /// idle, so this is the whole of the endgame: no deadline, no
    /// estimate of when the tail begins, no sentinel.
    fn donate(&self, crew: &[Chair]) -> bool {
        let cpu = self.cpu.swap(VACANT, AtomicOrdering::AcqRel);
        // An unseated crew — G2, and the whole of it.
        if cpu == VACANT {
            return false;
        }
        let fast = self.fast.load(AtomicOrdering::Relaxed);
        loop {
            let seen: Vec<Busy> = crew.iter().map(Chair::snapshot).collect();
            let Some(who) = rescue(fast, &seen) else {
                return true;
            };
            // Spec 0269 S6: two members can go idle at once and pick the
            // same victim. The claim is this exchange, and the loser
            // re-reads and looks again — without it one thread would be
            // pinned twice and one donated core wasted.
            if crew[who]
                .cpu
                .compare_exchange(
                    seen[who].cpu,
                    cpu,
                    AtomicOrdering::AcqRel,
                    AtomicOrdering::Relaxed,
                )
                .is_ok()
            {
                crew[who].fast.store(true, AtomicOrdering::Relaxed);
                crate::affinity::pin(crew[who].thread.load(AtomicOrdering::Relaxed), cpu);
                return true;
            }
        }
    }
}

/// Everything a puller shares, so that the spawned workers and the main
/// thread run the identical loop (spec 0269 S3).
struct Pull<'a> {
    pb: &'a [u8],
    graph: &'a ArchivedCompiledGraph,
    opts: &'a ScoringOpts,
    parts: &'a [Vec<u32>],
    cursor: &'a AtomicUsize,
    cancel: Option<&'a AtomicBool>,
    crew: &'a Crew,
}

impl Pull<'_> {
    /// Draw parts until the cursor is empty, then donate the seat.
    fn run(&self, me: usize) -> Vec<RankedCandidates> {
        let member = self.crew.member(me);
        // Spec 0218 S4: one run per part, kept separate. Concatenating a
        // thread's parts would break the sortedness `Merged` relies on.
        let mut runs = Vec::new();
        loop {
            let i = self.cursor.fetch_add(1, AtomicOrdering::Relaxed);
            let Some(part) = self.parts.get(i) else { break };
            let _walking = member.walking();
            runs.push(rank(score_subset(
                self.pb,
                self.graph,
                self.opts,
                part,
                self.cancel,
            )));
        }
        // These threads exit rather than serve anything else, so unlike
        // the pool (spec 0270 S4) there is no mask to widen afterwards —
        // and the main thread must not, since its mask is spec 0265's
        // drawing core.
        member.leave();
        runs
    }
}

/// The graph's roots cut into parts, once for the process (spec 0262
/// S1).
///
/// [`partition_roots`] is a pure function of the graph and the part
/// count, and both are fixed for the session — yet the heat worker used
/// to rebuild it inside every query. That is **7.3 ms** on googleapis
/// against a median visible query of 5.4–10.4 ms, so more than half of
/// a speculative query was spent re-deriving a constant.
///
/// It holds no borrow of the graph: a part is a list of root indices,
/// and `rank` turns each part's `EntryScore<'g>` into owned
/// `(String, i64)` before anyone stores it. So one of these can be
/// shared by every worker for the life of the process.
pub(crate) struct Partition {
    parts: Vec<Vec<u32>>,
    opts: ScoringOpts,
}

impl Partition {
    /// Cuts `graph`'s roots for a pool of `jobs` workers.
    ///
    /// `jobs` reaches [`target_parts`] through [`effective_jobs`], so a
    /// machine that can really run only one thread still takes the
    /// un-sharded single part — see `target_parts` for why that escape
    /// hatch is not merely a special case of the general rule.
    pub(crate) fn new(graph: &ArchivedCompiledGraph, jobs: usize) -> Self {
        Partition {
            parts: partition_roots(graph, target_parts(effective_jobs(jobs))),
            opts: ScoringOpts::default(),
        }
    }

    /// How many parts a query is, which is how many tasks it becomes.
    pub(crate) fn parts(&self) -> usize {
        self.parts.len()
    }

    /// Scores `pb` against part `index`'s roots alone, ranked.
    ///
    /// One part of one query — the unit of work the whole pool is built
    /// out of. `cancel` abandons the walk mid-field, and the run
    /// returned is then **partial and meaningless**: a caller that
    /// raised it must discard this and walk the part again.
    pub(crate) fn walk(
        &self,
        index: usize,
        pb: &[u8],
        graph: &ArchivedCompiledGraph,
        cancel: Option<&AtomicBool>,
    ) -> RankedCandidates {
        rank(score_subset(
            pb,
            graph,
            &self.opts,
            &self.parts[index],
            cancel,
        ))
    }
}

/// The ranking a query's parts add up to (spec 0262 S4).
///
/// The same [`Merged`] the sharded path uses, so a ranking assembled
/// from parts walked by different workers at different times is
/// bit-for-bit the one a single-threaded sweep produces —
/// [`candidate_order`] is a total order, which is what makes that
/// identity rather than equivalence.
pub(crate) fn merge(runs: Vec<RankedCandidates>) -> RankedCandidates {
    Merged::new(runs).collect()
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
///
/// The candidate is held as the whole `(fqdn, score)` tuple the runs are
/// made of, rather than split into two fields, so that [`Ord`] below can
/// be [`candidate_order`] itself rather than a copy of it.
struct Head {
    entry: (String, i64),
    run: usize,
}

impl Ord for Head {
    /// `BinaryHeap` is a max-heap and we want the *best* candidate on top,
    /// so this is [`candidate_order`] with its arguments swapped: higher
    /// score is greater, and among equal scores the lexicographically
    /// smaller FQDN is.
    ///
    /// Spelling the inverse out by hand instead would compile, agree, and
    /// then quietly stop agreeing the first time the tie-break moved —
    /// which is exactly the failure [`candidate_order`]'s own doc comment
    /// says sharding cannot tolerate, since [`Merged`] assumes each run
    /// is sorted under precisely the relation this compares with.
    fn cmp(&self, other: &Self) -> Ordering {
        candidate_order(&other.entry, &self.entry)
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
            if let Some(entry) = run.next() {
                heap.push(Head { entry, run: i });
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
        let Head { entry, run } = self.heap.pop()?;
        if let Some(next) = self.runs[run].next() {
            self.heap.push(Head { entry: next, run });
        }
        self.remaining -= 1;
        Some(entry)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for Merged {}

#[cfg(test)]
mod tests {
    use prototext_graph::build_scoring_graph::build_from_strings;
    use prototext_graph::score::load::LoadedGraph;

    use super::*;

    fn c(fqdn: &str, score: i64) -> (String, i64) {
        (fqdn.to_string(), score)
    }

    /// A graph with enough *structurally distinct* roots to cut into
    /// more than one part. Distinct is the operative word:
    /// `partition_roots` returns at most one part per state group, and
    /// messages with the same field shape share one — so the field
    /// numbers have to differ, not just the names.
    fn many_root_graph() -> LoadedGraph {
        let n = 60;
        let mut yaml = String::from("entries:\n");
        for i in 0..n {
            yaml.push_str(&format!("- Msg{i}\n"));
        }
        yaml.push_str("messages:\n");
        for i in 0..n {
            yaml.push_str(&format!(
                "  Msg{i}:\n    fields:\n    - number: {}\n      type: uint64\n\
                 \x20   - number: {}\n      type: string\n",
                i + 1,
                i + 100,
            ));
        }
        let (bytes, _, _) =
            build_from_strings(&[yaml], false, false, |_, _| {}).expect("test graph must build");
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        LoadedGraph::from_static_bytes(bytes).expect("test graph must load")
    }

    /// A blob every root in `many_root_graph` can be scored against:
    /// field 1 as a varint, field 100 as a length-delimited string.
    fn scorable_blob() -> Vec<u8> {
        vec![
            0x08, 0x05, // field 1, varint 5
            0xA2, 0x06, 0x02, b'h', b'i', // field 100, LEN "hi"
        ]
    }

    /// Spec 0218's core, and the reason it needs a test at all: the
    /// cursor hands out parts, so *which* thread walked *which* part is
    /// nondeterministic, and a bug there — a part drawn twice, a part
    /// never drawn, runs concatenated before the merge — changes the
    /// ranking rather than crashing. The un-sharded single-thread path
    /// is the reference, and every thread count must reproduce it
    /// exactly, not merely approximately.
    #[test]
    fn every_thread_count_produces_the_ranking_one_thread_produces() {
        assert!(
            available_cpus() > 1,
            "this machine reports one CPU, so `effective_jobs` clamps every \
             request to 1 and the sharded path cannot be exercised at all — \
             the test is not passing, it is unable to run"
        );
        let graph = many_root_graph();
        let blob = scorable_blob();

        // `jobs: 1` takes the un-sharded path (`target_parts(1) == 1`),
        // on this thread, with nothing spawned. That is the reference.
        let reference = ranked(&blob, graph.graph(), 1, None);
        assert!(
            !reference.is_empty(),
            "the fixture must actually score something"
        );

        // More parts than threads (so threads pull repeatedly), as many
        // threads as parts, and more threads than the machine has (so
        // the clamp is exercised too).
        for jobs in [2, 3, 4, 8, SWEEP_PARTS, SWEEP_PARTS + 8, 512] {
            assert_eq!(
                ranked(&blob, graph.graph(), jobs, None),
                reference,
                "the ranking must not depend on how many threads produced it \
                 (jobs = {jobs})"
            );
        }
    }

    /// The partition the threads pull from must be finer than the
    /// thread count for the cursor to mean anything — stated here so
    /// that a future `partition_roots` change that collapses the
    /// fixture into one part turns this into a failure rather than
    /// silently reducing the test above to comparing the un-sharded
    /// path with itself.
    #[test]
    fn the_fixture_is_cut_into_more_parts_than_a_thread_can_take_at_once() {
        let graph = many_root_graph();
        let parts = partition_roots(graph.graph(), target_parts(effective_jobs(4)));
        assert!(
            parts.len() > 4,
            "expected the roots to cut into more than 4 parts, got {}",
            parts.len()
        );
    }

    /// A raised `cancel` reaches every shard. The ranking is then
    /// meaningless by contract — the point is that the threads come
    /// back rather than that the answer is right.
    #[test]
    fn a_raised_cancel_stops_the_shards_rather_than_hanging() {
        let graph = many_root_graph();
        let blob = scorable_blob();
        let cancel = AtomicBool::new(true);
        let out = ranked(&blob, graph.graph(), 8, Some(&cancel));
        assert!(
            out.len() <= ranked(&blob, graph.graph(), 8, None).len(),
            "a cancelled sweep cannot produce more than a complete one"
        );
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

    /// Spec 0262 S2/S4: a query walked one part at a time, by whoever
    /// happens to take each part and in whatever order, must produce
    /// the ranking one whole sweep produces — not merely a similar one.
    /// That identity is what lets the pool hand a single query's parts
    /// to several workers at once.
    ///
    /// The parts are walked here in reverse and the runs merged in that
    /// order, because the pool guarantees nothing at all about which
    /// worker finishes which part first.
    #[test]
    fn a_query_walked_part_by_part_produces_the_whole_sweeps_ranking() {
        let graph = many_root_graph();
        let blob = scorable_blob();
        let reference = ranked(&blob, graph.graph(), 1, None);
        assert!(
            !reference.is_empty(),
            "the fixture must actually score something"
        );

        let partition = Partition::new(graph.graph(), 8);
        assert!(
            partition.parts() > 4,
            "the fixture must cut into enough parts to be shared at all, got {}",
            partition.parts()
        );
        let runs: Vec<RankedCandidates> = (0..partition.parts())
            .rev()
            .map(|i| partition.walk(i, &blob, graph.graph(), None))
            .collect();
        assert_eq!(merge(runs), reference);
    }

    /// Spec 0262 S1: the partition is the part count `target_parts`
    /// asks for, and one worker still takes the un-sharded single part.
    #[test]
    fn the_partition_follows_the_worker_budget() {
        let graph = many_root_graph();
        assert_eq!(Partition::new(graph.graph(), 1).parts(), 1);
        assert_eq!(
            Partition::new(graph.graph(), 4).parts(),
            partition_roots(graph.graph(), SWEEP_PARTS).len()
        );
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

    fn busy(cpu: usize, fast: bool, since: u64) -> Busy {
        Busy { cpu, fast, since }
    }

    /// Spec 0269 S5, over a fabricated crew — which is the only way to
    /// state it, since the machine running the test seats nobody.
    #[test]
    fn a_rescue_takes_the_longest_running_slow_seat() {
        let crew = [
            busy(0, true, 10),  // fast, and the longest-running of all
            busy(4, false, 40), // slow, started late
            busy(5, false, 20), // slow, started earliest of the slow
        ];
        assert_eq!(
            rescue(true, &crew),
            Some(2),
            "a fast seat is never a victim, however long it has run"
        );

        assert_eq!(
            rescue(false, &crew),
            None,
            "a donor that is not faster has nothing to offer"
        );

        assert_eq!(
            rescue(true, &[busy(4, false, 0), busy(5, false, 0)]),
            None,
            "a member between parts is not a straggler"
        );
        assert_eq!(
            rescue(true, &[busy(VACANT, false, 30)]),
            None,
            "a member that has already left its seat is not a straggler"
        );
        assert_eq!(rescue(true, &[]), None);
    }

    /// Spec 0269 S6. Two members go idle at once and see the same
    /// straggler; exactly one donation lands, and the loser moves on
    /// rather than pinning the same thread a second time.
    #[test]
    fn a_rescue_is_claimed_once() {
        // A thread id no thread has, so the `pin` inside `donate` fails
        // with ESRCH and this test moves no real thread anywhere. It is
        // the claim that is under test, not the migration.
        let nobody = i32::MAX;
        let crew: Vec<Chair> = (0..3).map(|_| Chair::vacant()).collect();
        for (i, cpu) in [(0usize, 2usize), (1, 3)] {
            crew[i].thread.store(nobody, AtomicOrdering::Relaxed);
            crew[i].fast.store(true, AtomicOrdering::Relaxed);
            crew[i].cpu.store(cpu, AtomicOrdering::Relaxed);
        }
        crew[2].thread.store(nobody, AtomicOrdering::Relaxed);
        crew[2].cpu.store(9, AtomicOrdering::Relaxed);
        crew[2].since.store(1, AtomicOrdering::Relaxed);

        thread::scope(|scope| {
            let crew = &crew;
            for i in 0..2 {
                scope.spawn(move || crew[i].donate(crew));
            }
        });

        let landed = crew[2].cpu.load(AtomicOrdering::Acquire);
        assert!(
            landed == 2 || landed == 3,
            "the straggler must hold exactly one donor's seat, got {landed}"
        );
        assert!(
            crew[2].fast.load(AtomicOrdering::Relaxed),
            "a rescued member must stop looking like a rescue target"
        );
        for donor in crew.iter().take(2) {
            assert_eq!(
                donor.cpu.load(AtomicOrdering::Acquire),
                VACANT,
                "a donor leaves its seat whether or not it won the claim"
            );
        }
    }

    /// Spec 0270 S1. `since` is what a donor sorts stragglers by, so a
    /// mark left behind by a walk that ended early makes an idle member
    /// look like the oldest straggler on the machine and attract every
    /// donation there is. The guard has to clear it however the walk
    /// ends.
    #[test]
    fn a_walk_mark_is_cleared_however_the_walk_ends() {
        let crew = Crew::with_seats(None, 1);
        let member = crew.member(0);
        let mark = || crew.chairs[0].since.load(AtomicOrdering::Relaxed);

        assert_eq!(mark(), 0, "a member that has not walked is not busy");
        // The scope exit covers the walk arm's ordinary end and its
        // three early ones alike — the shutdown `break`, the abandoned
        // part, the superseded epoch — since all four are the guard
        // going out of scope.
        {
            let _walking = member.walking();
            assert_ne!(mark(), 0, "a walking member must be visible as busy");
        }
        assert_eq!(mark(), 0, "the end of a walk, however it is reached");

        // The fifth way out, and the only one that is not a scope exit:
        // a walk that dies where scoring trips an assertion.
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _walking = member.walking();
            panic!("a walk that dies mid-part");
        }));
        assert!(unwound.is_err());
        assert_eq!(mark(), 0, "a panic through the walk");
    }

    /// Spec 0270 S2, over a fabricated seating — the machine running
    /// this test seats nobody, which is exactly what the last assertion
    /// covers.
    #[test]
    fn a_crew_seats_its_first_members_and_no_others() {
        static SEATS: [Seat; 2] = [
            Seat { cpu: 0, fast: true },
            Seat {
                cpu: 4,
                fast: false,
            },
        ];

        let crew = Crew::with_seats(Some(&SEATS), 4);
        assert!(crew.seated(0) && crew.seated(1), "a seat per seat");
        assert!(
            !crew.seated(2) && !crew.seated(3),
            "a member past the last seat is unseated, and stays unseated \
             — there is no claim protocol to change its mind"
        );
        assert!(!crew.member(1).barred() && crew.member(2).barred());

        let unseated = Crew::with_seats(None, 4);
        assert!(
            (0..4).all(|i| !unseated.seated(i) && !unseated.member(i).barred()),
            "a kernel that declares no fast cores seats nobody — and bars \
             nobody either, or a query nobody may walk would never finish"
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
