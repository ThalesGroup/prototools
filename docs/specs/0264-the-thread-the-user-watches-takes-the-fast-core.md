<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0264 — the thread the user watches takes the fast core

Status: implemented
Implemented in: 2026-08-09
App: protolens
Refs: docs/specs/0263-the-machine-sleeps-when-nobody-is-waiting.md (the
        input reader's thread, which this spec deliberately leaves
        alone), docs/specs/0218-the-sweep-is-pulled-from-a-shared-cursor.md
        (`available_cpus`, the sweep's thread count, and the `OnceLock`
        this spec must prime)

## Background

On a heterogeneous machine the core the main thread lands on is worth
more than any recent frame optimization. Measured on the development VM,
one CPU at a time, seven interleaved runs each under
`flock /tmp/prototools-bench.lock`:

| workload | fast CPU | slow CPU | ratio |
|---|---|---|---|
| startup, `--jobs 1 --raw`, googleapis.desc | 1.163 s (cpu 0) | 1.498 s (cpu 4) | **1.29x** |
| median frame, 200x50, 60 PageDowns | 434 / 441 µs (cpu 0) | 1965 / 1285 µs (cpu 4) | **3–4.5x** |

The frame gap is the wider one because a frame is short enough to sit
inside the fast core's turbo window.

The main thread draws every frame, and after spec 0263 it is the only
thread that runs at all while the user reads. It is the one thread whose
placement the user can feel.

### The two machines this has to work on

The **host** is an Intel Core Ultra 7 165U (Meteor Lake), kernel 6.12,
`online` = `0-13`, three tiers:

| CPUs | clock | topology | `cpu_atom`/`cpu_core` |
|---|---|---|---|
| 0-3 | 4.9 GHz | 2 cores, SMT pairs `0-1` and `2-3` | `cpu_core` |
| 4-11 | 3.8 GHz | 8 single-threaded cores | `cpu_atom` |
| 12-13 | 2.1 GHz | 2 single-threaded low-power cores | `cpu_atom` |

`/sys/devices/cpu_core/cpus` reads `0-3`, so S2's first source answers
outright. Note that `cpu_atom` spans a 1.8x internal spread: the PMU
lists are categorical, not a ranking. This spec needs only the fast set,
so that is sufficient — but nothing finer can be built on them.

The **development VM** passes the first eight of those CPUs through
one-to-one, with the same SMT pairing, which is why 4900/3800 = 1.289
and the measured startup ratio is 1.288. The VM is not distorting the
silicon; it is only failing to *describe* it. It exposes no
`cpu_core`/`cpu_atom`, no `cpu_capacity`, no `cpufreq`, and its topology
is actively wrong — `cpu0/topology/thread_siblings_list` reads `0` and
`cpu4/topology/physical_package_id` reads `4`, i.e. eight
single-threaded CPUs in eight packages. So the VM yields nothing and
protolens does nothing on it, which is the intended outcome rather than
a shortfall.

The host also sets the scale of the hazard S7 and S8 exist to prevent:
the fast set is **4 CPUs out of 14**. A narrowing that leaked into the
sweep's workers would cost 70% of the machine to buy a faster frame.

## Goals

- **G1.** Where the kernel *states* which CPUs are the fast ones,
  protolens' main thread runs on them.
- **G2.** Where the kernel says nothing, protolens changes nothing —
  no fallback, no guess, no heuristic.
- **G3.** No thread other than the main thread is constrained, and no
  sweep loses a core to this.
- **G4.** Cost is a handful of small sysfs reads, once. Startup latency
  is unaffected.

## Non-goals

- **N1. No calibration loop.** Timing a fixed workload on each CPU at
  first idle was designed and dropped. Two reasons, either sufficient:
  the environments with no kernel data are virtualized, and there the
  vCPU→pCPU mapping need not be stable, so a sample records where the
  host happened to place a vCPU during those milliseconds rather than a
  property of the CPU; and it burns CPU at precisely the moment spec
  0263 made free.
- **N2. `cpufreq/cpuinfo_max_freq` is not consulted.** A clock is not
  comparable across microarchitectures — an E-core does less per cycle
  than a P-core, so 3.8 against 4.9 GHz does not mean 78%. It also
  conflates a cluster's policy with a core's type, and per-core turbo
  binning (Intel Turbo Boost Max 3.0 favored cores) makes it report a
  two-CPU "fast set" on a machine that is otherwise uniform. Stated
  fairly: on the host it *would* have produced the right partition, and
  it is the only source that separates the 3.8 GHz E-cores from the
  2.1 GHz low-power ones. It is still declined, because S2's sources say
  what a CPU *is* and this one only says how fast it is counted.
