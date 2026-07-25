// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! MessageSet expansion for `message_set_wire_format` messages (spec 0100).

use std::ops::Range;

use prost_reflect::{Kind, MessageDescriptor};

use super::super::sink::{NestedKind, Sink, TagFacts};
use super::super::{enter_level, render_message, ANY_LOADER};
use super::len_field::FieldCtx;

use crate::helpers::{
    parse_varint, parse_wiretag, payload_end, WT_END_GROUP, WT_LEN, WT_START_GROUP, WT_VARINT,
};

// ── Heuristic ─────────────────────────────────────────────────────────────────

/// Return `true` when `desc` is a MessageSet (spec 0100 §1).
///
/// Both conditions must hold (from `google/protobuf/descriptor.proto`):
/// 1. `message_set_wire_format` option is `true` — the canonical binary flag.
/// 2. No regular fields (`desc.fields().count() == 0`) — required by the spec.
///
/// No name check: multiple MessageSet-typed messages with different names can
/// coexist in a single FileDescriptorSet and must each be handled independently.
pub(in super::super) fn is_message_set(desc: &MessageDescriptor) -> bool {
    let msf = desc
        .descriptor_proto()
        .options
        .as_ref()
        .and_then(|o| o.message_set_wire_format)
        .unwrap_or(false);
    msf && desc.fields().count() == 0
}

// ── Wire scan ─────────────────────────────────────────────────────────────────

/// One parsed item from a MessageSet wire payload.
struct MsItem<'a> {
    type_id: u64,
    message: Option<&'a [u8]>,
}

/// Scan `data` for repeated group records (field 1, wire type START_GROUP).
/// Each group should contain field 2 (type_id varint) and field 3 (message LEN).
/// Returns `None` on parse error.
fn scan_message_set(data: &[u8]) -> Option<Vec<MsItem<'_>>> {
    let mut items: Vec<MsItem<'_>> = Vec::new();
    let buflen = data.len();
    let mut pos = 0;

    while pos < buflen {
        let tag = parse_wiretag(data, pos);
        if tag.wtag_gar.is_some() {
            return None;
        }
        let field_number = tag.wfield.unwrap();
        let wire_type = tag.wtype.unwrap();
        pos = tag.next_pos;

        if wire_type != WT_START_GROUP || field_number != 1 {
            // Unexpected field: bail out; fallback renders raw.
            return None;
        }

        // Parse the group contents: field 2 (type_id) and field 3 (message).
        let mut type_id: Option<u64> = None;
        let mut message: Option<&[u8]> = None;

        loop {
            if pos >= buflen {
                return None; // truncated group
            }
            let inner_tag = parse_wiretag(data, pos);
            if inner_tag.wtag_gar.is_some() {
                return None;
            }
            let inner_field = inner_tag.wfield.unwrap();
            let inner_wire = inner_tag.wtype.unwrap();
            pos = inner_tag.next_pos;

            match (inner_field, inner_wire) {
                (1, WT_END_GROUP) => {
                    // End of this group item.
                    break;
                }
                (2, WT_VARINT) => {
                    let vr = parse_varint(data, pos);
                    if vr.varint_gar.is_some() {
                        return None;
                    }
                    type_id = Some(vr.varint.unwrap());
                    pos = vr.next_pos;
                }
                (3, WT_LEN) => {
                    let lr = parse_varint(data, pos);
                    if lr.varint_gar.is_some() {
                        return None;
                    }
                    pos = lr.next_pos;
                    let end = payload_end(pos, lr.varint.unwrap(), buflen)?;
                    message = Some(&data[pos..end]);
                    pos = end;
                }
                (_, WT_END_GROUP) => {
                    // END_GROUP for a different field number — malformed.
                    return None;
                }
                _ => {
                    // Unknown field inside group: skip.
                    pos = skip_field(data, pos, inner_wire)?;
                }
            }
        }

        items.push(MsItem {
            type_id: type_id.unwrap_or(0),
            message,
        });
    }

    Some(items)
}

