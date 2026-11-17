# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""ReMethodDescriptorProto - Method descriptor redescriptor."""

from __future__ import annotations

from typing import Any

from google.protobuf.descriptor_pb2 import MethodDescriptorProto

from .base import NodeBase
from .context import Context, Fqdn, descriptor_rerender_muted
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
    def parent(self, value: NodeBase[Any]) -> None:
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
        **kwargs: Any,
    ) -> None:
        """Initialize method-specific attributes."""
        # Methods have no child descriptors
        pass
    
    @descriptor_rerender_muted
    def render(self, ctx: Context, depth: int = 0) -> tuple[Block, Block]:
        """Reconstruct a MethodDescriptorProto as .proto."""
        from .names import Mode
        from .utils import scope_name, type_ref

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
        # Looked up among every symbol of the service (spec 0434 S3): a
        # method named as its type shadows it.
        scope = scope_name(self.parent)
        input = type_ref(ctx, self.input_type, scope, Mode.ALL)
        client_streaming = 'stream ' if self.client_streaming else ''
        output = type_ref(ctx, self.output_type, scope, Mode.ALL)
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
                out.insert(0, BlockLine(string1 + string2, depth))

        # --- Binary output side-channel (spec 0076) ---------------------------
        if ctx.out_desc is not None:
            from google.protobuf.descriptor_pb2 import MethodDescriptorProto as _MDP
            method_out = _MDP()
            method_out.CopyFrom(self.this)
            # input_type / output_type: canonize via variant namespace
            # rules (spec 0086), unless --keep-descriptor-path suppresses
            # namespace rewriting entirely (spec 0159)
            from .mappings import output_symbol
            if method_out.input_type:
                method_out.input_type = output_symbol(ctx, method_out.input_type)
            if method_out.output_type:
                method_out.output_type = output_symbol(ctx, method_out.output_type)
            if method_out.HasField('options') and ctx.target_syntax != "editions":
                method_out.options.ClearField('features')
            ctx.out_desc.out = method_out

        return out, inputs
