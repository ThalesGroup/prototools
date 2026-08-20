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
//! The format does spell severity in case, but as a convention with at
//! least one counterexample — `ENUM_UNKNOWN` is ALL CAPS and only
//! non-canonical — so a pair of case regexes would need its own
//! exception list and would miscolor the next exception in silence.

/// How serious an annotation keyword is. `None` — the absence of a
/// tier — is the third, commonest state: a wire-type name, a field
/// declaration or a plain modifier such as `pack_size` is not an
/// anomaly at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    /// Legal on the wire and round-trips, but no conformant writer
    /// emits one.
    NonCanonical,
    /// Not legal. The blob is malformed, or the schema cannot be this.
    Invalid,
}

/// The wire-type names an annotation echoes. Not anomalies — this is
/// the document's own vocabulary, quoted.
///
/// Only [`vocabulary`] and [`LEN_SHAPE_NAMES`]'s tests read it, so like
/// them this is test-only: the runtime classifier answers `None` for
/// these names by not listing them, and listing them here is what lets
/// the drift test assert that `highlights.scm` gives them the type
/// color rather than a tier — a wire-type name is what the field *is*
/// when no schema says otherwise, which is the slot a type name would
/// fill.
#[cfg(test)]
pub const WIRE_TYPE_NAMES: [&str; 5] = ["varint", "fixed64", "fixed32", "bytes", "group"];

/// The other two readings of wire type 2 (spec 0341).
///
/// `AnnWriter::push_wire` opens a schema-blind annotation with one of
/// **seven** names, not five: besides the wire types above it emits
/// `string` when the payload is valid UTF-8 and `message` when it
/// parses as one. They fill the same slot for the same reason and take
/// the same color.
///
/// Kept apart from [`WIRE_TYPE_NAMES`] rather than folded into it
/// because that list is also [`wire_type_clause`]'s domain, and that
/// function answers "wire type *N* — …", which neither of these two is:
/// both are wire type 2, already spoken for by `bytes`. Splitting the
/// list keeps the hover box's lexer at the five real wire types (spec
/// 0326 N3) while [`vocabulary`] still covers all seven, which is what
/// the drift test needs.
#[cfg(test)]
pub const LEN_SHAPE_NAMES: [&str; 2] = ["string", "message"];

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
pub const INVALID: [&str; 16] = [
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
    "TRUNCATED_MESSAGE",
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
    if NON_CANONICAL.contains(&keyword) {
        Some(Tier::NonCanonical)
    } else if INVALID.contains(&keyword) {
        Some(Tier::Invalid)
    } else {
        None
    }
}

/// The plain-English clause a keyword stands for (spec 0285 S1).
///
/// The keyword is what the row shows and what
/// `docs/prototext/annotation-format.md` documents; the clause is here
/// because a keyword is not an explanation. It lives beside [`tier_of`]
/// so that how serious a keyword is and what it says cannot come apart,
/// and so that the wire box and the document box print one string
/// rather than two that drift (spec 0285 G2).
///
/// `None` for anything unlisted, which prints as the bare keyword
/// rather than as a keyword and an empty dash. The drift test below
/// keeps the vocabulary itself from reaching that arm.
pub fn clause(keyword: &str) -> Option<&'static str> {
    Some(match keyword {
        "TAG_OOR" | "ETAG_OOR" => "a field number must be between 1 and 536870911",
        "tag_ohb" | "etag_ohb" => "the tag varint is padded, not minimal",
        "len_ohb" => "the length varint is padded, not minimal",
        "val_ohb" => "this varint is padded, not minimal",
        "ohb" => "this packed element's varint is padded, not minimal",
        "packed_ohb" => "the v1 spelling of ohb: one list for the whole packed run",
        "neg" | "truncated_neg" => "a negative value in five bytes, not the canonical ten",
        "packed_truncated_neg" => "the v1 spelling of neg: one list for the whole packed run",
        "nan_bits" => "a NaN, but not the bit pattern protoc writes",
        "ENUM_UNKNOWN" => "no name in the declared enum has this number",
        "TYPE_MISMATCH" => "the schema declares this field with another wire type",
        "OPEN_GROUP" => "this group is never closed",
        "END_MISMATCH" => "this group end names a different field than its start",
        "INVALID_TAG_TYPE" => "6 and 7 are not wire types",
        "INVALID_VARINT" => "this varint has no final byte",
        "INVALID_GROUP_END" => "this group-end tag has no final byte",
        "INVALID_LEN" => "the length prefix has no final byte",
        "INVALID_FIXED64" => "a 64-bit value needs eight bytes and fewer are left",
        "INVALID_FIXED32" => "a 32-bit value needs four bytes and fewer are left",
        "TRUNCATED_BYTES" => "the declared length runs past the end of the message",
        "TRUNCATED_MESSAGE" => {
            "the declared length runs past the end of the message; the available bytes are shown"
        }
        "MISSING" => "how many bytes short of its declared length the record is",
        "INVALID_PACKED_RECORDS" => "these bytes do not divide into whole packed elements",
        "INVALID_STRING" => "these bytes are not valid UTF-8",
        PACK_SIZE => "how many elements this one packed wire record holds",
        _ => return None,
    })
}

