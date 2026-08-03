// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The `#@` annotation vocabulary and its severity tiers (spec 0225
//! §S11 "one classifier, two rows", §S12).
//!
//! Two rows show the same anomaly — the rendered `#@` text and, under
//! it, the wire bytes it describes — and they must not be able to
//! disagree about how serious it is. This module is the single place
//! that decides. `theme`'s tier colors are the single place that
//! decides what a tier looks like.
//!
//! Severity is keyed on the **keyword**, not on its capitalization.
//! The format does spell severity in case, but as a convention with
//! counterexamples in both directions (`pack_size` is lower case and
//! not an anomaly; `ENUM_UNKNOWN` is ALL CAPS and only non-canonical),
//! so a pair of case regexes would need its own exception list and
//! would miscolor the next exception in silence.

/// How serious an annotation keyword is. `None` — the absence of a
/// tier — is the fourth, commonest state: a wire-type name or a field
/// declaration is not an anomaly at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    /// Structural, not an anomaly: where one packed wire record begins.
    /// Wears an accent so it stands out in a run of identical element
    /// lines, which is its whole job.
    Landmark,
    /// Legal on the wire and round-trips, but no conformant writer
    /// emits one.
    NonCanonical,
    /// Not legal. The blob is malformed, or the schema cannot be this.
    Invalid,
}

/// The wire-type names an annotation echoes. Not anomalies — this is
/// the document's own vocabulary, quoted.
///
/// Only [`vocabulary`] reads it, so like it this is test-only: the
/// runtime classifier answers `None` for these names by not listing
/// them, and listing them here is what lets the drift test assert that
/// `highlights.scm` gives them the type color rather than a tier — a
/// wire-type name is what the field *is* when no schema says otherwise,
/// which is the slot a type name would fill.
#[cfg(test)]
pub const WIRE_TYPE_NAMES: [&str; 5] = ["varint", "fixed64", "fixed32", "bytes", "group"];

/// [`Tier::Landmark`]'s sole member.
pub const LANDMARK: [&str; 1] = ["pack_size"];

/// [`Tier::NonCanonical`]'s members.
///
/// `ENUM_UNKNOWN` is here despite its capitalization and despite
/// `annotation-format.md` filing it as informational (spec 0225 §S12):
/// an undeclared value in a *closed* enum is exactly what charges the
/// scorer's `out_of_range`, whose own description — legal on the wire,
/// but no conformant writer emits one — is this tier's definition.
/// It over-accuses open enums, where the same token is routine and the
/// scorer charges nothing; that is accepted, because the annotation
/// text carries no trace of `is_closed` and saying nothing would lose
/// the closed case entirely.
pub const NON_CANONICAL: [&str; 11] = [
    "tag_ohb",
    "val_ohb",
    "len_ohb",
    "etag_ohb",
    "ohb",
    "packed_ohb",
    "nan_bits",
    "neg",
    "truncated_neg",
    "packed_truncated_neg",
    "ENUM_UNKNOWN",
];

/// [`Tier::Invalid`]'s members.
pub const INVALID: [&str; 15] = [
    "TAG_OOR",
    "ETAG_OOR",
    "TYPE_MISMATCH",
    "MISSING",
    "END_MISMATCH",
    "OPEN_GROUP",
    "INVALID_TAG_TYPE",
    "INVALID_VARINT",
    "INVALID_FIXED64",
    "INVALID_FIXED32",
    "INVALID_LEN",
    "INVALID_GROUP_END",
    "INVALID_STRING",
    "INVALID_PACKED_RECORDS",
    "TRUNCATED_BYTES",
];

/// The tier of one annotation keyword — the bare word, with any
/// `: value` already stripped.
///
/// `None` for a wire-type name, a field declaration, or anything else
/// unrecognized. An unlisted keyword deliberately gets no tier rather
/// than a guessed one: a modifier prototext-core adds later shows up
/// uncolored, which is visible, instead of being classified by a rule
/// that was never told about it.
pub fn tier_of(keyword: &str) -> Option<Tier> {
    if LANDMARK.contains(&keyword) {
        Some(Tier::Landmark)
    } else if NON_CANONICAL.contains(&keyword) {
        Some(Tier::NonCanonical)
    } else if INVALID.contains(&keyword) {
        Some(Tier::Invalid)
    } else {
        None
    }
}

/// Every keyword in the vocabulary, paired with its tier — the drift
/// test's input, so that `highlights.scm`'s copy of these lists cannot
/// quietly fall behind this one. Nothing at runtime enumerates the
/// vocabulary; `tier_of` answers one keyword at a time.
#[cfg(test)]
pub fn vocabulary() -> Vec<(&'static str, Option<Tier>)> {
    WIRE_TYPE_NAMES
        .iter()
        .map(|&k| (k, None))
        .chain(LANDMARK.iter().map(|&k| (k, Some(Tier::Landmark))))
        .chain(NON_CANONICAL.iter().map(|&k| (k, Some(Tier::NonCanonical))))
        .chain(INVALID.iter().map(|&k| (k, Some(Tier::Invalid))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_keyword_belongs_to_two_tiers() {
        let all = vocabulary();
        for (i, (k, _)) in all.iter().enumerate() {
            assert!(
                !all[i + 1..].iter().any(|(other, _)| other == k),
                "{k} is listed twice"
            );
        }
    }

    #[test]
    fn the_two_capitalization_counterexamples_are_where_they_belong() {
        // The whole reason this module keys on keywords rather than on
        // case. If either of these ever agrees with its capitalization
        // again, the case regexes become viable and this module does
        // not.
        assert_eq!(tier_of("pack_size"), Some(Tier::Landmark));
        assert_eq!(tier_of("ENUM_UNKNOWN"), Some(Tier::NonCanonical));
    }

    #[test]
    fn an_unlisted_keyword_has_no_tier() {
        assert_eq!(tier_of("varint"), None);
        assert_eq!(tier_of("SOMETHING_NEW"), None);
        assert_eq!(tier_of(""), None);
    }
}
