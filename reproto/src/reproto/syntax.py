# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Syntax helpers for proto descriptor rendering (specs 0016, 0019).

Convention: every helper takes ctx as its first argument and reads whatever
it needs (syntax, target_syntax, …) from it directly.  This keeps call sites
clean as the number of context-dependent decisions grows.

fdp_syntax() is the one exception: it is called before ctx.syntax is set
(it is what *sets* ctx.syntax), so it cannot receive ctx.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from google.protobuf.descriptor_pb2 import FieldDescriptorProto, FileDescriptorProto

from .context import Context
from .feature_resolution import (
    FIELD_PRESENCE_IMPLICIT,
    FIELD_PRESENCE_LEGACY_REQUIRED,
    MESSAGE_ENCODING_DELIMITED,
)

if TYPE_CHECKING:
    from google.protobuf.descriptor_pb2 import Edition, FeatureSet
    from .feature_resolution import ResolvedFeatures
    from .text import Block


def fdp_syntax(fdp: FileDescriptorProto) -> str:
    """Return the syntax of a FileDescriptorProto as a non-empty string.

    fdp.syntax is "" for proto2 files (protoc omits the field); normalise
    that to "proto2".  All other values ("proto3", "editions", …) are
    returned as-is.

    Not ctx-based: this is called to *populate* ctx.syntax, so ctx is not
    yet valid at that point.
    """
    return fdp.syntax or "proto2"


def packed_option(
    ctx: Context,
    has_field: bool,
    effective_packed: bool,
    features: ResolvedFeatures | None = None,
) -> str | None:
    """Return the string to emit for the packed option, or None to emit nothing.

    Args:
        ctx:              rendering context (reads ctx.syntax, ctx.target_syntax)
        has_field:        True if packed was explicitly set in the source .proto
        effective_packed: fo_msg.packed — the wire-level effective value
                          (includes proto3 defaults)

    Rules:
      - has_field=True  → emit the explicit value regardless of syntax
      - has_field=False, source==target → emit nothing (default preserved)
      - has_field=False, source=proto3, target=proto2 → emit "true" if
        effective_packed is True (preserve wire semantics across downconversion)
    """
    if features is not None:
        # Editions: packed intent is expressed via features { } (phase 3).
        # Only emit the legacy annotation if it was explicitly present in the source.
        if has_field:
            return "true" if effective_packed else "false"
        return None
    if has_field:
        return "true" if effective_packed else "false"
    if ctx.syntax == ctx.target_syntax:
        return None
    # Cross-syntax conversion: source=proto3 (packed by default), target=proto2
    # (unpacked by default) — must emit explicit annotation to preserve semantics.
    if effective_packed:
        return "true"
    return None


def field_label(
    ctx: Context,
    field: FieldDescriptorProto,
    is_oneof: bool,
    features: ResolvedFeatures | None = None,
) -> str:
    """Return the label keyword to emit before the field type (with trailing space), or ''.

    Args:
        ctx:     rendering context (reads ctx.target_syntax)
        field:   FieldDescriptorProto
        is_oneof: True when this field is rendered inside a real oneof block

    Rules:
        is_oneof                              → ''
        field.label == LABEL_REPEATED         → 'repeated '
        ctx.target_syntax == "proto2":
            field.label == LABEL_REQUIRED     → 'required '
            field.label == LABEL_OPTIONAL     → 'optional '
        ctx.target_syntax == "proto3":
            field.proto3_optional             → 'optional '
            else                              → ''  (implicit singular)
    """
    if is_oneof:
        return ''
    if field.label == FieldDescriptorProto.LABEL_REPEATED:
        return 'repeated '
    if ctx.target_syntax == "editions":
        # In editions output, singular field presence is expressed via
        # features.field_presence options — no label is emitted.
        return ''
    if features is not None:
        # Editions rendered as proto2 (--force-proto2-output): use resolved
        # presence to pick the proto2 label.
        if features.field_presence == FIELD_PRESENCE_LEGACY_REQUIRED:
            return 'required '
        if features.field_presence == FIELD_PRESENCE_IMPLICIT:
            return ''
        # EXPLICIT or unknown → optional
        return 'optional '
    if ctx.target_syntax == "proto3":
        if field.label == FieldDescriptorProto.LABEL_REQUIRED:
            # C3: comment line is inserted by re_field.py before calling field_label().
            return ''
        return 'optional ' if field.proto3_optional else ''
    # proto2
    if field.label == FieldDescriptorProto.LABEL_REQUIRED:
        return 'required '
    return 'optional '


