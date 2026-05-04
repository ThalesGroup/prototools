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
from .field_descriptor import ReFieldDescriptor  # re-export
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

        def escape_bytes(value: bytes) -> str:
            """Escape bytes for .proto syntax."""
            return '"' + ''.join(f'\\x{b:02x}' for b in value) + '"'
        
        msg = self.msg
        out = Block()

        # Use ListFields() to iterate only over fields that are actually set.
        # This is consistent with how options are rendered throughout the codebase.
        for field, value in msg.ListFields():
            # Handle repeated fields
            if field.label == FieldDescriptor.LABEL_REPEATED:
                for item in value:
                    if field.type == FieldDescriptor.TYPE_MESSAGE:
                        out.append(BlockLine(f'{field.name}: {{', depth))
                        out.extend(ReMessage(item).render(depth+1))
                        out.append(BlockLine('}', depth))
                    elif field.type == FieldDescriptor.TYPE_ENUM:
                        enum_name = field.enum_type.values_by_number[item].name
                        out.append(BlockLine(f'{field.name}: {enum_name}', depth))
                    elif field.type == FieldDescriptor.TYPE_BYTES:
                        out.append(BlockLine(f'{field.name}: {escape_bytes(item)}', depth))
                    else:
                        out.append(BlockLine(f'{field.name}: {repr(item)}', depth))

            # Singular fields
            else:
                if field.type == FieldDescriptor.TYPE_MESSAGE:
                    out.append(BlockLine(f'{field.name}: {{', depth))
                    out.extend(ReMessage(value).render(depth+1))
                    out.append(BlockLine('}', depth))
                elif field.type == FieldDescriptor.TYPE_ENUM:
                    enum_name = field.enum_type.values_by_number[value].name
                    out.append(BlockLine(f'{field.name}: {enum_name}', depth))
                elif field.type == FieldDescriptor.TYPE_BYTES:
                    out.append(BlockLine(
                        f'{field.name}: {escape_bytes(value)}', depth))
                else:
                    out.append(BlockLine(f'{field.name}: {repr(value)}', depth))

            # Maps (repeated message entries)
            if field.message_type and field.message_type.GetOptions().map_entry:
                for k, v in value.items():
                    out.append(BlockLine(f'{field.name}: {{', depth))
                    out.append(BlockLine(f'key: {repr(k)}', depth+1))
                    if isinstance(v, Message):
                        out.append(BlockLine('value {', depth+1))
                        out.extend(ReMessage(v).render(depth+2))
                        out.append(BlockLine('}', depth+1))
                    else:
                        out.append(BlockLine(f'value: {repr(v)}', depth+1))
                    out.append(BlockLine('}', depth))
        return out

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


    def render(self, ctx: Context, depth: int = 0) -> Block:
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
        from .base import render_options_from_message as _rom
        texts = _rom(
            ctx=ctx,
            opts_msg=self.options,
            options_descriptor=self.options.DESCRIPTOR,
            composite=True,
            depth=depth + 1,
            exclude={'declaration'},
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

