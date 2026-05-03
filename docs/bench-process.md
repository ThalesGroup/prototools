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

HTML reports from Criterion runs are written to `target/criterion/`.

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
