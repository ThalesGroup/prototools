// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Checked wire-format bounds arithmetic and the shared recursion cap
//! (spec 0171).
//!
//! Both wire-format walkers in this workspace — `prototext-core`'s
//! render-while-decode and `prototext-graph`'s score-while-decode — are
//! handed attacker-chosen lengths and nesting depths. They independently
//! wrote the same two bugs, so the arithmetic and the cap live here, in
//! the crate they both already depend on.

/// End offset of a wire-format payload of `len` bytes starting at `pos`,
/// or `None` if it does not fit within `buflen`.
///
/// The naive form of this check — `if pos + len > buflen` — is wrong for a
/// length read off the wire: `len` is an attacker-chosen u64, the sum wraps
/// in a release build (`overflow-checks` is off), the guard passes, and the
/// caller then slices `buf[pos..pos + len]` with `start > end`. Every
/// wire-format length check in this workspace goes through here so that the
/// wrapping form has no place left to reappear — including the fixed-width
/// `8`/`4` cases, which cannot actually wrap but are the same idiom written
/// by the same hand.
///
/// `pos > buflen` yields `None` rather than panicking, so callers need no
/// separate invariant check; in practice every caller maintains
/// `pos <= buflen` because `parse_varint` and `parse_wiretag` both clamp
/// `next_pos` to `buflen`.
#[inline]
pub fn payload_end(pos: usize, len: u64, buflen: usize) -> Option<usize> {
    if pos > buflen {
        return None;
    }
    // Comparing against the remaining span rather than forming `pos + len`
    // is what makes this total over u64: `buflen - pos` cannot underflow,
    // and widening it to u64 cannot overflow.
    if len > (buflen - pos) as u64 {
        return None;
    }
    Some(pos + len as usize)
}

/// How many bytes short a payload of `len` at `pos` falls of fitting in
/// `buflen` — the `MISSING: n` count on a truncated LEN record.
///
/// Only meaningful when [`payload_end`] returned `None` for the same
/// arguments, which is precisely what makes the subtraction total: `len`
/// exceeds the remaining span, so the difference is positive.
#[inline]
pub fn bytes_missing(pos: usize, len: u64, buflen: usize) -> u64 {
    len.saturating_sub((buflen.saturating_sub(pos)) as u64)
}

/// Hard cap on wire-format walk recursion depth, shared by every decoder
/// and scorer in this workspace.
///
/// Nesting depth on the wire is bounded only by the input's length: a LEN
/// nesting level costs two bytes (tag + length prefix) and a `START_GROUP`
/// level costs one, so a 1 MB blob can demand hundreds of thousands of
/// stack frames. 1000 is far beyond any legitimate schema's nesting depth —
/// protobuf's own reference implementations default to 100 — while staying
/// comfortably inside a default thread stack.
///
/// It is deliberately a constant rather than an option. A caller-tunable
/// depth would make a rendering a function of `(bytes, schema, depth)`,
/// breaking the property the whole override model rests on: that the same
/// bytes always render the same way.
pub const MAX_WIRE_DEPTH: usize = 1000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_end_accepts_an_exact_fit() {
        assert_eq!(payload_end(4, 6, 10), Some(10));
        assert_eq!(payload_end(10, 0, 10), Some(10));
    }

    #[test]
    fn payload_end_rejects_one_byte_over() {
        assert_eq!(payload_end(4, 7, 10), None);
    }

    /// The defect this module exists for: `4 + u64::MAX as usize` wraps to
    /// 3, so `pos + len > buflen` was false and the caller sliced
    /// `buf[4..3]`.
    #[test]
    fn payload_end_rejects_a_wrapping_length() {
        assert_eq!(payload_end(4, u64::MAX, 10), None);
        assert_eq!(payload_end(4, u64::MAX - 3, 10), None);
        assert_eq!(payload_end(0, 1 << 40, 10), None);
    }

    #[test]
    fn payload_end_rejects_a_position_past_the_end() {
        assert_eq!(payload_end(11, 0, 10), None);
    }

    #[test]
    fn bytes_missing_counts_the_shortfall() {
        assert_eq!(bytes_missing(4, 7, 10), 1);
        assert_eq!(bytes_missing(0, u64::MAX, 10), u64::MAX - 10);
    }
}
