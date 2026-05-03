// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// ── Wire-type constants ───────────────────────────────────────────────────────

pub const WT_VARINT: u32 = 0;
pub const WT_I64: u32 = 1;
pub const WT_LEN: u32 = 2;
pub const WT_START_GROUP: u32 = 3;
pub const WT_END_GROUP: u32 = 4;
pub const WT_I32: u32 = 5;

// ── Wire-encoding helpers ─────────────────────────────────────────────────────

/// Append a raw varint encoding of `value` to `buf`.
#[inline]
pub fn write_varint(value: u64, buf: &mut Vec<u8>) {
    let mut v = value;
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            buf.push(b | 0x80);
        } else {
            buf.push(b);
            break;
        }
    }
}

/// Append a field tag (wire_type 0–5) to `buf`.
#[inline]
pub fn write_tag(field_number: u32, wire_type: u32, buf: &mut Vec<u8>) {
    write_varint(((field_number as u64) << 3) | (wire_type as u64), buf);
}

/// Append a VARINT field to `buf`.
#[inline]
pub fn write_varint_field(field_number: u32, value: u64, buf: &mut Vec<u8>) {
    write_tag(field_number, WT_VARINT, buf);
    write_varint(value, buf);
}

/// Append a bool field (VARINT) to `buf` — only written when `true`.
#[inline]
pub fn write_bool_field(field_number: u32, value: bool, buf: &mut Vec<u8>) {
    if value {
        write_tag(field_number, WT_VARINT, buf);
        buf.push(1u8);
    }
}

/// Append an optional VARINT field to `buf` — only written when `Some`.
#[inline]
pub fn write_opt_varint_field(field_number: u32, value: Option<u64>, buf: &mut Vec<u8>) {
    if let Some(v) = value {
        write_varint_field(field_number, v, buf);
    }
}

/// Append a LEN-delimited field to `buf`.
#[inline]
pub fn write_len_field(field_number: u32, data: &[u8], buf: &mut Vec<u8>) {
    write_tag(field_number, WT_LEN, buf);
    write_varint(data.len() as u64, buf);
    buf.extend_from_slice(data);
}

/// Append a fixed-32 field to `buf`.
pub fn write_fixed32_field(field_number: u32, value: u32, buf: &mut Vec<u8>) {
    write_tag(field_number, WT_I32, buf);
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Append a fixed-64 field to `buf`.
pub fn write_fixed64_field(field_number: u32, value: u64, buf: &mut Vec<u8>) {
    write_tag(field_number, WT_I64, buf);
    buf.extend_from_slice(&value.to_le_bytes());
}
