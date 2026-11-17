// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A reference to a pool, counted as an `Arc` is, or not counted at all
//! for a leaked pool (spec 0442 S2).
//!
//! Each handle owns one. Counted, cloning and dropping it increment and
//! decrement the pool's reference count, which every thread using the pool
//! shares: on many threads, that one counter is contended. A leaked pool is
//! never freed (one count is kept for ever), so its references need not
//! count: cloning one copies a pointer, dropping one does nothing.
//!
//! Whether a reference counts is its own mark, the pointer's low bit
//! (`PoolInner` is at least 8-aligned), inherited by every reference cloned
//! from it: a reference either increments and decrements, or does neither.
//! A pool-wide flag, read once at clone and once at drop, could read two
//! values and decrement without having incremented.

use std::ops::Deref;
use std::sync::Arc;

use super::PoolInner;

/// The low bit: set, the reference does not count.
const UNCOUNTED: usize = 1;

pub(crate) struct PoolRef(usize);

// SAFETY: a `PoolRef` is a shared reference to a `PoolInner`, which is
// `Send` and `Sync`; counting goes through `Arc`'s atomic operations.
unsafe impl Send for PoolRef {}
unsafe impl Sync for PoolRef {}

impl PoolRef {
    /// A new pool, counted.
    pub(crate) fn new(inner: PoolInner) -> PoolRef {
        let p = Arc::into_raw(Arc::new(inner)) as usize;
        debug_assert_eq!(p & UNCOUNTED, 0, "PoolInner is at least 2-aligned");
        PoolRef(p)
    }

    fn ptr(&self) -> *const PoolInner {
        (self.0 & !UNCOUNTED) as *const PoolInner
    }

    fn counted(&self) -> bool {
        self.0 & UNCOUNTED == 0
    }

    /// This reference's count kept for ever, if it has one, and a reference
    /// that does not count in its place: the pool is never freed.
    pub(crate) fn leak(self) -> PoolRef {
        let p = self.ptr() as usize;
        std::mem::forget(self);
        PoolRef(p | UNCOUNTED)
    }

    /// Whether the reference counts (for tests).
    #[cfg(test)]
    pub(crate) fn is_counted(&self) -> bool {
        self.counted()
    }
}

impl Clone for PoolRef {
    fn clone(&self) -> PoolRef {
        if self.counted() {
            // SAFETY: `ptr` came from `Arc::into_raw`, and this reference
            // holds one count, so the pool is alive.
            unsafe { Arc::increment_strong_count(self.ptr()) };
        }
        PoolRef(self.0)
    }
}

impl Drop for PoolRef {
    fn drop(&mut self) {
        if self.counted() {
            // SAFETY: this reference holds one count, given back here once.
            unsafe { Arc::decrement_strong_count(self.ptr()) };
        }
    }
}

impl Deref for PoolRef {
    type Target = PoolInner;

    fn deref(&self) -> &PoolInner {
        // SAFETY: counted, this reference holds a count; uncounted, the
        // pool was leaked and is never freed.
        unsafe { &*self.ptr() }
    }
}

#[cfg(test)]
mod tests {
    use crate::Pool;

    #[test]
    fn a_leaked_pools_handles_do_not_count_and_outlive_every_pool_value() {
        let pool = Pool::with_descriptor_proto();
        let counted = pool
            .get_message_by_name("google.protobuf.FileDescriptorProto")
            .unwrap();
        assert!(counted.pool.is_counted());
        let leaked = pool.leak();
        let m = leaked
            .get_message_by_name("google.protobuf.FileDescriptorProto")
            .unwrap();
        assert!(!m.pool.is_counted());
        let f = m.get_field(4).unwrap();
        assert!(!f.pool.is_counted(), "a derived handle inherits the mark");
        drop(leaked);
        drop(counted);
        // Every Pool value and counted handle gone: the pool stays.
        assert_eq!(f.name(), "message_type");
        assert_eq!(
            f.containing_message().full_name(),
            "google.protobuf.FileDescriptorProto"
        );
    }

    #[test]
    #[should_panic(expected = "'google/protobuf/descriptor.proto' added to a sealed pool")]
    fn adding_a_file_to_a_sealed_pool_panics_naming_it() {
        let pool = Pool::new();
        pool.seal();
        let file = crate::set::split_set(crate::DESCRIPTOR_SET)
            .unwrap()
            .remove(0);
        let _ = pool.add_file(file);
    }

    #[test]
    fn a_sealed_pool_answers_lookups_without_its_lock() {
        let pool = Pool::with_descriptor_proto();
        pool.seal();
        assert!(pool.is_sealed());
        // The lock held for writing would block a locked read.
        let _write = pool.inner.write();
        assert!(pool
            .get_message_by_name("google.protobuf.FieldOptions")
            .is_some());
    }

    #[test]
    fn a_counted_pool_is_freed_with_its_last_handle() {
        let pool = Pool::with_descriptor_proto();
        let m = pool
            .get_message_by_name("google.protobuf.FileDescriptorProto")
            .unwrap();
        let clone = m.clone();
        drop(pool);
        drop(m);
        assert_eq!(clone.full_name(), "google.protobuf.FileDescriptorProto");
    }
}
