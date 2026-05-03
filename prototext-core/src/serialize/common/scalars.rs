// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// ── Per-type protoc-compatible scalar formatters ───────────────────────────────
//
// One function per proto2 type, matching `protoc --decode` output exactly.
// See ext/prototext_codec/FLOATS_AND_DOUBLES.md §1 for the ground-truth tables.
//
// All of these are trivial wrappers today; they exist so that:
//   (a) render_text.rs and format_protoc_value have a single named call-site per type,
//   (b) future changes (e.g. adopting ryu for double) are made in one place only.

/// `int32` → signed decimal.
#[inline]
pub fn format_int32_protoc(v: i32) -> String {
    v.to_string()
}
/// `int64` → signed decimal.
#[inline]
pub fn format_int64_protoc(v: i64) -> String {
    v.to_string()
}
/// `uint32` → unsigned decimal.
#[inline]
pub fn format_uint32_protoc(v: u32) -> String {
    v.to_string()
}
/// `uint64` → unsigned decimal.
#[inline]
pub fn format_uint64_protoc(v: u64) -> String {
    v.to_string()
}
/// `sint32` → signed decimal (caller has already applied zigzag decode).
#[inline]
pub fn format_sint32_protoc(v: i32) -> String {
    v.to_string()
}
/// `sint64` → signed decimal (caller has already applied zigzag decode).
#[inline]
pub fn format_sint64_protoc(v: i64) -> String {
    v.to_string()
}
/// `fixed32` → unsigned decimal (NOT hex; protoc renders `fixed32` as decimal).
#[inline]
pub fn format_fixed32_protoc(v: u32) -> String {
    v.to_string()
}
/// `fixed64` → unsigned decimal (NOT hex).
#[inline]
pub fn format_fixed64_protoc(v: u64) -> String {
    v.to_string()
}
/// `sfixed32` → signed decimal.
#[inline]
pub fn format_sfixed32_protoc(v: i32) -> String {
    v.to_string()
}
/// `sfixed64` → signed decimal.
#[inline]
pub fn format_sfixed64_protoc(v: i64) -> String {
    v.to_string()
}
/// `bool` → `"true"` or `"false"`.
#[inline]
pub fn format_bool_protoc(v: bool) -> &'static str {
    if v {
        "true"
    } else {
        "false"
    }
}
/// `enum` → signed decimal (same representation as `int32`).
#[inline]
pub fn format_enum_protoc(v: i32) -> String {
    v.to_string()
}

// ── Wire-type fallback formatters ─────────────────────────────────────────────
//
// Used for unknown fields (field number absent from schema) and wire-type
// mismatches (field in schema but wire type differs from declared type).
//
// Protoc renders any such field solely by its actual wire type — schema is ignored.
// `--decode_raw` and `--decode=Msg` produce byte-for-byte identical output.
// See FLOATS_AND_DOUBLES.md §1.3 and §2.2 D2 for ground truth.

/// Unknown / mismatch VARINT (wt=0) → unsigned decimal (uint64).
#[inline]
pub fn format_wire_varint_protoc(v: u64) -> String {
    v.to_string()
}
/// Unknown / mismatch FIXED32 (wt=5) → `0x` + 8 lowercase hex digits (zero-padded).
#[inline]
pub fn format_wire_fixed32_protoc(v: u32) -> String {
    format!("0x{:08x}", v)
}
/// Unknown / mismatch FIXED64 (wt=1) → `0x` + 16 lowercase hex digits (zero-padded).
#[inline]
pub fn format_wire_fixed64_protoc(v: u64) -> String {
    format!("0x{:016x}", v)
}
