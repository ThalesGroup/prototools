// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The Linux half of spec 0264: sysfs says which CPUs are fast,
//! `sched_setaffinity` acts on it.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use nix::sched::{sched_getaffinity, sched_setaffinity, CpuSet};
use nix::unistd::Pid;

use super::Seat;

/// The mask every thread protolens spawns adopts — `Some` only once the
/// main thread has actually been narrowed, so [`widen`] costs nothing on
/// the overwhelmingly common machine where [`apply`] declined.
///
/// Under spec 0264 it was the inherited mask, unchanged. Spec 0265
/// subtracts the drawing core from it, and that subtraction is the whole
/// of the reservation: every spawn site already calls [`widen`].
static WORKER: OnceLock<CpuSet> = OnceLock::new();

/// Spec 0269 S1: one seat per physical core of the worker mask, fast
/// cores first. Set exactly when [`WORKER`] is, so a machine spec 0264
/// declined on seats nobody.
static SEATS: OnceLock<Vec<Seat>> = OnceLock::new();

/// Spec 0269 S3: the seat the main thread names once `meanwhile` is
/// done. Set only when spec 0265 reserved a whole physical core, since
/// otherwise there is no core the main thread has to itself to lend.
static DRAWING: OnceLock<Seat> = OnceLock::new();

/// `Pid::from_raw(0)` is "the calling thread" to both affinity calls.
fn me() -> Pid {
    Pid::from_raw(0)
}

pub(super) fn apply() {
    apply_at(Path::new("/sys"));
}

/// The root is a parameter so that the tests can hand it a machine this
/// one is not.
fn apply_at(root: &Path) {
    let Ok(inherited) = sched_getaffinity(me()) else {
        return;
    };
    let Some(plan) = plan(root, &to_set(&inherited)) else {
        return;
    };

    // Spec 0264 S8: `available_parallelism` respects the affinity mask
    // and `available_cpus` caches its answer for the process. Asking now
    // fixes it at the machine's real width; asking first from inside a
    // sweep, after the narrowing below, would clamp every sweep in the
    // session to the size of the fast set.
    let _ = crate::sweep::available_cpus();

    let (Some(main), Some(worker)) = (to_mask(&plan.main), to_mask(&plan.worker)) else {
        return;
    };
    if sched_setaffinity(me(), &main).is_ok() {
        let _ = WORKER.set(worker);
        if !plan.seats.is_empty() {
            let _ = SEATS.set(plan.seats);
        }
        if let Some(drawing) = plan.drawing {
            let _ = DRAWING.set(drawing);
        }
    }
}

/// Everything this machine's topology decides, or `None` if it gives no
/// reason to touch anything.
struct Plan {
    /// Spec 0264/0265: the CPUs the main thread keeps.
    main: BTreeSet<usize>,
    /// Spec 0264 S7: the CPUs every spawned thread adopts through
    /// [`widen`].
    worker: BTreeSet<usize>,
    /// Spec 0269 S1: one seat per physical core of `worker`.
    seats: Vec<Seat>,
    /// Spec 0269 S3: the CPU the main thread lends the sweep, present
    /// only where spec 0265 gave it a whole physical core.
    drawing: Option<Seat>,
}

/// What this machine's topology decides, or `None` if it gives no reason
/// to touch anything.
///
/// Pure, so that the whole decision is testable on a machine that would
/// decline: only the two `sched_setaffinity` calls above are not.
fn plan(root: &Path, inherited: &BTreeSet<usize>) -> Option<Plan> {
    let online = read_cpu_list(&root.join("devices/system/cpu/online"))?;

    // Spec 0264 S4: a mask narrower than the machine means a human, a
    // `taskset` or a container already decided. This is also what keeps
    // `bin/bench` and the repo's `taskset -c 4-7` discipline honest, and
    // why no opt-out environment variable is needed.
    if *inherited != online {
        return None;
    }

    let fast = detect_fast(root, &online)?;

    // Spec 0265: the main thread takes one whole physical core and the
    // workers give it up. Declining leaves spec 0264 intact rather than
    // undoing it — the main thread still gets the fast cluster.
    //
    // Spec 0269 S3: the main thread lends *one* CPU of the core it was
    // given, and only when it was given a whole one. Where spec 0265
    // declined there is no core it has to itself, so there is nothing
    // to lend and the workers are already sitting on the fast cluster.
    let (main, worker, drawing) = match drawing_core(root, &fast) {
        Some(core) => {
            let seat = Seat {
                cpu: *core.first()?,
                fast: true,
            };
            (core.clone(), inherited - &core, Some(seat))
        }
        None => (fast.clone(), inherited.clone(), None),
    };
    Some(Plan {
        // An empty seating declines spec 0269 and nothing else: a
        // machine whose sibling lists are unreadable still gets spec
        // 0264, which needs no topology beyond the fast set.
        seats: seating(root, &worker, &fast).unwrap_or_default(),
        drawing,
        main,
        worker,
    })
}

