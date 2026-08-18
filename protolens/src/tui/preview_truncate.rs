// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Where a live preview's interior may be cut (specs 0174 S3, 0318).
//!
//! Its own file because it is a pure function of bytes: `TruncShape` and
//! the functions below take a payload, a budget and a wire shape and
//! return bytes. It is the one part of `override_apply.rs` that needs no
//! `use super::*` at all — it does not know what an `App` is.
//!
//! The budget is in **bytes**, and spec 0318 keeps it there: every
//! rendered row costs at least one byte of payload — a packed element at
//! least one, a record header at least two — so a byte budget bounds
//! rows whatever the data looks like. The converse is false. A row
//! budget bounds no bytes at all inside a packed run, where one record
//! can produce millions of rows (spec 0317).

use prost_reflect::prost_types::field_descriptor_proto::Type;
use prototext_core::helpers::{
    encode_varint_bytes, parse_varint, parse_wiretag, WT_END_GROUP, WT_I32, WT_I64, WT_LEN,
    WT_START_GROUP, WT_VARINT,
};

/// Spec 0318 S5: how faithful a preview is to the node it stands for.
///
/// Three decisions, two colors: the bar in the overlay's fold column
/// asks only whether the preview is all of the node, so `Clean` and
/// `Ragged` draw alike (`theme::preview_bar_color` says why). They are
/// still two decisions here, because they differ in what the rendering
/// below the bar contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewTier {
    /// Nothing was cut. The preview *is* the node.
    Whole,
    /// Cut on a boundary the renderer respects, so no field straddles
    /// the cut and the preview shows no malformity the full node would
    /// not have shown.
    Clean,
    /// Cut at the hard cap, mid-record (S4). Annotations may appear —
    /// `TRUNCATED_BYTES` and spec 0303's missing-byte count — that the
    /// full node would not have carried.
    Ragged,
}

impl PreviewTier {
    /// Whether the preview withheld nothing — the one question the bar
    /// in the fold column answers.
    pub(super) fn is_whole(self) -> bool {
        self == Self::Whole
    }
}

/// Spec 0174 §S3: where a live preview's interior may be cut. Framing
/// (LEN vs. group) is read from the node's own tag and is independent of
/// this; `TruncShape` only decides *how many* interior bytes to keep.
///
/// Every variant except `AnyByte` aligns on a boundary the renderer
/// itself respects, so truncation cannot manufacture a malformity marker
/// the untruncated data did not already have. `AnyByte` needs no
/// alignment for the same reason: to a `bytes` field every byte sequence
/// is already a valid value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TruncShape {
    /// `bytes`. Cut at exactly `soft` — there is nothing to align to and
    /// nothing to break.
    AnyByte,
    /// Spec 0318 S1: message, group, and the raw (un-retyped) LEN and
    /// group cases. Cut at the first top-level wire-record boundary at
    /// or after `soft`, so the kept prefix is a sequence of whole
    /// records — which is exactly what a shorter message is.
    ///
    /// This is the variant that exists because cutting a message at an
    /// arbitrary byte leaves one field straddling the cut, and that
    /// field then renders as `TRUNCATED_BYTES` on a node whose data is
    /// intact. The reader is deciding about that node; the budget must
    /// not put words in its mouth.
    RecordBoundary,
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
/// Returns the truncated bytes and how faithful they are, or `None` when
/// nothing was cut — which the caller reads as `PreviewTier::Whole`.
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
    soft: usize,
    shape: TruncShape,
) -> Option<(Vec<u8>, PreviewTier)> {
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
        let (kept, tier) = cut_at(payload, soft, shape)?;
        let mut out = Vec::with_capacity(tag.next_pos + kept);
        out.extend_from_slice(&field_bytes[..tag.next_pos]);
        out.extend_from_slice(&payload[..kept]);
        return Some((out, tier));
    }

    if wire_type != WT_LEN {
        return None;
    }
    let len = parse_varint(field_bytes, tag.next_pos);
    len.varint?;
    let payload = &field_bytes[len.next_pos..];
    let (kept, tier) = cut_at(payload, soft, shape)?;

    let new_prefix = encode_varint_bytes(kept as u64, None);
    let mut out = Vec::with_capacity(tag.next_pos + new_prefix.len() + kept);
    out.extend_from_slice(&field_bytes[..tag.next_pos]);
    out.extend_from_slice(&new_prefix);
    out.extend_from_slice(&payload[..kept]);
    Some((out, tier))
}

/// Spec 0318 S3: the hard cap, given the soft target. One knob — the
/// number the reader sets with `--override-preview-byte-budget` is still
/// the amount of payload a preview is guaranteed to show, and this is
/// the room the record-boundary seek is allowed to overshoot into.
pub(super) fn hard_cap(soft: usize) -> usize {
    soft.saturating_mul(2)
}

