<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0265 — the drawing core belongs to the drawing thread

Status: draft
App: protolens
Refs: docs/specs/0264-the-thread-the-user-watches-takes-the-fast-core.md
        (the detection, `apply`/`widen`, and the inheritance rule this
        spec extends), docs/specs/0263-the-machine-sleeps-when-nobody-is-waiting.md
        (why the main thread is asleep most of a session, which decides
        static against dynamic)

## Background

Spec 0264 puts the main thread on a kernel-declared fast core. It does
not stop protolens' own worker threads from piling onto the *same*
physical core, and on an SMT machine that costs most of what 0264 just
bought.

Measured on the development VM, frames drawn on cpu 0 while a spinner
saturates one other CPU:

| spinner | frame p90 | frame max |
|---|---|---|
| none | 2714 / 2776 µs | 4168 / 4303 µs |
| **cpu 1 — cpu 0's SMT sibling** | 4961 / 5192 µs (**1.8x**) | 6386 / 7060 µs |
| cpu 2 — a different physical core | 3089 / 3185 µs (1.15x) | 4347 / 4623 µs |

A busy sibling costs about five times what the same work costs anywhere
else, which is the ordinary consequence of two hyperthreads sharing one
core's execution resources.

**Read p90 and max, not the median.** The unloaded runs drew ~307 frames
for 60 keystrokes and the sibling-loaded runs exactly 60: starved of
CPU, the app never reached its follow-up repaints, so the cheap frames
left the population entirely. The median ratio looks like 7x and is an
artifact of comparing different *kinds* of frame. Any re-measurement
must check `n` before believing a ratio.

**What is not yet established** is that protolens' own heat workers
reproduce this. The spinner proves the mechanism exists on this
hardware; it does not prove the application triggers it. Confirming that
is the first item of the test plan, and it is a gate, not a formality.

## Goals

- **G1.** While the main thread runs, no *protolens* thread runs on its
  SMT sibling.
- **G2.** The cost is bounded and known: the workers lose one physical
  core, and nothing else changes.
- **G3.** Where the kernel does not describe SMT, or the shape of the
  machine makes the reservation unaffordable, nothing happens — the
  same posture as spec 0264.

## Non-goals

- **N1. Other processes are not excluded.** Keeping a core clear of
  everything on the machine needs `isolcpus`, a cpuset or root. This
  spec governs protolens' own threads, which is where the contention it
  measures comes from.
- **N2. No per-frame arm/disarm.** See the alternative below; it is the
  design the discussion started from and it loses on both cost and
  timeliness.
- **N3. No change to `--jobs`, the worker count or `target_parts`.** The
  workers get a narrower mask, not a smaller pool. Whether the two
  should be linked is a separate question with its own measurement.
- **N4.** Nothing here applies to a machine without SMT, and E-cores are
  single-threaded on every hybrid part shipped so far.

## Specification

- **S1. A third sysfs source: `cpu*/topology/thread_siblings_list`**,
  parsed by spec 0264's existing CPU-list parser. Unreadable or
  unparseable means no answer, as always.

- **S2. The *drawing core* is one physical core of spec 0264's fast
  set** — the one containing its lowest-numbered CPU, chosen only
  because the choice must be deterministic. It is the whole core: on the
  reference host, `thread_siblings_list` for cpu 0 reads `0-1`, so the
  drawing core is `{0, 1}`.

- **S3. The main thread is confined to the drawing core**, narrowing
  spec 0264 S6 from the fast cluster to one core of it. It keeps two
  CPUs to float between, so the scheduler retains a choice; it simply
  cannot leave the core.

- **S4. `affinity::widen()` returns the inherited mask *minus the
  drawing core*.** This is the whole of the enforcement, and it needs no
  new machinery: spec 0264 S7 already requires every spawned thread to
  call it, so redefining what it hands back covers all three spawn sites
  at once.

