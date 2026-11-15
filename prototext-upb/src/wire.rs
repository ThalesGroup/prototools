// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The protobuf wire format, at the byte level: varints, tags,
//! length-delimited records, and a walk over a message's records. What
//! `set`, `descriptor`, `mend` and `text_format` read and write without a
//! schema.

/// Read a varint at `*pos`, advancing it. `None` if the buffer ends inside
/// the varint or it runs past ten bytes.
pub fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for i in 0..10 {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

pub fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub const VARINT: u32 = 0;
pub const FIXED64: u32 = 1;
pub const LEN: u32 = 2;
pub const START_GROUP: u32 = 3;
pub const END_GROUP: u32 = 4;
pub const FIXED32: u32 = 5;

pub fn put_tag(out: &mut Vec<u8>, number: u32, wire_type: u32) {
    put_varint(out, (u64::from(number) << 3) | u64::from(wire_type));
}

/// A length-delimited record: tag, length, payload.
pub fn put_len(out: &mut Vec<u8>, number: u32, payload: &[u8]) {
    put_tag(out, number, LEN);
    put_varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

/// One record of a message: its field number, wire type, payload (the
/// length-delimited content), value (a varint's), and all its bytes.
#[derive(Debug, Clone, Copy)]
pub struct Record<'a> {
    pub number: u32,
    pub wire_type: u32,
    pub payload: &'a [u8],
    pub value: u64,
    pub bytes: &'a [u8],
}

/// The records of `b`, or `None` when it is not well formed. A group is
/// not well formed here: a descriptor's own fields have none, and an
/// option holding one is inside a length-delimited record.
pub fn records(b: &[u8]) -> Option<Vec<Record<'_>>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < b.len() {
        let start = pos;
        let tag = read_varint(b, &mut pos)?;
        let number = u32::try_from(tag >> 3).ok()?;
        let wire_type = (tag & 7) as u32;
        let (mut payload, mut value): (&[u8], u64) = (&[], 0);
        match wire_type {
            VARINT => value = read_varint(b, &mut pos)?,
            FIXED64 => pos = pos.checked_add(8).filter(|&p| p <= b.len())?,
            FIXED32 => pos = pos.checked_add(4).filter(|&p| p <= b.len())?,
            LEN => {
                let len = usize::try_from(read_varint(b, &mut pos)?).ok()?;
                let end = pos.checked_add(len).filter(|&e| e <= b.len())?;
                payload = &b[pos..end];
                pos = end;
            }
            _ => return None,
        }
        out.push(Record {
            number,
            wire_type,
            payload,
            value,
            bytes: &b[start..pos],
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip_and_stop_at_ten_bytes() {
        for v in [0u64, 1, 127, 128, 300, u64::MAX] {
            let mut out = Vec::new();
            put_varint(&mut out, v);
            let mut pos = 0;
            assert_eq!(read_varint(&out, &mut pos), Some(v));
            assert_eq!(pos, out.len());
        }
        assert_eq!(read_varint(&[0x80; 11], &mut 0), None);
        assert_eq!(read_varint(&[0x80], &mut 0), None);
    }

    #[test]
    fn records_walk_a_message() {
        let mut m = Vec::new();
        put_tag(&mut m, 1, VARINT);
        put_varint(&mut m, 150);
        put_len(&mut m, 2, b"ab");
        let r = records(&m).unwrap();
        assert_eq!((r[0].number, r[0].wire_type, r[0].value), (1, VARINT, 150));
        assert_eq!((r[1].number, r[1].payload), (2, &b"ab"[..]));
        assert_eq!(r[1].bytes, &m[3..]);
        let mut g = Vec::new();
        put_tag(&mut g, 1, START_GROUP);
        assert!(records(&g).is_none());
    }
}