/// One CPU per physical core of `cpus`, fast cores first and by CPU
/// number within that (spec 0269 S1).
///
/// A core contributes its lowest-numbered CPU — arbitrary, but a
/// function of the topology alone, so two runs on one machine seat the
/// crew identically. `None` if any CPU's sibling list is unreadable or
/// does not name itself, because a partial seating would silently put
/// two workers on one core, which is the whole thing S2 exists to
/// prevent.
fn seating(root: &Path, cpus: &BTreeSet<usize>, fast: &BTreeSet<usize>) -> Option<Vec<Seat>> {
    let mut cores: Vec<BTreeSet<usize>> = Vec::new();
    let mut seats: Vec<Seat> = Vec::new();
    // `cpus` iterates ascending, so the first CPU seen on a core is its
    // lowest-numbered one.
    for cpu in cpus {
        let path = root.join(format!(
            "devices/system/cpu/cpu{cpu}/topology/thread_siblings_list"
        ));
        let siblings = read_cpu_list(&path)?;
        if !siblings.contains(cpu) {
            return None; // A list that omits its own CPU is not one.
        }
        if cores.contains(&siblings) {
            continue;
        }
        cores.push(siblings);
        seats.push(Seat {
            cpu: *cpu,
            fast: fast.contains(cpu),
        });
    }
    // Fast first, so that the workers spawned earliest — which are the
    // ones that draw the first parts — sit on the best cores.
    seats.sort_by_key(|seat| (!seat.fast, seat.cpu));
    Some(seats)
}

pub(super) fn widen() {
    if let Some(worker) = WORKER.get() {
        let _ = sched_setaffinity(me(), worker);
    }
}

pub(super) fn seats() -> Option<&'static [Seat]> {
    SEATS.get().map(Vec::as_slice)
}

pub(super) fn drawing_seat() -> Option<Seat> {
    DRAWING.get().copied()
}

pub(super) fn this_thread() -> i32 {
    nix::unistd::gettid().as_raw()
}

pub(super) fn pin(thread: i32, cpu: usize) {
    let mut mask = CpuSet::new();
    if mask.set(cpu).is_ok() {
        // Spec 0269 S7: a single CPU, so the mask cannot contain the one
        // the thread is already on — which is the only condition under
        // which the kernel moves a running, never-sleeping thread.
        let _ = sched_setaffinity(Pid::from_raw(thread), &mask);
    }
}

/// The physical core to reserve for the main thread, or `None` if this
/// machine cannot afford it (spec 0265 S2, S6).
///
/// The core is the one holding the lowest-numbered fast CPU — an
/// arbitrary choice made deterministic. Two conditions decline:
///
/// 1. it has no SMT sibling, so there is nothing to reserve and
///    same-CPU contention is the scheduler's business;
/// 2. the fast set holds fewer than two physical cores, so reserving
///    one would leave the workers no fast core at all.
fn drawing_core(root: &Path, fast: &BTreeSet<usize>) -> Option<BTreeSet<usize>> {
    let mut cores: Vec<BTreeSet<usize>> = Vec::new();
    for cpu in fast {
        let path = root.join(format!(
            "devices/system/cpu/cpu{cpu}/topology/thread_siblings_list"
        ));
        let siblings = read_cpu_list(&path)?;
        if !siblings.contains(cpu) {
            return None; // A list that omits its own CPU is not one.
        }
        if !cores.contains(&siblings) {
            cores.push(siblings);
        }
    }
    if cores.len() < 2 {
        return None;
    }
    let drawing = cores
        .into_iter()
        .min_by_key(|core| *core.first().expect("a sibling list is never empty"))?;
    (drawing.len() >= 2).then_some(drawing)
}

