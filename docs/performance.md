<!-- SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis) -->
<!-- SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS -->
<!--
SPDX-License-Identifier: MIT
-->

# prototext performance

*Environment: Linux 6.12.57 x86-64, single core (virtualised).
Criterion: 100 samples, 3 s warmup.
Workload: `descriptor.pb` — a `FileDescriptorSet` with 17 KB binary /
118 KB annotated protoc text / 2,946 fields.*

---

## Benchmark summary

### Path A — binary → protoc text (`decode_and_render`)

| Benchmark | Time (median) | Throughput |
|---|---|---|
| A1 no schema, no annotations | **37 µs** | — |
| A2 schema + annotations | **205 µs** | 80 MiB/s |

Annotations cost **5.5×** the no-annotation baseline.
Schema lookup and annotation string writes account for the majority of the gap.

### Path B — protoc text → binary (`encode_text_to_binary`)

| Benchmark | Time (median) | Throughput |
|---|---|---|
| B1 annotated text (118 KB) | **441 µs** | 255 MiB/s |

Path B is **3.2× faster per byte** than Path A with annotations (255 vs 80 MiB/s),
because it scans pre-formatted text rather than formatting values from scratch.

### Micro-benchmarks (varint)

| Operation | Time |
|---|---|
| `parse_varint` 1-byte | 6.4 ns |
| `parse_varint` 2-byte | 6.9 ns |
| `parse_varint` 10-byte | 11.6 ns |
| `write_varint` into reused `Vec` | **1.7 ns** |
| `write_varint` into fresh `Vec` | 15 ns |
| `encode_varint_bytes(150)` — allocates Vec | 18 ns |

Reusing a write buffer is **10× faster** than allocating a new `Vec` per varint.

---

## perf flat profiles

Profiled with `perf record -e task-clock`.  Criterion itself contributes ~25% of
samples (rayon statistical analysis, `libm exp()`); percentages are of all samples.

### Path A2 — `decode_and_render` with schema + annotations

```
 12.18%  rayon (Criterion overhead)
  8.33%  __ieee754_exp_fma (Criterion overhead)
  6.25%  malloc
  5.54%  _int_free
  4.90%  __memmove_avx_unaligned_erms
  4.24%  render_message
  4.11%  core::fmt::write
  3.28%  String::write_str
  3.06%  RawVecInner::reserve
  3.01%  exp (Criterion overhead)
  2.45%  core::hash::BuildHasher::hash_one
  2.30%  escape_string
  2.14%  alloc::fmt::format
  2.09%  alloc::str::join_generic_copy
  1.84%  render_len_field
  1.55%  core::str::from_utf8
  1.36%  parse_varint
```

Allocator pressure (malloc + free + memmove + realloc): **~24%** of all samples.
`format!` machinery (fmt::write + format + join): **~8%**.

### Path B1 — `encode_text_to_binary`

```
 13.80%  core::slice::memchr::memchr_aligned
 10.89%  rayon (Criterion overhead)
  9.31%  encode_text_to_binary (self)
  8.57%  CharSearcher::next_match
  8.52%  SplitWhitespace::next
  7.43%  __ieee754_exp_fma (Criterion overhead)
  4.14%  parse_annotation
  3.47%  malloc
  3.29%  StrSearcher::new
  2.78%  str::trim_matches
  1.82%  __memmove_avx_unaligned_erms
  1.44%  unescape_bytes
  1.28%  write_varint_ohb
```

Text scanning (memchr + CharSearcher + SplitWhitespace + StrSearcher + trim): **~35%**.
Allocator: **~7%**.

---

## Optimisation history

All measurements on the `descriptor.pb` workload.

### P1–P4 (−23.8% A2, −29.0% B1)

**P1 — direct buffer writes for value formatting (Path A)**

Replaced the `format!("\"{}\"", escape_string(s))` pattern — which allocates an
intermediate `String` for the escape output and another for the `format!` wrapper
— with direct writes to the output `Vec<u8>` via `escape_bytes_into` /
`escape_string_into`.  Eliminated 2 heap allocations per LEN-field value.

**P2 — eliminate the `mods: Vec<String>` pattern (Path A)**