- **S5. A worker re-applies it at the top of each unit of work** — the
  heat worker after `queue.next_task()` returns (`heat_worker.rs:1153`),
  the sweep worker after it pulls a part (`sweep.rs:234`).

  This is what makes the design cheap. The alternative is for the main
  thread to reach into other threads' masks, which needs a registry of
  worker TIDs, covers only the threads that happen to be alive, and
  races with a `thread::scope` that creates and destroys sweep workers
  freely. Re-applying on the thread itself needs none of that. The cost
  is one `sched_setaffinity` against a unit of work measured in
  hundreds of microseconds at least, and it is skipped entirely when the
  mask has not changed since this thread last set it — a thread-local
  comparison, no syscall.

- **S6. Two conditions, either of which declines the whole spec.**
  1. The drawing core has no sibling. Then there is nothing to reserve,
     and same-CPU contention is the scheduler's job, not ours.
  2. The fast set holds fewer than **two** physical cores. Reserving the
     only fast core would leave the workers none, trading a throughput
     collapse for a latency gain. The reference host has exactly two, so
     it qualifies by the narrowest possible margin, and the workers do
     lose half the fast capacity — stated plainly because it is the
     real price of this spec.

- **S7. Armed from `apply()`, not after the first frame.** Startup's
  main-thread work is latency-critical in the same way a frame is — that
  is what spec 0257 exists for — so there is no window in which the
  reservation should be off. A conditional arming would also be a second
  mechanism to test for a benefit nobody has measured.

## Alternatives considered

**Reserve the sibling only while a frame is being drawn.** The original
proposal. It fails twice. The main thread would have to mutate every
worker's affinity at both ends of a frame — with eleven heat workers on
the reference host, twenty-two syscalls per frame, against a median
frame of 434 µs. And it would be late: `sched_setaffinity` does not
preempt a running thread, so a worker already on the sibling stays there
until the next scheduling point, which is exactly the interval a short
frame lives in. The permanent reservation is both cheaper and timelier;
what it costs instead is a core standing idle while the main thread
sleeps, and since spec 0263 that is most of a session. That cost is
accepted because the core is 2 CPUs of 14 and the work it would have
done while the user reads is speculative prefetch by construction (spec
0250).

**Leave it to the scheduler.** Linux does try to spread threads across
cores before filling siblings. It also does not know that one of our
threads is the one a human is looking at, and it is precisely under load
— when every core has a runnable thread — that it stops having a choice.

**Shrink the worker pool instead** (`--jobs` minus one). Fewer threads
do not stay off a particular core; the scheduler places them wherever it
likes, including the sibling. It also gives up throughput the workers
could still use on the remaining cores.

**Pin the main thread to one CPU and reserve only the other.** Saves
nothing — the sibling has to be cleared either way — and costs the main
thread its ability to step off a CPU something else has landed on.

## Test plan

Item 1 is a gate. If it shows heat workers do not measurably land on the
sibling during real interaction, this spec should be abandoned rather
than implemented.

1. **Confirm the premise on real work, on the host.** Frame-time
   distribution during a scroll burst with a warm read-ahead, with and
   without the reservation, on the Core Ultra 7 165U. Check `n` for each
   run before comparing anything, per the Background. **This cannot be
   measured on the development VM**, whose kernel reports no hybrid PMU
   and claims every CPU is its own single-threaded package — spec 0264
   therefore declines there and so does this.
2. `a_sibling_list_pairs_the_hyperthreads` — the host's real values,
   `0-1` for cpu 0 and `4` for cpu 4.
3. `a_drawing_core_is_the_whole_physical_core` — from a fabricated root
   with the host's layout, the drawing core is `{0, 1}` and not `{0}`.
4. `a_single_threaded_fast_core_reserves_nothing` — S6 condition 1.
5. `a_lone_fast_core_reserves_nothing` — S6 condition 2: a fast set of
   one physical core declines rather than starving the workers.
6. `a_worker_mask_excludes_the_drawing_core` — `widen()` returns
   inherited minus `{0, 1}`, using the fabricated-root technique spec
   0264 introduced so that this is testable on a machine that would
   otherwise decline.
7. `an_unchanged_mask_costs_no_syscall` — S5's thread-local skip, so
   that the per-unit-of-work call cannot regress into a syscall per
   request.

## Measured outcome

Filled in at implementation. It must state the frame-time change from
item 1 **and** the change to sweep throughput, since the workers give up
half the fast cores on the reference host. If the second is worse than
the first is better, the spec has failed and should say so.
