// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

mod codecs;
mod varint;
mod wire;

pub use codecs::*;
pub use varint::*;
pub use wire::*;

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── varint round-trips ────────────────────────────────────────────────────

    #[test]
    fn varint_zero() {
        let buf = [0x00u8];
        let r = parse_varint(&buf, 0);
        assert_eq!(r.varint, Some(0));
        assert_eq!(r.varint_ohb, None);
        assert_eq!(r.next_pos, 1);
    }

    #[test]
    fn varint_one_byte() {
        let buf = [0x01u8];
        let r = parse_varint(&buf, 0);
        assert_eq!(r.varint, Some(1));
        assert_eq!(r.next_pos, 1);
    }

    #[test]
    fn varint_150() {
        // 150 = 0x96 0x01
        let buf = [0x96u8, 0x01];
        let r = parse_varint(&buf, 0);
        assert_eq!(r.varint, Some(150));
        assert_eq!(r.next_pos, 2);
        assert_eq!(r.varint_ohb, None);
    }

    #[test]
    fn varint_max_u64() {
        // max u64: 10 bytes of 0xFF followed by 0x01
        let buf = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
        let r = parse_varint(&buf, 0);
        assert_eq!(r.varint, Some(u64::MAX));
        assert_eq!(r.next_pos, 10);
    }

    #[test]
    fn varint_truncated() {
        // Continuation byte with no terminator
        let buf = [0x80u8, 0x80];
        let r = parse_varint(&buf, 0);
        assert!(r.varint_gar.is_some());
        assert!(r.varint.is_none());
    }

    #[test]
    fn varint_empty_at_end() {
        let buf = [0x01u8];
        let r = parse_varint(&buf, 1); // start == buflen
        assert!(r.varint_gar.is_some());
        assert_eq!(r.varint_gar.unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn varint_overhang_one() {
        // 0x00 encoded non-canonically as 0x80 0x00
        let buf = [0x80u8, 0x00];
        let r = parse_varint(&buf, 0);
        assert_eq!(r.varint, Some(0));
        assert_eq!(r.varint_ohb, Some(1));
    }

    #[test]
    fn varint_overhang_two() {
        // 0x00 encoded as 0x80 0x80 0x00  (2 overhang bytes)
        let buf = [0x80u8, 0x80, 0x00];
        let r = parse_varint(&buf, 0);
        assert_eq!(r.varint, Some(0));
        assert_eq!(r.varint_ohb, Some(2));
    }

    #[test]
    fn varint_encode_with_overhang() {
        let bytes = encode_varint_bytes(0, Some(1));
        assert_eq!(bytes, vec![0x80, 0x00]);

        let bytes2 = encode_varint_bytes(0, Some(2));
        assert_eq!(bytes2, vec![0x80, 0x80, 0x00]);

        let bytes3 = encode_varint_bytes(150, None);
        assert_eq!(bytes3, vec![0x96, 0x01]);
    }

    #[test]
    fn varint_encode_roundtrip() {
        for val in [0u64, 1, 127, 128, 300, 16383, 16384, u64::MAX] {
            let encoded = encode_varint_bytes(val, None);
            let r = parse_varint(&encoded, 0);
            assert_eq!(r.varint, Some(val), "roundtrip failed for {val}");
            assert_eq!(r.next_pos, encoded.len());
        }
    }

    // ── wiretag ──────────────────────────────────────────────────────────────

    #[test]
    fn wiretag_field1_varint() {
        // tag for field 1, wire type 0: (1 << 3) | 0 = 0x08
        let buf = [0x08u8];
        let r = parse_wiretag(&buf, 0);
        assert_eq!(r.wtype, Some(0));
        assert_eq!(r.wfield, Some(1));
        assert_eq!(r.wfield_ohb, None);
        assert_eq!(r.wfield_oor, None);
    }

    #[test]
    fn wiretag_invalid_wire_type() {
        // wire type 6 is invalid
        let buf = [0x06u8, 0x00, 0x01];
        let r = parse_wiretag(&buf, 0);
        assert!(r.wtag_gar.is_some());
        assert!(r.wtype.is_none());
    }

    #[test]
    fn wiretag_field_number_zero_is_oor() {
        // field number 0: wire type 0, field 0 → (0 << 3) | 0 = 0x00
        // but parse_wiretag asserts start < buflen, so use a buffer with content
        let buf = [0x00u8]; // tag byte = 0 → wire_type=0, field=0
        let r = parse_wiretag(&buf, 0);
        assert_eq!(r.wfield, Some(0));
        assert_eq!(r.wfield_oor, Some(true));
    }

    #[test]
    fn wiretag_overhung() {
        // Field 1, wire type 0 encoded non-canonically: (0x08) as 0x88 0x00
        let buf = [0x88u8, 0x00];
        let r = parse_wiretag(&buf, 0);
        assert_eq!(r.wtype, Some(0));
        assert_eq!(r.wfield, Some(1));
        assert_eq!(r.wfield_ohb, Some(1));
    }

    // ── numeric codecs ────────────────────────────────────────────────────────

    #[test]
    fn int32_negative() {
        // -1 as int32 is stored as 0xFFFFFFFF in a varint
        assert_eq!(decode_int32(0xFFFFFFFF), -1i32);
    }

    #[test]
    fn int64_negative() {
        assert_eq!(decode_int64(u64::MAX), -1i64);
    }

    #[test]
    fn sint32_roundtrip() {
        for v in [-1i32, 0, 1, -2, 2, i32::MIN, i32::MAX] {
            let encoded = if v >= 0 {
                ((v as u32) << 1) as u64
            } else {
                ((!v as u32) * 2 + 1) as u64
            };
            assert_eq!(decode_sint32(encoded), v, "sint32 roundtrip for {v}");
        }
    }

    #[test]
    fn sint64_roundtrip() {
        for v in [-1i64, 0, 1, -2, 2, i64::MIN, i64::MAX] {
            let encoded = if v >= 0 {
                (v as u64) << 1
            } else {
                ((!v as u64) << 1) | 1
            };
            assert_eq!(decode_sint64(encoded), v, "sint64 roundtrip for {v}");
        }
    }

    #[test]
    fn fixed32_little_endian() {
        let data = [0x01u8, 0x00, 0x00, 0x00];
        assert_eq!(decode_fixed32(&data), 1u32);
        let data2 = [0xFFu8, 0xFF, 0xFF, 0xFF];
        assert_eq!(decode_fixed32(&data2), u32::MAX);
    }

    #[test]
    fn fixed64_little_endian() {
        let data = [0x01u8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(decode_fixed64(&data), 1u64);
    }

    #[test]
    fn double_roundtrip() {
        let val = std::f64::consts::PI;
        let data = val.to_le_bytes();
        assert_eq!(decode_double(&data), val);
    }

    #[test]
    fn float_roundtrip() {
        let val = 1.5f32;
        let data = val.to_le_bytes();
        assert_eq!(decode_float(&data), val);
    }

    #[test]
    fn write_varint_field_roundtrip() {
        let mut buf = Vec::new();
        write_varint_field(1, 300, &mut buf);
        // tag: (1<<3)|0 = 0x08; value 300 = 0xAC 0x02
        assert_eq!(buf, vec![0x08, 0xAC, 0x02]);
    }

    #[test]
    fn write_len_field_roundtrip() {
        let mut buf = Vec::new();
        write_len_field(2, b"hi", &mut buf);
        // tag: (2<<3)|2 = 0x12; length: 0x02; data: 0x68 0x69
        assert_eq!(buf, vec![0x12, 0x02, 0x68, 0x69]);
    }
}
