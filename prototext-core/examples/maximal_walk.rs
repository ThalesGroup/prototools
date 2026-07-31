// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Throwaway measurement for spec 0216 — how big is the *maximal* tree?
//!
//! Walks a blob as raw protobuf under spec 0216 S2's rule: recurse into
//! every length-delimited payload, never judge whether it "looks like" a
//! message. Reports the four numbers the spec is priced off:
//!
//! 1. node count, under S1/S6 (one node per field occurrence; packed
//!    elements are *not* nodes, since nothing here knows they are packed);
//! 2. how many of those are length-delimited payloads — the price of
//!    reserving a synthetic wrapper slot per payload, if that were needed;
//! 3. wall time of the counting pass;
//! 4. the depth histogram, which decides whether S9's outright rejection
//!    is dead code in practice.
//!
//! Deliberately does not use `parse_wiretag`: it copies the remainder of
//! the buffer into a `Vec` on every bad tag, and under "always recurse"
//! bad tags are the common case, not the exceptional one.

use prototext_core::helpers::{parse_varint, payload_end, MAX_WIRE_DEPTH};
use prototext_core::helpers::{WT_END_GROUP, WT_I32, WT_I64, WT_LEN, WT_START_GROUP, WT_VARINT};
use std::time::Instant;

#[derive(Default)]
struct Stats {
    nodes: u64,
    len_delimited: u64,
    malformed: u64,
    /// Leaves by wire type, indexed by wire type.
    by_wire_type: [u64; 6],
    /// How many nodes sit at each depth.
    depth_hist: Vec<u64>,
    max_depth: usize,
    depth_capped: u64,
    /// Groups whose body ran to the end of the enclosing payload without
    /// a matching `END_GROUP`. Each one may have swallowed siblings that
    /// a different recovery rule would have counted, so this is the
    /// measurement's known bias — toward *under*counting.
    unterminated_groups: u64,
}

impl Stats {
    fn node(&mut self, depth: usize, wire_type: u32) {
        self.nodes += 1;
        if (wire_type as usize) < 6 {
            self.by_wire_type[wire_type as usize] += 1;
        }
        if depth >= self.depth_hist.len() {
            self.depth_hist.resize(depth + 1, 0);
        }
        self.depth_hist[depth] += 1;
        self.max_depth = self.max_depth.max(depth);
    }
}

/// How a `START_GROUP` tag with no matching `END_GROUP` is recovered.
///
/// Under "always recurse" almost every group found is spurious — a
/// coincidence inside a string — so the recovery rule, not the protobuf
/// spec, decides what gets counted. The two rules bracket the answer.
#[derive(Clone, Copy, PartialEq)]
enum GroupMode {
    /// Recurse; an unterminated group consumes the rest of its enclosing
    /// payload. Undercounts, because the swallowed bytes yield no nodes.
    Nested,
    /// Treat the tag as a leaf and keep scanning at the next byte. Every
    /// following byte still gets a chance to be a sibling. Overcounts.
    Flat,
}

