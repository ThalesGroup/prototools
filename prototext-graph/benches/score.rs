// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

// benches/score.rs — criterion benchmarks for `score_all`, the multi-entry
// scoring walk (spec 0048).
//
// `score_all`'s cost is dominated not by the blob's length but by `A`, the
// number of *distinct states* alive in the active set. Hopcroft minimization
// collapses structurally identical root types onto one state, so `A` is the
// number of distinct root *shapes* the blob has not yet ruled out — which on a
// large corpus (the 24.5 MB descriptor set of docs/scoring-flaws.md) is in the
// thousands near the top of the walk.
//
// The workload therefore parameterizes on root count and holds the blob fixed:
//
//   - every root shares fields 1 (string) and 2 (nested message), so every tag
//     in the blob is a Match for all of them and nothing gets vetoed — the
//     active set stays at full width for the whole walk, which is the state
//     the cost is measured in;
//   - every root additionally declares one field number unique to itself,
//     which never appears in the blob. That is what keeps the roots
//     structurally distinct, so Hopcroft cannot merge them and `A` really is
//     the root count.
//
// DO NOT use this workload to reason about the walk's *allocation* shape.
// That second bullet is what makes `A` controllable, and it is also what makes
// the allocation profile unrepresentative: because Hopcroft can merge nothing,
// every `ActiveEntry` holds exactly one entry and `entries` never spills out of
// its inline buffer — erasing a term that is 17.1% of allocations on a real
// corpus. The workload also holds `A` large against only 64 records, which
// maximizes the O(log A) per-frame `Vec`s that real input barely reaches
// (2 608 `group_by_state` calls across the whole of googleapis). Reading this
// bench as an allocation profile says "the per-frame `Vec`s dominate"; the
// corpus says they are 1.3% and `occurrences` was 81.6%. See spec 0179 and the
// P3 entry of docs/scoring-flaws.md — measure allocations on `googleapis.desc`.
//
// Timing has its own caveat: this target's same-binary noise floor is +15.9%,
// so a single Criterion `--baseline` delta below that is not evidence.
//
// Run with:  cargo bench -p prototext-graph --bench score
// HTML report in:  target/criterion/

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

use prototext_graph::build_scoring_graph::{build_compiled, serial};
use prototext_graph::score::load::LoadedGraph;
use prototext_graph::score::{score_all, score_one, ScoringOpts};

/// Scoring-graph YAML declaring `n` mutually distinct root message types.
fn synthetic_yaml(n: usize) -> String {
    let mut s = String::from("entries:\n");
    for i in 0..n {
        s.push_str(&format!("- Msg{i}\n"));
    }
    s.push_str("messages:\n");
    s.push_str("  Leaf:\n    fields:\n    - number: 1\n      type: uint64\n");
    for i in 0..n {
        // Fields 1, 2 and 3 are shared by every root (so the blob matches all
        // of them); field `100 + i` is unique to this root and never encoded
        // (so Hopcroft keeps them apart without any of them being vetoed).
        //
        // Field 3 is a repeated scalar, i.e. packable: it is what the
        // packed-vs-expanded workload exercises (spec 0175). It is absent from
        // `blob()`, so the pre-existing benchmarks are unaffected beyond one
        // more transition in each root's table.
        //
        // Field 4 is packable *and* `int32`, which is what reaches the
        // per-element check (spec 0288 S8). `uint64` does not: the check is
        // gated on `wire_type` 8 or 9 or a non-empty range, so field 3's run is
        // validated for termination and then not looked at again. That gate is
        // why 35.99% of a real startup was invisible to this bench — the only
        // packed field it had was one the element loop skips.
        s.push_str(&format!(
            "  Msg{i}:\n    fields:\n    \
             - number: 1\n      type: string\n    \
             - number: 2\n      type: message\n      child: Leaf\n      label: repeated\n    \
             - number: 3\n      type: uint64\n      label: repeated\n    \
             - number: 4\n      type: int32\n      label: repeated\n    \
             - number: {}\n      type: uint64\n",
            100 + i
        ));
    }
    s
}

