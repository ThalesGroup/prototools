// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// ── Varint parser result ──────────────────────────────────────────────────────

/// Result of parsing one protobuf varint from a byte slice.
///
/// Mirrors the Python `Varint` class in `lib/varint.py`.
///
/// Exactly one of `varint` or `varint_gar` is `Some`:
/// * `varint_gar` is `Some` when the varint is truncated (buffer ends before
///   the terminator byte) or exceeds 64 bits.
/// * `varint` is `Some` for a successfully decoded varint.
/// * `varint_ohb` counts trailing non-canonical (overhung) bytes: set when
///   the terminating byte is `0x00` preceded by one or more `0x80` bytes.
#[derive(Debug)]
pub struct VarintResult {
    /// Byte position immediately after the parsed varint.
    pub next_pos: usize,
    /// `Some(raw_bytes)` when the varint is garbage (truncated / too large).
    pub varint_gar: Option<Vec<u8>>,
    /// The decoded varint value (valid only when `varint_gar` is `None`).
    pub varint: Option<u64>,
    /// Number of non-canonical overhang bytes (valid only when `varint_gar` is `None`).
    pub varint_ohb: Option<u64>,
}

/// Parse one protobuf varint starting at `start` in `buf`.
///
/// Mirrors `Varint.__init__` in `lib/varint.py`.
///
/// OPT-3: #[inline] allows the compiler to merge this function into the
/// parse_wiretag and decoder hot loops, enabling intra-procedural optimizations
/// (constant-fold the shift sequence, avoid call overhead).  perf showed
/// parse_varint at 4.33% and parse_wiretag at 10.49% of Path A samples.
#[inline]
pub fn parse_varint(buf: &[u8], start: usize) -> VarintResult {
    let buflen = buf.len();
    assert!(start <= buflen);

    if start == buflen {
        // Empty buffer at this position → garbage (empty)
        return VarintResult {
            next_pos: start,
            varint_gar: Some(vec![]),
            varint: None,
            varint_ohb: None,
        };
    }

    let mut v: u64 = 0;
    let mut shift: u32 = 0;
    let mut pos = start;
    let mut too_big = false;

    loop {
        if pos >= buflen {
            // Truncated varint — return rest of buffer as garbage (matches Python)
            return VarintResult {
                next_pos: buflen,
                varint_gar: Some(buf[start..].to_vec()),
                varint: None,
                varint_ohb: None,
            };
        }
        let b = buf[pos];
        pos += 1;

        let bits = (b & 0x7f) as u64;
        if shift < 64 {
            // shift == 63: the 10th byte.  Only bit 0 is valid for a u64;
            // bits ≥ 2 would produce a value ≥ 2^64.
            if shift == 63 && bits > 1 {
                too_big = true;
            } else {
                v |= bits << shift;
            }
        } else {
            // ≥ 11th byte: any set bit overflows u64.
            if bits != 0 {
                too_big = true;
            }
        }
        shift += 7;

        if b & 0x80 == 0 {
            break; // terminator found
        }

        if shift > 70 {
            // Absurdly long varint (> 10 bytes): consume continuation bytes and
            // flag as too_big.
            while pos < buflen {
                let b2 = buf[pos];
                pos += 1;
                if (b2 & 0x7f) != 0 {
                    too_big = true;
                }
                if b2 & 0x80 == 0 {
                    break;
                }
            }
            break;
        }
    }

    if too_big {
        // Python sets pos = buflen before its else-clause fires, so varint_gar
        // always contains buf[start..] (rest of buffer) on overflow.  Matching
        // that behaviour ensures identical INVALID_VARINT content.
        return VarintResult {
            next_pos: buflen,
            varint_gar: Some(buf[start..].to_vec()),
            varint: None,
            varint_ohb: None,
        };
    }

    // The byte at buf[pos-1] is the terminator (the byte that ended the varint).
    // Use it directly instead of tracking `last_b` across loop iterations.
    let last_b = buf[pos - 1];

    // Check for overhung bytes: terminator is 0x00 preceded by ≥1 × 0x80
    let ohb = if last_b == 0x00 && pos > start + 1 {
        // Count trailing 0x80 bytes before the 0x00 terminator
        let mut count: u64 = 1;
        let mut p = pos - 2; // byte before the 0x00
        while p > start && buf[p] == 0x80 {
            count += 1;
            p -= 1;
        }
        Some(count)
    } else {
        None
    };

    VarintResult {
        next_pos: pos,
        varint_gar: None,
        varint: Some(v),
        varint_ohb: ohb,
    }
}