/// Walk `buf[start..end]` as the body of a message at `depth`.
///
/// Returns the position it stopped at. A group body stops at its own
/// `END_GROUP` tag; a message body stops at `end`.
fn walk(
    buf: &[u8],
    start: usize,
    end: usize,
    depth: usize,
    st: &mut Stats,
    gm: GroupMode,
) -> usize {
    let mut pos = start;
    while pos < end {
        let tag_start = pos;

        // Wire type is the low three bits of the first tag byte. Checked
        // before parsing so a bad type costs nothing.
        if buf[pos] & 0x07 > 5 {
            st.node(depth, u32::MAX);
            st.malformed += 1;
            return end;
        }

        let vr = parse_varint(buf, pos);
        let Some(tag) = vr.varint else {
            st.node(depth, u32::MAX);
            st.malformed += 1;
            return end;
        };
        pos = vr.next_pos;

        let wire_type = (tag & 0x07) as u32;
        let field = tag >> 3;
        if field == 0 || field >= (1 << 29) {
            st.node(depth, u32::MAX);
            st.malformed += 1;
            return end;
        }

        if wire_type == WT_END_GROUP {
            // Closes the enclosing group; not a node of its own.
            return pos;
        }

        match wire_type {
            WT_VARINT => {
                let v = parse_varint(buf, pos.min(end));
                if v.varint.is_none() || v.next_pos > end {
                    st.node(depth, u32::MAX);
                    st.malformed += 1;
                    return end;
                }
                st.node(depth, wire_type);
                pos = v.next_pos;
            }
            WT_I64 | WT_I32 => {
                let width = if wire_type == WT_I64 { 8 } else { 4 };
                if pos + width > end {
                    st.node(depth, u32::MAX);
                    st.malformed += 1;
                    return end;
                }
                st.node(depth, wire_type);
                pos += width;
            }
            WT_LEN => {
                let lr = parse_varint(buf, pos);
                let Some(len) = lr.varint else {
                    st.node(depth, u32::MAX);
                    st.malformed += 1;
                    return end;
                };
                let Some(payload_stop) = payload_end(lr.next_pos, len, end) else {
                    st.node(depth, u32::MAX);
                    st.malformed += 1;
                    return end;
                };
                st.node(depth, wire_type);
                st.len_delimited += 1;

                // Spec 0216 S2: always recurse. A payload that yields
                // nothing simply produces a childless node (S3).
                if depth + 1 < MAX_WIRE_DEPTH {
                    walk(buf, lr.next_pos, payload_stop, depth + 1, st, gm);
                } else {
                    st.depth_capped += 1;
                }
                pos = payload_stop;
            }
            WT_START_GROUP => {
                st.node(depth, wire_type);
                if gm == GroupMode::Flat {
                    // Leaf: keep scanning where we are.
                } else if depth + 1 < MAX_WIRE_DEPTH {
                    pos = walk(buf, pos, end, depth + 1, st, gm);
                    if pos >= end {
                        st.unterminated_groups += 1;
                    }
                } else {
                    st.depth_capped += 1;
                    return end;
                }
            }
            _ => unreachable!("wire type was range-checked above"),
        }

        debug_assert!(pos > tag_start, "walk must make progress");
    }
    pos
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: maximal_walk <blob>");
    let buf = std::fs::read(&path).expect("read blob");
    println!("blob: {} ({} bytes)", path, buf.len());

    for (label, gm) in [
        ("Nested (undercounts)", GroupMode::Nested),
        ("Flat   (overcounts)", GroupMode::Flat),
    ] {
        let t0 = Instant::now();
        let mut st = Stats::default();
        walk(&buf, 0, buf.len(), 0, &mut st, gm);
        let elapsed = t0.elapsed();

        println!("\n=== group rule: {label} ===");
        println!("  counting pass        {:>12?}", elapsed);
        println!("  nodes                {:>12}", st.nodes);
        println!("  length-delimited     {:>12}", st.len_delimited);
        println!("  malformed            {:>12}", st.malformed);
        println!(
            "  varint leaves        {:>12}",
            st.by_wire_type[WT_VARINT as usize]
        );
        println!(
            "  i64 leaves           {:>12}",
            st.by_wire_type[WT_I64 as usize]
        );
        println!(
            "  i32 leaves           {:>12}",
            st.by_wire_type[WT_I32 as usize]
        );
        println!(
            "  groups               {:>12}",
            st.by_wire_type[WT_START_GROUP as usize]
        );
        println!("  unterminated groups  {:>12}", st.unterminated_groups);
        println!("  depth-capped         {:>12}", st.depth_capped);
        println!(
            "  max depth            {:>12}  (cap {})",
            st.max_depth, MAX_WIRE_DEPTH
        );

        // Spec 0216 Q1: the multiplier over today's googleapis arena.
        const TODAY: f64 = 4_501_014.0;
        println!(
            "  vs today ({} nodes)  {:.2}x",
            TODAY as u64,
            st.nodes as f64 / TODAY
        );

        println!("  depth histogram:");
        for (d, n) in st.depth_hist.iter().enumerate() {
            if *n > 0 {
                println!("    {:>4}  {:>12}", d, n);
            }
        }
    }
}
