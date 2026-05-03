// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// ── Numeric type codecs ───────────────────────────────────────────────────────
// Mirror prototext/helpers.py exactly.  All functions are trivial but must
// be correct — errors here silently break round-trip fidelity.

/// Decode varint as int64 (two's complement).
#[inline]
pub fn decode_int64(v: u64) -> i64 {
    v as i64
}

/// Decode varint as int32 (two's complement, low 32 bits).
#[inline]
pub fn decode_int32(v: u64) -> i32 {
    (v as u32) as i32
}

/// Decode varint as uint32.
#[inline]
pub fn decode_uint32(v: u64) -> u32 {
    v as u32
}

/// Decode varint as uint64.
#[inline]
pub fn decode_uint64(v: u64) -> u64 {
    v
}

/// Decode varint as bool (0 → false, 1 → true).
#[inline]
pub fn decode_bool(v: u64) -> bool {
    v != 0
}

/// Decode varint as sint32 (zig-zag).
#[inline]
pub fn decode_sint32(v: u64) -> i32 {
    let n = v as u32;
    ((n >> 1) as i32) ^ -((n & 1) as i32)
}

/// Decode varint as sint64 (zig-zag).
#[inline]
pub fn decode_sint64(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// Decode 4 little-endian bytes as fixed32 (uint32).
#[inline]
pub fn decode_fixed32(data: &[u8]) -> u32 {
    u32::from_le_bytes(data[..4].try_into().unwrap())
}

/// Decode 4 little-endian bytes as sfixed32 (int32).
#[inline]
pub fn decode_sfixed32(data: &[u8]) -> i32 {
    i32::from_le_bytes(data[..4].try_into().unwrap())
}

/// Decode 4 little-endian bytes as f32.
#[inline]
pub fn decode_float(data: &[u8]) -> f32 {
    f32::from_le_bytes(data[..4].try_into().unwrap())
}

/// Decode 8 little-endian bytes as fixed64 (uint64).
#[inline]
pub fn decode_fixed64(data: &[u8]) -> u64 {
    u64::from_le_bytes(data[..8].try_into().unwrap())
}

/// Decode 8 little-endian bytes as sfixed64 (int64).
#[inline]
pub fn decode_sfixed64(data: &[u8]) -> i64 {
    i64::from_le_bytes(data[..8].try_into().unwrap())
}

/// Decode 8 little-endian bytes as f64.
#[inline]
pub fn decode_double(data: &[u8]) -> f64 {
    f64::from_le_bytes(data[..8].try_into().unwrap())
}
