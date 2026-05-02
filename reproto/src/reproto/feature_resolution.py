# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Edition feature resolution engine (spec 0025).

Public API
----------
build_edition_defaults(descriptor_fdp) -> EditionDefaultTable
    Extract the per-feature edition-default table from the FeatureSet message
    defined in the variant's descriptor FileDescriptorProto.  Call once at
    startup; store the result in ctx.edition_defaults.

resolve_features(edition_defaults, file_edition, *feature_sets) -> ResolvedFeatures
    Merge an ordered chain of sparse FeatureSet proto messages against the
    edition-default table and return a fully resolved feature snapshot.

ResolvedFeatures
    Plain dataclass with one int field per RETENTION_RUNTIME FeatureSet field.
    Companion constants (e.g. FIELD_PRESENCE_EXPLICIT) are provided for
    readability at call sites.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from google.protobuf.descriptor_pb2 import FileDescriptorProto

if TYPE_CHECKING:
    from google.protobuf.message import Message


# ---------------------------------------------------------------------------
# Type alias
# ---------------------------------------------------------------------------

# Maps feature field name to either:
#   - a sorted list of (edition_number, value_name) pairs  (the defaults), or
#   - a dict[str, int] stored under the key "_enum_<field_name>" (value→number map).
# The two kinds are distinguished by key prefix at runtime.
EditionDefaultTable = dict[str, list[tuple[int, str]] | dict[str, int]]


# ---------------------------------------------------------------------------
# ResolvedFeatures dataclass
# ---------------------------------------------------------------------------

@dataclass
class ResolvedFeatures:
    """Fully resolved FeatureSet for a single descriptor element.

    All values are integer enum numbers.  Use the companion constants below
    for legible comparisons (e.g. ``resolved.field_presence ==
    FIELD_PRESENCE_IMPLICIT``).

    Fields with RETENTION_SOURCE (enforce_naming_style,
    default_symbol_visibility) are excluded: they are not stored in runtime
    descriptors and have no rendering implication.

    A value of 0 means the feature is absent from the variant's FeatureSet
    definition (older variant) or the edition is below all known defaults.
    """
    field_presence:          int = 0
    enum_type:               int = 0
    repeated_field_encoding: int = 0
    utf8_validation:         int = 0
    message_encoding:        int = 0
    json_format:             int = 0


# ---------------------------------------------------------------------------
# ResolvedFeatures companion constants
# (values match google.protobuf.FeatureSet enum numbers)
# ---------------------------------------------------------------------------

# FieldPresence
FIELD_PRESENCE_UNKNOWN:         int = 0
FIELD_PRESENCE_EXPLICIT:        int = 1
FIELD_PRESENCE_IMPLICIT:        int = 2
FIELD_PRESENCE_LEGACY_REQUIRED: int = 3

# EnumType
ENUM_TYPE_UNKNOWN: int = 0
ENUM_TYPE_OPEN:    int = 1
ENUM_TYPE_CLOSED:  int = 2

# RepeatedFieldEncoding
REPEATED_FIELD_ENCODING_UNKNOWN: int = 0
REPEATED_FIELD_ENCODING_PACKED:  int = 1
REPEATED_FIELD_ENCODING_EXPANDED: int = 2

# Utf8Validation
UTF8_VALIDATION_UNKNOWN: int = 0
UTF8_VALIDATION_VERIFY:  int = 2
UTF8_VALIDATION_NONE:    int = 3

# MessageEncoding
MESSAGE_ENCODING_UNKNOWN:        int = 0
MESSAGE_ENCODING_LENGTH_PREFIXED: int = 1
MESSAGE_ENCODING_DELIMITED:       int = 2

# JsonFormat
JSON_FORMAT_UNKNOWN:         int = 0
JSON_FORMAT_ALLOW:           int = 1
JSON_FORMAT_LEGACY_BEST_EFFORT: int = 2


# ---------------------------------------------------------------------------
# The six RETENTION_RUNTIME FeatureSet fields we care about.
# enforce_naming_style and default_symbol_visibility are RETENTION_SOURCE
# and intentionally excluded.
# ---------------------------------------------------------------------------
_RESOLVED_FIELDS: tuple[str, ...] = (
    "field_presence",
    "enum_type",
    "repeated_field_encoding",
    "utf8_validation",
    "message_encoding",
    "json_format",
)


# ---------------------------------------------------------------------------
# build_edition_defaults
# ---------------------------------------------------------------------------

