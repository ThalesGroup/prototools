<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0443 — protoback's serial time

Status: draft (P1 and P2 implemented, not measured; the rest planned)
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
- **P3. The allocator, chosen at compile time**, as the hasher is
  (spec 0442 S4): the system's, mimalloc or jemalloc; the default to
  decide by measurement.
- **P4. Each file written by the thread that rendered it.** The main
  thread writes the ~2,600 `.proto` and `.pb` files in sequence after
  rendering; only the warnings need reproto's order, which their replay
  keeps.
- **P5. The setup overlapped.** The options pool, the pool loads, the
  symbol table and the definitions table (0442 S3) are independent:
  built together (`rayon::join`), ~1 s of serial time down to the
  longest of them. Also moves 0442 S3's table off the main thread, the
  ~0.3 s it cost on eight threads.
- **P6. The schema database's scoring graph overlapped with rendering.**
  It needs only the input's descriptors, complete before phase 7.
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
