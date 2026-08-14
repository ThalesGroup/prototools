<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# Criterion Benchmark Process

Benches and profiling are **not** part of the regression test suite.  They are
run manually when investigating performance.

## Running benchmarks

Run every measurement through **`bin/bench`**, which is `cargo bench` plus CPU
pinning and a lock (see *Machine configuration* below).  Arguments pass
straight through:

```sh
cd /path/to/prototools
bin/bench -p prototext-core --bench <bench_name>
```

`prototext-core` is a pure Rust library with no `pyo3` dependency, so no
special `RUSTFLAGS` are needed.

Use `-- --list` to verify the bench binary links and lists its targets without
running measurements:

```sh
bin/bench -p prototext-core --bench <bench_name> -- --list
```

`prototext-graph` has a bench of its own for the scoring walk:

```sh
bin/bench -p prototext-graph --bench score
```

It parameterizes on **A**, the number of distinct states alive in the
active set, because that — not the blob's length — is what `score_all`'s
cost scales with: Hopcroft collapses structurally identical roots onto
one state, so A is the number of distinct root *shapes* the blob has not
yet ruled out. Its synthetic graph is built so that no candidate is ever
vetoed and none of them merge, holding A at the full root count for the
whole walk. See spec 0173 for the numbers it was introduced to take.

HTML reports from Criterion runs are written to `target/criterion/`.

## Machine configuration

The development VM is given the host's **eight E-cores, mapped one-to-one**
onto guest vCPUs 0–7.  The host is an Intel Core Ultra 7 165U, whose full
topology is asymmetric, but the slice handed to the VM deliberately is not:

| vCPU | kind | clock | L2 |
| --- | --- | --- | --- |
| 0–3 | E-core cluster | uniform | shared by the four |
| 4–7 | E-core cluster | uniform | shared by the four |

The host's P-cores and its two LP E-cores are **excluded from the VM
entirely**.  A guest scheduler cannot see capacity asymmetry — ITMT/EAS
capacity is not exposed to a VM — so given a mixed set it would happily place
a single-threaded link step on the slowest core and make build times
unpredictable for no gain.  Handing the guest one uniform class removes the
placement lottery at its source rather than compensating for it downstream.

What is left is a single asymmetry, and it is a *cache* one: the two clusters
each have their own L2.

**This structure is invisible from inside the VM.**  Guest sysfs reports every
vCPU with a private 2 MB L2 (`cache/index2/shared_cpu_list` naming only
itself) and empty `thread_siblings_list`/`core_siblings_list`; no `cpufreq` is
exposed at all.  The table above is therefore knowledge about the host, not
something a probe or a program can recover — which also means code that plans
thread placement from kernel-declared topology, such as protolens'
`affinity::plan`, treats all eight vCPUs as unrelated peers here.

**Compilation is unrestricted** across all 8 vCPUs.  **Measurements are
pinned** to the 4–7 cluster, which is exactly one L2: the measurement owns
that cache outright, and nothing running on 0–3 can evict from it.

`bin/bench` defaults to `taskset -c 4`, one core, which is right for the
library benches — they are single-threaded, and confining them leaves the rest
of the cluster's shared L2 alone.  Override for a wallclock measurement of a
genuinely threaded program:

```sh
BENCH_CPUS=4-7 bin/bench -p protolens --bench <name>
```

protolens is the case that needs the whole cluster: it runs three threads, and
its heat worker deliberately does the expensive `score_all` with no lock held
(`heat_worker.rs:387`), so it really does overlap with the TUI thread.

A measurement that needs more than four cores takes **both clusters, whole**
(`BENCH_CPUS=0-7`).  Never straddle the boundary — a range such as `2-5` gets
half of each L2 and adds cross-cluster traffic, which is the worst of both and
is not comparable with either.

### Non-overlap is enforced, not remembered