def is_synthetic_oneof(
    ctx: Context,
    oneof_name: str,
    members: list,
    features: ResolvedFeatures | None = None,
) -> bool:
    """Return True iff the given oneof is a proto3 synthetic oneof.

    Only meaningful when ctx.target_syntax == "proto3"; returns False
    immediately for any other target syntax.

    Detection rule (all conditions must hold):
        1. ctx.target_syntax == "proto3"
        2. oneof_name starts with '_'
        3. exactly one field is in members
        4. that field has proto3_optional == True
    """
    if features is not None:
        # Editions: synthetic oneof ↔ single IMPLICIT member (proto3_optional not set).
        if not oneof_name.startswith('_'):
            return False
        if len(members) != 1:
            return False
        return features.field_presence == FIELD_PRESENCE_IMPLICIT
    if ctx.target_syntax != "proto3":
        return False
    if not oneof_name.startswith('_'):
        return False
    if len(members) != 1:
        return False
    return bool(members[0].proto3_optional)


def should_render_default(ctx: Context, field: FieldDescriptorProto, features: ResolvedFeatures | None = None) -> bool:
    """Return True iff [default = ...] should be rendered for this field.

    Always False when ctx.target_syntax == "proto3" (proto3 forbids explicit
    defaults).  The caller must emit a cli_warning in that case if the field
    actually has a default_value set.
    For editions: False when field_presence == IMPLICIT (implicit fields have no default).
    """
    if not field.HasField('default_value'):
        return False
    if features is not None:
        return features.field_presence != FIELD_PRESENCE_IMPLICIT
    return ctx.target_syntax != "proto3"


def _camel_case(name: str) -> str:
    """Derive the default JSON name (camelCase) for a proto field name.

    Matches protoc's algorithm: split on '_', keep first component as-is,
    capitalize the first letter of each subsequent non-empty component, join.

    Examples:
        'field_name'        -> 'fieldName'
        'already_camel'     -> 'alreadyCamel'
        'x'                 -> 'x'
        'under_score_heavy' -> 'underScoreHeavy'
    """
    parts = name.split('_')
    return parts[0] + ''.join(p.capitalize() for p in parts[1:] if p)


def should_render_json_name(field: FieldDescriptorProto) -> bool:
    """Return True iff [json_name = "..."] should be emitted for this field.

    Emit only when the stored json_name differs from the auto-derived camelCase
    of field.name.  Syntax-independent — applies in both proto2 and proto3.
    """
    return field.json_name != _camel_case(field.name)


def _edition_name(edition: 'Edition.ValueType') -> str:
    """Map Edition enum value to the string used in .proto source (e.g. \"2023\")."""
    from google.protobuf.descriptor_pb2 import Edition
    name = Edition.Name(edition)   # e.g. "EDITION_2023"
    if name.startswith("EDITION_"):
        return name[len("EDITION_"):]
    return name


def allow_weak_import(ctx: Context) -> bool:
    """Return True iff import weak is legal in the target syntax."""
    return ctx.target_syntax in ("proto2", "editions")


# The nine *Options FQNs that proto3 allows extending (custom options).
_DESCRIPTOR_OPTIONS_FQNS: frozenset[str] = frozenset({
    ".google.protobuf.FileOptions",
    ".google.protobuf.MessageOptions",
    ".google.protobuf.FieldOptions",
    ".google.protobuf.OneofOptions",
    ".google.protobuf.ExtensionRangeOptions",
    ".google.protobuf.EnumOptions",
    ".google.protobuf.EnumValueOptions",
    ".google.protobuf.ServiceOptions",
    ".google.protobuf.MethodOptions",
})


