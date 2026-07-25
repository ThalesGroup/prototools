// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Blind group-extent scanning (spec 0171 §S3).

use crate::helpers::{
    parse_varint, parse_wiretag, payload_end, WT_END_GROUP, WT_I32, WT_I64, WT_LEN, WT_START_GROUP,
    WT_VARINT,
};

/// Offset just past the `END_GROUP` tag that closes the `START_GROUP` whose
/// own tag ends at `pos`, or `None` if the buffer runs out or a tag along
/// the way is unparsable.
///
/// `expected` is the opening tag's field number when the caller wants the
/// closing tag checked against it — `None` is returned on a mismatch — or
/// `None` when the caller only wants the extent. The depth-cap site in
/// `render_message` passes `None`: the field number on the closing tag does
/// not affect the extent, demanding a match would be stricter than the
/// uncapped path (which tolerates a mismatch and records `END_MISMATCH`),
/// and a `None` there would cost every following sibling.
///
/// Iterative on purpose. Matching group nesting needs a counter, not a call
/// stack — a `START_GROUP` tag costs one byte, so the recursive form this
/// replaced could be made to demand a million frames from a 1 MB payload.
/// Being unable to overflow is also what lets this be used *as* the recovery
/// path for `render_message`'s own depth cap, rather than being subject to
/// it.
///
/// One check is given up in exchange. The recursive form validated every
/// level's closing field number against its own opener; doing that here
/// would need a `Vec<u64>` of open field numbers, an allocation in a routine
/// that exists to be allocation-free. Only the outermost is checked, and
/// only when `expected` is `Some`. That is safe because the answer is only
/// ever used to bound a span that is reproduced verbatim, so a tolerated
/// inner mismatch changes nothing about the bytes.
pub(in super::super) fn scan_group_extent(
    buf: &[u8],
    mut pos: usize,
    expected: Option<u64>,
) -> Option<usize> {
    let buflen = buf.len();
    let mut depth: usize = 0;
    loop {
        if pos == buflen {
            return None;
        }
        let tag = parse_wiretag(buf, pos);
        if tag.wtag_gar.is_some() {
            return None;
        }
        let field_number = tag.wfield.unwrap();
        let wire_type = tag.wtype.unwrap();
        pos = tag.next_pos;
        match wire_type {
            WT_VARINT => {
                let vr = parse_varint(buf, pos);
                if vr.varint_gar.is_some() {
                    return None;
                }
                pos = vr.next_pos;
            }
            WT_I64 => {
                pos = payload_end(pos, 8, buflen)?;
            }
            WT_LEN => {
                let lr = parse_varint(buf, pos);
                if lr.varint_gar.is_some() {
                    return None;
                }
                pos = lr.next_pos;
                pos = payload_end(pos, lr.varint.unwrap(), buflen)?;
            }
            WT_START_GROUP => {
                depth += 1;
            }
            WT_END_GROUP => {
                if depth == 0 {
                    if let Some(want) = expected {
                        if field_number != want {
                            return None;
                        }
                    }
                    return Some(pos);
                }
                depth -= 1;
            }
            WT_I32 => {
                pos = payload_end(pos, 4, buflen)?;
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tag byte for a single-digit field number and wire type.
    fn tag(field: u32, wire: u32) -> u8 {
        ((field << 3) | wire) as u8
    }

    #[test]
    fn finds_the_extent_of_a_well_formed_nested_group() {
        // group 1 { group 2 {} varint 3 = 7 } trailing-byte
        let buf = vec![
            tag(2, WT_START_GROUP),
            tag(2, WT_END_GROUP),
            tag(3, WT_VARINT),
            7,
            tag(1, WT_END_GROUP),
            0xAA,
        ];
        // `pos` starts *after* the opening tag, which the caller consumed.
        assert_eq!(scan_group_extent(&buf, 0, Some(1)), Some(5));
    }

    #[test]
    fn rejects_a_truncated_group() {
        let buf = vec![tag(2, WT_START_GROUP), tag(2, WT_END_GROUP)];
        assert_eq!(scan_group_extent(&buf, 0, Some(1)), None);
    }

    #[test]
    fn rejects_a_garbled_inner_tag() {
        // Wire type 6 is not a valid wire type.
        let buf = vec![tag(3, 6), tag(1, WT_END_GROUP)];
        assert_eq!(scan_group_extent(&buf, 0, Some(1)), None);
    }

    #[test]
    fn a_mismatched_outermost_close_depends_on_expected() {
        let buf = vec![tag(9, WT_END_GROUP)];
        assert_eq!(scan_group_extent(&buf, 0, Some(1)), None);
        // The depth-cap site does not care which field number closes the
        // group — it only wants the extent.
        assert_eq!(scan_group_extent(&buf, 0, None), Some(1));
    }

    #[test]
    fn a_mismatched_inner_close_is_tolerated_either_way() {
        // group 2 { } closed by an END_GROUP naming field 7.
        let buf = vec![
            tag(2, WT_START_GROUP),
            tag(7, WT_END_GROUP),
            tag(1, WT_END_GROUP),
        ];
        assert_eq!(scan_group_extent(&buf, 0, Some(1)), Some(3));
        assert_eq!(scan_group_extent(&buf, 0, None), Some(3));
    }

    #[test]
    fn deep_nesting_does_not_overflow_the_stack() {
        let mut buf = vec![tag(2, WT_START_GROUP); 200_000];
        buf.extend(std::iter::repeat_n(tag(2, WT_END_GROUP), 200_000));
        buf.push(tag(1, WT_END_GROUP));
        assert_eq!(scan_group_extent(&buf, 0, Some(1)), Some(buf.len()));
    }
}
