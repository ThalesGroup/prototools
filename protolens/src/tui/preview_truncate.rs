// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Where a live preview's interior may be cut (spec 0174 S3).
//!
//! Its own file because it is a pure function of bytes: `TruncShape` and
//! the three functions below take a payload, a budget and a wire shape
//! and return bytes. It is the one part of `override_apply.rs` that needs
//! no `use super::*` at all — it does not know what an `App` is.

use prost_reflect::prost_types::field_descriptor_proto::Type;
use prototext_core::helpers::{
    encode_varint_bytes, parse_varint, parse_wiretag, WT_LEN, WT_START_GROUP,
};
use prototext_core::serialize::render_text::NodeSpan;

/// Spec 0174 §S3: where a live preview's interior may be cut. Framing
/// (LEN vs. group) is read from the node's own tag and is independent of
/// this; `TruncShape` only decides *how many* interior bytes to keep.
///
/// Every variant aligns on a boundary the renderer itself respects, so
/// truncation can never manufacture a malformity marker the untruncated
/// data did not already have — that is the shared invariant, and the
/// reason `string` cannot simply fall under `Exact`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TruncShape {
    /// Message, group, `bytes`. Cut at exactly `budget`. For a message or
    /// group the cut is *meant* to land deep inside, so every field that
    /// fits still renders in full and correctly typed; the one straddling
    /// field degrades and its line is dropped by the caller. For `bytes`
    /// there is no alignment to respect — any byte sequence is a valid
    /// value.
    Exact,
    /// `string`. Cut at the last UTF-8 character boundary at or before
    /// `budget`, so the shortened payload stays valid UTF-8 and renders
    /// as an ordinary string rather than `INVALID_STRING`.
    CharBoundary,
    /// A packed run of varint elements (spec 0219 S6). Cut at the last
    /// element boundary at or before `budget`.
    ///
    /// Alignment is not cosmetic here: `decode_packed_elems` is
    /// all-or-nothing, so a cut landing inside a varint turns the whole
    /// record into one `INVALID_PACKED_RECORDS` line. The preview would
    /// then say the bytes are invalid where the commit renders a clean
    /// run — the preview/commit divergence spec 0185 G3 forbids.
    PackedVarint,
    /// A packed run of fixed-width elements, `usize` being that width
    /// (4 or 8). Cut at the last whole element. Same reasoning as
    /// `PackedVarint`: a payload whose length is not a multiple of the
    /// element size is rejected outright.
    PackedFixed(usize),
    /// Every other target: bounded by construction (varint, I32 and I64
    /// are at most ten bytes), or not truncatable without lying. Never
    /// cut.
    Never,
}

/// Spec 0174 §S3: a copy of `field_bytes` (a node's complete
/// `tag[+length]+payload` span) whose *interior* is cut to at most
/// `budget` bytes and re-framed so the result is still a well-formed
/// field — which is what keeps the cut inside the interior instead of
/// overrunning the synthetic wrapper's single field.
///
/// Returns the truncated bytes, or `None` when nothing was cut.
///
/// No `span_shift` accompanies them — the interior does move left when
/// the rewritten length varint comes out narrower than the original, but
/// nothing needs to add that back. Spec 0216 S12: the spans a splice
/// keeps take their byte ranges from the arena, which is expressed
/// against the real blob and never saw this rewritten copy.
///
/// Preview-only. A confirmed override must render completely, so its
/// bytes never come through here (G5).
pub(super) fn truncate_interior(
    field_bytes: &[u8],
    budget: usize,
    shape: TruncShape,
) -> Option<Vec<u8>> {
    if shape == TruncShape::Never {
        return None;
    }
    let tag = parse_wiretag(field_bytes, 0);
    let wire_type = tag.wtype?;

    // Group framing: no length prefix to rewrite, so a plain prefix of
    // the original bytes is already a well-formed (open-ended) group.
    // `render_group_field` reports `close_facts: None` when a group
    // reaches end-of-buffer without a close tag, so `end_nested` still
    // emits a plain `}` with no annotation.
    if wire_type == WT_START_GROUP {
        let payload = &field_bytes[tag.next_pos..];
        let kept = cut_at(payload, budget, shape)?;
        let mut out = Vec::with_capacity(tag.next_pos + kept);
        out.extend_from_slice(&field_bytes[..tag.next_pos]);
        out.extend_from_slice(&payload[..kept]);
        return Some(out);
    }

    if wire_type != WT_LEN {
        return None;
    }
    let len = parse_varint(field_bytes, tag.next_pos);
    len.varint?;
    let payload = &field_bytes[len.next_pos..];
    let kept = cut_at(payload, budget, shape)?;

    let new_prefix = encode_varint_bytes(kept as u64, None);
    let mut out = Vec::with_capacity(tag.next_pos + new_prefix.len() + kept);
    out.extend_from_slice(&field_bytes[..tag.next_pos]);
    out.extend_from_slice(&new_prefix);
    out.extend_from_slice(&payload[..kept]);
    Some(out)
}

/// Spec 0302: rewrite a TRUNCATED_BYTES field's declared length varint to
/// match the bytes actually present, so the decoder opens it as a message
/// instead of emitting another TRUNCATED_BYTES line.
///
/// Returns `None` for non-LEN fields and for fields whose declared length
/// already equals their actual payload (the normal case).
pub(super) fn reframe_to_actual_length(field_bytes: &[u8]) -> Option<Vec<u8>> {
    let tag = parse_wiretag(field_bytes, 0);
    if tag.wtype? != WT_LEN {
        return None;
    }
    let len = parse_varint(field_bytes, tag.next_pos);
    let declared = len.varint? as usize;
    let payload = &field_bytes[len.next_pos..];
    if payload.len() == declared {
        return None;
    }
    let new_prefix = encode_varint_bytes(payload.len() as u64, None);
    let mut out = Vec::with_capacity(tag.next_pos + new_prefix.len() + payload.len());
    out.extend_from_slice(&field_bytes[..tag.next_pos]);
    out.extend_from_slice(&new_prefix);
    out.extend_from_slice(payload);
    Some(out)
}

