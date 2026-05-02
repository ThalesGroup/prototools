# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""ReEnumValueDescriptorProto - Enum value descriptor (not a Node)."""

from __future__ import annotations

from typing import TYPE_CHECKING

from google.protobuf.descriptor_pb2 import (
    EnumValueDescriptorProto,
    EnumValueOptions,
)
from google.protobuf.message import Message

from .context import Context
from .fake_types import parse_fqdn
from .globals import ENUM
from .text import Block, BlockLine

if TYPE_CHECKING:
    from .re_enum import ReEnumDescriptorProto


class ReEnumValueDescriptorProto:
    """
    Redescriptor for EnumValueDescriptorProto.
    
    Note: This is NOT a Node - enum values are not independently addressable
    in the descriptor graph.
    """
    
    def __init__(
        self,
        value: EnumValueDescriptorProto,
        parent: ReEnumDescriptorProto,
    ) -> None:
        assert isinstance(value, Message)
        self.this = value
        self.parent = parent

    @property
    def options(self) -> EnumValueOptions:
        return self.this.options

    @property
    def name(self) -> str:
        return self.this.name

    @property
    def number(self) -> int:
        return self.this.number

    def render(self, ctx: Context, depth: int = 0) -> Block:
        """Render enum value as .proto text."""
        assert isinstance(depth, int)

        # Get descriptor for EnumValueOptions
        evo_desc = ctx.evo_desc
        EvOClass = ctx.evo_cls

        # Look up the target enum descriptor
        # fqdn format: "enum:.package.EnumName"
        # FindEnumTypeByName expects: "package.EnumName" (no leading dot)
        prefix, ref = parse_fqdn(self.parent.fqdn)
        if prefix != ENUM:
            raise ValueError(f"Expected enum FQDN, got: {self.parent.fqdn}")
        # parse_fqdn returns ref with leading dot (e.g., '.google.protobuf.MyEnum')
        # Strip the dot for pool lookup
        full_type_name = ref.lstrip('.')
        enum_desc = ctx.pool.FindEnumTypeByName(full_type_name)
        enum_value_desc = enum_desc.values_by_name[self.name]

        # Create a dynamic EnumValueOptions instance and parse
        # serialized data
        evo_msg: Message = EvOClass()
        evo_msg.ParseFromString(
            enum_value_desc.GetOptions().SerializeToString()
        )

        out = Block()
        prolog = BlockLine(f'{self.name} = {self.number}', depth)

        # Emit features.X = Y overrides (editions only; no-op for proto2/proto3).
        from .syntax import render_features_block
        if self.this.HasField('options') and self.this.options.HasField('features'):
            feat_block = render_features_block(
                ctx, self.this.options.features, depth + 1, inline=True)
            out.extend(feat_block)

        # Render options using parent's helper method
        # (composite=True for inline format with commas)
        option_blocks = self.parent.render_options_from_message(
            ctx=ctx,
            opts_msg=evo_msg,
            options_descriptor=evo_desc,
            composite=True,
            depth=depth + 1,
        )
        for block in option_blocks:
            out.extend(block)

        # Format composite options using helper
        result = self.parent.format_composite_options(
            option_blocks=out,
            prolog=prolog,
            depth=depth,
        )

        result.postpend(';')
        return result