Compilation being unrestricted means a build running during a measurement
would contend for the pinned cores.  `bin/bench` takes
`$TMPDIR/prototools-bench.lock` **exclusively** and `bin/build` takes it
**shared**, so builds may overlap each other freely but never a measurement.
The lock is machine-global rather than under `target/`, because two worktrees
share one set of physical cores.  `bin/bench`
uses `flock -n`, failing loudly instead of queueing silently behind a long
build; `bin/build` blocks, since waiting out a measurement is what a build
should do.  Use `bin/build` in place of `cargo` for anything that compiles
while a measurement might be running.

`bin/bench` also runs `cargo bench --no-run` unpinned before pinning, because
compiling the bench target on one E-core would take minutes and none of it is
being measured.  That prebuild is a build like any other, so it takes the lock
shared for its duration and drops it before the exclusive acquisition.

`nix-build` is **not** lock-aware.  Do not start one while measuring.

To confirm the vCPU set on a reconfigured machine, time a fixed-work
register-bound spin loop on each vCPU in turn.  Wallclock is inversely
proportional to core clock, so any class mixed into the VM shows up as a
cleanly separated group, and a vCPU that is not there at all fails the
`taskset` outright:

```sh
TIMEFORMAT='%3R'
for c in $(seq 0 13); do printf "vcpu %2d: " "$c"; { time taskset -c $c ./spin; } 2>&1; done
```

Measured 2026-08-14: vCPUs 0–7 at 0.264–0.272 s — a 1.03× spread, i.e. one
uniform class, as intended — and vCPUs 8–13 rejected with `failed to set
affinity: Invalid argument`.  A grouping in this output means the VM has been
given a mixed set and needs reconfiguring, not that the benchmark should route
around it.

The probe says nothing about the L2 clusters: two cores sharing an L2 run a
register-bound loop at identical speed, and, as noted above, the guest cannot
see the sharing either.  Cluster membership has to come from the host
configuration.

For historical context when reading older measurements in this repo: until
2026-08-14 the VM was given host CPUs 0–11, comprising four P-core threads
(0–3, 4.9 GHz) and two E-core clusters (4–7 and 8–11, 3.8 GHz).  The probe
then separated them by 1.30×, matching 4.9/3.8.  Numbers taken under that
configuration on vCPUs 4–7 remain comparable with today's; anything measured
on 0–3, or across 8–11, does not.

## Measurement noise

Earlier runs of this suite were taken on a single-core VM where identical
unchanged code varied 30–45% between consecutive runs, which made
single-run wall-clock comparisons worthless on their own. Under the pinned
configuration above the same check — `packed_vs_expanded`, run twice, no code
change, 2026-07-26 — gives **−3.0% to +1.7%**, so an effect above roughly 5%
is readable directly. Re-run that check before trusting a number on any new
machine, and pair it with structural evidence — an allocation count from a
counting `GlobalAlloc`, or an operation count — whenever the wall clock is
close.

A quiet machine is not the same as a comparable one: a *before/after* pair
must be taken under the same pinning. The cheapest way to get one across a
commit is a worktree, which leaves the main tree untouched:

```sh
git worktree add /tmp/before HEAD~1
# copy in the new bench file if the bench itself is part of the change
(cd /tmp/before && bin/bench -p <crate> --bench <name>)
git worktree remove /tmp/before --force
```

---

## Performance profiling

`perf record` requires `perf_event_paranoid ≤ 1`.  On machines where
`/proc/sys/kernel/perf_event_paranoid = 2` (the default on many Linux
distributions), sampling is blocked for unprivileged users.

Check the current value:

```sh
cat /proc/sys/kernel/perf_event_paranoid
```

If sampling is unavailable, `objdump` disassembly gives equivalent structural
insight for tight inner loops:

```sh
# Find the bench binary (hash suffix changes with each build)
ls -t target/release/deps/<bench_name>-* | grep -v '\.d$' | head -1

# Disassemble
objdump -d --no-show-raw-insn -M intel <binary> | less
```

Useful objdump patterns:
- Look for hot inner loops: tight blocks of arithmetic and branch instructions
  with no function calls.
- `call` instructions inside a loop indicate unexpected allocation or dispatch.
- SIMD instructions (`vmovd`, `vpshufb`, etc.) confirm vectorisation of
  hot string-scanning paths.
