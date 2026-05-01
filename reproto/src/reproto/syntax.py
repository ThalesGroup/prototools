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

from .context import Context


def fdp_syntax(fdp) -> str:
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
    field,
    is_oneof: bool,
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
    from google.protobuf.descriptor_pb2 import FieldDescriptorProto
    from lib.warnings import cli_warning

    if is_oneof:
        return ''
    if field.label == FieldDescriptorProto.LABEL_REPEATED:
        return 'repeated '
    if ctx.target_syntax == "proto3":
        if field.label == FieldDescriptorProto.LABEL_REQUIRED:
            cli_warning(
                f"field '{field.name}': 'required' label is not valid in proto3; "
                f"rendering as implicit singular"
            )
            return ''
        return 'optional ' if field.proto3_optional else ''
    # proto2
    if field.label == FieldDescriptorProto.LABEL_REQUIRED:
        return 'required '
    return 'optional '


def is_synthetic_oneof(ctx: Context, oneof_name: str, members: list) -> bool:
    """Return True iff the given oneof is a proto3 synthetic oneof.

    Only meaningful when ctx.target_syntax == "proto3"; returns False
    immediately for any other target syntax.

    Detection rule (all conditions must hold):
        1. ctx.target_syntax == "proto3"
        2. oneof_name starts with '_'
        3. exactly one field is in members
        4. that field has proto3_optional == True
    """
    if ctx.target_syntax != "proto3":
        return False
    if not oneof_name.startswith('_'):
        return False
    if len(members) != 1:
        return False
    return bool(members[0].proto3_optional)


def should_render_default(ctx: Context, field) -> bool:
    """Return True iff [default = ...] should be rendered for this field.

    Always False when ctx.target_syntax == "proto3" (proto3 forbids explicit
    defaults).  The caller must emit a cli_warning in that case if the field
    actually has a default_value set.
    """
    if not field.HasField('default_value'):
        return False
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


def should_render_json_name(field) -> bool:
    """Return True iff [json_name = "..."] should be emitted for this field.

    Emit only when the stored json_name differs from the auto-derived camelCase
    of field.name.  Syntax-independent — applies in both proto2 and proto3.
    """
    return field.json_name != _camel_case(field.name)


def allow_weak_import(ctx: Context) -> bool:
    """Return True iff import weak is legal in the target syntax."""
    return ctx.target_syntax == "proto2"


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

    Proto2: always True.
    Proto3: True only when extendee is one of the nine descriptor *Options FQNs
            (custom options are the only proto3-legal extension target).
    """
    if ctx.target_syntax == "proto2":
        return True
    return extendee in _DESCRIPTOR_OPTIONS_FQNS


def allow_extension_ranges(ctx: Context) -> bool:
    """Return True iff `extensions N to M;` declarations are legal in the target syntax."""
    return ctx.target_syntax == "proto2"


def allow_extensions(ctx: Context) -> bool:
    """Return True iff extension ranges and extend blocks are legal in the target syntax.

    Deprecated: prefer allow_extend_block / allow_extension_ranges.
    This alias is kept for call sites that use it only for extension-range decisions.
    """
    return ctx.target_syntax == "proto2"


def allow_groups(ctx: Context) -> bool:
    """Return True iff TYPE_GROUP fields may be rendered as groups."""
    return ctx.target_syntax == "proto2"


def allow_message_set_wire_format(ctx: Context) -> bool:
    """Return True iff MessageOptions.message_set_wire_format may be rendered."""
    return ctx.target_syntax == "proto2"
