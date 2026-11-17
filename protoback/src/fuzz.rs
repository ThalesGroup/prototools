// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! rapidfuzz's `fuzz.ratio`, which reproto's "Did you mean" suggestion
//! ranks the nodes with: the normalized Indel similarity of two strings,
//! by code point, times 100. The rapidfuzz crate computes it, by the
//! author of Python's; a table of Python's answers checks it gives the
//! same floats, bit for bit.
//!
//! One pattern is compared with every node: [`Ratio`] caches it, through
//! the crate's `indel::BatchComparator`. Not its `fuzz::RatioBatchComparator`,
//! which in 0.5.0 computes the LCS's normalized similarity instead of the
//! Indel's (`self.scorer.scorer`, one `.scorer` too many): 95.24 for
//! `file:prune_bse.proto` and `file:prune_base.proto`, 20/21, where Python
//! gives 97.56, 40/41.

use rapidfuzz::distance::indel;

/// `fuzz.ratio(pattern, b)` for one pattern and many `b`: Python's scales
/// the crate's 0 to 1 by 100.
pub struct Ratio(indel::BatchComparator<char>);

impl Ratio {
    pub fn new(pattern: &str) -> Ratio {
        Ratio(indel::BatchComparator::new(pattern.chars()))
    }

    /// `fuzz.ratio(pattern, b)`.
    pub fn of(&self, b: &str) -> f64 {
        self.0.normalized_similarity(b.chars()) * 100.0
    }
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
            // The crate's own fuzz::ratio, and the cached pattern.
            let got = rapidfuzz::fuzz::ratio(a.chars(), b.chars()) * 100.0;
            assert_eq!(
                got.to_bits(),
                expected.to_bits(),
                "ratio({a:?}, {b:?}) = {got}"
            );
            let cached = super::Ratio::new(a).of(b);
            assert_eq!(
                cached.to_bits(),
                expected.to_bits(),
                "Ratio::new({a:?}).of({b:?}) = {cached}"
            );
        }
    }
}
