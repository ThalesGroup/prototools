// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

// benches/codec.rs — criterion benchmarks for the hot paths in prototext-core.
//
// Two representative workloads, each profiled in one direction:
//
//   Path A: .pb  →  protoc text   (decode_and_render)
//   Path B: protoc text  →  .pb   (encode_text_to_binary)
//
// The workload is fixtures/descriptor.pb / its protoc text rendering — a
// real, schema-annotated protobuf document (google/protobuf/*.proto well-
// known types, `--include_imports`) with nested messages, repeated fields
// and strings, representative of normal prototext usage.
//
// Ported from a sibling (private) repo's profiling harness — see spec 0110
// Step 7 ("Zero-cost benchmark checkpoint") for context: this bench exists
// to confirm the Sink-based render refactor (spec 0110) did not regress the
// `decode_and_render` hot path relative to the pre-refactor baseline
// recorded in docs/prototext/performance.md (205 µs · 80.4 MiB/s on a 17 KB input).
//
// Micro-benchmarks for the varint and encode_varint hot paths are retained
// for isolation.
//
// Run with:  cargo bench -p prototext-core --bench codec
// HTML report in:  target/criterion/

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

use prototext_core::helpers::{encode_varint_bytes, parse_varint, write_varint};
use prototext_core::parse_schema;
use prototext_core::serialize::encode_text::encode_text_to_binary;
use prototext_core::serialize::render_text::{decode_and_render, DecodeRenderOpts};

// ── Workload data ─────────────────────────────────────────────────────────────

/// Raw bytes of fixtures/descriptor.pb — a real FileDescriptorSet spanning
/// all `google/protobuf/*.proto` well-known types (`--include_imports`).
///
/// 18 753 bytes, deeply nested, repeated fields, strings throughout.
/// Represents the `decode_and_render` hot path.
static DESCRIPTOR_PB: &[u8] = include_bytes!("../fixtures/descriptor.pb");

/// Schema bytes for google.protobuf.FileDescriptorSet (same file, used as
/// its own schema — descriptor.pb is self-describing).
static DESCRIPTOR_SCHEMA_PB: &[u8] = DESCRIPTOR_PB;
static DESCRIPTOR_ROOT: &str = "google.protobuf.FileDescriptorSet";

/// Protoc text rendering of descriptor.pb with annotations.
///
/// Generated once (offline) via:
///   prototext --descriptor-set fixtures/descriptor.pb decode \
///     --type google.protobuf.FileDescriptorSet fixtures/descriptor.pb
///
/// 3062 lines.  Represents the `encode_text_to_binary` hot path.
static DESCRIPTOR_PROTOC_TEXT: &[u8] = include_bytes!("../fixtures/descriptor_protoc.txtpb");

// ── Schema helper ─────────────────────────────────────────────────────────────

fn make_descriptor_schema() -> prototext_core::ParsedSchema {
    parse_schema(DESCRIPTOR_SCHEMA_PB, DESCRIPTOR_ROOT).expect("descriptor schema")
}

// ── Path A: .pb → protoc text ─────────────────────────────────────────────────

fn bench_pb_to_protoc(c: &mut Criterion) {
    let schema = make_descriptor_schema();
    let root_desc = schema.root_descriptor();
    let mut g = c.benchmark_group("path_A_pb_to_protoc");

    let pb = DESCRIPTOR_PB;
    g.throughput(Throughput::Bytes(pb.len() as u64));

    // A1: no schema — untyped decode+render
    g.bench_function("A1_decode_and_render (no schema)", |b| {
        b.iter(|| {
            decode_and_render(
                black_box(pb),
                None,
                DecodeRenderOpts {
                    annotations: false,
                    ..Default::default()
                },
            )
        })
    });

    // A2: full schema + annotations — the pipeline hot path
    g.bench_function("A2_decode_and_render (schema + annotations)", |b| {
        b.iter(|| {
            decode_and_render(
                black_box(pb),
                root_desc.as_ref(),
                DecodeRenderOpts {
                    annotations: true,
                    emit_header: true,
                    ..Default::default()
                },
            )
        })
    });

    g.finish();
}

// ── Path B: protoc text → binary ──────────────────────────────────────────────

fn bench_protoc_to_pb(c: &mut Criterion) {
    let text = DESCRIPTOR_PROTOC_TEXT;
    let mut g = c.benchmark_group("path_B_protoc_to_pb");

    g.throughput(Throughput::Bytes(text.len() as u64));
    g.bench_function("B1_encode_text_to_binary (annotated)", |b| {
        b.iter(|| encode_text_to_binary(black_box(text)))
    });

    g.finish();
}

// ── Micro: parse_varint ───────────────────────────────────────────────────────

fn bench_parse_varint(c: &mut Criterion) {
    let mut g = c.benchmark_group("parse_varint");
    g.throughput(Throughput::Elements(1));

    let buf1 = vec![0x08u8];
    g.bench_function("1-byte (0x08)", |b| {
        b.iter(|| parse_varint(black_box(&buf1), 0))
    });

    let buf2 = vec![0x96u8, 0x01];
    g.bench_function("2-byte (150)", |b| {
        b.iter(|| parse_varint(black_box(&buf2), 0))
    });

    let buf10 = vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01u8];
    g.bench_function("10-byte (u64::MAX)", |b| {
        b.iter(|| parse_varint(black_box(&buf10), 0))
    });

    let buf_ohb = vec![0x80u8, 0x80, 0x00];
    g.bench_function("3-byte overhang", |b| {
        b.iter(|| parse_varint(black_box(&buf_ohb), 0))
    });

    g.finish();
}

// ── Micro: encode_varint allocation comparison ────────────────────────────────

fn bench_encode_varint(c: &mut Criterion) {
    let mut g = c.benchmark_group("encode_varint");

    g.bench_function("encode_varint_bytes(150) [alloc]", |b| {
        b.iter(|| encode_varint_bytes(black_box(150u64), None))
    });

    g.bench_function("write_varint into fresh Vec", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(10);
            write_varint(black_box(150u64), &mut buf);
            black_box(buf)
        })
    });

    g.bench_function("write_varint into reused Vec [no alloc]", |b| {
        let mut reuse = Vec::with_capacity(16);
        b.iter(|| {
            reuse.clear();
            write_varint(black_box(150u64), &mut reuse);
            black_box(reuse.len())
        })
    });

    g.finish();
}

// ── Micro: decode_and_render on random bytes ──────────────────────────────────

fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn pseudo_random_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut s = seed;
    (0..len).map(|_| xorshift64(&mut s) as u8).collect()
}

fn bench_decode_random(c: &mut Criterion) {
    let mut g = c.benchmark_group("decode_and_render_random");

    for &size in &[64usize, 256, 512, 1024] {
        let payload = pseudo_random_bytes(0xdeadbeef, size);
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::new("random bytes", size), &payload, |b, p| {
            b.iter(|| {
                decode_and_render(
                    black_box(p),
                    None,
                    DecodeRenderOpts {
                        annotations: false,
                        ..Default::default()
                    },
                )
            })
        });
    }

    g.finish();
}

criterion_group!(
    benches,
    bench_pb_to_protoc,
    bench_protoc_to_pb,
    bench_parse_varint,
    bench_encode_varint,
    bench_decode_random,
);
criterion_main!(benches);
