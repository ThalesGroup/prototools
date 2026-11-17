# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

from typing import Any

from google._upb._message import (
    RepeatedCompositeContainer,
    RepeatedScalarContainer,
)
from google.protobuf.descriptor import FieldDescriptor
from google.protobuf.descriptor_pb2 import FieldDescriptorProto
from google.protobuf.message import Message

from .anomalies import report as anomaly_report
from .context import Context
from .mappings import canonize_opt_name
from .scalar import Scalar
from .text import Block, BlockLine


def fd_is_repeated(field: FieldDescriptor) -> bool:
    """Return True if field is a repeated FieldDescriptor.

    Uses is_repeated when available (protobuf >= 4.x / upb) and falls back to
    field.label for older versions (protobuf 3.x).  This avoids AttributeError
    on raw upb FieldDescriptor objects that surface when imported _pb2 modules
    are missing (the Python wrapper layer is never applied in that case, so
    .label is absent even before its official deprecation in 6.x).
    """
    try:
        return field.is_repeated
    except AttributeError:
        return field.label == FieldDescriptor.LABEL_REPEATED


def fd_is_required(field: FieldDescriptor) -> bool:
    """Return True if field is a required FieldDescriptor.

    Same compatibility rationale as fd_is_repeated.
    """
    try:
        return field.is_required
    except AttributeError:
        return field.label == FieldDescriptor.LABEL_REQUIRED


def fd_is_packed(field: FieldDescriptor) -> bool:
    """Return True if field is *declared* packed (spec 0371 S1/S2).

    A field is declared packed iff it is repeated, its type is packable (a
    scalar numeric type, bool or enum), and its syntax says so: proto2 only
    with `[packed = true]`, proto3 unless `[packed = false]` (whatever other
    options the field carries), editions when its resolved
    `features.repeated_field_encoding` is PACKED. `FieldDescriptor.is_packed`
    already resolves all three, so this only narrows it to repeated
    packable fields and supplies the fallback its siblings above use.

    This is the declaration, which is what a *writer* follows. A reader
    accepts both encodings (spec 0175); the scorer uses this to charge a
    record whose encoding contradicts it.
    """
    from .globals import PACKABLE_TYPES
    if not fd_is_repeated(field) or field.type not in PACKABLE_TYPES:
        return False
    try:
        return bool(field.is_packed)
    except AttributeError:
        return field.has_options and field.GetOptions().packed


class ReFieldDescriptor:
    def __init__(self, field: FieldDescriptor) -> None:
        assert isinstance(field, FieldDescriptor)
        self.this = field

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
    def label(self) -> int:
        if fd_is_repeated(self.this):
            return FieldDescriptor.LABEL_REPEATED
        if fd_is_required(self.this):
            return FieldDescriptor.LABEL_REQUIRED
        return FieldDescriptor.LABEL_OPTIONAL

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
            scope: str = '',
    ) -> tuple[Block, bool]:
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
            # Spec 0439 S1: named as googleapis names it, from `scope`.
            from .names import option_name
            name = f'({option_name(ctx, self.full_name, scope)})'

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
                # Lazy import to avoid circular dependency with simple_types.py
                from .simple_types import ReMessage
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
                # Lazy import to avoid circular dependency with simple_types.py
                from .simple_types import ReMessage
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