Annotation modifiers were accumulated in a `Vec<String>` then joined:
`mods.join(" ")`.  Replaced with `AnnWriter`, a zero-allocation helper that
writes each modifier directly to the output buffer with an inline separator.
Eliminated 3–6 heap allocations per annotated field.

**P3 — zero-alloc field declaration parsing (Path B)**

`parse_field_decl_into` collected tokens with `split_whitespace().collect::<Vec<&str>>()`,
allocating a fresh `Vec` per annotated line.  Replaced with a lazy
`split_ascii_whitespace()` iterator consuming at most 5 tokens — zero allocation.

Isolated benchmark: manual split is **18× faster** than a compiled regex for
this task (16.9 ns vs 310 ns per 10-token corpus).

**P4 — byte scan in `split_at_annotation` (Path B)**

`line.rfind("   # ")` constructed a `StrSearcher` (Boyer-Moore) per line.
Replaced with `memrchr(b'#')` + 5-byte pattern verify.  Eliminates searcher
construction overhead on every line.

| Benchmark | Baseline | After P1–P4 | Change |
|---|---|---|---|
| A2 decode_and_render (schema + annotations) | 637 µs · 25.8 MiB/s | 485 µs · 33.9 MiB/s | **−23.8%** |
| B1 encode_text_to_binary | 642 µs · 175 MiB/s | 456 µs · 247 MiB/s | **−29.0%** |

---

### P8–P10 (−57.8% A2 vs P1–P4 baseline)

After P1–P4 two String allocations per rendered schema field remained:

1. `field_name() -> String`: either `field_number.to_string()` or
   `field_schema.unwrap().name.clone()` — immediately consumed as `&str`.
2. `field_decl() -> Option<String>`: `format!("{} {}{} = {};", ...)` — one
   per annotated schema field.

**P8** — replaced `field_name() -> String` with `wfl_prefix_n` / `wob_prefix_n`
helpers that write the field name directly to the output buffer via
`out.extend_from_slice(fi.name.as_bytes())` or `write_dec_u64`.

**P9** — replaced `field_decl() -> Option<String>` with `AnnWriter::push_field_decl`,
which writes the field declaration directly to the output buffer.  `type_str(fi)`
is a `&'static str` match — zero allocation; `fi.type_display_name.as_deref()`
borrows from the schema with no clone.

**P10** — removed scattered `.clone()` / `.to_string()` / `.to_owned()` in
`render_len_field` and `render_group_field` at call sites subsumed by P8.

| Benchmark | After P1–P4 | After P8–P10 | Change vs P1–P4 | Cumulative |
|---|---|---|---|---|
| A2 decode_and_render | 485 µs · 33.9 MiB/s | **205 µs · 80.4 MiB/s** | **−57.8%** | **−67.9%** |
| B1 encode_text_to_binary | 456 µs · 247 MiB/s | 462 µs · 244 MiB/s | +1% (noise) | −28.1% |

---

### P14–P15 (−4.6% B1)

**P14** — added `#[inline]` to 16 cross-module hot-path helpers (`write_varint_ohb`,
`parse_varint`, `escape_bytes_into`, etc.).  Without `#[inline]`, function bodies
are invisible across codegen-unit boundaries, preventing inlining.

**P15** — added `codegen-units = 1` and `lto = "thin"` to `[profile.release]`.
Single-CGU gives the optimiser full intra-crate visibility; thin LTO extends it
across crate boundaries (into `prost`, `memchr`).

| Benchmark | After P8–P10 | After P14–P15 | Change |
|---|---|---|---|
| A2 decode_and_render | 205 µs · 80.4 MiB/s | **205 µs · 80.4 MiB/s** | ~0% (escape loops are the bottleneck) |
| B1 encode_text_to_binary | 462 µs · 244 MiB/s | **441 µs · 255 MiB/s** | **−4.6%** |
| `write_varint` reused Vec | 3.1 ns | **1.7 ns** | **−44%** |

The A2 residual (~200 µs) is dominated by `escape_bytes_into` / `escape_string_into`
— byte-by-byte scanning of all string/bytes field values.  This is inherent to
lossless output and cannot be eliminated without algorithmic changes (e.g. SIMD
escape scanning).