/// Spec 0174 §S3: which cut rule a preview of this candidate needs.
///
/// `field_type` is the synthetic wrapper field's declared type (`None`
/// for a raw, un-retyped node); `wire_type` is the node's real framing
/// on the wire; `packed` is `render_node_as`'s own decision (spec 0219
/// S3), threaded in rather than re-derived so preview and commit cannot
/// disagree about what is being rendered.
pub(super) fn trunc_shape_for(
    field_type: Option<Type>,
    wire_type: u32,
    packed: bool,
) -> TruncShape {
    let Some(ft) = field_type else {
        // Raw: `render_message` probes a LEN payload and renders it as a
        // nested message or as bytes. Either way an exact cut is safe —
        // a shorter probe still decodes, and shorter bytes are still
        // bytes.
        return if wire_type == WT_LEN || wire_type == WT_START_GROUP {
            TruncShape::Exact
        } else {
            TruncShape::Never
        };
    };
    match ft {
        Type::Message | Type::Group | Type::Bytes => TruncShape::Exact,
        Type::String => TruncShape::CharBoundary,
        // Everything below is numeric/bool/enum — exactly the set
        // protobuf lets a field pack, so `packed` needs no further
        // filtering here. Unpacked, each is a single value bounded by
        // construction; packed, it is a run and needs an
        // element-aligned cut.
        _ if !packed => TruncShape::Never,
        Type::Double | Type::Fixed64 | Type::Sfixed64 => TruncShape::PackedFixed(8),
        Type::Float | Type::Fixed32 | Type::Sfixed32 => TruncShape::PackedFixed(4),
        _ => TruncShape::PackedVarint,
    }
}

/// Spec 0174 §S4: mark a truncated preview's rendering with a trailing
/// `...` line.
///
/// The straddling field — the one `TruncShape::Exact` cut in half — is
/// *replaced* by the marker rather than merely deleted, so its
/// `TRUNCATED_BYTES` annotation never reaches the user (G4) and the line
/// count is unchanged. Only that field's own leaf spans are dropped;
/// enclosing spans legitimately keep covering the line.
///
/// The aligned cut rules leave nothing straddling, so there the marker is
/// *inserted* instead — inside the closing brace for a nested render,
/// where the elided content would have been, or after the value line for
/// a `string`/`bytes`/packed target, which has no brace. Insertion shifts
/// every later line, so span text ranges are corrected accordingly.
///
/// Spec 0187 S2: the marker carries no highlighting because
/// `window_text` blanks it before the parser ever sees it — `...` is not
/// in the prototext grammar, and highlighting happens at draw time, with
/// no "colorize first, splice the marker after" ordering to hide behind.
pub(super) fn insert_truncation_marker(
    lines: &mut Vec<String>,
    spans: &mut Vec<NodeSpan>,
    indent_size: usize,
) {
    let indent_of = |l: &str| l.len() - l.trim_start().len();
    let straddler = lines.iter().rposition(|l| l.contains("TRUNCATED_BYTES"));

    let (at, indent, replacing) = match straddler {
        Some(i) => (i, indent_of(&lines[i]), true),
        None if lines.last().is_some_and(|l| l.trim() == "}") => {
            let close = lines.len() - 1;
            (close, indent_of(&lines[close]) + indent_size, false)
        }
        None => (lines.len(), lines.last().map_or(0, |l| indent_of(l)), false),
    };

    let marker = format!("{}...", " ".repeat(indent));
    let at_line = u32::try_from(at).expect("a line index within the input cap fits a u32");
    if replacing {
        lines[at] = marker;
        // Leaf spans confined to this one line only — an enclosing
        // message's span spills past it and must survive.
        spans.retain(|s| !(s.text_range.start >= at_line && s.text_range.end <= at_line + 1));
    } else {
        lines.insert(at, marker);
        for s in spans.iter_mut() {
            if s.text_range.start >= at_line {
                s.text_range.start += 1;
            }
            if s.text_range.end > at_line {
                s.text_range.end += 1;
            }
        }
    }
}

/// How many of `payload`'s bytes to keep under `shape`, or `None` when
/// the payload already fits and nothing needs cutting.
pub(super) fn cut_at(payload: &[u8], budget: usize, shape: TruncShape) -> Option<usize> {
    if payload.len() <= budget {
        return None;
    }
    let kept = match shape {
        TruncShape::Never => return None,
        TruncShape::Exact => budget,
        TruncShape::CharBoundary => {
            // Walk back to the start of the character straddling the cut:
            // continuation bytes are `0b10xxxxxx`. At most three steps for
            // valid UTF-8, and bounded anyway so invalid input terminates.
            let mut k = budget;
            while k > 0 && (payload[k] & 0xC0) == 0x80 {
                k -= 1;
            }
            k
        }
        // A varint's last byte is the one with the continuation bit
        // clear, so any position right after such a byte ends a whole
        // number of elements. At most ten steps.
        TruncShape::PackedVarint => {
            let mut k = budget;
            while k > 0 && (payload[k - 1] & 0x80) != 0 {
                k -= 1;
            }
            k
        }
        TruncShape::PackedFixed(width) => budget - budget % width,
    };
    Some(kept)
}
