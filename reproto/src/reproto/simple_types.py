# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import logging
from typing import Any

# Note: These types must be imported from the actual runtime backend (_upb)
# rather than from google.protobuf.internal.containers because:
# 1. Pattern matching requires the actual runtime types
# 2. The upb backend (used in protobuf 6.x) has different class names
# 3. Internal containers has RepeatedCompositeFieldContainer (different name)
from google._upb._message import (
    RepeatedCompositeContainer,
    RepeatedScalarContainer,
)
from google.protobuf.descriptor import FieldDescriptor
from google.protobuf.descriptor_pb2 import (
    DescriptorProto,
    ExtensionRangeOptions,
    FieldDescriptorProto,
)
from google.protobuf.message import Message

from .anomalies import report as anomaly_report
from .context import Context
from .globals import FIELD_NUM_MAX
from .mappings import canonize_opt_name
from .scalar import Scalar
from .text import (
    MAX_LINE_LENGTH,
    ORPHAN,
    TAB_SIZE,
    Block,
    BlockLine,
)

logger = logging.getLogger(__name__)


### ############################################################################
### Non-Proto Decorators
### ############################################################################

## === FieldDescriptor decorator ===============================================


class ReFieldDescriptor:
    def __init__(self, field: FieldDescriptor) -> None:
        assert isinstance(field, FieldDescriptor)

        self.this = field
        #super().__setattr__('this', field)

    @property
    def full_name(self) -> str:
        full_name = self.this.full_name
        if full_name[0] != '.':
            return '.' + full_name
        else:
            return full_name

    @property
    def enum_type(self) -> Any:
        return self.this.enum_type

    @property
    def label(self) -> Any:
        return self.this.label

    @property
    def name(self) -> str:
        return self.this.name

    @property
    def type(self) -> Any:
        return self.this.type

    def get_scalar(self, value: Any) -> Scalar:
        assert isinstance(self.type, int)
        match value:
            # Try to match for bool() before int(), because int() matches bool
            case bool():
                match self.type:
                    case FieldDescriptorProto.TYPE_BOOL:
                        return Scalar(value)
                    case _:
                        raise RuntimeError(
                            f'Unexpected FieldDescriptor type: {self.type}.')

            case int():
                match self.type:
                    case (
                         FieldDescriptorProto.TYPE_INT64
                       | FieldDescriptorProto.TYPE_UINT64
                       | FieldDescriptorProto.TYPE_INT32
                       | FieldDescriptorProto.TYPE_FIXED64
                       | FieldDescriptorProto.TYPE_FIXED32
                       | FieldDescriptorProto.TYPE_UINT32
                       | FieldDescriptorProto.TYPE_SFIXED32
                       | FieldDescriptorProto.TYPE_SFIXED64
                       | FieldDescriptorProto.TYPE_SINT32
                       | FieldDescriptorProto.TYPE_SINT64
                    ):
                        return Scalar(value)
                    case FieldDescriptor.TYPE_ENUM:
                        return Scalar(
                            f'{self.enum_type.values_by_number.get(value).name}',
                            True)
                    case _:
                        raise RuntimeError(
                            f'Unexpected FieldDescriptor type: {self.type}.')

            case float():
                match self.type:
                    case (
                          FieldDescriptorProto.TYPE_DOUBLE
                        | FieldDescriptorProto.TYPE_FLOAT
                    ):
                        return Scalar(value)
                    case _:
                        raise RuntimeError(
                            f'Unexpected FieldDescriptor type: {self.type}.')

            case str():
                match self.type:
                    case FieldDescriptorProto.TYPE_STRING:
                        return Scalar(value)
                    case _:
                        raise RuntimeError(
                            f'Unexpected FieldDescriptor type: {self.type}.')

            case bytes():
                match self.type:
                    case FieldDescriptorProto.TYPE_BYTES:
                        return Scalar(value)
                    case _:
                        raise RuntimeError(
                            f'Unexpected FieldDescriptor type: {self.type}.')

            case _:
                # D1: caller (dump_option) is responsible for inserting the comment.
                raise TypeError(type(value).__name__)

    def dump_option(
            self,
            ctx: Context,
            value: Any,
            lev: int,
            custom: bool = False,
    )-> tuple[Block, bool]:
        """
        Fully faithful rendering of protobuf option value for proto2 syntax.
        Handles:
            - Scalars (int, float, bool, string, bytes)
            - RepeatedScalarContainer (repeated scalars)
            - RepeatedCompositeContainer (repeated messages)
            - Nested messages (Message)
        """
        assert isinstance(self.type, int)
        assert isinstance(lev, int)

        block = Block()
        name, is_orphan = canonize_opt_name(ctx, self.full_name, custom=custom)
        if name == '':
            return (Block(), is_orphan)
        if custom:
            name = f'({name})'

        match value:
            # Scalars
            case int() | bool() | float() | str() | bytes():
                try:
                    scalar = self.get_scalar(value)
                except TypeError as e:
                    # D1: get_scalar() raised because the Python type does not
                    # match the proto field type — emit comment + fallback 0.
                    block.insert(0, anomaly_report("D1", lev,
                                                   name=name, type=str(e)))
                    block.append(BlockLine(f'{name} = 0', lev))
                    return block, is_orphan
                block.append(BlockLine(f'{name} = {str(scalar)}', lev))
                return block, is_orphan

            # Nested messages
            case Message():
                message = ReMessage(value)
                if message.is_empty():
                    block.append(BlockLine(f'{name} = {{}}', lev))
                else:
                    block.append(BlockLine(f'{name} = {{', lev))
                    block.extend(message.render(lev+1))
                    block.append(BlockLine('}', lev))
                return block, is_orphan

            case RepeatedScalarContainer():
                scalars: list[Scalar] = list()
                for v in value:
                    scalar = self.get_scalar(v)
                    block.append(BlockLine(f'{name} = {str(scalar)}', lev))
                    scalars.append(scalar)
                for index, line in enumerate(block):
                    if index < len(block) - 1:
                        line.postpend(',')
                return block, is_orphan

            case RepeatedCompositeContainer():
                blocks: list[Block] = []
                messages: list[ReMessage] = list()
                for m in value:
                    assert isinstance(m, Message)
                    message = ReMessage(m)
                    b = Block()
                    b.append(BlockLine(f'{name} = {{', lev))
                    b.extend(message.render(lev+1))
                    b.append(BlockLine('}', lev))
                    blocks.append(b)
                    messages.append(message)
                for b in blocks[:-1]:
                    b.postpend(',')
                    block.extend(b)
                if blocks:
                    block.extend(blocks[-1])
                return block, is_orphan

            case _:
                # D2: descriptor type is not any of the known match arms.
                block.insert(0, anomaly_report("D2", lev, name=name))
                return block, True






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
                out.append(BlockLine('// [', depth))
                for text in texts:
                    out.extend(text)
                out.append(BlockLine('// ]', depth))

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

