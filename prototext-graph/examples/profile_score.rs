// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

// examples/profile_score.rs — the scoring walk under an external profiler.
//
// `benches/score.rs` measures; this profiles.  The difference matters because
// a profiler attributes the *whole process*, and the bench's one-time setup
// (building and Hopcroft-minimizing a synthetic graph) is large enough to
// dominate a naive profile of it — measured at 52% of the process on the
// sibling `prototext-core --bench codec`.
//
// So the region of interest is marked, not inferred: `profile_region` is
// `#[inline(never)]`, which gives it a symbol an external tool can toggle
// collection on.  Everything expensive that is not the walk happens before it.
//
//   valgrind --tool=callgrind --collect-atstart=no \
//            --toggle-collect='*profile_region*' ...
//
// `score_all` itself cannot be used as that boundary: it is inlined into its
// caller and has no symbol.  Toggling on the inner `walk::score_subset`
// instead — the obvious workaround — silently excludes whatever `score_all`
// does around the walk, which is the error this file exists to prevent.
//
// Run through `bin/profile`, which builds with line tables and refuses a run
// that collected nothing.  Usage:
//
//   bin/profile score [roots] [records] [iterations] [packed] [string_len]

use std::hint::black_box;

use prototext_graph::build_scoring_graph::{build_compiled, serial};
use prototext_graph::score::load::LoadedGraph;
use prototext_graph::score::{score_all, ScoringOpts};

/// Scoring-graph YAML declaring `n` mutually distinct root message types.
///
/// Kept identical to `benches/score.rs`'s generator on purpose: a profile and
/// a benchmark that disagree about the workload cannot be read together.
/// Fields 1-3 are shared by every root, so the blob matches all of them, while
/// field `100 + i` is unique and never encoded — which keeps Hopcroft from
/// merging the roots without vetoing any of them, holding the active set at
/// the full root count for the whole walk.
fn synthetic_yaml(n: usize) -> String {
    let mut s = String::from("entries:\n");
    for i in 0..n {
        s.push_str(&format!("- Msg{i}\n"));
    }
    s.push_str("messages:\n");
    s.push_str("  Leaf:\n    fields:\n    - number: 1\n      type: uint64\n");
    for i in 0..n {
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

/// A blob every synthetic root matches, with spec 0288 S8's two density knobs.
///
/// Kept identical to `benches/score.rs`'s generator, for the reason given
/// above `synthetic_yaml`. `packed` is elements in a packed `4: [int32]` run
/// per record — the only one of the fields that reaches the per-element varint
/// check — and `string_len` is field 1's payload length, which is what the
/// UTF-8 test scans.
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
                write_varint(&mut payload, (k % 1000) as u64);
            }
            write_varint(&mut out, (4 << 3) | 2);
            write_varint(&mut out, payload.len() as u64);
            out.extend_from_slice(&payload);
        }
    }
    out
}

/// The collection boundary.  Everything a profiler should attribute is inside
/// this call and nothing else is.
///
/// `#[inline(never)]` is load-bearing twice over: it keeps the symbol alive for
/// `--toggle-collect`, and it stops the loop being hoisted out of the region.
/// The returned checksum is what keeps `score_all` from being optimized away.
#[inline(never)]
fn profile_region(
    blob: &[u8],
    graph: &LoadedGraph,
    opts: &ScoringOpts,
    iterations: usize,
) -> usize {
    let mut sink = 0usize;
    for _ in 0..iterations {
        let scores = score_all(black_box(blob), graph.graph(), opts);
        sink = sink.wrapping_add(scores.len());
    }
    sink
}

fn arg(n: usize, default: usize) -> usize {
    std::env::args()
        .nth(n)
        .map_or(default, |a| a.parse().unwrap_or(default))
}

fn main() {
    let roots = arg(1, 1024);
    let records = arg(2, 64);
    let iterations = arg(3, 20);
    // Spec 0288 S8. Both default to the pre-0288 workload, which reaches
    // neither the per-element check nor a UTF-8 scan worth measuring, so an
    // unqualified `bin/profile score` keeps comparing with older runs.
    let packed = arg(4, 0);
    let string_len = arg(5, 8);

    // Setup, deliberately outside the region.
    let graph = build_graph(roots);
    let pb = blob(records, packed, string_len);
    let opts = ScoringOpts::default();

    let sink = profile_region(&pb, &graph, &opts, iterations);

    println!(
        "roots={roots} records={records} packed={packed} string_len={string_len} \
         blob={} bytes iterations={iterations} checksum={sink}",
        pb.len()
    );
}