fn to_mask(cpus: &BTreeSet<usize>) -> Option<CpuSet> {
    let mut mask = CpuSet::new();
    for cpu in cpus {
        mask.set(*cpu).ok()?;
    }
    Some(mask)
}

/// The CPUs the kernel calls fast, or `None` if it does not say.
///
/// Sources in order; the first that names a non-empty *proper* subset of
/// the online CPUs wins. A source that names everything has told us
/// nothing, so it falls through rather than confining the thread to the
/// whole machine.
fn detect_fast(root: &Path, online: &BTreeSet<usize>) -> Option<BTreeSet<usize>> {
    // Intel hybrid: the perf PMU that only the P-cores implement. An
    // explicit statement of core type rather than a proxy for one.
    // Note it has no counterpart worth reading — `cpu_atom` spans a 1.8x
    // internal spread (3.8 GHz E-cores and 2.1 GHz low-power ones on
    // Meteor Lake), so the lists are categorical, not a ranking.
    if let Some(set) = read_cpu_list(&root.join("devices/cpu_core/cpus")) {
        if is_proper_subset(&set, online) {
            return Some(set);
        }
    }
    // arm64 big.LITTLE: a capacity already normalized against 1024, with
    // frequency *and* microarchitecture folded in, so the numbers are
    // comparable to each other in a way raw MHz never is.
    let set = max_capacity_cpus(root, online)?;
    is_proper_subset(&set, online).then_some(set)
}

fn is_proper_subset(set: &BTreeSet<usize>, online: &BTreeSet<usize>) -> bool {
    !set.is_empty() && set.len() < online.len() && set.is_subset(online)
}

/// The online CPUs holding the maximum `cpu_capacity`.
///
/// `None` if any CPU lacks the attribute — a partial ranking is not a
/// ranking — or if every value is equal, which is the kernel saying the
/// machine is uniform rather than that all of it is fast.
fn max_capacity_cpus(root: &Path, online: &BTreeSet<usize>) -> Option<BTreeSet<usize>> {
    let mut caps = Vec::with_capacity(online.len());
    for cpu in online {
        let path = root.join(format!("devices/system/cpu/cpu{cpu}/cpu_capacity"));
        let raw = fs::read_to_string(path).ok()?;
        caps.push((*cpu, raw.trim().parse::<u64>().ok()?));
    }
    let top = caps.iter().map(|(_, c)| *c).max()?;
    if caps.iter().all(|(_, c)| *c == top) {
        return None;
    }
    Some(
        caps.iter()
            .filter(|(_, c)| *c == top)
            .map(|(cpu, _)| *cpu)
            .collect(),
    )
}

fn read_cpu_list(path: &Path) -> Option<BTreeSet<usize>> {
    parse_cpu_list(&fs::read_to_string(path).ok()?)
}

/// The comma-separated range syntax shared by `online`, `cpu_core/cpus`
/// and every other CPU list in sysfs: `0-3,8`.
fn parse_cpu_list(raw: &str) -> Option<BTreeSet<usize>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(BTreeSet::new());
    }
    let mut out = BTreeSet::new();
    for part in raw.split(',') {
        match part.split_once('-') {
            Some((lo, hi)) => {
                let lo: usize = lo.trim().parse().ok()?;
                let hi: usize = hi.trim().parse().ok()?;
                if hi < lo {
                    return None;
                }
                out.extend(lo..=hi);
            }
            None => {
                out.insert(part.trim().parse().ok()?);
            }
        }
    }
    Some(out)
}

