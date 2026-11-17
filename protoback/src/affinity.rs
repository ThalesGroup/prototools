// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! protoback's thread placement (spec 0444 S3): its serial chain (phases
//! 1 to 6, the setup) on the CPUs the kernel calls fast, every thread it
//! starts on every CPU the process inherited. Unlike protolens' (spec
//! 0265), no core is withheld from the workers: the main thread only
//! waits while they render.
//!
//! Inert where the kernel does not say which CPUs are fast, under a
//! `taskset` and in a container (`prototools_affinity::detect`).

use std::collections::BTreeSet;
use std::sync::OnceLock;

/// The CPUs every thread protoback starts adopts: set only once the main
/// thread has been confined, so that [`widen`] costs nothing elsewhere.
static INHERITED: OnceLock<BTreeSet<usize>> = OnceLock::new();

/// The CPUs the kernel calls fast, set with [`INHERITED`].
static FAST: OnceLock<BTreeSet<usize>> = OnceLock::new();

/// Confine the calling (main) thread to the fast CPUs, where the kernel
/// names them. Called once, after the plugin's process has started (it
/// inherits the mask its parent has when it starts), before phase 1.
pub fn apply() {
    let Some(topology) = prototools_affinity::detect() else {
        return;
    };
    if prototools_affinity::set_current(topology.fast()) {
        let _ = INHERITED.set(topology.online().clone());
        let _ = FAST.set(topology.fast().clone());
    }
}

/// Give the calling thread every CPU the process inherited: the first
/// thing every thread protoback starts does, the mask being inherited.
pub fn widen() {
    if let Some(cpus) = INHERITED.get() {
        let _ = prototools_affinity::set_current(cpus);
    }
}

/// `f` run with the calling thread on the fast CPUs, then given every
/// CPU back: a serial chain inside a parallel phase (the scoring graph's
/// merge, spec 0443 P6), while the main thread only waits. The new mask
/// excludes the CPU the thread runs on unless it is a fast one, so the
/// kernel moves it at once (spec 0269 S7).
pub fn on_fast<T>(f: impl FnOnce() -> T) -> T {
    let Some(fast) = FAST.get() else {
        return f();
    };
    let _ = prototools_affinity::set_current(fast);
    let out = f();
    widen();
    out
}
