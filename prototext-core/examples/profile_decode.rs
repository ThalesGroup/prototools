// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

// examples/profile_decode.rs — the decode walk under an external profiler.
//
// The companion to prototext-graph's `examples/profile_score.rs`; see that
// file for why a marked `#[inline(never)]` region is used instead of letting
// a profiler attribute the whole process.  Here the setup being excluded is
// `parse_schema` over the self-describing descriptor, which measured 52% of
// the process when `benches/codec.rs` was profiled without scoping.
//
// Run through `bin/profile`:
//
//   bin/profile decode [iterations] [annotated]
//
// `annotated` selects the workload: 1 (default) is the pipeline hot path,
// schema + annotations, matching the bench's A2; 0 is the untyped decode,
// matching A1.  They have materially different profiles — A2 spends 8% of its
// instructions in `prost_reflect` field lookups that A1 never performs.

use std::hint::black_box;

use prototext_core::parse_schema;
use prototext_core::serialize::render_text::{decode_and_render, DecodeRenderOpts};

/// Raw bytes of fixtures/descriptor.pb — a real FileDescriptorSet spanning all
/// `google/protobuf/*.proto` well-known types, 18 753 bytes, deeply nested.
/// The same fixture `benches/codec.rs` uses, so the two are comparable.
static DESCRIPTOR_PB: &[u8] = include_bytes!("../fixtures/descriptor.pb");
static DESCRIPTOR_ROOT: &str = "google.protobuf.FileDescriptorSet";

/// The collection boundary.  Everything a profiler should attribute is inside
/// this call and nothing else is.
///
/// `#[inline(never)]` keeps the symbol alive for `--toggle-collect` and stops
/// the loop being hoisted out; the checksum keeps the render from being
/// optimized away.
#[inline(never)]
fn profile_region(
    pb: &[u8],
    root: Option<&prost_reflect::MessageDescriptor>,
    annotated: bool,
    iterations: usize,
) -> usize {
    let mut sink = 0usize;
    for _ in 0..iterations {
        let out = decode_and_render(
            black_box(pb),
            root,
            DecodeRenderOpts {
                annotations: annotated,
                emit_header: annotated,
                ..Default::default()
            },
        );
        sink = sink.wrapping_add(out.len());
    }
    sink
}

fn arg(n: usize, default: usize) -> usize {
    std::env::args()
        .nth(n)
        .map_or(default, |a| a.parse().unwrap_or(default))
}

fn main() {
    let iterations = arg(1, 50);
    let annotated = arg(2, 1) != 0;

    // Setup, deliberately outside the region.
    let schema = parse_schema(DESCRIPTOR_PB, DESCRIPTOR_ROOT).expect("descriptor schema");
    let root_desc = schema.root_descriptor();
    let root = if annotated { root_desc.as_ref() } else { None };

    let sink = profile_region(DESCRIPTOR_PB, root, annotated, iterations);

    println!(
        "input={} bytes annotated={annotated} iterations={iterations} checksum={sink}",
        DESCRIPTOR_PB.len()
    );
}
