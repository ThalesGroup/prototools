// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Any-field expansion for `google.protobuf.Any` (spec 0089).

use std::ops::Range;
use std::sync::Arc;

use prost_reflect::MessageDescriptor;

use super::super::sink::{NestedKind, Sink};
use super::super::{descend, enter_level, render_message, ANY_LOADER, EXPAND_ANY};
use super::len_field::FieldCtx;

use super::group_scan::scan_group_extent;
use crate::helpers::{
    parse_varint, parse_wiretag, payload_end, WT_I32, WT_I64, WT_LEN, WT_START_GROUP, WT_VARINT,
};
use crate::serialize::common::escape_string_into;

// ── Wire scan helper ──────────────────────────────────────────────────────────

/// Minimal scan result from a flat `Any` payload.
struct AnyFields<'a> {
    type_url: &'a str,
    value: Option<&'a [u8]>,
}

/// Scan `data` for field 1 (type_url string) and field 2 (value bytes).
/// Returns `None` if the payload cannot be scanned, type_url is absent/empty,
/// or if value appeared before type_url.
fn scan_any_fields(data: &[u8]) -> Option<AnyFields<'_>> {
    let mut pos = 0;
    let buflen = data.len();

    let mut type_url: Option<&str> = None;
    let mut value: Option<&[u8]> = None;
    let mut value_before_type_url = false;

    while pos < buflen {
        let tag = parse_wiretag(data, pos);
        if tag.wtag_gar.is_some() {
            return None;
        }
        let field_number = tag.wfield.unwrap();
        let wire_type = tag.wtype.unwrap();
        pos = tag.next_pos;

        match wire_type {
            WT_VARINT => {
                let vr = parse_varint(data, pos);
                if vr.varint_gar.is_some() {
                    return None;
                }
                pos = vr.next_pos;
            }
            WT_I64 => {
                pos = payload_end(pos, 8, buflen)?;
            }
            WT_LEN => {
                let lr = parse_varint(data, pos);
                if lr.varint_gar.is_some() {
                    return None;
                }
                pos = lr.next_pos;
                let end = payload_end(pos, lr.varint.unwrap(), buflen)?;
                let payload = &data[pos..end];
                pos = end;
                match field_number {
                    1 => {
                        // type_url
                        let s = std::str::from_utf8(payload).ok()?;
                        if s.is_empty() {
                            return None;
                        }
                        type_url = Some(s);
                    }
                    2 => {
                        // value bytes
                        if type_url.is_none() {
                            value_before_type_url = true;
                        }
                        value = Some(payload);
                    }
                    _ => {}
                }
            }
            WT_START_GROUP => {
                // Skip blindly to the matching END_GROUP. `Some(field_number)`
                // keeps this caller's pre-spec-0171 strictness: a group whose
                // closing tag names a different field makes the whole Any scan
                // decline, and the field falls back to an ordinary render.
                pos = scan_group_extent(data, pos, Some(field_number))?;
            }
            WT_I32 => {
                pos = payload_end(pos, 4, buflen)?;
            }
            _ => return None,
        }
    }

    if value_before_type_url {
        return None;
    }
    let type_url = type_url?;
    Some(AnyFields { type_url, value })
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Try to render a `google.protobuf.Any` field with expansion.
///
/// Called in place of the normal nested-message dispatch in `render_len_field`
/// when:
/// - `EXPAND_ANY` is true, and
/// - the field's schema type is `google.protobuf.Any`.
///
/// Returns `true` when expansion was performed (caller must return immediately).
/// Returns `false` when expansion is not possible (caller falls through to
/// normal rendering).
pub(in super::super) fn render_any_expansion<S: Sink>(
    ctx: FieldCtx<'_>,
    schema_present: bool,
    raw_range: Range<usize>,
    data: &[u8],
    sink: &mut S,
) -> bool {
    let FieldCtx {
        field_number,
        field_schema,
        tag,
    } = ctx;
    if !EXPAND_ANY.with(|c| c.get()) {
        return false;
    }

    let fields = match scan_any_fields(data) {
        Some(f) => f,
        None => return false,
    };

    // Resolve FQDN from type_url: segment after last '/'.
    let fqdn = if let Some(slash) = fields.type_url.rfind('/') {
        &fields.type_url[slash + 1..]
    } else {
        fields.type_url
    };

    // JIT-load via ANY_LOADER (spec 0099); re-derives fresh from the
    // canonical pool on every call, never caches a value across calls
    // (spec 0106 S2 — the `all_schemas` snapshot fast path is gone).
    let resolved_desc: Option<Arc<MessageDescriptor>> =
        ANY_LOADER.with(|l| l.borrow_mut().as_mut().and_then(|f| f(fqdn)));
    let resolved_desc = match resolved_desc {
        Some(d) => d,
        None => return false,
    };

    // ── Outer Any field opener ────────────────────────────────────────────────
    let outer_mark = sink.begin_nested(
        field_number,
        field_schema,
        tag,
        NestedKind::Message {
            // An `Any` expansion: the schema decided, no probe ran.
            probed_as_message: None,
        },
        raw_range.start,
        raw_range.end - data.len(),
    );
    let outer_guard = enter_level(sink);

    // ── type_url line (virtual: not backed by a real FieldOrExt) ─────────────
    let mut tv = Vec::new();
    tv.push(b'"');
    escape_string_into(fields.type_url, &mut tv);
    tv.push(b'"');
    let tv = String::from_utf8(tv).expect("escape_string_into produces valid UTF-8");
    sink.virtual_scalar("type_url", Some("string = 1"), &tv, raw_range.clone());

    // ── value { opener (virtual) ──────────────────────────────────────────────
    let value_bytes: &[u8] = fields.value.unwrap_or(&[]);
    // Local offset of `value_bytes` within `data` (the Any's own payload,
    // itself already the active coordinate frame after `outer_mark` was
    // opened above) — both are genuine sub-slices of the same underlying
    // buffer, so pointer subtraction gives the exact offset (spec 0110 §3).
    // `fields.value: None` falls back to a dangling empty slice above, which
    // isn't a sub-slice of `data` — guard against that case explicitly.
    let value_payload_start = fields
        .value
        .map(|v| v.as_ptr() as usize - data.as_ptr() as usize)
        .unwrap_or(0);
    let value_ann = format!("{} = 2", resolved_desc.name());
    let value_mark = sink.begin_virtual_nested(
        "value",
        Some(&value_ann),
        Some(resolved_desc.full_name()),
        raw_range.start,
        value_payload_start,
    );

    // ── recurse into value bytes ──────────────────────────────────────────────
    // `descend` decrements LEVEL on the way out, before "value" is closed.
    let descended = descend(sink, |sink| {
        render_message(
            value_bytes,
            0,
            None,
            Some(&*resolved_desc),
            schema_present,
            sink,
        );
    });

    // ── close value block ─────────────────────────────────────────────────────
    // Same coordinate frame as `value_mark` was opened in (the Any's own
    // payload `data`, active since `outer_mark`'s push above): use the exact
    // `value_bytes` sub-slice bounds, not the outer field's `raw_range`
    // (spec 0110 §3).
    sink.end_nested(
        value_mark,
        value_payload_start..value_payload_start + value_bytes.len(),
        None,
    );
    // Spec 0249 S1: `value` is the node that lost its body, not the `Any`
    // field around it — the wrapper's own `type_url` line was emitted.
    if !descended {
        sink.note_undescended();
    }

    // ── close Any block ───────────────────────────────────────────────────────
    // Drop outer_guard to decrement LEVEL back to the Any field's level before
    // closing.
    drop(outer_guard);
    sink.end_nested(outer_mark, raw_range, None);

    true
}