---

## Cumulative journey

### Path A2 (binary → annotated text, 17 KB input)

| Checkpoint | Time | Throughput | Δ |
|---|---|---|---|
| Original | 637 µs | 25.8 MiB/s | — |
| After P1–P4 | 485 µs | 33.9 MiB/s | −23.8% |
| After P8–P10 | 205 µs | 80.4 MiB/s | −57.8% |
| After P14–P15 | **205 µs** | **80 MiB/s** | ~0% |
| **Total** | | | **−67.9%** |

### Path B1 (annotated text → binary, 118 KB input)

| Checkpoint | Time | Throughput | Δ |
|---|---|---|---|
| Original | 636 µs | 177 MiB/s | — |
| After P3–P4 | 456 µs | 247 MiB/s | −28.3% |
| After P8–P10 | 462 µs | 244 MiB/s | +1% (noise) |
| After P14–P15 | **441 µs** | **255 MiB/s** | −4.6% |
| **Total** | | | **−30.7%** |

---

## Remaining bottlenecks

### Path A — residual ~200 µs

| Bottleneck | Share | Notes |
|---|---|---|
| `escape_bytes_into` / `escape_string_into` | largest | byte-by-byte scan of all string/bytes values; unavoidable for lossless output |
| `parse_varint` | modest | already tight; inherent per-field cost |
| HashMap lookups (`schema.fields.get`, `all_schemas.get`) | < 1% | P11 (FxHashMap) would save < 1% |
| Criterion framework overhead | ~25% of samples | not real codec cost |

### Path B — remaining opportunities

| Proposal | Estimated gain | Status |
|---|---|---|
| P12 — byte-level trim / ends_with in parse loop | ~4–6% on B1 | Pending |

P12 replaces `str::trim_end()`, `ends_with(char)`, `starts_with(char)` (which use
`CharSearcher`, a Unicode-aware state machine) with direct byte comparisons.
For ASCII-only annotation content these are equivalent and ~3–5× cheaper.

---

## Earlier analysis: `ingest_pb` / `encode_to_binary` paths

These numbers come from an earlier profiling round on the `ingest_pb` /
`encode_to_binary` inner functions (before the single-pass render path existed).
They remain useful context for the binary decode/encode kernel.

### Criterion results (`ingest_pb`)

| Input | Mean | Throughput |
|---|---|---|
| 50-field structured | 4.85 µs | 112 MiB/s |
| 200-field structured | 17.75 µs | 127 MiB/s |
| Random bytes 64 B | 60.9 ns | 1,003 MiB/s |
| Random bytes 1024 B | 70.0 ns | 13.6 GiB/s |

Random bytes parse very quickly because most bytes are consumed as garbage on the
first invalid wire tag.  The structured path (~115 MiB/s) is the meaningful
real-world figure: ~96 ns per field for a 50-field message.

### Criterion results (`encode_to_binary`)

| Input | Mean | Throughput |
|---|---|---|
| 50-field structured | 4.25 µs | 128 MiB/s |
| 200-field structured | 15.95 µs | 142 MiB/s |

Nearly symmetric with `ingest_pb` — both paths have similar per-field allocation
cost.

### Hot functions (perf, `ingest_pb` bench)

After removing Criterion overhead (~27%):

| Symbol | Approx % (codec only) | Notes |
|---|---|---|
| `__memmove_avx_unaligned_erms` | ~21% | Vec copies from allocate-copy-free pattern |
| `parse_wiretag` | ~15% | called once per field |
| `ingest_pb` + `parse_message` | ~16% | dispatch and field loop |
| `parse_varint` | ~6% | called twice per field |
| `malloc` + `free` + realloc | ~11% | heap allocator overhead |
| `drop_in_place<ProtoTextContent>` | ~2% | destructors |

The `memmove` (21%) and allocator (11%) costs were driven by `encode_varint_bytes`
returning a fresh `Vec<u8>` per call, immediately `extend_from_slice`d into a
parent buffer.  This was replaced by `write_varint_ohb(value, ohb, &mut out)` —
an in-place write with zero allocation.
