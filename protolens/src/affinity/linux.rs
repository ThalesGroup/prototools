// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The Linux half of protolens' policy (specs 0264, 0265, 0269), on the
//! detection and the operations `prototools-affinity` shares (spec 0444).

use std::collections::BTreeSet;
use std::sync::OnceLock;

use prototools_affinity::{Seat, Topology};

/// The mask every thread protolens spawns adopts — `Some` only once the
/// main thread has actually been narrowed, so [`widen`] costs nothing on
/// the overwhelmingly common machine where [`apply`] declined.
///
/// Under spec 0264 it was the inherited mask, unchanged. Spec 0265
/// subtracts the drawing core from it, and that subtraction is the whole
/// of the reservation: every spawn site already calls [`widen`].
static WORKER: OnceLock<BTreeSet<usize>> = OnceLock::new();

/// Spec 0269 S1: one seat per physical core of the worker mask, fast
/// cores first. Set exactly when [`WORKER`] is, so a machine spec 0264
/// declined on seats nobody.
static SEATS: OnceLock<Vec<Seat>> = OnceLock::new();

/// Spec 0269 S3: the seat the main thread names once `meanwhile` is
/// done. Set only when spec 0265 reserved a whole physical core, since
/// otherwise there is no core the main thread has to itself to lend.
static DRAWING: OnceLock<Seat> = OnceLock::new();

pub(super) fn apply() {
    if let Some(topology) = prototools_affinity::detect() {
        apply_with(&topology);
    }
}

fn apply_with(topology: &Topology) {
    let Some(plan) = plan(topology) else {
        return;
    };
    // Spec 0264 S8: `available_parallelism` respects the affinity mask
    // and `available_cpus` caches its answer for the process. Asking now
    // fixes it at the machine's real width; asking first from inside a
    // sweep, after the narrowing below, would clamp every sweep in the
    // session to the size of the fast set.
    let _ = crate::sweep::available_cpus();
    if prototools_affinity::set_current(&plan.main) {
        let _ = WORKER.set(plan.worker);
        if !plan.seats.is_empty() {
            let _ = SEATS.set(plan.seats);
        }
        if let Some(drawing) = plan.drawing {
            let _ = DRAWING.set(drawing);
        }
    }
}

/// Everything this machine's topology decides.
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

/// What protolens does on this topology. Pure, so that the whole decision
/// is testable on a machine that would decline. The inherited mask is
/// the online CPUs: `detect` declines otherwise (spec 0264 S4).
fn plan(topology: &Topology) -> Option<Plan> {
    let inherited = topology.online();
    let fast = topology.fast();
    // Spec 0265: the main thread takes one whole physical core and the
    // workers give it up. Declining leaves spec 0264 intact rather than
    // undoing it — the main thread still gets the fast cluster.
    //
    // Spec 0269 S3: the main thread lends *one* CPU of the core it was
    // given, and only when it was given a whole one. Where spec 0265
    // declined there is no core it has to itself, so there is nothing
    // to lend and the workers are already sitting on the fast cluster.
    let (main, worker, drawing) = match drawing_core(topology) {
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
        seats: topology.seating(&worker).unwrap_or_default(),
        drawing,
        main,
        worker,
    })
}

pub(super) fn widen() {
    if let Some(worker) = WORKER.get() {
        let _ = prototools_affinity::set_current(worker);
    }
}

pub(super) fn seats() -> Option<&'static [Seat]> {
    SEATS.get().map(Vec::as_slice)
}

