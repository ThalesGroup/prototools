<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# Criterion Benchmark Process

Benches and profiling are **not** part of the regression test suite.  They are
run manually when investigating performance.

## Running benchmarks

`prototext-core` is a pure Rust library with no `pyo3` dependency.  Benches
can be run without special `RUSTFLAGS`:

```sh
cd /path/to/prototools
cargo bench -p prototext-core --bench <bench_name>
```

Use `-- --list` to verify the bench binary links and lists its targets without
running measurements:

```sh
cargo bench -p prototext-core --bench <bench_name> -- --list
```

`prototext-graph` has a bench of its own for the scoring walk:

```sh
cargo bench -p prototext-graph --bench score
```

It parameterizes on **A**, the number of distinct states alive in the
active set, because that — not the blob's length — is what `score_all`'s
cost scales with: Hopcroft collapses structurally identical roots onto
one state, so A is the number of distinct root *shapes* the blob has not
yet ruled out. Its synthetic graph is built so that no candidate is ever
vetoed and none of them merge, holding A at the full root count for the
whole walk. See spec 0173 for the numbers it was introduced to take.

HTML reports from Criterion runs are written to `target/criterion/`.

## Measurement noise

Earlier runs of this suite were taken on a single-core VM where identical
unchanged code varied 30–45% between consecutive runs, which made
single-run wall-clock comparisons worthless on their own. On the 4-core
configuration used from 2026-07-26 the same check gives 1–4%, so an
effect above roughly 5% is readable directly. Re-run that check (same
bench, twice, no code change) before trusting a number on any new
machine, and pair it with structural evidence — an allocation count from
a counting `GlobalAlloc`, or an operation count — whenever the wall
clock is close.

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
