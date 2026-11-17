// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The Linux half: `sched_getaffinity` and `sched_setaffinity` on the
//! calling thread, or on another by its kernel id.

use std::collections::BTreeSet;

use nix::sched::{sched_getaffinity, sched_setaffinity, CpuSet};
use nix::unistd::Pid;

/// `Pid::from_raw(0)` is "the calling thread" to both affinity calls.
fn me() -> Pid {
    Pid::from_raw(0)
}

pub(crate) fn current() -> Option<BTreeSet<usize>> {
    sched_getaffinity(me()).ok().map(|mask| to_set(&mask))
}

pub(crate) fn set_current(cpus: &BTreeSet<usize>) -> bool {
    to_mask(cpus).is_some_and(|mask| sched_setaffinity(me(), &mask).is_ok())
}

pub(crate) fn this_thread() -> i32 {
    nix::unistd::gettid().as_raw()
}

pub(crate) fn pin(thread: i32, cpu: usize) {
    let mut mask = CpuSet::new();
    if mask.set(cpu).is_ok() {
        let _ = sched_setaffinity(Pid::from_raw(thread), &mask);
    }
}

fn to_mask(cpus: &BTreeSet<usize>) -> Option<CpuSet> {
    let mut mask = CpuSet::new();
    for cpu in cpus {
        mask.set(*cpu).ok()?;
    }
    Some(mask)
}

fn to_set(mask: &CpuSet) -> BTreeSet<usize> {
    (0..CpuSet::count())
        .filter(|cpu| mask.is_set(*cpu).unwrap_or(false))
        .collect()
}
