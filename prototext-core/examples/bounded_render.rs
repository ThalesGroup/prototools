// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway measurement for spec 0249 S1/S2 — what does a row-budgeted
//! render of a large document actually cost, and how wide is the right
//! frontier it has to emit to keep the line counts exact?
//!
//! S2 predicts ~7 771 one-line entries at the googleapis root to support a
//! 50-row screen, against 5.28 M rows unbounded. This checks that, and
//! prices the render the confirmed override would pay.

use prototext_core::serialize::render_text::{
    decode_and_render_indexed, DecodeRenderOpts, FqdnTable,
};
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let blob_path = args.next().expect("usage: bounded_render <blob> <fqdn>");
    let root = args
        .next()
        .unwrap_or_else(|| "google.protobuf.FileDescriptorSet".to_string());

    let buf = std::fs::read(&blob_path).expect("read blob");
    println!("blob: {} ({} bytes)", blob_path, buf.len());

    let t0 = Instant::now();
    let schema = prototext_core::parse_schema(&buf, &root).expect("parse schema");
    let desc = schema.root_descriptor();
    println!("schema {root}: parsed in {:?}\n", t0.elapsed());

    println!(
        "{:>10}  {:>10}  {:>12}  {:>10}  {:>12}  {:>12}",
        "budget", "wall", "text bytes", "rows", "spans", "undescended"
    );

    // `None` last: it is the expensive one, and every bounded row above is
    // meant to be read against it.
    for budget in [
        Some(1usize),
        Some(51),
        Some(101),
        Some(1001),
        Some(10_001),
        None,
    ] {
        let mut fqdns = FqdnTable::new();
        let t = Instant::now();
        let r = decode_and_render_indexed(
            &buf,
            desc.as_ref(),
            &mut fqdns,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                row_budget: budget,
                ..DecodeRenderOpts::default()
            },
        )
        .expect("render");
        let wall = t.elapsed();
        let rows = r.text.iter().filter(|&&b| b == b'\n').count();
        println!(
            "{:>10}  {:>10.1?}  {:>12}  {:>10}  {:>12}  {:>12}",
            budget.map_or("none".to_string(), |b| b.to_string()),
            wall,
            r.text.len(),
            rows,
            r.spans.len(),
            r.undescended.len(),
        );
        if let Some(b) = budget {
            // S2's claim, made checkable: everything past the budget is the
            // walk unwinding, and every one of those nodes is one row.
            let frontier = rows.saturating_sub(b);
            let stopped_rows: usize = r
                .undescended
                .iter()
                .map(|&i| {
                    (r.spans[i as usize].text_range.end - r.spans[i as usize].text_range.start)
                        as usize
                })
                .sum();
            println!(
                "{:>10}  frontier {} rows over budget, {} stops holding {} rows",
                "",
                frontier,
                r.undescended.len(),
                stopped_rows
            );
        }
    }

    // Spec 0249 S6 / open question 2: the multi-site case renders each site
    // on its own. Price one call per site against the single batched call
    // above, and separate the per-call setup from the per-row work by
    // rendering the same sites at two budgets.
    //
    // The sites here are the blob's top-level `file` records, whose payload
    // is a `FileDescriptorProto` — a real type, so no synthetic wrapper is
    // needed to render one on its own.
    let sites = top_level_len_payloads(&buf);
    println!("\nper-site: {} top-level records", sites.len());
    let file_schema = prototext_core::parse_schema(&buf, "google.protobuf.FileDescriptorProto")
        .expect("parse FileDescriptorProto");
    let file_desc = file_schema.root_descriptor();
    for budget in [Some(1usize), Some(51), None] {
        let mut fqdns = FqdnTable::new();
        let t = Instant::now();
        let mut rows = 0usize;
        let mut bytes = 0usize;
        for &(s, e) in &sites {
            let r = decode_and_render_indexed(
                &buf[s..e],
                file_desc.as_ref(),
                &mut fqdns,
                DecodeRenderOpts {
                    annotations: true,
                    indent_size: 2,
                    row_budget: budget,
                    ..DecodeRenderOpts::default()
                },
            )
            .expect("render site");
            rows += r.text.iter().filter(|&&b| b == b'\n').count();
            bytes += r.text.len();
        }
        let wall = t.elapsed();
        println!(
            "{:>10}  {:>10.1?}  {:>12}  {:>10}  {:>12.1?} per site",
            budget.map_or("none".to_string(), |b| b.to_string()),
            wall,
            bytes,
            rows,
            wall / sites.len() as u32,
        );
    }
}

/// Byte ranges of the payloads of the blob's top-level length-delimited
/// records, in document order.
fn top_level_len_payloads(buf: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < buf.len() {
        let (tag, n) = varint(buf, i);
        i += n;
        if tag & 7 != 2 {
            panic!("top-level record at {i} is not length-delimited");
        }
        let (len, n) = varint(buf, i);
        i += n;
        out.push((i, i + len as usize));
        i += len as usize;
    }
    out
}

fn varint(buf: &[u8], mut i: usize) -> (u64, usize) {
    let start = i;
    let (mut v, mut shift) = (0u64, 0u32);
    loop {
        let b = buf[i];
        v |= u64::from(b & 0x7f) << shift;
        i += 1;
        if b & 0x80 == 0 {
            return (v, i - start);
        }
        shift += 7;
    }
}