def build_edition_defaults(descriptor_fdp: FileDescriptorProto) -> EditionDefaultTable:
    """Extract the edition-default table from the variant's descriptor FDP.

    Reads FieldOptions.edition_defaults on each field of the FeatureSet
    message defined in descriptor_fdp.  Each entry is a (edition_number,
    value_name) pair.  The returned table maps feature field name to a list
    of such pairs sorted ascending by edition_number.

    Returns an empty dict if the descriptor does not define a FeatureSet
    message (variant does not support editions).
    """
    # Locate the FeatureSet message by name (top-level only).
    feature_set_msg = None
    for msg in descriptor_fdp.message_type:
        if msg.name == "FeatureSet":
            feature_set_msg = msg
            break
    if feature_set_msg is None:
        return {}

    # Build the enum-value name → number maps for each FeatureSet field.
    # These live as nested enums inside FeatureSet (or inside nested messages
    # for default_symbol_visibility, which we skip).
    enum_by_name: dict[str, dict[str, int]] = {}
    for nested_enum in feature_set_msg.enum_type:
        mapping = {v.name: v.number for v in nested_enum.value}
        enum_by_name[nested_enum.name] = mapping
    # Also descend one level for nested messages (e.g. VisibilityFeature).
    for nested_msg in feature_set_msg.nested_type:
        for nested_enum in nested_msg.enum_type:
            mapping = {v.name: v.number for v in nested_enum.value}
            enum_by_name[nested_enum.name] = mapping

    table: EditionDefaultTable = {}
    for field in feature_set_msg.field:
        if field.name not in _RESOLVED_FIELDS:
            continue

        # Derive the simple enum type name from the fully-qualified type_name,
        # e.g. ".google.protobuf.FeatureSet.FieldPresence" -> "FieldPresence".
        enum_type_name = field.type_name.rsplit(".", 1)[-1]
        value_to_num = enum_by_name.get(enum_type_name, {})

        entries: list[tuple[int, str]] = []
        for ed in field.options.edition_defaults:
            edition_num: int = ed.edition   # int (Edition enum value)
            value_name: str = ed.value      # string like "EXPLICIT"
            entries.append((edition_num, value_name))

        entries.sort(key=lambda t: t[0])
        # Attach the value→number map as metadata so resolve can convert names
        # to ints without re-scanning the descriptor.
        table[field.name] = entries
        # Store the name→number mapping under a private key.
        table[f"_enum_{field.name}"] = value_to_num

    return table


# ---------------------------------------------------------------------------
# _resolve_one — resolve a single feature for a given edition + override chain
# ---------------------------------------------------------------------------

def _resolve_one(
    table: EditionDefaultTable,
    feature_name: str,
    file_edition: int,
    *feature_sets: "Message | None",
) -> int:
    """Return the resolved integer enum value for one feature.

    Walks the override chain (coarsest to finest); the last non-None
    FeatureSet that has the field set wins.  Falls back to the edition
    default if no override is found.
    """
    # Start from the edition default.
    _entries = table.get(feature_name, [])
    entries: list[tuple[int, str]] = _entries if isinstance(_entries, list) else []
    _enum_entry = table.get(f"_enum_{feature_name}", {})
    name_to_num: dict[str, int] = _enum_entry if isinstance(_enum_entry, dict) else {}

    default_value = 0
    for edition_num, value_name in reversed(entries):
        if edition_num <= file_edition:
            default_value = name_to_num.get(value_name, 0)
            break

    # Walk override chain.
    result = default_value
    for fs in feature_sets:
        if fs is None:
            continue
        try:
            if fs.HasField(feature_name):
                result = getattr(fs, feature_name)
        except ValueError:
            # HasField raises ValueError for repeated / non-message fields;
            # should not happen for FeatureSet but guard defensively.
            pass

    return result


# ---------------------------------------------------------------------------
# resolve_features — public API
# ---------------------------------------------------------------------------

def resolve_features(
    edition_defaults: EditionDefaultTable,
    file_edition: int,
    *feature_sets: "Message | None",
) -> ResolvedFeatures:
    """Resolve the effective FeatureSet for a descriptor element.

    Args:
        edition_defaults: table produced by build_edition_defaults().
        file_edition:     integer edition of the file (fdp.edition).
        *feature_sets:    zero or more FeatureSet proto messages ordered from
                          coarsest to finest (file → message → field/enum/oneof).
                          Pass None for absent levels; they are skipped.

    Returns:
        ResolvedFeatures with all six RETENTION_RUNTIME features resolved.
    """
    def _r(name: str) -> int:
        return _resolve_one(edition_defaults, name, file_edition, *feature_sets)

    return ResolvedFeatures(
        field_presence=          _r("field_presence"),
        enum_type=               _r("enum_type"),
        repeated_field_encoding= _r("repeated_field_encoding"),
        utf8_validation=         _r("utf8_validation"),
        message_encoding=        _r("message_encoding"),
        json_format=             _r("json_format"),
    )


# ---------------------------------------------------------------------------
# feature_name_for — convert integer enum value back to string name
# (used by --dump-resolved-features YAML output)
# ---------------------------------------------------------------------------

def feature_value_name(
    table: EditionDefaultTable,
    feature_name: str,
    value: int,
) -> str:
    """Return the string name for an integer enum value of a feature field.

    Falls back to the decimal string if the value is not found.
    """
    _enum_entry = table.get(f"_enum_{feature_name}", {})
    name_to_num: dict[str, int] = _enum_entry if isinstance(_enum_entry, dict) else {}
    for name, num in name_to_num.items():
        if num == value and not name.endswith("_UNKNOWN") and num != 0:
            return name
    # For value 0 (unknown), return the UNKNOWN name if present.
    for name, num in name_to_num.items():
        if num == value:
            return name
    return str(value)
