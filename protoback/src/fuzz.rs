// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! rapidfuzz's `fuzz.ratio`, which reproto's "Did you mean" suggestion
//! ranks the nodes with: the normalized Indel similarity of two strings,
//! by code point, times 100. The rapidfuzz crate computes it, by the
//! author of Python's; a table of Python's answers checks it gives the
//! same floats, bit for bit.
//!
//! The crate's `fuzz::ratio`, not its `RatioBatchComparator`: in 0.5.0 the
//! comparator is off for some pairs (95.24 for `file:prune_bse.proto` and
//! `file:prune_base.proto`, where Python and `fuzz::ratio` give 97.56).

/// `fuzz.ratio(a, b)`: Python's scales the crate's 0 to 1 by 100.
pub fn ratio(a: &str, b: &str) -> f64 {
    rapidfuzz::fuzz::ratio(a.chars(), b.chars()) * 100.0
}

#[cfg(test)]
mod tests {
    /// `(a, b, fuzz.ratio(a, b))`, as Python's rapidfuzz gives it.
    mod cases {
        include!("fuzz/cases.rs");
    }

    #[test]
    fn ratios_are_rapidfuzz_s() {
        for &(a, b, expected) in cases::CASES {
            let got = super::ratio(a, b);
            assert_eq!(
                got.to_bits(),
                expected.to_bits(),
                "ratio({a:?}, {b:?}) = {got}"
            );
        }
    }
}
