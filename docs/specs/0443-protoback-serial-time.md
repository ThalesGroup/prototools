<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0443 — protoback's serial time

Status: draft (P1 and P2 implemented and measured; the rest planned)
App: protoback, prototext-graph
Refs: docs/specs/0442-handles-without-shared-refcounts.md (the contention
        removed; its measured outcome is this spec's starting point),
      docs/specs/0440-one-renaming-table.md (S2: the symbol collision
        check)

## Background

After spec 0442, googleapis (googleapisDb's flags) takes 10.24 s on one
thread and 6.95 s on eight, and uses the same CPU either way: nothing
contends any more. What remains is work done on one thread. On eight
threads (perf, software events, ~8.7 s of samples):

- the main thread, ~3.8 s: the frees at exit ~0.9 s (the symbol table,
  the run, the graph); the options pool ~0.6 s; the pool loads ~0.5 s;
  the 0440 collision check ~0.47 s; the graph ~0.38 s; the symbol table
  ~0.37 s; phase 2 ~0.27 s; the writes of every output file, in order,
  after rendering;
- one thread building the schema database, ~1 s: protoback's
  `schema_db::build` ~0.6 s, prototext-graph ~0.4 s (building, merging
  and minimizing the graph; hashing ~0.1 s of it);
- the seven render workers, ~0.45 s each.

glibc's allocator (`malloc_consolidate`, `unlink_chunk`) is near the top
on every thread; the release profile sets nothing (no LTO, 16 codegen
units).

## Goals

- **G1.** Less serial time on the critical path, each change measured
  on its own.
- **G2.** No output changes: the harness, both modes, identical.

## Plan

In this order, cheapest first; each item measured (M) before the next.

- **P1. The collision check only when a rule renames** (implemented). Spec
  0440 S2's check runs only when a namespace rule rewrites and
  `--keep-descriptor-path` is off: without one, two different input
  names never become one output name (googleapis, ~0.45 s). Both tools.
- **P2. Exit without freeing** (implemented). After a successful run, the
  plugin's process ended and reaped, the standard streams flushed,
  protoback exits without dropping what `main` holds; the system takes
  the memory back at once (~0.9 s). Every output is written and closed
  before; no other destructor acts outside the process. The error paths
  return as before.
- **P3. jemalloc, protoback's allocator** (implemented). Measured below
  against glibc's and mimalloc: jemalloc is faster at the same peak
  memory. It is what a build without features gets (the Nix builds pass
  `--no-default-features`); the feature `alloc-system` selects glibc's.
  Through `tikv-jemallocator`, a 287-line wrapper whose free passes the
  size Rust knows (`sdallocx`), around jemalloc's C (~60,000 lines),
  built by `tikv-jemalloc-sys`: the costs are the build and ~7 MB of
  binary, none at run time. No mature pure-Rust allocator matches it;
  porting jemalloc was dismissed (its size and its years of hardening,
  for ~0.25–0.6 s a run).
  - A global allocator is a binary's choice: the libraries set none, and
    so are unchanged. The other command-line tools (prototext,
    protolens, protoscan, fdp-scan) may opt in the same way, each on a
    measurement of its own. Not the Python extensions: loaded into
    Python's process, where jemalloc through `dlopen` is known to fail
    on some systems (static TLS).
- **P4. Each file written by the thread that rendered it**
  (implemented). The main thread wrote the ~2,600 `.proto` and `.pb`
  files in sequence after rendering; each render's thread now writes its
  own (`write_rendered`: the directory, the descriptor, the text, in
  reproto's order), and the main thread keeps, in file order, what needs
  it: the warnings replayed, the descriptors collected, and the first
  I/O error raised where the sequential writes raised it (a directory's
  before the file's warnings, a file's after). Spec 0437 leaves no two
  files one path.
- **P5. The setup overlapped** (implemented). The pool loads then the
  options pool (which reads them), the symbol table, and each node's
  definition (0442 S3), built side by side on scoped threads when
  `--jobs` is above 1, in the former order on one thread otherwise:
  `--jobs 1` stays one thread. Rayon's global pool would not honor
  `--jobs`. A panic on a helper thread is the run's, as on one thread.
- **P6. The schema database's scoring graph built beside rendering**
  (implemented). Its steps 2 and 3 (each file's scoring graph; the
  merge and minimization, ~0.4 s mostly on one thread) read the input's
  definitions, not the rendered output: they need only the list of
  files, in collection order, which is known before rendering but for
  the files whose render fails. They run ahead, for the summoned files
  then the WKT dependencies, longest chain first: step 2 on every thread
  before rendering (`schema_db::speculate`, ~2,600 tasks each lighter
  than a render), then step 3, the merge on one thread, as the closure
  `rayon::join` runs on its own thread while rendering fills the others
  (`Speculation::compile`). Run among the render tasks instead (the
  first version), step 2's tasks queued behind them and the merge
  started only after rendering: no gain; `build` uses the result when the files collected
  are those. A miss is a render that failed (a file the pool refused
  holding an enum; an option value no renderer can name), and the files
  collected are then the expected ones less those: their step-2 graphs
  are reused, only step 3 runs again (`Speculation::regraph`); a file
  not expected would run both again. Pruning does not miss: the
  expectation is made after it, from the files the renderer takes.
  Measured: the harness's 299 database builds all hit; the miss path is
  unit-tested only, nothing in the corpora triggering it. The speculation prints
  nothing: the phases' labels and the warnings stay in their order.
  Its result carries the error's message, a `Box<dyn Error>` not being
  `Send`. Checked used on the wkt and custom cases.
- **P10. Where the threads run** (next, before P7). The VM's vCPUs are
  of three classes (docs/prototext/bench-process.md): 0–7 E-cores in
  two clusters of four, each with its L2 (3.8 GHz), 8–11 the two
  P-cores' threads (4.9 GHz, uneven between the two threads of a core).
  Every timing above is pinned to 0–7; unpinned, protoback sees 12
  vCPUs, makes 12 threads, and its critical path (phases 1 to 6, the
  setup, the merge: over half the run) runs wherever the system puts
  it. protolens measured 1.29x at startup between two E-cores (spec
  0264) and seats its critical thread on the fast core (specs 0264,
  0269; `protolens/src/affinity`). To explore: `--jobs` 4, 6, 8, 12
  unpinned; the critical path seated on a P-core, the mechanism shared
  with protolens rather than written again; `--jobs` clamped to the CPUs
  the process may use (spec 0217 S4).
- **P11. No text where none is written.** With `--no-proto-out` or
  `--dry-run`, the database's descriptors still come from a full render,
  text included: they are a by-product of it (spec 0429's side-channel),
  and their SourceCodeInfo is synthesized from the text. Its spans then
  point into files that are never written (protolens reads them to open
  a declaration, which needs the file); its comments may keep a value.
  Proposed: no SourceCodeInfo by default when no text is written, and
  the descriptors built without rendering the text. Changes the
  database's bytes in that mode: reproto changed too, or the harness's
  expectation for it. Pairs with P7: then the merge is no longer hidden
  behind rendering.
- **P7. prototext-graph.**
  - Its internal maps use the hasher alias, chosen as protoback's (~0.1
    s). Its public fields (`node_ids`, `states`, `node_kinds`, …) keep
    the standard hasher: a feature changing their type would change it
    for every dependent of the build at once (features are unified),
    breaking whoever builds those structs. Decided 2026-11-17: they stay
    on the standard hasher (std's `RandomState`, SipHash-1-3 keyed at
    random per process). All hashing in that thread, internal and public
    maps together, was ~0.1 s on googleapis, on a thread running beside
    the main one: the public fields' share is not worth a breaking
    change. Should it ever be, FxHash for good, no feature, so that their
    type never depends on the build. `fds_index`'s FxHasher is the format of a file on
    disk, and stays.
  - Scrutiny: where its ~0.4 s goes (build, merge, minimization), and
    what of it can run in parallel (the files' graphs before the merge;
    the minimization's refinement).
  - Profiled 2026-11-17 (googleapis, eight threads, after P6): ~0.39 s
    of CPU in all: building the graph ~0.14 s, Hopcroft's minimization
    ~0.14 s, merging the files ~0.07 s, compiling and serializing the
    rest. Since P6 it runs beside rendering (~1.9 s), off the critical
    path: parallelizing it gains at most the core it holds (~0.2 s)
    there. It is the whole job where rendering shrinks (P11) or is
    absent (a graph built from descriptors alone): after P10 and P11,
    measured on a database-only run. The merge (one table from many)
    likely parallelizes; the minimization (partition refinement) is
    sequential by nature: independent components minimized apart, or
    the per-step bookkeeping in parallel, to study.
- **P8. The input decoded in parallel.** Phase 1 decodes each of the
  input's descriptors (~3,000 for googleapis) one after another; they
  are independent. The whole phase is ~0.3 s: only if it still shows
  once the rest is done.
- **P9. The release profile, last.** `lto = "fat"`, `codegen-units = 1`:
  on every thread count, at the cost of slower release builds; last, so
  that every step before it builds as fast as today. To measure.

## Measurement

As spec 0442's: googleapis with googleapisDb's flags, a release build
with line tables, vCPUs 0-7, host idle (asked first), 1 and 8 threads, 3
runs each, phase by phase (`phases.py`), `perf stat` with software
events, and a profile on eight threads split by thread. Each item's
numbers recorded here as it lands.

## Non-goals

- **N1.** Changing what is written.
- **N2.** reproto's speed: P1 is mirrored there to keep both tools
  alike, nothing else.

## Measured outcome

Each item as it lands, the protocol above (mean of 3, range):

| | 1 thread | 8 threads | after rendering, 8 | CPU, 1 / 8 |
|---|---|---|---|---|
| start (0442 S4) | 10.24 s [9.21–11.51] | 6.95 s [6.75–7.24] | 2.33 s | 9.6 / 9.7 s |
| P1 | 8.73 s [8.64–8.82] | 6.21 s [6.13–6.36] | 2.35 s | 9.2 / 9.1 s |
| P2 | 8.21 s [8.12–8.34] | 5.47 s [5.41–5.52] | 1.59 s | 8.1 / 8.4 s |

P1 saved more than the ~0.45 s the profile gave the check; the start's
one-thread runs spread widely, and part of it may be noise. P2's gain is
where expected: the time after rendering, the frees.

P3, the allocators compared (P2's tree, the features of 00c5164):

| | 1 thread | 8 threads | CPU, 1 / 8 | peak RSS, 8 |
|---|---|---|---|---|
| glibc (P2) | 8.21 s [8.12–8.34] | 5.47 s [5.41–5.52] | 8.1 / 8.4 s | 1.35–1.37 GB |
| mimalloc | 8.86 s [8.76–8.91] | 5.66 s [5.47–5.89] | 8.9 / 8.7 s | 1.39–1.40 GB |
| jemalloc | 7.64 s [7.58–7.70] | 5.23 s [5.10–5.31] | 7.3 / 7.9 s | 1.37 GB |

jemalloc: −0.57 s on one thread, −0.24 s on eight, the ranges apart, the
same peak memory (20 % more page faults). mimalloc: slower than glibc.

jemalloc made the default (9e1c9fe), then P4 (2024333), timed in one
later session:

| | 1 thread | 8 threads | render, 8 | CPU, 1 / 8 |
|---|---|---|---|---|
| jemalloc by default | 7.59 s [7.48–7.67] | 4.88 s [4.74–5.14] | 2.48 s | 7.2 / 7.7 s |
| P4 | 7.53 s [7.45–7.65] | 4.80 s [4.77–4.83] | 2.33 s | 7.2 / 7.8 s |

The default's code is the feature build's, yet eight threads came out
0.35 s faster than in the session before: between sessions, a few
tenths of a second on eight threads is variation, not effect; each
comparison is to be made within one session. P4: the render phase down
0.15 s, the total 0.08 s, within the ranges: the writes were a small
cost; a gain of ~0.1 s is plausible, not established.

P4, P5, P6 in one session:

| | 1 thread | 8 threads | setup / render / after, 8 | CPU, 1 / 8 |
|---|---|---|---|---|
| P4 | 7.64 s [7.43–7.74] | 4.72 s [4.52–4.98] | 1.17 / 2.31 / 1.23 s | 7.4 / 7.6 s |
| P5 | 7.44 s [7.20–7.75] | 4.35 s [4.32–4.38] | 1.20 / 1.92 / 1.24 s | 7.4 / 7.9 s |
| P6 | 7.71 s [7.57–7.81] | 4.48 s [4.26–4.81] | 1.24 / 1.99 / 1.25 s | 7.2 / 8.1 s |

(P5's setup is built after "Marking summoned nodes": it counts in the
render column.) P5: −0.37 s on eight threads. P6: no gain. Its merge
was skipped after rendering (0.41 s from collection to the database
against P5's 0.69 s), but rendering ended later and collection took
longer: `rayon::join` returns when both sides are done, and the
speculation's own parallel loop queues its tasks in the render pool
among the ~2,600 render tasks, so that its merge, on one thread, starts
only once they are all done: the speculation ran after rendering, not
beside it. To try: the speculation on a thread of its own, outside the
render pool; else P6 reverted.

P6 reworked (a4463e5) against P5 and the first P6, in one session that
was disturbed: every binary ~30 % slower than in the session before (P5
6.04 s on eight threads against 4.35 s, its CPU 12.2 s against 7.9 s),
something else using the host or the hypervisor withholding CPU. The
absolute figures are unusable; the comparison within the session:

| | 1 thread | 8 threads | render / collection / merge-to-database, 8 |
|---|---|---|---|
| P5 | 9.78 s [9.72–9.84] | 6.04 s [5.95–6.15] | 2.69 / 0.55 / 1.03 s |
| P6, first | 10.39 s [10.30–10.45] | 5.93 s [5.89–6.02] | 2.88 / 0.72 / 0.48 s |
| P6, reworked | 9.48 s [9.27–9.65] | 5.67 s [5.54–5.89] | 2.82 / 0.52 / 0.54 s |

The reworked P6: −0.37 s on eight threads, the ranges apart; the merge
gone from after rendering (1.03 to 0.54 s), the collection as short as
P5's. Kept provisionally; confirmed below, in a clean session.

Re-timed in a clean session (the hypervisor's steal 0.02 % of the
VM's CPU time over the run):

| | 1 thread | 8 threads | render / merge-to-database, 8 | CPU, 1 / 8 |
|---|---|---|---|---|
| P5 | 7.59 s [7.45–7.77] | 4.49 s [4.32–4.58] | 1.93 / 0.73 s | 7.4 / 8.0 s |
| P6, reworked | 7.70 s [7.38–7.95] | 4.34 s [4.19–4.48] | 2.14 / 0.37 s | 7.6 / 8.3 s |

P6: −0.15 s on eight threads, the ranges overlapping, faster in both
sessions: kept. The merge is gone from after rendering (0.73 to 0.37
s), but holds one of the eight cores while rendering runs, which takes
0.21 s longer. One thread: no change, as expected. The disturbed
session's cause is outside the VM (its load ~0.2, nothing running):
another workload on the host, or the CPU's frequency lowered. Each
timing now records the steal counter (`/proc/stat`) before and after.

P10 (spec 0444, aa4f2bc), unpinned on the VM's 12 vCPUs, one binary
with and without a fabricated sysfs tree declaring vCPUs 9 and 11 fast
(its feature `affinity-sysfs-override`; the VM's own kernel says
nothing), 3 interleaved rounds, steal 19 of 46,943 ticks:

| | total (3 runs) | phases 1–6 (3 runs) | render | after |
|---|---|---|---|---|
| `--jobs 8`, unseated | 3.15 s (3.53, 2.61, 3.33) | 1.20, 0.69, 0.84 s | 1.59 s | 0.65 s |
| `--jobs 8`, seated | 2.68 s (2.67, 2.71, 2.65) | 0.67, 0.68, 0.68 s | 1.43 s | 0.57 s |
| default (12), unseated | 3.38 s (2.43, 3.65, 4.06) | 0.70, 1.20, 1.23 s | 1.66 s | 0.68 s |
| default (12), seated | 2.64 s (2.60, 2.62, 2.71) | 0.67, 0.69, 0.72 s | 1.39 s | 0.56 s |

Seated, phases 1 to 6 always take the fast mode, and the runs no longer
spread: −0.5 to −0.7 s on the mean, the render faster too (the merge on
a fast core). The default thread count needs no change. Measurements
of the steps after this one go back to vCPUs 0–7: uniform and
reproducible, where only the cores' count matters.
