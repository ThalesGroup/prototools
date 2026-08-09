// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Which CPUs the drawing thread may run on (spec 0264).
//!
//! On a hybrid machine the core the main thread lands on is worth more
//! than any recent frame optimization: measured one CPU at a time on an
//! Intel Core Ultra 7 165U, single-threaded startup on googleapis.desc
//! is 1.163 s on a P-core against 1.498 s on an E-core, and the median
//! frame 434 µs against 1965 µs.
//!
//! So: if the kernel *states* which CPUs are the fast ones, the main
//! thread is confined to them. If it does not, nothing happens — there
//! is no fallback, no benchmark and no guess.

#[cfg(target_os = "linux")]
mod linux;

/// Confine the calling thread to the CPUs the kernel calls fast.
///
/// Called once, from `main`, before the app is built. A no-op unless
/// every condition of spec 0264 S2-S5 holds, and silent either way.
pub(crate) fn apply() {
    #[cfg(target_os = "linux")]
    linux::apply();
}

/// Put the calling thread on the CPUs the *workers* may use.
///
/// **Every thread protolens spawns must call this as its first
/// statement** (spec 0264 S7). An affinity mask is inherited across
/// `clone(2)`, so without it a narrowed main thread confines every
/// worker it spawns to the fast cluster — on the machine above, 4 CPUs
/// out of 14 — and a latency optimization becomes a 70% throughput
/// loss.
///
/// It is not quite the inverse of [`apply`]: under spec 0265 the mask it
/// hands back is everything the process inherited *minus* the physical
/// core the main thread draws on, so that no protolens thread runs on
/// that core's other hyperthread. A busy sibling costs a frame about
/// 1.8x; a busy CPU on any other core costs it 1.15x.
pub(crate) fn widen() {
    #[cfg(target_os = "linux")]
    linux::widen();
}
