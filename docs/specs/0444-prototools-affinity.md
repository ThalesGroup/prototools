<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0444 — prototools-affinity: one detection, two policies

Status: draft
App: prototools-affinity (new crate), protolens, protoback
Refs: docs/specs/0264-the-thread-the-user-watches-takes-the-fast-core.md
        (the detection, the inherited-mask rule, `widen`),
      docs/specs/0265-the-drawing-core-belongs-to-the-drawing-thread.md and
      docs/specs/0269-the-last-part-of-a-sweep-finishes-on-the-best-core.md
        (protolens' policy: the drawing core, the seats),
      docs/specs/0443-protoback-serial-time.md (P10: the measurement
        that asks for it)

## Background

On a hybrid machine the core a critical thread lands on matters.
protolens measured it (spec 0264) and seats its drawing thread on a
fast core; its code, `protolens/src/affinity`, reads which CPUs the
kernel declares fast and confines threads accordingly. protoback now
needs the same: on the development VM, unpinned, its serial phases 1 to
6 take ~0.7 s or ~1.25 s from one run to the next, never in between,
the slow case adding ~0.55 s to a ~3 s run (spec 0443 P10) — the main
thread landing on a fast core or a slow one.

The detection is the same for both; the policies differ:

- **protolens** keeps drawing on its main thread for the whole session:
  a physical core is reserved for it, every other thread is kept off
  that core, and the core is lent to the sweep once the main thread
  blocks (specs 0265, 0269).
- **protoback** has a serial chain (phases 1 to 6, the setup, the
  merge), then a parallel render during which its main thread only
  waits: the chain on fast cores, every other thread on every CPU the
  process inherited, none withheld.

## Goals

- **G1.** The detection and the low-level operations in one crate,
  shared; each tool's policy in the tool.
- **G2.** protolens' behavior unchanged.
- **G3.** protoback's serial chain on fast cores where the kernel says
  which they are; nothing changed where it does not (this VM, CI, a
  `taskset`, a container).
- **G4.** No harder integration: nixpkgs' recipes, the crates.io
  release, the Nix builds.

## Specification

- **S1. The crate `prototools-affinity`** (workspace member, published
  before `prototext-lens`), Linux through `nix`, inert elsewhere:
  - `Topology::detect()`: the online CPUs and those the kernel declares
    fast (Intel's `cpu_core` PMU list, else the maximum
    `cpu_capacity`), or `None` when it says nothing, names every CPU,
    or when the inherited mask is narrower than the online CPUs (a
    human, a `taskset` or a container decided: spec 0264 S4).
    `detect_at(root, inherited)` takes a sysfs root, for tests.
  - `Topology::core_of(cpu)` (the CPUs sharing its physical core) and
    `Topology::seating(cpus)` (one CPU per physical core, fast first:
    spec 0269 S1), both from `thread_siblings_list`.
  - `current()`, `set_current(cpus)`, `this_thread()`, `pin(thread,
    cpu)`: the calling thread's mask, read and set; a thread pinned to
    one CPU (spec 0269 S7).
- **S2. protolens** keeps its `affinity` module's API and behavior: its
  plan (the drawing core, the worker mask without it, the seats, the
  drawing seat) and its statics, now built on S1. Its tests of the
  detection move to the crate; those of its policy stay.
- **S3. protoback's policy** (`protoback/src/affinity.rs`): at startup,
  where `Topology::detect()` answers, `--jobs`'s default computed
  first (`available_parallelism` follows the mask: computed after the
  narrowing, it would be the fast set's size), then the main thread
  confined to the fast CPUs. Every thread protoback starts adopts the
  inherited mask first: the rayon pool's (`start_handler`), P5's scoped
  threads. The plugin's process starts before the confinement, on the
  whole mask: confining after it starts is all it needs.
- **S3b. The merge on the fast CPUs.** The scoring graph's merge (spec
  0443 P6), a serial chain inside the render phase while the main
  thread only waits, runs with its thread confined to the fast CPUs,
  then given every CPU back (`affinity::on_fast`).
- **S4. Integration.** `nix` is already locked: no `cargoHash` changes
  in nixpkgs' recipes, which build the tag's whole source with `-p`.
  The crate joins `scripts/publishable-crates` and
  `nix/crates-io.nix` (patch, package, copy) before `prototext-lens`,
  the dev shell's staleness lists, and `REUSE` as every crate; a first
  release publishes it by hand (`scripts/check-new-crates`, spec 0423
  S8).

## Measurement

On this VM the kernel says nothing (no `cpu_core` device, a uniform
`cpu_capacity`, each vCPU its own core): S3 is inert there, checked for
no change in output. Its mapping is known, though
(docs/prototext/bench-process.md): vCPUs 8–11 are the P-cores' threads,
9 and 11 the fast ones. The crate's feature `sysfs-override` (off by
default, never in a release build; protoback's
`affinity-sysfs-override`) reads the sysfs tree from
`$PROTOTOOLS_AFFINITY_SYSFS`: a fabricated tree declaring 9 and 11 fast,
with the real core pairs, lets the policy act on the VM, unpinned on its
12 vCPUs (pinned to 0–7, `detect` declines: S1). Checked so: the main
thread on 9 and 11 throughout, the render workers and P5's helpers on
0–11, the merge's thread on 9 and 11 while it merges.

## Non-goals

- **N1.** A fallback, a benchmark or a guess where the kernel says
  nothing (spec 0264's rule).
