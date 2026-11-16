// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Feature resolution (`feature_resolution.resolve_features`), for the
//! features protoback reads: a field's resolved value is the default of
//! the latest edition at or before the file's, then each feature set,
//! coarsest to finest, that sets it.

use prototext_upb::descriptor_access::FeatureSet;

use crate::phase2::{EditionDefault, EditionDefaults};

/// `FieldPresence.IMPLICIT`.
pub const FIELD_PRESENCE_IMPLICIT: i32 = 2;
/// `MessageEncoding.DELIMITED`.
pub const MESSAGE_ENCODING_DELIMITED: i32 = 2;
/// `RepeatedFieldEncoding.PACKED`.
pub const REPEATED_FIELD_ENCODING_PACKED: i32 = 1;

#[derive(Debug, Clone, Copy)]
pub enum Feature {
    FieldPresence,
    MessageEncoding,
    RepeatedFieldEncoding,
}

impl Feature {
    fn name(self) -> &'static str {
        match self {
            Feature::FieldPresence => "field_presence",
            Feature::MessageEncoding => "message_encoding",
            Feature::RepeatedFieldEncoding => "repeated_field_encoding",
        }
    }

    fn get(self, fs: &FeatureSet<'_>) -> Option<i32> {
        match self {
            Feature::FieldPresence => fs.has_field_presence().then(|| fs.field_presence()),
            Feature::MessageEncoding => fs.has_message_encoding().then(|| fs.message_encoding()),
            Feature::RepeatedFieldEncoding => fs
                .has_repeated_field_encoding()
                .then(|| fs.repeated_field_encoding()),
        }
    }
}

/// `feature` resolved, for a file of `edition`, through `sets`, the
/// feature sets from the file's to the field's, each its element's own
/// (`None` where an element sets none).
pub fn resolve(
    table: &EditionDefaults,
    feature: Feature,
    edition: i32,
    sets: &[Option<FeatureSet<'_>>],
) -> i32 {
    let defaults: &[(i32, String)] = match table.get(feature.name()) {
        Some(EditionDefault::Defaults(d)) => d,
        _ => &[],
    };
    let numbers = match table.get(&format!("_enum_{}", feature.name())) {
        Some(EditionDefault::Numbers(n)) => Some(n),
        _ => None,
    };
    let mut result = defaults
        .iter()
        .rev()
        .find(|(e, _)| *e <= edition)
        .and_then(|(_, name)| numbers.and_then(|n| n.get(name)).copied())
        .unwrap_or(0);
    for fs in sets.iter().flatten() {
        if let Some(v) = feature.get(fs) {
            result = v;
        }
    }
    result
}
