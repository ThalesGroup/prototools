// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A pool built, then read from every core at once (spec 0424 S19).
//!
//! This is how protoback uses a pool: phases 1–6 build it on one thread,
//! then files are rendered in parallel, each thread asking the pool's
//! definitions everything and decoding messages of its own. The work each
//! thread does must give what it gives on one thread. `scripts/upb-tsan`
//! runs this file with upb's C under ThreadSanitizer, which would report a
//! write upb made on a path that only reads.
//!
//! The descriptor set is `tests/fixtures/options.pb`, or the one
//! `PROTOTEXT_UPB_THREADS_SET` names (googleapis, in the spec's test).
//!
//! No test harness (`harness = false`): libtest runs each test on a thread
//! of its own and collects its result through a std channel, which
//! ThreadSanitizer cannot see the ordering of, and reports. Here the only
//! threads are the ones the checks start and join.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use prototext_upb::{DynamicMessage, Kind, MessageRef, Pool, Value};

const OPTIONS: &[u8] = include_bytes!("fixtures/options.pb");

fn set_bytes() -> Vec<u8> {
    match std::env::var_os("PROTOTEXT_UPB_THREADS_SET") {
        Some(path) => std::fs::read(path).expect("PROTOTEXT_UPB_THREADS_SET"),
        None => OPTIONS.to_vec(),
    }
}

/// Fold a message's set fields and values, recursively, into `h`.
fn walk_message(m: &MessageRef<'_>, h: &mut DefaultHasher) {
    for (field, value) in m.set_fields() {
        field.full_name().hash(h);
        walk_value(&value, h);
    }
}

fn walk_value(v: &Value<'_>, h: &mut DefaultHasher) {
    match v {
        Value::Bool(b) => b.hash(h),
        Value::I32(n) | Value::Enum(n) => n.hash(h),
        Value::I64(n) => n.hash(h),
        Value::U32(n) => n.hash(h),
        Value::U64(n) => n.hash(h),
        Value::F32(x) => x.to_bits().hash(h),
        Value::F64(x) => x.to_bits().hash(h),
        Value::Bytes(b) => b.hash(h),
        Value::Message(Some(m)) => walk_message(m, h),
        Value::Message(None) => 0u8.hash(h),
        Value::List(l) => l.iter().for_each(|e| walk_value(&e, h)),
        Value::Map(map) => map.entries().iter().for_each(|(k, v)| {
            walk_value(k, h);
            walk_value(v, h);
        }),
    }
}

/// Everything a renderer asks of a pool, folded into one number: every
/// definition's questions, every message type's extensions, and the set
/// decoded, walked and encoded again.
fn survey(pool: &Pool, set: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    for m in pool.all_messages() {
        m.full_name().hash(&mut h);
        m.declaration_path().hash(&mut h);
        m.extension_ranges()
            .iter()
            .for_each(|r| (r.start, r.end).hash(&mut h));
        for f in m.fields().chain(m.all_extensions()) {
            (f.number(), f.is_packed(), f.has_presence(), f.is_group()).hash(&mut h);
            (f.json_name(), f.is_group_like(), f.is_map()).hash(&mut h);
            if let Kind::Enum(e) = f.kind() {
                (e.full_name(), e.is_closed()).hash(&mut h);
            }
        }
    }
    for e in pool.all_enums() {
        e.values()
            .for_each(|v| (v.full_name(), v.number()).hash(&mut h));
    }
    for file in pool.files() {
        for s in file.services() {
            for m in s.methods() {
                (m.full_name(), m.input_type().full_name()).hash(&mut h);
            }
        }
    }
    let fds = pool
        .get_message_by_name("google.protobuf.FileDescriptorSet")
        .expect("descriptor.proto is in the set");
    let decoded = DynamicMessage::decode(&fds, set).expect("decodes");
    walk_message(&decoded.as_ref(), &mut h);
    assert_eq!(decoded.encode().unwrap(), set, "encodes back");
    h.finish()
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

/// Run `work` on every core at once, and wait for all of them.
///
/// Spawned and joined rather than scoped: `join` waits in `pthread_join`,
/// which ThreadSanitizer sees, so what a thread did is ordered before what
/// the caller does next. `std::thread::scope` waits with atomics and
/// futexes, which it does not see, and a free after the scope would be
/// reported as racing with the threads' reads (scripts/upb-tsan). The
/// caller keeps its own references until this returns, so the last one,
/// which frees, is dropped after the joins.
fn on_every_core(work: impl Fn() + Send + Sync + 'static) {
    let work = Arc::new(work);
    let handles: Vec<_> = (0..threads())
        .map(|_| {
            let work = Arc::clone(&work);
            std::thread::spawn(move || work())
        })
        .collect();
    for h in handles {
        h.join().expect("a thread panicked");
    }
}

fn a_built_pool_reads_the_same_from_every_thread() {
    let set = Arc::new(set_bytes());
    let pool = Pool::new();
    let report = pool.load_set(&set);
    assert!(report.set_error.is_none() && !report.loaded.is_empty());
    let expected = survey(&pool, &set);

    let (pool2, set2) = (pool.clone(), Arc::clone(&set));
    on_every_core(move || {
        for _ in 0..3 {
            assert_eq!(survey(&pool2, &set2), expected);
        }
    });
    drop((pool, set));
}

fn one_message_reads_the_same_from_every_thread() {
    let set = set_bytes();
    let pool = Pool::new();
    pool.load_set(&set);
    let fds = pool
        .get_message_by_name("google.protobuf.FileDescriptorSet")
        .unwrap();
    let decoded = Arc::new(DynamicMessage::decode(&fds, &set).unwrap());
    let mut h = DefaultHasher::new();
    walk_message(&decoded.as_ref().as_ref(), &mut h);
    let expected = h.finish();

    let (decoded2, set) = (Arc::clone(&decoded), Arc::new(set));
    on_every_core(move || {
        for _ in 0..3 {
            let mut h = DefaultHasher::new();
            walk_message(&decoded2.as_ref().as_ref(), &mut h);
            assert_eq!(h.finish(), expected);
            assert_eq!(decoded2.encode().unwrap(), *set);
        }
    });
    drop(decoded);
}

fn main() {
    let checks: [(&str, fn()); 2] = [
        (
            "a_built_pool_reads_the_same_from_every_thread",
            a_built_pool_reads_the_same_from_every_thread,
        ),
        (
            "one_message_reads_the_same_from_every_thread",
            one_message_reads_the_same_from_every_thread,
        ),
    ];
    for (name, check) in checks {
        check();
        println!("{name} ... ok");
    }
}
