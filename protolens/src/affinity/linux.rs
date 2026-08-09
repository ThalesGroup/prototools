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

/// The mask to hand back to spawned threads — `Some` only once the main
/// thread has actually been narrowed, so [`widen`] costs nothing on the
/// overwhelmingly common machine where [`apply`] declined.
static INHERITED: OnceLock<CpuSet> = OnceLock::new();

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
    let Some(online) = read_cpu_list(&root.join("devices/system/cpu/online")) else {
        return;
    };

    // S4: a mask narrower than the machine means a human, a `taskset` or
    // a container already decided. This is also what keeps `bin/bench`
    // and the repo's `taskset -c 4-7` discipline honest, and why no
    // opt-out environment variable is needed.
    if to_set(&inherited) != online {
        return;
    }

    let Some(fast) = detect_fast(root, &online) else {
        return;
    };

    // S8: `available_parallelism` respects the affinity mask and
    // `available_cpus` caches its answer for the process. Asking now
    // fixes it at the machine's real width; asking first from inside a
    // sweep, after the narrowing below, would clamp every sweep in the
    // session to the size of the fast set.
    let _ = crate::sweep::available_cpus();

    let mut mask = CpuSet::new();
    for cpu in &fast {
        if mask.set(*cpu).is_err() {
            return;
        }
    }
    if sched_setaffinity(me(), &mask).is_ok() {
        let _ = INHERITED.set(inherited);
    }
}

pub(super) fn widen() {
    if let Some(inherited) = INHERITED.get() {
        let _ = sched_setaffinity(me(), inherited);
    }
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
        let _ = INHERITED.set(inherited);

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

        sched_setaffinity(me(), INHERITED.get().unwrap()).expect("restore");

        // The first half is the hazard itself: a spawned thread really
        // does inherit the narrowing.
        assert_eq!(seen.0.len(), 1, "a spawned thread inherits the mask");
        assert_eq!(seen.1, cpus, "widen() gives the whole machine back");
    }
}