fn build_graph(n: usize) -> LoadedGraph {
    let compiled = build_compiled(&[synthetic_yaml(n)]).expect("build synthetic graph");
    let mut bytes = Vec::from(*b"PTSGRAPH");
    bytes.extend_from_slice(&serial::GRAPH_VERSION.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes()); // reserved
    bytes.extend_from_slice(&24u64.to_le_bytes()); // root offset
    bytes.extend_from_slice(&serial::to_bytes(&compiled).expect("serialize"));
    LoadedGraph::from_static_bytes(Box::leak(bytes.into_boxed_slice())).expect("load graph")
}

fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

/// A blob every synthetic root matches: `records` repetitions of
/// `1: "<string>"` followed by `2 { 1: <varint> }`, plus the two spec-0288 S8
/// density knobs.
///
/// * `packed` — elements in a packed `4: [int32]` run per record, `0` to omit
///   the field. This is the only knob that reaches the per-element varint
///   check, and its payload is re-read by every candidate, so it is the axis
///   the S1 buffer acts on.
/// * `string_len` — bytes in field 1's payload. UTF-8 validity is tested over
///   the whole payload once per candidate, so the S6 memo's saving scales with
///   this and not with the record count.
///
/// Both are reported as *corners*, never averaged into one number: a blob with
/// neither knob raised does not enter either path at all, and one with both
/// raised is not more representative than either, only different.
fn blob(records: usize, packed: usize, string_len: usize) -> Vec<u8> {
    let filler: String = std::iter::repeat_n('x', string_len).collect();
    let mut out = Vec::new();
    for i in 0..records {
        write_varint(&mut out, (1 << 3) | 2);
        write_varint(&mut out, filler.len() as u64);
        out.extend_from_slice(filler.as_bytes());

        write_varint(&mut out, (2 << 3) | 2);
        let mut inner = Vec::new();
        write_varint(&mut inner, 1 << 3);
        write_varint(&mut inner, i as u64);
        write_varint(&mut out, inner.len() as u64);
        out.extend_from_slice(&inner);

        if packed > 0 {
            let mut payload = Vec::new();
            for k in 0..packed {
                // In range for an int32, so the run is accepted and every
                // element is visited — the case with no early break, which is
                // the expensive one and therefore the one to measure.
                write_varint(&mut payload, (k % 1000) as u64);
            }
            write_varint(&mut out, (4 << 3) | 2);
            write_varint(&mut out, payload.len() as u64);
            out.extend_from_slice(&payload);
        }
    }
    out
}

/// Field 3 (`repeated uint64`) carrying `elements` values, in the **expanded**
/// encoding: one varint tag per value.
fn blob_expanded(elements: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..elements {
        write_varint(&mut out, 3 << 3);
        write_varint(&mut out, i as u64);
    }
    out
}

/// The same values as `blob_expanded`, in the **packed** encoding: one LEN tag
/// whose payload is the varint run.
///
/// Before spec 0175 this vetoed every candidate at the first tag, so the walk
/// exited immediately — which is why a before/after on this blob measures an
/// early exit rather than a slowdown. The comparison that carries information
/// is packed vs expanded *after* the fix: both are accepted, so both hold the
/// active set at full width, and packed should stay the cheaper of the two
/// (no tag to parse and no `find_transition` per element).
fn blob_packed(elements: usize) -> Vec<u8> {
    let mut payload = Vec::new();
    for i in 0..elements {
        write_varint(&mut payload, i as u64);
    }
    let mut out = Vec::new();
    write_varint(&mut out, (3 << 3) | 2);
    write_varint(&mut out, payload.len() as u64);
    out.extend_from_slice(&payload);
    out
}