fn to_set(mask: &CpuSet) -> BTreeSet<usize> {
    (0..CpuSet::count())
        .filter(|cpu| mask.is_set(*cpu).unwrap_or(false))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(cpus: &[usize]) -> BTreeSet<usize> {
        cpus.iter().copied().collect()
    }

    fn root_with(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("protolens-0264-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (rel, body) in files {
            let path = dir.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        dir
    }

    #[test]
    fn a_cpu_list_parses_ranges_and_singletons() {
        assert_eq!(parse_cpu_list("0-3,8\n"), Some(set(&[0, 1, 2, 3, 8])));
        assert_eq!(parse_cpu_list("5"), Some(set(&[5])));
        assert_eq!(parse_cpu_list("0-13"), Some((0..=13).collect()));
        assert_eq!(parse_cpu_list("  "), Some(BTreeSet::new()));
        // Malformed input is "no answer", never a panic and never a
        // partial list: a half-parsed mask would confine the thread to
        // an arbitrary subset.
        assert_eq!(parse_cpu_list("0-"), None);
        assert_eq!(parse_cpu_list("3-1"), None);
        assert_eq!(parse_cpu_list("0,x"), None);
    }

    #[test]
    fn an_intel_hybrid_root_names_the_p_cores() {
        // The real Meteor Lake layout: 0-3 are the P-cores, and
        // `cpu_atom` covers both the E-cores and the low-power ones.
        let dir = root_with(
            "hybrid",
            &[
                ("devices/cpu_core/cpus", "0-3\n"),
                ("devices/cpu_atom/cpus", "4-13\n"),
            ],
        );
        let online = (0..=13).collect();
        assert_eq!(detect_fast(&dir, &online), Some(set(&[0, 1, 2, 3])));
    }

    #[test]
    fn a_hybrid_list_naming_every_cpu_names_nobody() {
        // A source that covers the machine has told us nothing. Without
        // the proper-subset rule this would "succeed" and then S5 would
        // have to catch it.
        let dir = root_with("hybrid-all", &[("devices/cpu_core/cpus", "0-3\n")]);
        let online = (0..=3).collect();
        assert_eq!(detect_fast(&dir, &online), None);
    }

    #[test]
    fn a_big_little_root_names_the_big_cores() {
        let dir = root_with(
            "big-little",
            &[
                ("devices/system/cpu/cpu0/cpu_capacity", "406\n"),
                ("devices/system/cpu/cpu1/cpu_capacity", "406\n"),
                ("devices/system/cpu/cpu2/cpu_capacity", "1024\n"),
                ("devices/system/cpu/cpu3/cpu_capacity", "1024\n"),
            ],
        );
        let online = (0..=3).collect();
        assert_eq!(detect_fast(&dir, &online), Some(set(&[2, 3])));
    }

    #[test]
    fn a_uniform_capacity_root_names_nobody() {
        let dir = root_with(
            "uniform",
            &[
                ("devices/system/cpu/cpu0/cpu_capacity", "1024\n"),
                ("devices/system/cpu/cpu1/cpu_capacity", "1024\n"),
            ],
        );
        let online = (0..=1).collect();
        assert_eq!(detect_fast(&dir, &online), None);
    }

    #[test]
    fn a_partial_capacity_root_names_nobody() {
        // Half a ranking is not a ranking: cpu1 might be the fast one.
        let dir = root_with(
            "partial",
            &[("devices/system/cpu/cpu0/cpu_capacity", "1024\n")],
        );
        let online = (0..=1).collect();
        assert_eq!(detect_fast(&dir, &online), None);
    }

    #[test]
    fn an_empty_root_names_nobody() {
        let dir = root_with("empty", &[]);
        let online = (0..=3).collect();
        assert_eq!(detect_fast(&dir, &online), None);
    }

    /// The reference host's own topology, as `lscpu` reports it: the
    /// P-cores are SMT pairs and the E-cores are not.
    fn host_root(name: &str) -> std::path::PathBuf {
        let mut files = vec![
            ("devices/system/cpu/online".to_string(), "0-13".to_string()),
            ("devices/cpu_core/cpus".to_string(), "0-3".to_string()),
            ("devices/cpu_atom/cpus".to_string(), "4-13".to_string()),
        ];
        for cpu in 0..=13 {
            let siblings = match cpu {
                0 | 1 => "0-1".to_string(),
                2 | 3 => "2-3".to_string(),
                _ => cpu.to_string(),
            };
            files.push((
                format!("devices/system/cpu/cpu{cpu}/topology/thread_siblings_list"),
                siblings,
            ));
        }
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        root_with(name, &refs)
    }

    /// Spec 0265 S2: the drawing core is a whole physical core, not the
    /// single CPU that names it.
    #[test]
    fn a_drawing_core_is_the_whole_physical_core() {
        let dir = host_root("drawing-core");
        let fast = set(&[0, 1, 2, 3]);
        assert_eq!(drawing_core(&dir, &fast), Some(set(&[0, 1])));
    }

    /// S6 condition 1: with nothing on the sibling there is nothing to
    /// reserve, and same-CPU contention is the scheduler's business.
    #[test]
    fn a_single_threaded_fast_core_reserves_nothing() {
        let dir = root_with(
            "no-smt",
            &[
                ("devices/system/cpu/cpu0/topology/thread_siblings_list", "0"),
                ("devices/system/cpu/cpu1/topology/thread_siblings_list", "1"),
            ],
        );
        assert_eq!(drawing_core(&dir, &set(&[0, 1])), None);
    }

    /// S6 condition 2: reserving the only fast core would leave the
    /// workers none, trading a throughput collapse for a latency gain.
    #[test]
    fn a_lone_fast_core_reserves_nothing() {
        let dir = host_root("lone-core");
        assert_eq!(drawing_core(&dir, &set(&[0, 1])), None);
    }

    /// S4, the whole of the enforcement: what a spawned thread adopts is
    /// the inherited mask minus the drawing core.
    #[test]
    fn a_worker_mask_excludes_the_drawing_core() {
        let dir = host_root("worker-mask");
        let inherited: BTreeSet<usize> = (0..=13).collect();
        let plan = plan(&dir, &inherited).expect("the host layout is answerable");
        assert_eq!(plan.main, set(&[0, 1]));
        assert_eq!(plan.worker, (2..=13).collect::<BTreeSet<_>>());
    }

    /// Spec 0269 S1: one CPU per *physical* core, the drawing core
    /// absent because the workers gave it up, and the fast seat first.
    #[test]
    fn a_seating_plan_is_one_cpu_per_physical_core() {
        let dir = host_root("seating");
        let inherited: BTreeSet<usize> = (0..=13).collect();
        let plan = plan(&dir, &inherited).expect("the host layout is answerable");

        // Core {2,3} contributes cpu 2 alone — cpu 3 is its hyperthread,
        // and a second worker there would buy 4% throughput for 1.92x
        // the latency on the part each is walking.
        let expected: Vec<Seat> = std::iter::once(Seat { cpu: 2, fast: true })
            .chain((4..=13).map(|cpu| Seat { cpu, fast: false }))
            .collect();
        assert_eq!(plan.seats, expected);

        // Spec 0269 S3: the main thread lends one CPU of core {0,1}.
        assert_eq!(plan.drawing, Some(Seat { cpu: 0, fast: true }));
    }

    /// G2, which is every machine this repo is developed and tested on:
    /// a kernel that names no fast CPUs seats nobody, and spec 0269 is
    /// then not merely inert but absent.
    #[test]
    fn a_silent_kernel_seats_nobody() {
        let dir = root_with("silent", &[]);
        assert!(plan(&dir, &(0..=3).collect()).is_none());

        // And a kernel that says which CPUs are online but nothing about
        // how fast they are — a VM, and the development one in
        // particular.
        let dir = root_with("uniform-host", &[("devices/system/cpu/online", "0-3\n")]);
        assert!(plan(&dir, &(0..=3).collect()).is_none());
    }

    /// Spec 0269 S7, and the one kernel behavior the whole endgame rests
    /// on: `sched_setaffinity` moves a running, never-sleeping thread
    /// only when the new mask excludes the CPU it is already on. [`pin`]
    /// passes a single CPU for exactly this reason.
    #[test]
    fn a_migrated_thread_leaves_its_cpu() {
        use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};

        let cpus = to_set(&sched_getaffinity(me()).expect("read this thread's affinity"));
        let mut it = cpus.iter().copied();
        let (Some(from), Some(to)) = (it.next(), it.next()) else {
            return; // One CPU, so there is nowhere to move to.
        };

        let stop = AtomicBool::new(false);
        let thread = AtomicI32::new(0);
        let seen = AtomicUsize::new(from);

        let observed = std::thread::scope(|scope| {
            scope.spawn(|| {
                pin(nix::unistd::gettid().as_raw(), from);
                thread.store(nix::unistd::gettid().as_raw(), Ordering::Release);
                // Spin without ever yielding: a thread that sleeps would
                // be re-placed on wake-up and would prove nothing.
                while !stop.load(Ordering::Acquire) {
                    if let Ok(cpu) = nix::sched::sched_getcpu() {
                        seen.store(cpu, Ordering::Relaxed);
                    }
                }
            });

            while thread.load(Ordering::Acquire) == 0 {
                std::hint::spin_loop();
            }
            pin(thread.load(Ordering::Relaxed), to);

            // Measured at under a millisecond on the reference host; a
            // second is not a race, it is a failure.
            let mut observed = from;
            for _ in 0..1000 {
                observed = seen.load(Ordering::Relaxed);
                if observed != from {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            stop.store(true, Ordering::Release);
            observed
        });

        assert_eq!(
            observed, to,
            "a running thread must leave cpu{from} for cpu{to}"
        );
    }

    /// G1 itself, on a machine that cannot demonstrate it: the fake root
    /// declares this process's *own* CPUs online and half of them fast,
    /// which is the one arrangement under which `apply` acts.
    #[test]
    fn a_declared_fast_set_narrows_this_thread() {
        let before = sched_getaffinity(me()).expect("read this thread's affinity");
        let cpus = to_set(&before);
        if cpus.len() < 2 {
            return;
        }
        let fast: BTreeSet<usize> = cpus.iter().copied().take(cpus.len() / 2).collect();
        let list = |s: &BTreeSet<usize>| {
            s.iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        let dir = root_with(
            "declared",
            &[
                ("devices/system/cpu/online", &format!("{}\n", list(&cpus))),
                ("devices/cpu_core/cpus", &format!("{}\n", list(&fast))),
            ],
        );

        apply_at(&dir);
        let after = to_set(&sched_getaffinity(me()).unwrap());
        sched_setaffinity(me(), &before).expect("restore");

        assert_eq!(after, fast);
    }

    /// S4, and the reason `bin/bench` needs no opt-out: a mask narrower
    /// than the machine is somebody's decision, and outranks ours.
    #[test]
    fn a_narrowed_inherited_mask_is_left_alone() {
        let before = sched_getaffinity(me()).expect("read this thread's affinity");
        let cpus = to_set(&before);
        if cpus.len() < 2 {
            return; // Nothing to narrow to.
        }
        // A machine that *would* answer: online is wider than the mask
        // this thread is about to be given, and cpu 0 is declared fast.
        let dir = root_with(
            "narrowed",
            &[
                ("devices/system/cpu/online", "0-3\n"),
                ("devices/cpu_core/cpus", "0-1\n"),
            ],
        );

        let mut narrow = CpuSet::new();
        narrow.set(*cpus.iter().next().unwrap()).unwrap();
        sched_setaffinity(me(), &narrow).expect("narrow this thread");
        apply_at(&dir);
        let after = to_set(&sched_getaffinity(me()).unwrap());
        sched_setaffinity(me(), &before).expect("restore");

        assert_eq!(after, to_set(&narrow), "apply must not touch a taskset");
    }

    /// The runtime half of S7. Narrowing this thread by hand stands in
    /// for `apply`, which declines on any machine whose kernel is silent
    /// — including every CI box and the development VM.
    #[test]
    fn a_spawned_thread_runs_on_every_cpu_the_process_inherited() {
        let inherited = sched_getaffinity(me()).expect("read this thread's affinity");
        let cpus = to_set(&inherited);
        if cpus.len() < 2 {
            return; // Nothing to narrow to.
        }
        // `WORKER` is process-wide and `apply_at`'s own test writes it
        // too; both write the mask this process inherited, because every
        // fabricated root in this module declines spec 0265.
        let _ = WORKER.set(inherited);

        let mut narrow = CpuSet::new();
        narrow.set(*cpus.iter().next().unwrap()).unwrap();
        sched_setaffinity(me(), &narrow).expect("narrow this thread");

        let seen = std::thread::spawn(|| {
            let before = to_set(&sched_getaffinity(me()).unwrap());
            widen();
            (before, to_set(&sched_getaffinity(me()).unwrap()))
        })
        .join()
        .unwrap();

        sched_setaffinity(me(), WORKER.get().unwrap()).expect("restore");

        // The first half is the hazard itself: a spawned thread really
        // does inherit the narrowing.
        assert_eq!(seen.0.len(), 1, "a spawned thread inherits the mask");
        assert_eq!(seen.1, cpus, "widen() gives the whole machine back");
    }
}
