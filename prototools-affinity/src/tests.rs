// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use super::*;

fn set(cpus: &[usize]) -> BTreeSet<usize> {
    cpus.iter().copied().collect()
}

/// A fabricated sysfs tree, one file per entry.
fn root_with(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prototools-affinity-{name}"));
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
    // Malformed input is "no answer", never a panic and never a partial
    // list: a half-parsed mask would confine a thread to an arbitrary
    // subset.
    assert_eq!(parse_cpu_list("0-"), None);
    assert_eq!(parse_cpu_list("3-1"), None);
    assert_eq!(parse_cpu_list("0,x"), None);
}

#[test]
fn an_intel_hybrid_root_names_the_p_cores() {
    // The real Meteor Lake layout: 0-3 are the P-cores, and `cpu_atom`
    // covers both the E-cores and the low-power ones.
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
    // A source that covers the machine has told us nothing.
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

/// The reference host's own topology, as `lscpu` reports it: the P-cores
/// are SMT pairs and the E-cores are not.
fn host_root(name: &str) -> PathBuf {
    let mut files = vec![
        ("devices/system/cpu/online".to_string(), "0-13".to_string()),
        ("devices/cpu_core/cpus".to_string(), "0-3".to_string()),
        ("devices/cpu_atom/cpus".to_string(), "4-13".to_string()),
    ];
    for cpu in 0..=13 {
        let siblings = match cpu {
            0 | 1 => "0-1".to_string(),
            2 | 3 => "2-3".to_string(),
            _ => cpu.to_string(),
        };
        files.push((
            format!("devices/system/cpu/cpu{cpu}/topology/thread_siblings_list"),
            siblings,
        ));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    root_with(name, &refs)
}

#[test]
fn a_whole_machine_mask_detects_the_fast_cpus() {
    let dir = host_root("detect");
    let t = detect_at(&dir, &(0..=13).collect()).expect("the host layout is answerable");
    assert_eq!(t.online(), &(0..=13).collect::<BTreeSet<_>>());
    assert_eq!(t.fast(), &set(&[0, 1, 2, 3]));
    assert_eq!(t.core_of(1), Some(set(&[0, 1])));
    assert_eq!(t.core_of(7), Some(set(&[7])));
}

/// Spec 0264 S4: a mask narrower than the machine is somebody's decision
/// (a `taskset`, a container), and outranks ours.
#[test]
fn a_narrowed_inherited_mask_detects_nothing() {
    let dir = host_root("narrowed");
    assert!(detect_at(&dir, &(0..=7).collect()).is_none());
}

#[test]
fn a_silent_kernel_detects_nothing() {
    let dir = root_with("silent", &[]);
    assert!(detect_at(&dir, &(0..=3).collect()).is_none());
    // Online CPUs but nothing about their speed: a VM, the development
    // one in particular.
    let dir = root_with("uniform-host", &[("devices/system/cpu/online", "0-3\n")]);
    assert!(detect_at(&dir, &(0..=3).collect()).is_none());
}

/// Spec 0269 S1: one CPU per *physical* core, fast first.
#[test]
fn a_seating_is_one_cpu_per_physical_core_fast_first() {
    let dir = host_root("seating");
    let t = detect_at(&dir, &(0..=13).collect()).unwrap();
    let seats = t.seating(&(2..=13).collect()).unwrap();
    // Core {2,3} contributes cpu 2 alone: cpu 3 is its hyperthread.
    let expected: Vec<Seat> = std::iter::once(Seat { cpu: 2, fast: true })
        .chain((4..=13).map(|cpu| Seat { cpu, fast: false }))
        .collect();
    assert_eq!(seats, expected);
}

#[test]
fn a_thread_confined_and_given_back() {
    let Some(before) = current() else {
        return;
    };
    let Some(&first) = before.iter().next() else {
        return;
    };
    assert!(set_current(&set(&[first])));
    let narrowed = current();
    assert!(set_current(&before), "restore");
    assert_eq!(narrowed, Some(set(&[first])));
}

/// Spec 0269 S7: `sched_setaffinity` moves a running, never-sleeping
/// thread only when the new mask excludes the CPU it is on; [`pin`]
/// passes a single CPU for exactly this reason.
#[cfg(target_os = "linux")]
#[test]
fn a_pinned_running_thread_leaves_its_cpu() {
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};

    let Some(cpus) = current() else {
        return;
    };
    let mut it = cpus.iter().copied();
    let (Some(from), Some(to)) = (it.next(), it.next()) else {
        return; // One CPU, so there is nowhere to move to.
    };

    let stop = AtomicBool::new(false);
    let thread = AtomicI32::new(0);
    let seen = AtomicUsize::new(from);

    let observed = std::thread::scope(|scope| {
        scope.spawn(|| {
            pin(this_thread(), from);
            thread.store(this_thread(), Ordering::Release);
            // Spin without ever yielding: a thread that sleeps would be
            // re-placed on wake-up and would prove nothing.
            while !stop.load(Ordering::Acquire) {
                if let Ok(cpu) = nix::sched::sched_getcpu() {
                    seen.store(cpu, Ordering::Relaxed);
                }
            }
        });

        while thread.load(Ordering::Acquire) == 0 {
            std::hint::spin_loop();
        }
        pin(thread.load(Ordering::Relaxed), to);

        // Measured at under a millisecond on the reference host; a second
        // is not a race, it is a failure.
        let mut observed = from;
        for _ in 0..1000 {
            observed = seen.load(Ordering::Relaxed);
            if observed != from {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        stop.store(true, Ordering::Release);
        observed
    });

    assert_eq!(
        observed, to,
        "a running thread must leave cpu{from} for cpu{to}"
    );
}