/// Packed vs expanded, across a widening active set (spec 0175).
///
/// Both blobs encode the same 256 values of the same repeated scalar, and
/// neither vetoes, so `A` is the root count in both. Two readings come out of
/// this: whether packed remains cheaper than expanded (spec 0175's S4 claim),
/// and whether the cost in root count is linear or bends.
fn bench_packed_vs_expanded(c: &mut Criterion) {
    const ELEMENTS: usize = 256;
    let packed = blob_packed(ELEMENTS);
    let expanded = blob_expanded(ELEMENTS);
    let opts = ScoringOpts::default();
    let mut g = c.benchmark_group("packed_vs_expanded");

    for &n in &[64usize, 256, 1024, 4096] {
        let graph = build_graph(n);
        g.throughput(Throughput::Bytes(packed.len() as u64));
        g.bench_with_input(BenchmarkId::new("packed", n), &n, |b, _| {
            b.iter(|| score_all(black_box(&packed), graph.graph(), &opts))
        });
        g.throughput(Throughput::Bytes(expanded.len() as u64));
        g.bench_with_input(BenchmarkId::new("expanded", n), &n, |b, _| {
            b.iter(|| score_all(black_box(&expanded), graph.graph(), &opts))
        });
    }

    g.finish();
}

/// The two spec-0288 S8 knobs, at corners of (packed density, string density,
/// root count).
///
/// `packed=0, strings=8` is the pre-0288 workload and enters neither path;
/// raising `packed` is what reaches the per-element check that was 54.1% of a
/// real startup and 0.37% of this bench; raising `string_len` is what makes the
/// UTF-8 test cost anything. Each corner is its own Criterion id — collapsing
/// them into a mean would report a number describing no workload.
fn bench_payload_density(c: &mut Criterion) {
    let opts = ScoringOpts::default();
    let mut g = c.benchmark_group("payload_density");

    for &roots in &[256usize, 1024] {
        let graph = build_graph(roots);
        for &(packed, string_len) in &[(0usize, 8usize), (64, 8), (0, 512), (64, 512)] {
            let pb = blob(64, packed, string_len);
            g.throughput(Throughput::Bytes(pb.len() as u64));
            g.bench_with_input(
                BenchmarkId::new(format!("packed{packed}_str{string_len}"), roots),
                &roots,
                |b, _| b.iter(|| score_all(black_box(&pb), graph.graph(), &opts)),
            );
        }
    }

    g.finish();
}

/// `score_all` against a widening active set — the O(A) vs O(A²) question.
fn bench_score_all_roots(c: &mut Criterion) {
    let pb = blob(64, 0, 8);
    let opts = ScoringOpts::default();
    let mut g = c.benchmark_group("score_all_by_root_count");
    g.throughput(Throughput::Bytes(pb.len() as u64));

    for &n in &[64usize, 256, 1024, 4096] {
        let graph = build_graph(n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| score_all(black_box(&pb), graph.graph(), &opts))
        });
    }

    g.finish();
}

/// The setup-only cost: a blob so short every candidate is settled almost
/// immediately, so what is left is what `score_all` pays *before* it looks at
/// a byte — one `EntryScore` per root.
fn bench_score_all_setup(c: &mut Criterion) {
    let pb = blob(1, 0, 8);
    let opts = ScoringOpts::default();
    let mut g = c.benchmark_group("score_all_setup");

    for &n in &[1024usize, 4096] {
        let graph = build_graph(n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| score_all(black_box(&pb), graph.graph(), &opts))
        });
    }

    g.finish();
}

/// `score_one` on the same graph: a single candidate, so `A == 1` throughout.
/// This isolates the per-record walk cost from anything that scales with the
/// active set.
fn bench_score_one(c: &mut Criterion) {
    let pb = blob(64, 0, 8);
    let opts = ScoringOpts::default();
    let graph = build_graph(1024);
    let mut g = c.benchmark_group("score_one");
    g.throughput(Throughput::Bytes(pb.len() as u64));

    g.bench_function("Msg0 out of 1024 roots", |b| {
        b.iter(|| score_one(black_box(&pb), "Msg0", graph.graph(), &opts))
    });

    g.finish();
}

criterion_group!(
    benches,
    bench_score_all_roots,
    bench_score_all_setup,
    bench_score_one,
    bench_packed_vs_expanded,
    bench_payload_density,
);
criterion_main!(benches);
