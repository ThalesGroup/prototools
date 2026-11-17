# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import logging

from google.protobuf.descriptor import FieldDescriptor
from google.protobuf.descriptor_pb2 import (
    DescriptorProto,
    ExtensionRangeOptions,
)
from google.protobuf.message import Message

from .context import Context
from .field_descriptor import ReFieldDescriptor, fd_is_repeated  # re-export
from .globals import FIELD_NUM_MAX
from .text import (
    MAX_LINE_LENGTH,
    ORPHAN,
    TAB_SIZE,
    Block,
    BlockLine,
)

logger = logging.getLogger(__name__)

__all__ = ['ReFieldDescriptor', 'ReMessage', 'ReReservedRange', 'ReExtensionRange']






# === Message decorator ========================================================

class ReMessage:
    def __init__(self, msg: Message) -> None:
        assert isinstance(msg, Message)
        self.msg = msg

    def is_empty(self) -> bool:
        return len(self.msg.ListFields()) == 0

    def render(self, depth: int = 0) -> Block:
        assert isinstance(depth, int)
        out = Block()

        # Use ListFields() to iterate only over fields that are actually set.
        # This is consistent with how options are rendered throughout the codebase.
        # A map field arrives as the repeated message it is on the wire, its
        # entries in order: option values are parsed with classes from
        # ctx.options_pool, where no message is a map entry (spec 0424 S1).
        for field, value in self.msg.ListFields():
            name = _aggregate_field_name(field)
            if fd_is_repeated(field):
                for item in value:
                    _render_aggregate_value(out, field, item, name, depth)
            else:
                _render_aggregate_value(out, field, value, name, depth)
        return out


def _aggregate_field_name(field: FieldDescriptor) -> str:
    """A field's name in text format: `[pkg.ext]` for an extension, the
    type's name for a group, the field's own name otherwise (spec 0424 S1)."""
    if field.is_extension:
        return f'[{field.full_name}]'
    if field.type == FieldDescriptor.TYPE_GROUP:
        return field.message_type.name
    return field.name


def _render_aggregate_value(
    out: Block,
    field: FieldDescriptor,
    value: object,
    name: str,
    depth: int,
) -> None:
    """Append one value of `field`, named `name`, to an aggregate."""
    if field.type in (FieldDescriptor.TYPE_MESSAGE, FieldDescriptor.TYPE_GROUP):
        assert isinstance(value, Message)
        out.append(BlockLine(f'{name}: {{', depth))
        out.extend(ReMessage(value).render(depth + 1))
        out.append(BlockLine('}', depth))
    elif field.type == FieldDescriptor.TYPE_ENUM:
        enum_name = field.enum_type.values_by_number[value].name
        out.append(BlockLine(f'{name}: {enum_name}', depth))
    elif field.type == FieldDescriptor.TYPE_BYTES:
        assert isinstance(value, bytes)
        escaped = ''.join(f'\\x{b:02x}' for b in value)
        out.append(BlockLine(f'{name}: "{escaped}"', depth))
    else:
        out.append(BlockLine(f'{name}: {repr(value)}', depth))

# === ReservedRange decorator ==================================================

class ReReservedRange:
    def __init__(self, range: DescriptorProto.ReservedRange) -> None:
        assert isinstance(range, Message)
        self.this = range

    @property
    def end(self) -> int:
        return self.this.end

    @property
    def start(self) -> int:
        return self.this.start


    def render(self, depth: int = 0) -> Block:
        """Render a ReservedRange in proto2 syntax"""
        assert isinstance(depth, int)
        out = Block()

        if self.start == self.end - 1:
            out.append(BlockLine(
                f'reserved {self.start};', depth))
        else:
            out.append(BlockLine(
                f'reserved {self.start} '
                f'to {self.end - 1};', depth))
        return out


# === ExtensionRange decorator =================================================

class ReExtensionRange:
    def __init__(self, range: DescriptorProto.ExtensionRange):
        assert isinstance(range, Message)
        self.this = range

    @property
    def end(self) -> int:
        return self.this.end

    @property
    def options(self) -> ExtensionRangeOptions:
        return self.this.options

    @property
    def start(self) -> int:
        return self.this.start


    def render(self, ctx: Context, depth: int = 0, *, scope: str) -> Block:
        assert isinstance(depth, int) and depth >= 0
        extension_range = self.this
        out = Block()

        if self.start == self.end - 1:
            string = f'extensions {self.start}'
        elif self.end - 1 >= FIELD_NUM_MAX:
            # Extensions numbers cannot be greater than 536,870,911
            # aka "max"
            string = f'extensions {extension_range.start} to max'
        else:
            string = (f'extensions {extension_range.start} '
                    f'to {extension_range.end - 1}')

        # --- ExtensionRange options -------------------------------------------
        # Parsed with the dynamic class, as every other element's options:
        # the generated descriptor_pb2 type keeps custom options as unknown
        # fields, which render as nothing (spec 0424 S1).
        from .base import render_options_from_message as _rom
        from .option_messages import parse_options
        opts_msg: Message = self.options
        opts_desc = self.options.DESCRIPTOR
        if ctx.ero_desc is not None and ctx.ero_cls is not None:
            opts_desc = ctx.ero_desc
            opts_msg = parse_options(
                ctx, opts_desc, ctx.ero_cls, self.options.SerializeToString())
        texts = _rom(
            ctx=ctx,
            opts_msg=opts_msg,
            options_descriptor=opts_desc,
            composite=True,
            depth=depth + 1,
            exclude={'declaration'},
            scope=scope,
        )
        # Strip trailing comma from last non-orphan block
        for text in reversed(texts):
            if text and text[0].type != ORPHAN:
                text[-1].text = text[-1].text.rstrip(',')
                break
        set_options = 0
        for text in texts:
            if text and text[0].type != ORPHAN:
                set_options += 1

        # Maybe there were no options?
        if set_options == 0:
            # No options
            string += ';'
            out.append(BlockLine(string, depth))
            if texts:
                out.append(BlockLine('[', depth, ORPHAN))
                for text in texts:
                    out.extend(text)
                out.append(BlockLine(']', depth, ORPHAN))

        # Maybe there was a single option that fits on a single line?
        elif set_options == 1 and len(texts) == 1 and len(texts[0]) == 1:
            # A single option that fits on the same line
            short_option = f'[{texts[0][0].text}];'
            string2 = string + ' ' + short_option
            if len(string2) <= MAX_LINE_LENGTH - depth * TAB_SIZE:
                out.append(BlockLine(string2, depth))
            else:
                out.append(BlockLine(string, depth))
                out.append(BlockLine(short_option, depth+1))

        # If none of the above, the default is:
        else:
            string += ' ['
            out.append(BlockLine(string, depth))
            for text in texts:
                out.extend(text)
            out.append(BlockLine('];', depth))

        return out


# === EnumOptions decorator ====================================================

