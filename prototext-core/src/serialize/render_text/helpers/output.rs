// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use super::super::{CBL_START, INDENT_SIZE, LEVEL};

// ── Low-level output helpers ──────────────────────────────────────────────────

/// Write `bits` as zero-padded lowercase hex into `out` without heap allocation.
/// Uses 16 digits for doubles (bits > u32::MAX), 8 digits for floats.
#[inline]
pub(super) fn write_nan_hex(bits: u64, out: &mut Vec<u8>) {
    use std::io::Write as IoWrite;
    if bits > 0xFFFF_FFFF {
        let mut buf = [0u8; 16];
        write!(&mut buf[..], "{:016x}", bits).expect("write to stack buffer");
        out.extend_from_slice(&buf);
    } else {
        let mut buf = [0u8; 8];
        write!(&mut buf[..], "{:08x}", bits).expect("write to stack buffer");
        out.extend_from_slice(&buf);
    }
}

/// Push the current indentation (INDENT_SIZE × LEVEL spaces) into `out`.
#[inline]
pub(in super::super) fn push_indent(out: &mut Vec<u8>) {
    let n = INDENT_SIZE.with(|c| c.get()) * LEVEL.with(|c| c.get());
    for _ in 0..n {
        out.push(b' ');
    }
}

/// Write `v` as decimal ASCII digits into `out` — no heap allocation.
#[inline]
pub(in super::super) fn write_dec_i32(v: i32, out: &mut Vec<u8>) {
    if v < 0 {
        out.push(b'-');
        write_dec_u64(-(v as i64) as u64, out);
    } else {
        write_dec_u64(v as u64, out);
    }
}

/// Write `v` as decimal ASCII digits into `out` — no heap allocation.
#[inline]
pub(in super::super) fn write_dec_u64(v: u64, out: &mut Vec<u8>) {
    if v == 0 {
        out.push(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut pos = 20usize;
    let mut n = v;
    while n > 0 {
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    out.extend_from_slice(&buf[pos..]);
}

// ── Prefix writers ────────────────────────────────────────────────────────────

/// Write field-line prefix: `{spaces}name: `
#[inline]
pub(in super::super) fn wfl_prefix(name: &str, out: &mut Vec<u8>) {
    push_indent(out);
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b": ");
}

/// Write field-line prefix without String allocation.
///
/// When `is_wire_or_mismatch` is false and `fs` is `Some`, writes the schema
/// field name (or `[pkg.ext]` for extensions) directly from `fs.display_name()`.
/// Otherwise writes `num` as decimal.
#[inline]
pub(in super::super) fn wfl_prefix_n(
    num: u64,
    fs: Option<&super::super::FieldOrExt>,
    is_wire_or_mismatch: bool,
    out: &mut Vec<u8>,
) {
    push_indent(out);
    match fs.filter(|_| !is_wire_or_mismatch) {
        Some(fi) => out.extend_from_slice(fi.display_name().as_bytes()),
        None => write_dec_u64(num, out),
    }
    out.extend_from_slice(b": ");
}

/// Write open-brace prefix without String allocation.
///
/// When `is_wire_or_mismatch` is false and `fs` is `Some`, writes the schema
/// field name (or `[pkg.ext]` for extensions) directly from `fs.display_name()`.
/// Otherwise writes `num` as decimal.
#[inline]
pub(in super::super) fn wob_prefix_n(
    num: u64,
    fs: Option<&super::super::FieldOrExt>,
    is_wire_or_mismatch: bool,
    out: &mut Vec<u8>,
) {
    push_indent(out);
    match fs.filter(|_| !is_wire_or_mismatch) {
        Some(fi) => out.extend_from_slice(fi.display_name().as_bytes()),
        None => write_dec_u64(num, out),
    }
    out.extend_from_slice(b" {");
}

/// Write `{indent}}\n`.
#[inline]
pub(in super::super) fn write_close_brace(out: &mut Vec<u8>) {
    CBL_START.with(|c| c.set(out.len()));
    push_indent(out);
    out.push(b'}');
    out.push(b'\n');
}
