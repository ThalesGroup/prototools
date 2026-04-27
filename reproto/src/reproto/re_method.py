# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""ReMethodDescriptorProto - Method descriptor redescriptor."""

from __future__ import annotations

from google.protobuf.descriptor_pb2 import MethodDescriptorProto

from .base import NodeBase
from .context import Context, Fqdn
from .fake_types import Ref
from .globals import METHOD
from .text import MAX_LINE_LENGTH, TAB_SIZE, Block, BlockLine


class ReMethodDescriptorProto(NodeBase[MethodDescriptorProto]):
    """Redescriptor for MethodDescriptorProto."""

    @property
    def client_streaming(self) -> bool:
        return self.this.client_streaming

    @property
    def input_type(self) -> str:
        return self.this.input_type

    @property
    def output_type(self) -> str:
        return self.this.output_type

    @property
    def parent(self):
        from .re_service import ReServiceDescriptorProto
        assert isinstance(self._parent, ReServiceDescriptorProto)
        return self._parent

    @parent.setter
    def parent(self, value) -> None:
        self._parent = value

    @property
    def server_streaming(self) -> bool:
        return self.this.server_streaming

    @classmethod
    def fqdn_from_ref(cls, ref: Ref) -> Fqdn:
        return Fqdn(f'{METHOD}:{str(ref)}')
    
    def _initialize_from_message(
        self,
        ctx: Context,
        message: MethodDescriptorProto,
        **kwargs
    ) -> None:
        """Initialize method-specific attributes."""
        # Methods have no child descriptors
        pass
    
    def render(self, ctx: Context, depth: int = 0) -> tuple[Block, Block]:
        """Reconstruct a MethodDescriptorProto as .proto."""
        from .utils import shorten_type_name

        assert isinstance(depth, int)
        out = Block()
        inputs = Block()

        # --- Method options ---------------------------------------------------
        option_blocks = self.render_options(
            ctx=ctx,
            options_descriptor=ctx.meo_desc,
            options_class=ctx.meo_cls,
            composite=False,
            depth=depth
        )

        # --- Method signature -------------------------------------------------
        input = shorten_type_name(ctx, self.parent, self.input_type)
        client_streaming = 'stream ' if self.client_streaming else ''
        output = shorten_type_name(ctx, self.parent, self.output_type)
        server_streaming = 'stream ' if self.server_streaming else ''

        # Use semicolon syntax if no options, brace syntax if options exist
        if not option_blocks:
            # No options - use semicolon syntax
            string1 = f'rpc {self.name}({client_streaming}{input}) '
            string2 = f'returns ({server_streaming}{output});'
            if depth*TAB_SIZE + len(string1) + len(string2) > MAX_LINE_LENGTH:
                out.insert(0, BlockLine(string2, depth+2))
                out.insert(0, BlockLine(string1, depth))
            else:
                out.insert(0, BlockLine(string1 + string2, depth))
        else:
            # Has options - use brace syntax
            for block in option_blocks:
                out.extend(block)
            out.append_div_maybe(depth)

            # Method outro
            while len(out) > 1 and not out[-1].text:
                out.pop()
            out.append(BlockLine('}', depth))
            out.append_div_maybe(depth)

            # Method intro
            string1 = f'rpc {self.name}({client_streaming}{input}) '
            string2 = f'returns ({server_streaming}{output}) {{'
            if depth*TAB_SIZE + len(string1) + 1 + len(string2) > MAX_LINE_LENGTH:
                out.insert(0, BlockLine(string2, depth+2))
                out.insert(0, BlockLine(string1, depth))
            else:
                out.insert(0, BlockLine(string1 + ' ' + string2, depth))

        return out, inputs