/// Skip one field value of `wire_type` starting at `pos`.  Returns new `pos`.
fn skip_field(buf: &[u8], mut pos: usize, wire_type: u32) -> Option<usize> {
    let buflen = buf.len();
    match wire_type {
        WT_VARINT => {
            let vr = parse_varint(buf, pos);
            if vr.varint_gar.is_some() {
                return None;
            }
            pos = vr.next_pos;
        }
        1 /* WT_I64 */ => {
            pos = payload_end(pos, 8, buflen)?;
        }
        WT_LEN => {
            let lr = parse_varint(buf, pos);
            if lr.varint_gar.is_some() {
                return None;
            }
            pos = lr.next_pos;
            pos = payload_end(pos, lr.varint.unwrap(), buflen)?;
        }
        5 /* WT_I32 */ => {
            pos = payload_end(pos, 4, buflen)?;
        }
        _ => return None,
    }
    Some(pos)
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Render a `MessageSet` field with expansion (spec 0100 §2).
///
/// Emits the outer block, then for each group item:
/// - JIT-loads the extension FDP via `ANY_LOADER` sentinel.
/// - Resolves the extension descriptor from `msg_desc`.
/// - Renders field 1 (group), field 2 (type_id varint), field 3 (message LEN)
///   as schemaless child lines.
/// - Falls back to schemaless LENDEL cascade for unresolved items.
pub(in super::super) fn render_message_set_expansion<S: Sink>(
    msg_desc: &MessageDescriptor,
    ctx: FieldCtx<'_>,
    schema_present: bool,
    raw_range: Range<usize>,
    data: &[u8],
    sink: &mut S,
) {
    let FieldCtx {
        field_number,
        field_schema,
        tag,
    } = ctx;
    // ── Outer block opener ────────────────────────────────────────────────────
    let outer_mark = sink.begin_nested(
        field_number,
        field_schema,
        tag,
        NestedKind::Message,
        raw_range.start,
        raw_range.end - data.len(),
    );

    let _outer = enter_level(sink);

    // ── Parse groups ──────────────────────────────────────────────────────────
    let items = match scan_message_set(data) {
        Some(items) => items,
        None => {
            // Malformed: render raw via schemaless LENDEL cascade.
            super::len_field::render_len_field(
                FieldCtx {
                    field_number: 1,
                    field_schema: None,
                    tag: TagFacts::default(),
                },
                schema_present,
                raw_range.clone(),
                data,
                sink,
            );
            drop(_outer);
            sink.end_nested(outer_mark, raw_range, None);
            return;
        }
    };

    let extendee_fqdn = msg_desc.full_name();

    for item in &items {
        let type_id = item.type_id;

        // ── JIT-load extension FDP via ANY_LOADER sentinel ────────────────────
        // Sentinel key: "<extendee_fqdn>/<type_id>" (spec 0100 §5.2).
        //
        // The loader JIT-loads the FDP containing the extension into the lazy
        // pool and returns the extension's payload MessageDescriptor from the
        // updated pool.  We use that return value directly rather than calling
        // msg_desc.get_extension() on the stale descriptor: prost-reflect uses
        // Arc::make_mut when adding FDPs, so lazy.pool may have forked away
        // from the Arc that msg_desc holds, making get_extension() blind to the
        // newly-registered extension.
        let sentinel_key = format!("{extendee_fqdn}/{type_id}");
        let inner_msg_desc_opt: Option<MessageDescriptor> = ANY_LOADER
            .with(|l| l.borrow_mut().as_mut().and_then(|f| f(&sentinel_key)))
            .map(|arc| (*arc).clone());

        // Fallback: if no loader is installed (eager pool path where the full
        // FDS was decoded upfront), look up via msg_desc directly.
        let inner_msg_desc_opt: Option<MessageDescriptor> = if inner_msg_desc_opt.is_none() {
            msg_desc.get_extension(type_id as u32).and_then(|ext| {
                if let Kind::Message(inner) = ext.kind() {
                    Some(inner)
                } else {
                    None
                }
            })
        } else {
            inner_msg_desc_opt
        };

        if let Some(inner_msg_desc) = inner_msg_desc_opt {
            // ── Resolved: render with canonical virtual field names ────────────
            // Item {  #@ group; Item = 1
            // `Item` doesn't itself recurse into a new sub-slice — its
            // children (type_id, message) stay in the same coordinate
            // frame as the MessageSet's own payload (`data`), hence `0`.
            let item_mark = sink.begin_virtual_nested(
                "Item",
                Some("group; Item = 1"),
                None,
                raw_range.start,
                0,
            );
            {
                let _group = enter_level(sink);

                // type_id: <value>  #@ int32 = 2
                sink.virtual_scalar(
                    "type_id",
                    Some("int32 = 2"),
                    &type_id.to_string(),
                    raw_range.clone(),
                );

                // message {  #@ InnerTypeName = 3
                let msg_bytes = item.message.unwrap_or(&[]);
                // Local offset of `msg_bytes` within `data` (the MessageSet's
                // own payload, itself already the active coordinate frame
                // after `outer_mark` was opened above) — both are genuine
                // sub-slices of the same underlying buffer, so pointer
                // subtraction gives the exact offset (spec 0110 §3).
                // `item.message: None` falls back to a dangling empty slice,
                // which isn't a sub-slice of `data` — guard against that.
                let msg_payload_start = item
                    .message
                    .map(|m| m.as_ptr() as usize - data.as_ptr() as usize)
                    .unwrap_or(0);
                let msg_ann = format!("{} = 3", inner_msg_desc.name());
                let msg_mark = sink.begin_virtual_nested(
                    "message",
                    Some(&msg_ann),
                    Some(inner_msg_desc.full_name()),
                    raw_range.start,
                    msg_payload_start,
                );
                {
                    let _msg = enter_level(sink);
                    render_message(
                        msg_bytes,
                        0,
                        None,
                        Some(&inner_msg_desc),
                        schema_present,
                        sink,
                    );
                }
                // Same coordinate frame as `msg_mark` was opened in (the
                // MessageSet's own payload `data`): use the exact `msg_bytes`
                // sub-slice bounds, not the outer field's `raw_range`
                // (spec 0110 §3).
                sink.end_nested(
                    msg_mark,
                    msg_payload_start..msg_payload_start + msg_bytes.len(),
                    None,
                ); // closes message
            }
            // `Item`'s own frame is `data` itself (payload_start was `0`
            // above), and `scan_message_set` doesn't track each item's own
            // precise group byte-range — use the full extent of `data` as a
            // coarse but correctly in-bounds/nested range (spec 0110 §3).
            sink.end_nested(item_mark, 0..data.len(), None); // closes Item group
        } else {
            // ── Unresolved: schemaless LENDEL cascade for the group payload ───
            // Render the group's `message` bytes (field 3 payload) as a
            // schemaless LEN field at number 1.
            let msg_bytes = item.message.unwrap_or(&[]);
            super::len_field::render_len_field(
                FieldCtx {
                    field_number: 1,
                    field_schema: None,
                    tag: TagFacts::default(),
                },
                schema_present,
                raw_range.clone(),
                msg_bytes,
                sink,
            );
        }
    }

    drop(_outer);
    sink.end_nested(outer_mark, raw_range, None);
}
