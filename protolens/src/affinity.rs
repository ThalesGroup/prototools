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

/// One CPU a sweep worker owns for as long as it holds it (spec 0269
/// S1).
///
/// A seat is a single CPU and never a set, because a donation re-pins a
/// thread that is running and never sleeps, and the kernel moves such a
/// thread only when the new mask excludes the CPU it is already on
/// (spec 0269 S7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Seat {
    pub(crate) cpu: usize,
    /// Whether the kernel calls this CPU fast — the same `detect_fast`
    /// spec 0264 acts on, so the flag is binary and says nothing finer.
    pub(crate) fast: bool,
}

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

/// One seat per physical core a sweep worker may use, fast cores first
/// (spec 0269 S1).
///
/// `Some` exactly where [`apply`] acted, so the whole of spec 0269 is
/// inert on a VM, in CI, under a `taskset`, and on any machine whose
/// kernel does not say which cores are fast.
pub(crate) fn seats() -> Option<&'static [Seat]> {
    #[cfg(target_os = "linux")]
    {
        linux::seats()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// The CPU the main thread lends the sweep once its own startup work is
/// done (spec 0269 S3).
///
/// One CPU of the physical core spec 0265 reserved for drawing, and
/// `None` where that reservation did not happen. The main thread does
/// not narrow itself to it: it is blocked in `join` for the rest of the
/// sweep, so the seat is a name for a CPU to hand out, not a confinement.
pub(crate) fn drawing_seat() -> Option<Seat> {
    #[cfg(target_os = "linux")]
    {
        linux::drawing_seat()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// The calling thread's kernel id, which is what [`pin`] takes.
pub(crate) fn this_thread() -> i32 {
    #[cfg(target_os = "linux")]
    {
        linux::this_thread()
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// Move `thread` onto `cpu` and nothing else.
///
/// Best-effort and silent (spec 0269 S8): a thread that does not move
/// finishes its part where it is.
pub(crate) fn pin(thread: i32, cpu: usize) {
    #[cfg(target_os = "linux")]
    {
        linux::pin(thread, cpu);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (thread, cpu);
    }
}
