# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import logging

from google.protobuf.descriptor import Descriptor, FieldDescriptor
from google.protobuf.message import Message


from .context import Context
from .option_messages import option_extensions
from .field_descriptor import fd_is_repeated
from .simple_types import ReFieldDescriptor
from .text import (
    CODE,
    ORPHAN,
    Block,
)

logger = logging.getLogger(__name__)


# === Options decorator (generic) =============================================

class ReOptions:
    def __init__(self, options: Message) -> None:
        assert isinstance(options, Message)
        self.this = options

    def render(
        self,
        ctx: Context,
        composite: bool,
        lev: int,
        exclude: set[str] | None = None,
    ) -> list[Block]:
        """The built-in options set on the message, but those in `exclude`
        (e.g. ``{"packed"}``)."""
        assert isinstance(lev, int)
        blocks: list[Block] = list()

        # Use ListFields() to get only fields that are actually set (non-default)
        # This works across proto2, proto3, and editions syntax
        # ListFields() returns: list of (FieldDescriptor, value) tuples
        # - For message fields: only included if HasField() would return true
        # - For singular primitives: only if set in proto2, or non-zero in proto3
        # - For repeated fields: only if contains at least one element

        for fd_desc, val in self.this.ListFields():
            # Skip extension fields - they're handled separately via FindAllExtensions()
            if fd_desc.is_extension:
                continue
            if exclude and fd_desc.name in exclude:
                continue

            opt = ReFieldDescriptor(fd_desc)

            if fd_is_repeated(fd_desc):
                # val is already the list/repeated container
                for v in val:
                    block, is_orp = opt.dump_option(ctx, v, lev)
                    if not block:  # nothing rendered
                        continue
                    if not composite:
                        block.prepend('option ')
                        block.postpend(';')
                    else:
                        block.postpend(',')
                    block.set_type(ORPHAN if is_orp else CODE)
                    blocks.append(block)

            else:
                # Singular field (val is already the value)
                block, is_orp = opt.dump_option(ctx, val, lev)
                if not block:  # nothing rendered
                    continue
                if not composite:
                    block.prepend('option ')
                    block.postpend(';')
                else:
                    block.postpend(',')
                block.set_type(ORPHAN if is_orp else CODE)
                blocks.append(block)

        return blocks
    

# === Extensions decorator (generic) =====================================

class ReExtensions:
    def __init__(self, options: Message) -> None:
        assert isinstance(options, Message)
        self.this = options

    def render(
        self,
        ctx: Context,
        options_desc: Descriptor,
        composite: bool,
        lev: int,
        *,
        scope: str,
    ) -> tuple[list[Block], bool]:
        """The extensions set on the message, by number: `option (NAME) =
        V;`, or `(NAME) = V,` in a composite, NAME written from `scope`
        (spec 0439 S1)."""
        assert isinstance(lev, int)
        blocks: list[Block] = list()
        is_orphan = True

        # Sort by extension number for reproductibility
        for ext_desc in sorted(option_extensions(ctx, options_desc),
                               key=lambda d: d.number):
            opt = ReFieldDescriptor(ext_desc)
            val = self.this.Extensions[ext_desc]

            if fd_is_repeated(ext_desc):
                for v in val:
                    block, is_orp = opt.dump_option(
                        ctx, v, lev, True, scope)
                    is_orphan &= is_orp
                    if not composite:
                        block.prepend('option ')
                        block.postpend(';')
                    else:
                        block.postpend(',')
                    block.set_type(ORPHAN if is_orp else CODE)
                    blocks.append(block)
            else:
                if self.this.HasExtension(ext_desc):
                    block, is_orp = opt.dump_option(
                        ctx, val, lev, True, scope)
                    is_orphan &= is_orp
                    if not composite:
                        block.prepend('option ')
                        block.postpend(';')
                    else:
                        block.postpend(',')
                    block.set_type(ORPHAN if is_orp else CODE)
                    blocks.append(block)
        return blocks, is_orphan


# === Fields the options type does not declare (spec 0438 S6) ============

