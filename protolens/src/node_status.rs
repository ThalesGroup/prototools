// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0247: how bad a rendered row is, on a five-rung ladder.
//!
//! The rungs are ordered, so rolling a subtree up is `Ord::max` — see
//! `tui::node_status` for the roll-up itself. This module only decides
//! what one row says.
//!
//! Deliberately *not* [`annotation::Tier`]: `Unknown` is not a tier at
//! all but a rendering convention (see [`row_status`]), and `Unbaked`
//! is a fact about how much of the document has been looked at rather
//! than about the bytes.

use prototext_core::serialize::encode_text::annotation_start;

use crate::annotation::{self, Tier};

/// How bad a node is, worst last. `Ord` is the severity order, so
/// `worst_of` is `max` on a single byte (spec 0247 S1).
///
/// Spec 0349: `Shadowed` sits between `Unbaked` and `Unknown`. A
/// shadowed scalar is valid on the wire and round-trips — the last
/// occurrence wins — so it is less severe than `Unknown`, which signals
/// a field the schema has nothing to say about. The glyph (hollow vs
/// filled) is the visual distinction from `NonCanonical`; the color
/// (amber) is the same.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(u8)]
pub enum Status {
    /// Nothing to say.
    #[default]
    Ok = 0,
    /// A bounded confirm stopped here and the bake has not reached it
    /// yet, so nothing below has been looked at (spec 0249 S12).
    ///
    /// **The rank looks wrong and is right.** An unbaked subtree might
    /// hide an `Invalid`, so ranking it just *above* `Ok` lets a known
    /// bad sibling still win the fold toggle's color, while "everything
    /// known is fine, something has not been looked at" reads as the
    /// neutral gray — provisional, an answer not yet given rather than
    /// a bad one. What it must never do is claim `Ok`, because that
    /// would make spec 0247's promise — that a toggle carries the worst
    /// news below it — simply false over an auto-fold.
    Unbaked = 1,
    /// This scalar is overridden by a later occurrence of the same
    /// singular field (spec 0343 / spec 0349). The last occurrence wins
    /// on the wire; this one's value is silently discarded. Less severe
    /// than `Unknown`: the bytes are well-formed and the schema knows
    /// the field — it is just not the one that counts.
    Shadowed = 2,
    /// No schema declares this field, so the row shows a field *number*
    /// and prototext could say nothing about what the bytes mean.
    Unknown = 3,
    /// Legal on the wire and round-trips, but no conformant writer
    /// emits one.
    NonCanonical = 4,
    /// Not legal. The blob is malformed, or the schema cannot be this.
    Invalid = 5,
}

impl From<Tier> for Status {
    fn from(tier: Tier) -> Self {
        match tier {
            Tier::NonCanonical => Status::NonCanonical,
            Tier::Invalid => Status::Invalid,
        }
    }
}

