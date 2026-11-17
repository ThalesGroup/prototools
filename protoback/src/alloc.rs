// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! protoback's global allocator, chosen when it is compiled (spec 0443
//! P3): the system's (glibc's malloc), unless the feature `alloc-mimalloc`
//! or `alloc-jemalloc` is enabled; `alloc-jemalloc` wins over
//! `alloc-mimalloc`. glibc's `malloc_consolidate` and `unlink_chunk` are
//! near the top of the profile on every thread.

#[cfg(feature = "alloc-jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "alloc-mimalloc", not(feature = "alloc-jemalloc")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
