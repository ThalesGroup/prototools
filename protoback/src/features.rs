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
/// `FieldPresence.LEGACY_REQUIRED`.
pub const FIELD_PRESENCE_LEGACY_REQUIRED: i32 = 3;
/// `MessageEncoding.DELIMITED`.
pub const MESSAGE_ENCODING_DELIMITED: i32 = 2;
/// `RepeatedFieldEncoding.PACKED`.
pub const REPEATED_FIELD_ENCODING_PACKED: i32 = 1;

/// The six features reproto resolves (`ResolvedFeatures`, its runtime ones).
#[derive(Debug, Clone, Copy)]
pub enum Feature {
    FieldPresence,
    EnumType,
    RepeatedFieldEncoding,
    Utf8Validation,
    MessageEncoding,
    JsonFormat,
}

impl Feature {
    /// In `ResolvedFeatures`' order.
    pub const ALL: [Feature; 6] = [
        Feature::FieldPresence,
        Feature::EnumType,
        Feature::RepeatedFieldEncoding,
        Feature::Utf8Validation,
        Feature::MessageEncoding,
        Feature::JsonFormat,
    ];

    /// Its field's name in `FeatureSet`.
    pub fn name(self) -> &'static str {
        match self {
            Feature::FieldPresence => "field_presence",
            Feature::EnumType => "enum_type",
            Feature::RepeatedFieldEncoding => "repeated_field_encoding",
            Feature::Utf8Validation => "utf8_validation",
            Feature::MessageEncoding => "message_encoding",
            Feature::JsonFormat => "json_format",
        }
    }

    /// Its value in `fs`, when set there.
    pub fn get(self, fs: &FeatureSet<'_>) -> Option<i32> {
        match self {
            Feature::FieldPresence => fs.has_field_presence().then(|| fs.field_presence()),
            Feature::EnumType => fs.has_enum_type().then(|| fs.enum_type()),
            Feature::RepeatedFieldEncoding => fs
                .has_repeated_field_encoding()
                .then(|| fs.repeated_field_encoding()),
            Feature::Utf8Validation => fs.has_utf8_validation().then(|| fs.utf8_validation()),
            Feature::MessageEncoding => fs.has_message_encoding().then(|| fs.message_encoding()),
            Feature::JsonFormat => fs.has_json_format().then(|| fs.json_format()),
        }
    }
}

/// `feature_value_name`: a value's name in the feature's enum, the
/// `*_UNKNOWN` name only for 0, else the number written out.
pub fn value_name(table: &EditionDefaults, feature: Feature, value: i32) -> String {
    let Some(EditionDefault::Numbers(numbers)) = table.get(&format!("_enum_{}", feature.name()))
    else {
        return value.to_string();
    };
    numbers
        .iter()
        .find(|(name, &n)| n == value && !name.ends_with("_UNKNOWN") && n != 0)
        .or_else(|| numbers.iter().find(|(_, &n)| n == value))
        .map_or_else(|| value.to_string(), |(name, _)| name.clone())
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