def allow_extend_block(ctx: Context, extendee: str) -> bool:
    """Return True iff an extend block for extendee is legal in the target syntax.

    Proto2/editions: always True.
    Proto3: True only when extendee (after variant namespace rewriting) is one
            of the nine descriptor *Options FQNs (custom options are the only
            proto3-legal extension target).
    """
    if ctx.target_syntax in ("proto2", "editions"):
        return True
    from .fake_types import Ref
    from .mappings import apply_variant_namespace
    canonical = str(apply_variant_namespace(ctx, Ref(extendee)))
    return canonical in _DESCRIPTOR_OPTIONS_FQNS


def allow_extension_ranges(ctx: Context) -> bool:
    """Return True iff `extensions N to M;` declarations are legal in the target syntax."""
    return ctx.target_syntax in ("proto2", "editions")


def allow_extensions(ctx: Context) -> bool:
    """Return True iff extension ranges and extend blocks are legal in the target syntax.

    Deprecated: prefer allow_extend_block / allow_extension_ranges.
    This alias is kept for call sites that use it only for extension-range decisions.
    """
    return ctx.target_syntax in ("proto2", "editions")


def allow_groups(ctx: Context, features: ResolvedFeatures | None = None) -> bool:
    """Return True iff TYPE_GROUP fields may be rendered as groups."""
    if features is not None:
        return features.message_encoding == MESSAGE_ENCODING_DELIMITED
    return ctx.target_syntax in ("proto2", "editions")


def allow_message_set_wire_format(ctx: Context) -> bool:
    """Return True iff MessageOptions.message_set_wire_format may be rendered."""
    return ctx.target_syntax in ("proto2", "editions")


# The six standard RETENTION_RUNTIME fields of FeatureSet, in declaration order.
_FEATURE_FIELDS: tuple[str, ...] = (
    "field_presence",
    "enum_type",
    "repeated_field_encoding",
    "utf8_validation",
    "message_encoding",
    "json_format",
)
_FEATURE_FIELDS_SET: frozenset[str] = frozenset(_FEATURE_FIELDS)


def render_features_block(
    ctx: Context,
    fs: FeatureSet,
    depth: int,
    inline: bool = False,
) -> 'Block':
    """Emit features { } option lines for an element with explicit FeatureSet overrides.

    Args:
        ctx:    Rendering context.  Guard: returns empty Block immediately when
                ctx.target_syntax != "editions" (proto2/proto3 files, or edition
                files rendered with --force-proto2-output).
        fs:     The raw FeatureSet proto from the element's options.  Only fields
                for which fs.HasField(name) is True are emitted.
        depth:  Indentation depth for the output lines.
        inline: True → emit `features.<name> = <val>,` (for composite field
                options); False → emit `option features.<name> = <val>;`
                (standalone, for file/message/enum).

    Returns:
        A Block (possibly empty) with the rendered feature option lines.
    """
    from .text import Block, BlockLine

    out = Block()
    if ctx.target_syntax != "editions":
        return out

    from .feature_resolution import feature_value_name
    # Use ListFields() to iterate only explicitly-set fields, avoiding the
    # HasField(Literal) stub constraint.  Filter to the known RETENTION_RUNTIME
    # fields and emit them in declaration order.
    set_fields: dict[str, int] = {
        fd.name: val
        for fd, val in fs.ListFields()
        if fd.name in _FEATURE_FIELDS_SET
    }
    for fname in _FEATURE_FIELDS:
        if fname not in set_fields:
            continue
        value_name = feature_value_name(ctx.edition_defaults, fname, set_fields[fname])
        if inline:
            out.append(BlockLine(f'features.{fname} = {value_name},', depth))
        else:
            out.append(BlockLine(f'option features.{fname} = {value_name};', depth))

    return out