pub(super) fn drawing_seat() -> Option<Seat> {
    DRAWING.get().copied()
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
fn drawing_core(topology: &Topology) -> Option<BTreeSet<usize>> {
    let mut cores: Vec<BTreeSet<usize>> = Vec::new();
    for &cpu in topology.fast() {
        let siblings = topology.core_of(cpu)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn set(cpus: &[usize]) -> BTreeSet<usize> {
        cpus.iter().copied().collect()
    }

    fn root_with(name: &str, files: &[(&str, &str)]) -> PathBuf {
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

    /// The reference host's own topology, as `lscpu` reports it: the
    /// P-cores are SMT pairs and the E-cores are not.
    fn host_root(name: &str) -> PathBuf {
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

    fn host(name: &str) -> Topology {
        prototools_affinity::detect_at(&host_root(name), &(0..=13).collect())
            .expect("the host layout is answerable")
    }

    /// Spec 0265 S2: the drawing core is a whole physical core, not the
    /// single CPU that names it.
    #[test]
    fn a_drawing_core_is_the_whole_physical_core() {
        assert_eq!(drawing_core(&host("drawing-core")), Some(set(&[0, 1])));
    }

    /// S6 condition 1: with nothing on the sibling there is nothing to
    /// reserve, and same-CPU contention is the scheduler's business.
    #[test]
    fn a_single_threaded_fast_core_reserves_nothing() {
        let dir = root_with(
            "no-smt",
            &[
                ("devices/system/cpu/online", "0-3"),
                ("devices/cpu_core/cpus", "0-1"),
                ("devices/system/cpu/cpu0/topology/thread_siblings_list", "0"),
                ("devices/system/cpu/cpu1/topology/thread_siblings_list", "1"),
            ],
        );
        let topology = prototools_affinity::detect_at(&dir, &(0..=3).collect()).unwrap();
        assert_eq!(drawing_core(&topology), None);
    }

    /// S6 condition 2: reserving the only fast core would leave the
    /// workers none, trading a throughput collapse for a latency gain.
    #[test]
    fn a_lone_fast_core_reserves_nothing() {
        let dir = host_root("lone-core");
        fs::write(dir.join("devices/cpu_core/cpus"), "0-1").unwrap();
        let topology = prototools_affinity::detect_at(&dir, &(0..=13).collect()).unwrap();
        assert_eq!(drawing_core(&topology), None);
    }

    /// S4, the whole of the enforcement: what a spawned thread adopts is
    /// the inherited mask minus the drawing core.
    #[test]
    fn a_worker_mask_excludes_the_drawing_core() {
        let plan = plan(&host("worker-mask")).expect("the host layout is answerable");
        assert_eq!(plan.main, set(&[0, 1]));
        assert_eq!(plan.worker, (2..=13).collect::<BTreeSet<_>>());
    }

    /// Spec 0269 S1: one CPU per *physical* core, the drawing core
    /// absent because the workers gave it up, and the fast seat first.
    #[test]
    fn a_seating_plan_is_one_cpu_per_physical_core() {
        let plan = plan(&host("seating")).expect("the host layout is answerable");
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

    /// G1 itself, on a machine that cannot demonstrate it: the fake root
    /// declares this process's *own* CPUs online and half of them fast,
    /// which is the one arrangement under which `apply` acts.
    #[test]
    fn a_declared_fast_set_narrows_this_thread() {
        let before = prototools_affinity::current().expect("read this thread's affinity");
        if before.len() < 2 {
            return;
        }
        let fast: BTreeSet<usize> = before.iter().copied().take(before.len() / 2).collect();
        let list = |s: &BTreeSet<usize>| {
            s.iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        let dir = root_with(
            "declared",
            &[
                ("devices/system/cpu/online", &format!("{}\n", list(&before))),
                ("devices/cpu_core/cpus", &format!("{}\n", list(&fast))),
            ],
        );
        let topology = prototools_affinity::detect_at(&dir, &before).unwrap();
        apply_with(&topology);
        let after = prototools_affinity::current().unwrap();
        assert!(prototools_affinity::set_current(&before), "restore");
        assert_eq!(after, fast);
    }

    /// The runtime half of S7. Narrowing this thread by hand stands in
    /// for `apply`, which declines on any machine whose kernel is silent
    /// — including every CI box and the development VM.
    #[test]
    fn a_spawned_thread_runs_on_every_cpu_the_process_inherited() {
        let inherited = prototools_affinity::current().expect("read this thread's affinity");
        if inherited.len() < 2 {
            return; // Nothing to narrow to.
        }
        // `WORKER` is process-wide and `apply_with`'s own test writes it
        // too; both write the mask this process inherited, because every
        // fabricated root in this module that reaches `apply_with`
        // declines spec 0265.
        let _ = WORKER.set(inherited.clone());

        let first = *inherited.iter().next().unwrap();
        assert!(prototools_affinity::set_current(&set(&[first])));

        let seen = std::thread::spawn(|| {
            let before = prototools_affinity::current().unwrap();
            widen();
            (before, prototools_affinity::current().unwrap())
        })
        .join()
        .unwrap();

        assert!(
            prototools_affinity::set_current(WORKER.get().unwrap()),
            "restore"
        );

        // The first half is the hazard itself: a spawned thread really
        // does inherit the narrowing.
        assert_eq!(seen.0.len(), 1, "a spawned thread inherits the mask");
        assert_eq!(seen.1, inherited, "widen() gives the whole machine back");
    }
}