/// The same, for the wire-type token an annotation opens with
/// (spec 0285 S4) — and, by answering `Some`, the recognizer for one.
///
/// Kept apart from [`clause`] rather than folded into it because
/// `bytes`, `fixed32` and `fixed64` are *also* proto scalar type names
/// and mean something else in that position (spec 0285's rejected
/// word-keyed table). Two functions is how the caller declares which
/// position it is asking about.
pub fn wire_type_clause(token: &str) -> Option<&'static str> {
    Some(match token {
        "varint" => "wire type 0 — a base-128 integer, one to ten bytes",
        "fixed64" => "wire type 1 — eight bytes, little-endian",
        "bytes" => "wire type 2 — a length prefix, then that many bytes",
        "group" => "wire types 3 and 4 — a start tag, the fields, a matching end tag",
        "fixed32" => "wire type 5 — four bytes, little-endian",
        _ => return None,
    })
}

/// The one keyword that is a landmark rather than a defect (0225's
/// 2026-08-06 amendment): it says the record carries a packed run.
///
/// Named because two callers must agree about it — the wire box prints
/// it as part of its length line and so must not also print it as a
/// flaw, and [`clause`] answers for it despite it having no tier.
pub const PACK_SIZE: &str = "pack_size";

/// Every keyword in the vocabulary, paired with its tier — the drift
/// test's input, so that `highlights.scm`'s copy of these lists cannot
/// quietly fall behind this one. Nothing at runtime enumerates the
/// vocabulary; `tier_of` answers one keyword at a time.
#[cfg(test)]
pub fn vocabulary() -> Vec<(&'static str, Option<Tier>)> {
    WIRE_TYPE_NAMES
        .iter()
        .chain(LEN_SHAPE_NAMES.iter())
        .map(|&k| (k, None))
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
    fn the_capitalization_counterexample_is_where_it_belongs() {
        // The whole reason this module keys on keywords rather than on
        // case. If this one ever agrees with its capitalization again,
        // the case regexes become viable and this module does not.
        assert_eq!(tier_of("ENUM_UNKNOWN"), Some(Tier::NonCanonical));
        // And the other direction, spec 0267: a lower-case modifier
        // that is not an anomaly gets no tier rather than the mildest
        // one.
        assert_eq!(tier_of("pack_size"), None);
    }

    /// Spec 0285 S2. A modifier prototext-core adds later shows up
    /// uncolored *and* unexplained, and the second is the one nothing
    /// else would notice: an unlisted keyword prints as a bare word in
    /// a box whose whole purpose is to not be a bare word.
    #[test]
    fn every_keyword_has_a_clause() {
        for keyword in NON_CANONICAL.iter().chain(INVALID.iter()) {
            assert!(clause(keyword).is_some(), "{keyword} has no clause");
        }
        assert!(clause(PACK_SIZE).is_some(), "{PACK_SIZE} has no clause");
        for token in WIRE_TYPE_NAMES {
            assert!(wire_type_clause(token).is_some(), "{token} has no clause");
        }
    }

    /// The two positions a word-keyed table would confuse: every
    /// wire-type token that is also a proto scalar type name must be
    /// unknown to the *other* function, or a caller asking in one
    /// position could be answered from the other.
    #[test]
    fn the_two_clause_tables_do_not_overlap() {
        for token in WIRE_TYPE_NAMES {
            assert_eq!(clause(token), None, "{token} is in both tables");
        }
        for keyword in NON_CANONICAL.iter().chain(INVALID.iter()) {
            assert_eq!(wire_type_clause(keyword), None, "{keyword} is in both");
        }
    }

    #[test]
    fn an_unlisted_keyword_has_no_tier() {
        assert_eq!(tier_of("varint"), None);
        assert_eq!(tier_of("SOMETHING_NEW"), None);
        assert_eq!(tier_of(""), None);
    }
}