/// Encode a varint value (with optional overhang bytes) back to bytes.
///
/// Mirrors `Varint.__bytes__` in `lib/varint.py`.
pub fn encode_varint_bytes(value: u64, ohb: Option<u64>) -> Vec<u8> {
    let mut out = Vec::new();
    write_varint_ohb(value, ohb, &mut out);
    out
}

/// Append a varint encoding of `value` (with optional overhang bytes) directly
/// into `out`, with no allocation.
///
/// OPT-2: This is the in-place replacement for `encode_varint_bytes`.  The old
/// function allocated a fresh Vec<u8> per call (~18 ns each; 6× slower than
/// appending to an existing Vec).  Callers that already have a target buffer
/// should call this instead, eliminating the allocate-copy-free cycle that
/// showed up as 21% memmove + 11% malloc/free in the perf profile of Path A.
#[inline]
pub fn write_varint_ohb(value: u64, ohb: Option<u64>, out: &mut Vec<u8>) {
    let mut v = value;
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            out.push(b | 0x80);
        } else {
            out.push(b);
            break;
        }
    }
    if let Some(count) = ohb {
        if count > 0 {
            *out.last_mut().unwrap() |= 0x80; // make last byte a continuation
            for _ in 0..count - 1 {
                out.push(0x80);
            }
            out.push(0x00); // final terminator
        }
    }
}

// ── Wiretag parser result ─────────────────────────────────────────────────────

/// Result of parsing one protobuf wire tag (field number + wire type).
///
/// Mirrors the Python `Wiretag` class in `lib/wiretag.py`.
///
/// Exactly one of `wtag_gar` or `wtype` is valid:
/// * `wtag_gar` is `Some` when the wire type is > 5 (invalid) or the
///   field-number varint is truncated / too large.
/// * Otherwise `wtype` holds the wire type (0–5) and `wfield` the field number.
#[derive(Debug, Clone)]
pub struct WiretagResult {
    pub next_pos: usize,
    /// Raw bytes when the tag is garbage.
    pub wtag_gar: Option<Vec<u8>>,
    /// Wire type (0–5); valid only when `wtag_gar` is `None`.
    pub wtype: Option<u32>,
    /// Field number; valid only when `wtag_gar` is `None`.
    pub wfield: Option<u64>,
    /// Overhang count in the field-number varint.
    pub wfield_ohb: Option<u64>,
    /// `true` when field number is 0 or ≥ 2²⁹.
    pub wfield_oor: Option<bool>,
}

/// Parse one wire tag starting at `start` in `buf`.
///
/// Mirrors `Wiretag.__init__` in `lib/wiretag.py`.
///
/// OPT-3: #[inline] pairs with #[inline] on parse_varint so the compiler can
/// fold both into the decoder.rs hot loop without separate call frames.
#[inline]
pub fn parse_wiretag(buf: &[u8], start: usize) -> WiretagResult {
    let buflen = buf.len();
    assert!(start < buflen, "parse_wiretag called at end of buffer");

    let first_byte = buf[start];
    let wtype = (first_byte & 0x07) as u32;

    if wtype > 5 {
        // Invalid wire type: consume rest of buffer as garbage
        return WiretagResult {
            next_pos: buflen,
            wtag_gar: Some(buf[start..].to_vec()),
            wtype: None,
            wfield: None,
            wfield_ohb: None,
            wfield_oor: None,
        };
    }

    // The field number occupies bits 3.. of the varint.
    // Parse the whole tag as a varint, then extract field number from bits 3+.
    let vr = parse_varint(buf, start);

    if let Some(gar) = vr.varint_gar {
        // Truncated or too-large tag varint
        return WiretagResult {
            next_pos: vr.next_pos,
            wtag_gar: Some(gar),
            wtype: None,
            wfield: None,
            wfield_ohb: None,
            wfield_oor: None,
        };
    }

    let raw = vr.varint.unwrap();
    let field_number = raw >> 3;
    let ohb = vr.varint_ohb;
    let oor = if field_number == 0 || field_number >= (1 << 29) {
        Some(true)
    } else {
        None
    };

    WiretagResult {
        next_pos: vr.next_pos,
        wtag_gar: None,
        wtype: Some(wtype),
        wfield: Some(field_number),
        wfield_ohb: ohb,
        wfield_oor: oor,
    }
}