- **N3. SMT topology is not consulted *here*.** It governs *throughput*
  — four-thread startup on the VM is 12.910 s pinned to `0-3` against
  9.587 s pinned to `4-7`, because `0-3` is two physical cores wearing
  four CPU numbers — but it does not change how fast one thread runs
  while its sibling is idle, and this spec places one thread.

  It does matter when the sibling is *busy*: a spinner on cpu 1 takes
  cpu 0's frame p90 from 2714/2776 µs to 4961/5192 µs (**1.8x**), where
  the same spinner on a different core costs only 1.15x. Keeping
  protolens' own workers off the sibling while the main thread draws is
  therefore worth roughly 1.8x on heavy frames, and `thread_siblings_list`
  is truthful on the host, so the data is there. It is left to a
  follow-up spec because it needs a mechanism this one does not have —
  the reservation must be *dynamic*. Since spec 0263 the main thread is
  asleep whenever the user is reading, so a permanent reservation would
  idle a fast core through most of a session and shrink the startup
  sweep the user is actually waiting on.
- **N4.** `--jobs`, the worker count and `target_parts` are untouched.
- **N5. Linux only.** `sched_setaffinity` is a Linux interface; macOS
  has no thread-affinity API at all. Every other target compiles the
  module out.
- **N6.** Evaluated once per process. No re-evaluation, no timer.

## Specification

- **S1.** A new `protolens/src/affinity.rs`, compiled only under
  `#[cfg(target_os = "linux")]`, exposing `apply()` and `widen()`.

- **S2. Detection reads two sources, in order; the first that yields a
  non-empty proper subset of the online CPUs wins.**
  1. `/sys/devices/cpu_core/cpus` — the Intel hybrid perf PMU's P-core
     list. Present from Alder Lake on a recent kernel, and an explicit
     statement of core type rather than a proxy for one.
  2. `/sys/devices/system/cpu/cpu*/cpu_capacity` — arm64 big.LITTLE,
     already normalized against 1024. The fast set is the CPUs holding
     the maximum value. **All values equal means no answer**, not "every
     CPU is fast".

  A missing file, an unreadable one or an unparseable one is *no
  answer*, never an error.

- **S3.** All of these files, and `/sys/devices/system/cpu/online`, use
  the same comma-separated range syntax (`0-3,8`). One parser, unit
  tested against fixture strings.

- **S4. An inherited narrowing is final.** `sched_getaffinity` first; if
  the inherited mask is not the whole online set, a human, a `taskset`
  or a container already decided, and protolens does nothing. This is
  what keeps `bin/bench` and the repo's `taskset -c 4-7` discipline
  honest, and it is why no opt-out environment variable is needed.

- **S5.** The detected set is intersected with the inherited mask. An
  empty intersection, or one equal to the mask, means do nothing.

- **S6. Exactly one thread is ever narrowed — the main thread.**
  `sched_setaffinity(0, fast)`, called from `main` before the app is
  built, so that startup runs under it too.

- **S7. Every thread protolens spawns widens itself back to the
  inherited mask as its first statement.** Three sites: the scoped sweep
  worker (`sweep.rs:225`), the heat worker (`heat_worker.rs:1464`) and
  the input reader (`event.rs:366`).

  This is the load-bearing rule of the spec. **An affinity mask is
  inherited across `clone(2)`**, so without it, narrowing the main
  thread confines every sweep worker to the fast cluster and throws the
  rest of the machine away — on the host, 4 CPUs of 14.
  `a_spawned_thread_runs_on_every_cpu_the_process_inherited` asserts the
  inheritance itself before asserting the fix, so the hazard cannot
  quietly stop being real.

- **S8. `sweep::available_cpus()` is primed before the narrowing.**
  `available_parallelism` respects the affinity mask and
  `available_cpus` caches it in a process-wide `OnceLock`
  (`sweep.rs:111`); a first call made after the narrowing would clamp
  every sweep in the session to the size of the fast set. `apply()`
  calls it itself, so no caller can get the ordering wrong.

