// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Which CPUs the kernel calls fast, and threads confined to them (spec
//! 0444): the detection and the low-level operations protolens and
//! protoback share. Each tool keeps its own policy, what to confine and
//! when.
//!
//! The rule is spec 0264's: if the kernel *states* which CPUs are the
//! fast ones, they can be used; if it does not, [`detect`] answers `None`
//! and nothing happens — no fallback, no benchmark, no guess. Linux only;
//! elsewhere every call is inert.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
mod linux;

/// One CPU a worker owns while it holds it (spec 0269 S1): a single CPU,
/// never a set, so that pinning a running thread to it moves the thread
/// (spec 0269 S7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seat {
    pub cpu: usize,
    /// Whether the kernel calls this CPU fast: binary, nothing finer.
    pub fast: bool,
}

/// What the kernel says of this machine's CPUs, when it says something.
#[derive(Clone, Debug)]
pub struct Topology {
    root: PathBuf,
    online: BTreeSet<usize>,
    fast: BTreeSet<usize>,
}

/// This machine's topology, or `None` when there is no reason to touch
/// anything: the kernel names no fast CPUs, names all of them, or the
/// calling thread's mask is narrower than the online CPUs (a human, a
/// `taskset` or a container decided: spec 0264 S4).
///
/// Read from `/sys`; with the feature `sysfs-override`, from the tree
/// `$PROTOTOOLS_AFFINITY_SYSFS` names, if set (measurements only).
pub fn detect() -> Option<Topology> {
    let inherited = current()?;
    detect_at(&sysfs_root(), &inherited)
}

fn sysfs_root() -> PathBuf {
    #[cfg(feature = "sysfs-override")]
    if let Some(root) = std::env::var_os("PROTOTOOLS_AFFINITY_SYSFS") {
        return PathBuf::from(root);
    }
    PathBuf::from("/sys")
}

/// [`detect`] on the sysfs tree `root`, for a calling thread whose mask
/// is `inherited`: the decision is pure, and testable on a machine that
/// would decline.
pub fn detect_at(root: &Path, inherited: &BTreeSet<usize>) -> Option<Topology> {
    let online = read_cpu_list(&root.join("devices/system/cpu/online"))?;
    if *inherited != online {
        return None;
    }
    let fast = detect_fast(root, &online)?;
    Some(Topology {
        root: root.to_path_buf(),
        online,
        fast,
    })
}

impl Topology {
    /// The online CPUs.
    pub fn online(&self) -> &BTreeSet<usize> {
        &self.online
    }

    /// The CPUs the kernel calls fast: a non-empty proper subset of the
    /// online ones.
    pub fn fast(&self) -> &BTreeSet<usize> {
        &self.fast
    }

    /// The CPUs of `cpu`'s physical core (its SMT siblings and itself),
    /// or `None` if the kernel's list is unreadable or omits `cpu`.
    pub fn core_of(&self, cpu: usize) -> Option<BTreeSet<usize>> {
        let path = self.root.join(format!(
            "devices/system/cpu/cpu{cpu}/topology/thread_siblings_list"
        ));
        let siblings = read_cpu_list(&path)?;
        // A list that omits its own CPU is not one.
        siblings.contains(&cpu).then_some(siblings)
    }

    /// One CPU per physical core of `cpus`, fast cores first and by CPU
    /// number within that (spec 0269 S1); a core contributes its
    /// lowest-numbered CPU. `None` if any sibling list is unreadable: a
    /// partial seating would put two workers on one core.
    pub fn seating(&self, cpus: &BTreeSet<usize>) -> Option<Vec<Seat>> {
        let mut cores: Vec<BTreeSet<usize>> = Vec::new();
        let mut seats: Vec<Seat> = Vec::new();
        for &cpu in cpus {
            let siblings = self.core_of(cpu)?;
            if cores.contains(&siblings) {
                continue;
            }
            cores.push(siblings);
            seats.push(Seat {
                cpu,
                fast: self.fast.contains(&cpu),
            });
        }
        seats.sort_by_key(|seat| (!seat.fast, seat.cpu));
        Some(seats)
    }
}

/// The CPUs the kernel calls fast, or `None` if it does not say.
///
/// Sources in order; the first that names a non-empty *proper* subset of
/// the online CPUs wins: one that names everything has told us nothing.
fn detect_fast(root: &Path, online: &BTreeSet<usize>) -> Option<BTreeSet<usize>> {
    // Intel hybrid: the perf PMU only the P-cores implement, an explicit
    // statement of core type. `cpu_atom` has no use: it spans 3.8 GHz
    // E-cores and 2.1 GHz low-power ones on Meteor Lake.
    if let Some(set) = read_cpu_list(&root.join("devices/cpu_core/cpus")) {
        if is_proper_subset(&set, online) {
            return Some(set);
        }
    }
    // arm64 big.LITTLE: a capacity normalized against 1024, frequency
    // and microarchitecture folded in.
    let set = max_capacity_cpus(root, online)?;
    is_proper_subset(&set, online).then_some(set)
}

fn is_proper_subset(set: &BTreeSet<usize>, online: &BTreeSet<usize>) -> bool {
    !set.is_empty() && set.len() < online.len() && set.is_subset(online)
}

/// The online CPUs holding the maximum `cpu_capacity`; `None` if any CPU
/// lacks it (a partial ranking is not a ranking) or all are equal (the
/// kernel saying the machine is uniform).
fn max_capacity_cpus(root: &Path, online: &BTreeSet<usize>) -> Option<BTreeSet<usize>> {
    let mut caps = Vec::with_capacity(online.len());
    for cpu in online {
        let path = root.join(format!("devices/system/cpu/cpu{cpu}/cpu_capacity"));
        let raw = std::fs::read_to_string(path).ok()?;
        caps.push((*cpu, raw.trim().parse::<u64>().ok()?));
    }
    let max = caps.iter().map(|&(_, c)| c).max()?;
    let min = caps.iter().map(|&(_, c)| c).min()?;
    if max == min {
        return None;
    }
    Some(
        caps.into_iter()
            .filter(|&(_, c)| c == max)
            .map(|(cpu, _)| cpu)
            .collect(),
    )
}

fn read_cpu_list(path: &Path) -> Option<BTreeSet<usize>> {
    parse_cpu_list(&std::fs::read_to_string(path).ok()?)
}

/// The range syntax of every CPU list in sysfs: `0-3,8`. Malformed input
/// is no answer, never a partial list.
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

/// The calling thread's CPUs, or `None` where they cannot be read.
pub fn current() -> Option<BTreeSet<usize>> {
    #[cfg(target_os = "linux")]
    {
        linux::current()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Confine the calling thread to `cpus`; whether it was.
pub fn set_current(cpus: &BTreeSet<usize>) -> bool {
    #[cfg(target_os = "linux")]
    {
        linux::set_current(cpus)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cpus;
        false
    }
}

/// The calling thread's kernel id, which is what [`pin`] takes.
pub fn this_thread() -> i32 {
    #[cfg(target_os = "linux")]
    {
        linux::this_thread()
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// Move `thread` onto `cpu` and nothing else; best-effort and silent
/// (spec 0269 S8). A single CPU: the kernel moves a running thread only
/// when the new mask excludes the CPU it is on (spec 0269 S7).
pub fn pin(thread: i32, cpu: usize) {
    #[cfg(target_os = "linux")]
    {
        linux::pin(thread, cpu);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (thread, cpu);
    }
}

#[cfg(test)]
mod tests;
