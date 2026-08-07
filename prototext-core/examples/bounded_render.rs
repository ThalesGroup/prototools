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
}