- **S9. Failure is silent and total.** Any syscall error, parse failure
  or missing file leaves the process exactly as it was found: no
  message, no exit code, no log line. Nobody asked for this and nobody
  should have to diagnose it.

## Alternatives considered

**Calibrate at first idle.** Ruled out by N1. It was the original
proposal and it is the only option that would do anything on the
development VM, which is precisely why it is tempting and why its
failure mode matters: it is least trustworthy in the one environment
where it is the only choice.

**Raise the main thread's `uclamp` minimum** (`sched_setattr` with
`SCHED_FLAG_UTIL_CLAMP_MIN`), which is the interface EAS actually
consumes when choosing a core. Raising a clamp above the default
requires `CAP_SYS_NICE`; an unprivileged CLI cannot.

**`nice`.** Changes the share of a CPU, not which CPU.

**Pin to a single CPU rather than to the set.** No extra clock, and it
forbids the scheduler from stepping off a core that something else has
just landed on.

**Move the input reader to the slow cores.** After spec 0263 that thread
is blocked in `poll(2)` with zero timed wakeups, so its cost at rest is
already zero. There is nothing to move.

**Trust the scheduler and do nothing at all.** The strongest objection:
Linux ITMT and EAS already steer latency-sensitive work toward the fast
cores, with instantaneous load and thermal information this spec does
not have. The answer is the shape of S5 and S6 — protolens acts only
where the kernel has *declared* heterogeneity, and it narrows to a
*cluster*, leaving the scheduler every choice it had within it.

## Test plan

**Not only detection but `apply` itself takes the sysfs root as a
parameter**, so the tests can hand it a machine this one is not. That is
what makes G1 testable on a VM whose kernel says nothing.

1. `a_cpu_list_parses_ranges_and_singletons` — ranges, singletons,
   empty, trailing newline; and `0-`, `3-1`, `0,x` yield `None` rather
   than a panic or a partial list.
2. `an_intel_hybrid_root_names_the_p_cores` — the real Meteor Lake
   layout; `cpu_core` wins and `cpu_atom` is never consulted.
3. `a_hybrid_list_naming_every_cpu_names_nobody` — a source covering the
   whole machine has said nothing, so it must fall through S2 rather
   than be caught later by S5.
4. `a_big_little_root_names_the_big_cores` — mixed capacities yield the
   maximum group only.
5. `a_uniform_capacity_root_names_nobody` — all capacities equal.
6. `a_partial_capacity_root_names_nobody` — one CPU lacks the attribute;
   half a ranking is not a ranking.
7. `an_empty_root_names_nobody` — no source present.
8. `a_declared_fast_set_narrows_this_thread` — **G1**: a fake root
   declares this process's own CPUs online and half of them fast, and
   the thread ends up on that half. The only test that exercises the
   whole path including the syscall.
9. `a_narrowed_inherited_mask_is_left_alone` — **S4**: same fake root,
   but the thread was pinned first, and `apply` leaves it alone.
10. `a_spawned_thread_runs_on_every_cpu_the_process_inherited` — **S7**,
    asserting the inheritance hazard *and* the fix.

## Measured outcome

**Implemented 2026-08-09. G1 is not observable on the development VM**,
which exposes no source, so `apply` declines there and the honest local
result is that nothing changed. Sampling a live session's
`/proc/<pid>/task/*/status` shows all thirteen threads on `0-11`, the
inherited mask, as specified.

The evidence is therefore split three ways:

- **The host answers.** `/sys/devices/cpu_core/cpus` reads `0-3` on the
  Core Ultra 7 165U, so S2's first source resolves and no fallback is
  reached.
- **The syscall path is exercised on the VM anyway**, by pointing
  `apply` at a fabricated root (tests 8 and 9). Both the act and the
  refusal are asserted, not argued.
- **Startup is unaffected** (G4). Where the kernel is silent the whole
  cost is one `sched_getaffinity`, one successful read of `online`, and
  **two** failed opens — `max_capacity_cpus` gives up on the first
  missing `cpu_capacity` rather than walking every CPU. Single-core
  startup remeasured at 0.895 s (cpu 0) / 1.029 s (cpu 4); the absolute
  figures drift substantially between sessions on this VM, so they
  establish the absence of a regression and nothing finer.

`Cargo.lock` is unchanged: `sched` is an existing `nix` feature and
pulls in no crate, so the Nix vendor set was untouched.