/// The worst thing one rendered row says about itself (spec 0247 S3).
///
/// Two independent readings, combined with `max`:
///
/// - **the key**, for `Unknown`. A row whose key is a field *number*
///   rather than a name is one no schema declares. That is exact rather
///   than a guess: the renderer's numeric-key rule is driven by the same
///   `is_known` that suppresses the field declaration —
///   `use_numeric_key = unknown || is_wire` in `helpers/scalar.rs`,
///   `wob_prefix_n(…, !is_known, …)` and the bare number for an unknown
///   group in `sink.rs`. An unknown submessage header is literally
///   `N { #@ message`. A proto field name cannot begin with a digit, and
///   neither can either synthetic name spec 0237 derives (`f<number>`,
///   `p<position>`) — which is what lets naming a field clear the rung
///   with no special case anywhere (S9).
/// - **the annotation keywords**, for the two anomaly rungs, via
///   [`annotation::tier_of`] — the one classifier, so this cannot
///   disagree with the color the annotation itself wears.
///
/// The two overlap on purpose and the `max` resolves it: an invalid row
/// and a wire-type mismatch both render numeric keys, and both carry a
/// keyword (`INVALID_*`, `TYPE_MISMATCH`) that outranks `Unknown`.
///
/// A *known* field rendered as raw wire (`WireBytes`/`WireFixed*`) also
/// gets a numeric key with no anomaly keyword, so it reads `Unknown`.
/// That is intended — the row is being shown un-typed — but it is a
/// decision rather than a side effect.
pub fn row_status(row: &str) -> Status {
    let body = row.trim_start();
    let mut worst = match body.as_bytes().first() {
        Some(b) if b.is_ascii_digit() => Status::Unknown,
        _ => Status::Ok,
    };
    let Some(at) = annotation_start(row) else {
        return worst;
    };
    // `annotation_start` reports where the *value* ends, before the two
    // separator spaces, so the marker is still ahead of us.
    let annotation = row[at..].trim_start();
    let annotation = annotation.strip_prefix("#@").unwrap_or(annotation);
    for token in annotation.split(';') {
        let token = token.trim_start();
        // The commonest token by far is the field declaration, and
        // `push_field_decl` is the only thing that writes `" = "` into
        // an annotation — every modifier spells itself `key: value`. So
        // this one substring search is what keeps the ordinary row off
        // `tier_of`'s twenty-seven comparisons.
        if token.contains(" = ") {
            continue;
        }
        let keyword = token.split([':', ' ', '(']).next().unwrap_or("");
        if let Some(tier) = annotation::tier_of(keyword) {
            worst = worst.max(Status::from(tier));
            if worst == Status::Invalid {
                // Nothing outranks it, so nothing later can change the
                // answer.
                break;
            }
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_is_ordered_by_severity() {
        assert!(Status::Ok < Status::Unbaked);
        // Spec 0349: Shadowed sits between Unbaked and Unknown.
        assert!(Status::Unbaked < Status::Shadowed);
        // Spec 0249 S12: below every *known* defect, deliberately.
        assert!(Status::Shadowed < Status::Unknown);
        assert!(Status::Unknown < Status::NonCanonical);
        assert!(Status::NonCanonical < Status::Invalid);
        assert_eq!(Status::default(), Status::Ok);
    }

    #[test]
    fn a_named_field_with_a_plain_declaration_is_ok() {
        assert_eq!(row_status("  options {  #@ Options = 3"), Status::Ok);
        assert_eq!(row_status("  name: \"x\"  #@ string = 1"), Status::Ok);
        assert_eq!(row_status("  }"), Status::Ok);
        assert_eq!(row_status(""), Status::Ok);
    }

    #[test]
    fn a_numeric_key_is_an_unknown_field() {
        assert_eq!(row_status("  12: \"abc\"  #@ bytes"), Status::Unknown);
        assert_eq!(row_status("  7 {  #@ message"), Status::Unknown);
        // The synthetic names spec 0237 derives must *not* read as
        // numbers, or naming a field could never clear the rung.
        assert_eq!(row_status("  f12: \"abc\"  #@ string = 12"), Status::Ok);
        assert_eq!(row_status("  p3 {  #@ Payload = 4"), Status::Ok);
    }

    #[test]
    fn pack_size_is_not_a_defect() {
        // Spec 0267: it counts a record's elements and accuses nothing,
        // so it carries no tier and cannot raise a row's rung.
        assert_eq!(annotation::tier_of("pack_size"), None);
        // A packed element row. Its key is always symbolic: `render_packed`
        // calls `wfl_prefix_n(…, Some(foe), false, …)`, and it only reaches
        // that call at all because a schema said the field was packed.
        assert_eq!(
            row_status("  ids: 1  #@ int32 = 4; pack_size: 3"),
            Status::Ok
        );
    }

    #[test]
    fn an_anomaly_keyword_outranks_a_numeric_key() {
        // Both readings fire; the worse one wins.
        assert_eq!(
            row_status("  4: \"x\"  #@ varint; TYPE_MISMATCH"),
            Status::Invalid
        );
        assert_eq!(
            row_status("  n: 1  #@ int32 = 2; tag_ohb: 4"),
            Status::NonCanonical
        );
        // A single-token invalid annotation: there is no `;` to find,
        // which is why the field-declaration test is per token rather
        // than a whole-annotation prefilter.
        assert_eq!(
            row_status("  3: \"\\x80\"  #@ INVALID_VARINT"),
            Status::Invalid
        );
    }

    #[test]
    fn a_hash_at_inside_a_string_value_is_not_an_annotation() {
        // `annotation_start` is format-driven (spec 0187), and this is
        // the case that would misclassify under a naive `find("#@")`.
        assert_eq!(
            row_status("  note: \"#@ INVALID_LEN\"  #@ string = 1"),
            Status::Ok
        );
    }
}
