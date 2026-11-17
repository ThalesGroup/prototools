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
