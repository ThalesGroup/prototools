# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""ReServiceDescriptorProto - Service descriptor redescriptor."""

from __future__ import annotations

from typing import Any, cast

from google.protobuf.descriptor_pb2 import (
    MethodDescriptorProto,
    ServiceDescriptorProto,
)
from google.protobuf.internal.containers import RepeatedCompositeFieldContainer

from .base import NodeBase
from .context import Context, Fqdn
from .fake_types import Ref
from .globals import SERVICE
from .text import Block, BlockLine


class ReServiceDescriptorProto(NodeBase[ServiceDescriptorProto]):
    """Redescriptor for ServiceDescriptorProto."""

    @property
    def method(self) -> RepeatedCompositeFieldContainer:
        return self.this.method

    @property
    def parent(self):
        from .re_descriptor import ReDescriptorProto
        from .re_file import ReFileDescriptorProto
        
        assert isinstance(self._parent, (ReFileDescriptorProto, ReDescriptorProto))
        return self._parent

    @parent.setter
    def parent(self, value: NodeBase[Any]) -> None:
        self._parent = value

    @classmethod
    def fqdn_from_ref(cls, ref: Ref) -> Fqdn:
        return Fqdn(f'{SERVICE}:{str(ref)}')
    
    def _initialize_from_message(
        self,
        ctx: Context,
        message: ServiceDescriptorProto,
        **kwargs: Any,
    ) -> None:
        """Initialize service-specific attributes and build dependency graph."""
        # Lazy import
        from .re_method import ReMethodDescriptorProto
        
        # Service methods
        for m in self.method:
            method_proto = cast(MethodDescriptorProto, m)
            method = ReMethodDescriptorProto(ctx, method_proto, parent=self)
            self.targets.add(method)
    
    def render(self, ctx: Context, depth: int = 0, group: bool = False) -> tuple[Block, Block]:
        """Reconstruct service as .proto."""
        from .re_method import ReMethodDescriptorProto

        assert isinstance(depth, int)
        out = Block()
        inputs = Block()

        # --- Service options --------------------------------------------------
        option_blocks = self.render_options(
            ctx=ctx,
            options_descriptor=ctx.svo_desc,
            options_class=ctx.svo_cls,
            composite=False,
            depth=depth
        )

        for block in option_blocks:
            out.extend(block)
        out.append_div_maybe(depth)

        # --- Service methods --------------------------------------------------
        for m in self.method:
            method_proto = cast(MethodDescriptorProto, m)
            method = ReMethodDescriptorProto(ctx, method_proto, parent=self)
            if not method.is_visible():
                continue
            lines, inp = method.render(ctx, depth+1)
            lines.append_div_maybe(depth)
            out.extend(lines)
            inputs.extend(inp)
        out.append_div_maybe(depth)

        # --- Service outro ----------------------------------------------------
        while len(out) > 1 and not out[-1].text:
            out.pop()
        out.append(BlockLine('}', depth))
        out.append_div_maybe(depth)

        # --- Service intro ----------------------------------------------------
        if not group:
            out.insert(0, BlockLine(f'service {self.name} {{', depth))

        return out, inputs
    
