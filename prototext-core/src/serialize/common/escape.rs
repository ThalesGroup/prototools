// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// ── Byte / string escaping ────────────────────────────────────────────────────
// Mirrors escape_bytes() / escape_string() in common.py.

pub fn escape_bytes(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    for &byte in b {
        match byte {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            32..=126 => out.push(byte as char),
            _ => {
                out.push('\\');
                out.push_str(&format!("{:03o}", byte));
            }
        }
    }
    out
}

/// Zero-allocation variant of `escape_bytes`: appends escaped bytes directly to `out`.
#[inline]
pub fn escape_bytes_into(b: &[u8], out: &mut Vec<u8>) {
    for &byte in b {
        match byte {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'"' => out.extend_from_slice(b"\\\""),
            b'\'' => out.extend_from_slice(b"\\'"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            32..=126 => out.push(byte),
            _ => {
                // Octal escape: \NNN
                out.push(b'\\');
                out.push(b'0' + (byte >> 6));
                out.push(b'0' + ((byte >> 3) & 7));
                out.push(b'0' + (byte & 7));
            }
        }
    }
}

pub fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 32 => {
                out.push_str(&format!("\\{:03o}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Zero-allocation variant of `escape_string`: appends escaped bytes directly to `out`.
#[inline]
pub fn escape_string_into(s: &str, out: &mut Vec<u8>) {
    for c in s.chars() {
        match c {
            '\\' => out.extend_from_slice(b"\\\\"),
            '"' => out.extend_from_slice(b"\\\""),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            c if (c as u32) < 32 => {
                // Octal escape: \NNN (c < 32 so top octal digit is always 0)
                let v = c as u32;
                out.push(b'\\');
                out.push(b'0' + (v >> 6) as u8);
                out.push(b'0' + ((v >> 3) & 7) as u8);
                out.push(b'0' + (v & 7) as u8);
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
}
