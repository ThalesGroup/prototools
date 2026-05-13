// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use super::super::FieldOrExt;
use super::super::{ANNOTATIONS, CBL_START};
use super::annotations::{push_tag_modifiers, AnnWriter};
use super::output::{wfl_prefix, wfl_prefix_n, write_nan_hex};

use crate::serialize::common::escape_bytes_into;

// ── Field renderers ───────────────────────────────────────────────────────────

/// Context for `render_scalar` — groups the per-field rendering parameters
/// to avoid an excessive argument list.
pub(in super::super) struct ScalarCtx<'a> {
    pub(in super::super) field_number: u64,
    pub(in super::super) field_schema: Option<&'a FieldOrExt>,
    pub(in super::super) tag_ohb: Option<u64>,
    pub(in super::super) tag_oor: bool,
    pub(in super::super) len_ohb: Option<u64>,
    /// Lowercase v2 wire-type name ("fixed64", "fixed32", "bytes", …).
    /// Only emitted for unknown or raw-wire fields.
    pub(in super::super) wire_type_name: &'a str,
    /// Non-canonical NaN bit pattern; emits `nan_bits: 0x…` annotation modifier.
    pub(in super::super) nan_bits: Option<u64>,
    /// True when the wire type is known but doesn't match the schema type.
    pub(in super::super) type_mismatch: bool,
}

/// Render a non-varint scalar (FIXED64, FIXED32, string, bytes, wire-bytes).
///
/// `wire_type_name` (in `ctx`) is only emitted for unknown or raw-wire fields.
/// Known fields emit field_decl FIRST, then modifiers.
pub(in super::super) fn render_scalar(
    ctx: &ScalarCtx<'_>,
    value_str: &str,
    is_wire: bool, // true for WireBytes/WireFixed* or wire-type mismatch
    out: &mut Vec<u8>,
) {
    let ScalarCtx {
        field_number,
        field_schema,
        tag_ohb,
        tag_oor,
        len_ohb,
        wire_type_name,
        nan_bits,
        type_mismatch,
    } = *ctx;
    let annotations = ANNOTATIONS.with(|c| c.get());
    let unknown = field_schema.is_none();

    if !annotations && (unknown || is_wire) {
        return;
    }

    // v2 key rule: numeric for unknown/wire, named for known fields.
    let use_numeric_key = unknown || is_wire;
    wfl_prefix_n(field_number, field_schema, use_numeric_key, out);
    out.extend_from_slice(value_str.as_bytes());
    if annotations {
        let mut aw = AnnWriter::new();
        if unknown || is_wire {
            // Unknown/wire: wire type FIRST, then modifiers, NO field_decl
            aw.push_wire(out, wire_type_name);
            if type_mismatch {
                aw.push(out, b"TYPE_MISMATCH");
            }
            push_tag_modifiers(&mut aw, out, tag_ohb, tag_oor, len_ohb);
        } else {
            // Known field: field_decl FIRST, then modifiers
            aw.push_field_decl(out, field_number, field_schema, None, None);
            push_tag_modifiers(&mut aw, out, tag_ohb, tag_oor, len_ohb);
            if let Some(bits) = nan_bits {
                aw.sep(out);
                out.extend_from_slice(b"nan_bits: 0x");
                // Write hex digits: 16 for double (u64), 8 for float (u32 stored as u64)
                write_nan_hex(bits, out);
            }
        }
    }
    out.push(b'\n');
    CBL_START.with(|c| c.set(out.len())); // content line: set past-end to inhibit folding
}

/// Render INVALID_VARINT / INVALID_FIXED64 / INVALID_FIXED32 / INVALID_LEN /
/// INVALID_PACKED_RECORDS / INVALID_STRING / INVALID_GROUP_END as `N: "bytes"`.
///
/// v2: always uses numeric key; emits INVALID_* wire type name; no field_decl.
pub(in super::super) fn render_invalid(
    field_number: u64,
    _field_schema: Option<&FieldOrExt>,
    tag_ohb: Option<u64>,
    tag_oor: bool,
    inv_name: &str,
    raw: &[u8],
    out: &mut Vec<u8>,
) {
    let annotations = ANNOTATIONS.with(|c| c.get());
    // v2: always numeric key for invalid fields (no field name, no field_decl).
    wfl_prefix_n(field_number, None, true, out);
    out.push(b'"');
    escape_bytes_into(raw, out);
    out.push(b'"');
    if annotations {
        let mut aw = AnnWriter::new();
        aw.push_wire(out, inv_name);
        push_tag_modifiers(&mut aw, out, tag_ohb, tag_oor, None);
        // v2: NO field_decl for invalid fields.
    }
    out.push(b'\n');
    CBL_START.with(|c| c.set(out.len())); // content line: set past-end to inhibit folding
}

/// Special case: InvalidTagType has no valid field number.
pub(in super::super) fn render_invalid_tag_type(raw: &[u8], out: &mut Vec<u8>) {
    let annotations = ANNOTATIONS.with(|c| c.get());
    wfl_prefix("0", out);
    out.push(b'"');
    escape_bytes_into(raw, out);
    out.push(b'"');
    if annotations {
        let mut aw = AnnWriter::new();
        aw.push(out, b"INVALID_TAG_TYPE"); // v2: no trailing `;`
    }
    out.push(b'\n');
    CBL_START.with(|c| c.set(out.len())); // content line: set past-end to inhibit folding
}

/// Render a TRUNCATED_BYTES field.
///
/// v2: always numeric key; `TRUNCATED_BYTES; MISSING: N`; no field_decl.
pub(in super::super) fn render_truncated_bytes(
    field_number: u64,
    tag_ohb: Option<u64>,
    tag_oor: bool,
    len_ohb: Option<u64>,
    missing: u64,
    raw: &[u8],
    out: &mut Vec<u8>,
) {
    let annotations = ANNOTATIONS.with(|c| c.get());
    // v2: always numeric key for invalid fields.
    wfl_prefix_n(field_number, None, true, out);
    out.push(b'"');
    escape_bytes_into(raw, out);
    out.push(b'"');
    if annotations {
        let mut aw = AnnWriter::new();
        aw.push(out, b"TRUNCATED_BYTES"); // invalid wire type, ALL CAPS
        push_tag_modifiers(&mut aw, out, tag_ohb, tag_oor, len_ohb);
        aw.push_u64_mod(out, b"MISSING: ", missing); // invalid modifier, ALL CAPS
                                                     // v2: NO field_decl for invalid fields.
    }
    out.push(b'\n');
    CBL_START.with(|c| c.set(out.len())); // content line: set past-end to inhibit folding
}
