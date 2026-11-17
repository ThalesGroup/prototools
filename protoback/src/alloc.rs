// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! protoback's global allocator (spec 0443 P3): jemalloc, unless the
//! feature `alloc-system` selects the system's (glibc's malloc). Measured
//! on googleapis: 0.57 s less on one thread and 0.24 s on eight than
//! glibc's, at the same peak memory; mimalloc was slower than both.
//! jemalloc is no feature of its own, so that a build without default
//! features (the workspace's Nix builds) has it. A binary's choice: the
//! libraries protoback uses set none.

#[cfg(not(feature = "alloc-system"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