/// Spec 0318 S2: the first top-level wire-record boundary in `payload` at
/// or after `soft`, or `None` when there is no such boundary at or before
/// `hard_cap(soft)`.
///
/// Skips one record at a time without descending into it, so the walk
/// costs the bytes it steps over and never reads past the cap. Group
/// framing nests, so `START_GROUP`/`END_GROUP` move a depth counter and
/// only depth 0 yields a boundary: an `END_GROUP` closing a nested group
/// does not end a top-level record.
///
/// Any tag or length varint the walk cannot parse ends it. The data is
/// malformed from there on, S4 takes over, and guessing past it is how a
/// preview would invent a boundary the bytes do not have.
fn top_level_boundary(payload: &[u8], soft: usize) -> Option<usize> {
    let hard = hard_cap(soft);
    let mut pos = 0usize;
    let mut depth = 0usize;
    while pos < payload.len() {
        if depth == 0 && pos >= soft {
            return (pos <= hard).then_some(pos);
        }
        if pos > hard {
            return None;
        }
        let tag = parse_wiretag(payload, pos);
        let next = match tag.wtype? {
            WT_VARINT => {
                let v = parse_varint(payload, tag.next_pos);
                v.varint?;
                v.next_pos
            }
            WT_I64 => tag.next_pos.checked_add(8)?,
            WT_I32 => tag.next_pos.checked_add(4)?,
            WT_LEN => {
                let len = parse_varint(payload, tag.next_pos);
                let n = usize::try_from(len.varint?).ok()?;
                len.next_pos.checked_add(n)?
            }
            WT_START_GROUP => {
                depth += 1;
                tag.next_pos
            }
            WT_END_GROUP => {
                // A close with nothing open means the payload is not the
                // sequence of records it claimed to be. Stop rather than
                // let `depth` wrap and the walk run on.
                depth = depth.checked_sub(1)?;
                tag.next_pos
            }
            _ => return None,
        };
        if next > payload.len() || next <= pos {
            return None;
        }
        pos = next;
    }
    // The whole payload walked cleanly and its end is a boundary — but
    // only the caller's `payload.len() > soft` guard got us here, so the
    // end is past `soft` and the only question is the cap.
    (pos <= hard).then_some(pos)
}

/// Spec 0303 S3: how many bytes the declared length exceeds the actual payload
/// for a TRUNCATED_BYTES (LEN) field — i.e. `declared - actual_payload`.
///
/// Returns `None` for non-LEN fields and for fields whose declared length
/// already equals their actual payload (the normal case, where no annotation
/// is needed).  Called on the *original* `field_bytes`, before
/// `reframe_to_actual_length` rewrites the varint.
pub(super) fn missing_bytes_for(field_bytes: &[u8]) -> Option<u64> {
    let tag = parse_wiretag(field_bytes, 0);
    if tag.wtype? != WT_LEN {
        return None;
    }
    let len = parse_varint(field_bytes, tag.next_pos);
    let declared = len.varint?;
    let actual = (field_bytes.len() - len.next_pos) as u64;
    declared.checked_sub(actual).filter(|&d| d > 0)
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
        // nested message or as bytes. The probe can go either way, so
        // the cut has to satisfy the stricter reading — whole records,
        // which a bytes rendering is indifferent to.
        return if wire_type == WT_LEN || wire_type == WT_START_GROUP {
            TruncShape::RecordBoundary
        } else {
            TruncShape::Never
        };
    };
    match ft {
        Type::Message | Type::Group => TruncShape::RecordBoundary,
        Type::Bytes => TruncShape::AnyByte,
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

/// How many of `payload`'s bytes to keep under `shape`, and how faithful
/// keeping that many is — or `None` when the payload already fits and
/// nothing needs cutting, which is `PreviewTier::Whole`.
///
/// Spec 0318 S5: the tier is returned rather than derived by the caller
/// from `kept`, because a record boundary landing exactly on the hard cap
/// is `Clean` and a mid-record cut at the same offset is `Ragged`. The
/// length cannot tell them apart.
pub(super) fn cut_at(
    payload: &[u8],
    soft: usize,
    shape: TruncShape,
) -> Option<(usize, PreviewTier)> {
    if payload.len() <= soft {
        return None;
    }
    match shape {
        TruncShape::Never => None,
        // To a `bytes` field every byte sequence is a valid value, so a
        // shorter one annotates nothing: clean, with no alignment to do.
        TruncShape::AnyByte => Some((soft, PreviewTier::Clean)),
        TruncShape::RecordBoundary => match top_level_boundary(payload, soft) {
            // The walk consumed the payload: every record fits, so
            // nothing is cut after all.
            Some(b) if b >= payload.len() => None,
            Some(b) => Some((b, PreviewTier::Clean)),
            // Spec 0318 S4: no boundary in reach — one record longer
            // than the room, or a length varint the walk could not
            // parse. Cut at the hard cap and let the rendering say so.
            // Showing the reader less than they asked for with no way to
            // explain it would be worse.
            None => Some((hard_cap(soft).min(payload.len()), PreviewTier::Ragged)),
        },
        TruncShape::CharBoundary => {
            // Walk back to the start of the character straddling the cut:
            // continuation bytes are `0b10xxxxxx`. At most three steps for
            // valid UTF-8, and bounded anyway so invalid input terminates.
            let mut k = soft;
            while k > 0 && (payload[k] & 0xC0) == 0x80 {
                k -= 1;
            }
            Some((k, PreviewTier::Clean))
        }
        // A varint's last byte is the one with the continuation bit
        // clear, so any position right after such a byte ends a whole
        // number of elements. At most ten steps.
        TruncShape::PackedVarint => {
            let mut k = soft;
            while k > 0 && (payload[k - 1] & 0x80) != 0 {
                k -= 1;
            }
            Some((k, PreviewTier::Clean))
        }
        TruncShape::PackedFixed(width) => Some((soft - soft % width, PreviewTier::Clean)),
    }
}