def _unknown_value(wire_type: int, data: object) -> str:
    """An unknown field's value, by its wire type alone: a varint or a
    fixed-width number in decimal, bytes quoted with octal escapes, a group
    as its fields, `{ <N>: V ... }`, recursively."""
    if wire_type == 2:
        assert isinstance(data, bytes)
        out = []
        for b in data:
            c = chr(b)
            if c in '"\\':
                out.append('\\' + c)
            elif 0x20 <= b < 0x7f:
                out.append(c)
            else:
                out.append(f'\\{b:03o}')
        return '"' + ''.join(out) + '"'
    if wire_type == 3:
        from google.protobuf.unknown_fields import UnknownFieldSet
        assert isinstance(data, UnknownFieldSet)
        inner = [f'<{f.field_number}>: {_unknown_value(f.wire_type, f.data)}'
                 for f in data if f.wire_type != 4]
        return '{ ' + ' '.join(inner) + ' }' if inner else '{ }'
    return str(data)


def unknown_option_blocks(
    ctx: Context,
    options: Message,
    composite: bool,
    lev: int,
) -> list[Block]:
    """Each field of `options` its type does not declare, and no loaded
    file declares as an extension of it, as an orphan: ``option <N> =
    V;`` (``<N> = V,`` in a composite), with W22, which is said even when
    --redact-orphans removes the line."""
    from google.protobuf.unknown_fields import UnknownFieldSet
    from .lib.warnings import get_collector
    from .text import BlockLine
    blocks: list[Block] = []
    type_name = options.DESCRIPTOR.full_name
    for field in UnknownFieldSet(options):
        if field.wire_type == 4:   # an end-group marker
            continue
        text = f'<{field.field_number}> = {_unknown_value(field.wire_type, field.data)}'
        text = text + ',' if composite else f'option {text};'
        blocks.append(Block([BlockLine(text, lev, type=ORPHAN)]))
        get_collector().squash(
            "W22",
            f"'{ctx.current_file}': option field {field.field_number} of {type_name} "
            "is declared nowhere: written as an orphan",
        )
    return blocks


# === Custom features, as paths (spec 0439 S2) ===========================

def _is_flat_feature(ext: FieldDescriptor, value: Message) -> bool:
    """What protoc admits in a feature (feature_resolver.cc): a singular
    message whose fields are singular enums or bools, no extension, and
    nothing unknown."""
    from google.protobuf.unknown_fields import UnknownFieldSet
    if fd_is_repeated(ext) or ext.message_type is None:
        return False
    for fd, _ in value.ListFields():
        if fd.is_extension or fd_is_repeated(fd):
            return False
        if fd.type not in (FieldDescriptor.TYPE_ENUM, FieldDescriptor.TYPE_BOOL):
            return False
    return len(UnknownFieldSet(value)) == 0


def feature_blocks(
    ctx: Context,
    features: Message,
    descriptor: Descriptor,
    composite: bool,
    lev: int,
    *,
    scope: str,
) -> list[Block]:
    """The custom features set on `features` (a FeatureSet read with the
    options pool, `descriptor` its type), by number, each field of each a path:
    `option features.(E).f = V;`, `features.(E).f = V,` in a composite; an
    empty one `features.(E) = {}`. A value protoc's rules exclude is written
    as an aggregate, as any message-valued option."""
    from .names import option_name
    from .mappings import canonize_opt_name
    from .text import BlockLine
    blocks: list[Block] = []

    def statement(text: str, orphan: bool) -> None:
        text = f'{text},' if composite else f'option {text};'
        blocks.append(Block([BlockLine(text, lev, type=ORPHAN if orphan else CODE)]))

    for ext in sorted(option_extensions(ctx, descriptor), key=lambda d: d.number):
        if not features.HasExtension(ext):
            continue
        value = features.Extensions[ext]
        full_name = ReFieldDescriptor(ext).full_name   # with its leading dot
        _, orphan = canonize_opt_name(ctx, full_name, custom=True)
        name = f'features.({option_name(ctx, full_name, scope)})'
        if not _is_flat_feature(ext, value):
            block, orphan = ReFieldDescriptor(ext).dump_option(ctx, value, lev, True, scope)
            block.prepend('features.')
            if composite:
                block.postpend(',')
            else:
                block.prepend('option ')
                block.postpend(';')
            block.set_type(ORPHAN if orphan else CODE)
            blocks.append(block)
            continue
        fields = value.ListFields()
        if not fields:
            statement(f'{name} = {{}}', orphan)
        for fd, v in fields:
            statement(f'{name}.{fd.name} = {ReFieldDescriptor(fd).get_scalar(v)}', orphan)
    return blocks

