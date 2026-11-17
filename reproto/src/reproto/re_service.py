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
from .context import Context, Fqdn, descriptor_rerender_muted
from .fake_types import Ref
from .globals import SERVICE
from .source_info import closing_line
from .text import Block, BlockLine


class ReServiceDescriptorProto(NodeBase[ServiceDescriptorProto]):
    """Redescriptor for ServiceDescriptorProto."""

    # Methods stripped from the FDP before pool_db.Add() because their
    # input_type or output_type was not resolvable (spec 0087).
    # Elements are StrippedMethod instances at runtime.
    stripped_methods: list[Any]

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
    
    @descriptor_rerender_muted
    def render(
        self,
        ctx: Context,
        depth: int = 0,
        group: bool = False,
        sci_path: list[int] | None = None,
    ) -> tuple[Block, Block]:
        """Reconstruct service as .proto.

        Args:
            ctx: Rendering context.
            depth: Indentation depth.
            group: When True, omit the enclosing `service Name {` line.
            sci_path: This service's own SourceCodeInfo path (spec 0141), as
                computed by the caller; used as the prefix for its own
                methods' paths. None if source-info synthesis is off.
        """
        from .re_method import ReMethodDescriptorProto

        assert isinstance(depth, int)
        # The children's text rendered with no slot, once; the descriptor
        # built after it, from muted re-renders (spec 0429 S11).
        outer_slot, ctx.out_desc = ctx.out_desc, None
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

        # --- Orphan methods (stripped unresolvable types, spec 0087) ----------
        from .text import ORPHAN
        for sm in self.stripped_methods:
            out.append(BlockLine(
                f'rpc {sm.name} ({sm.input_type}) returns ({sm.output_type});',
                depth + 1, ORPHAN,
            ))
        if self.stripped_methods:
            out.append_div_maybe(depth)

        # --- Service methods --------------------------------------------------
        # sci_idx (spec 0141) matches the binary side-channel's own
        # is_visible() filter below, so indices align with svc_out.method.
        sci_method_idx = 0
        for m in self.method:
            method_proto = cast(MethodDescriptorProto, m)
            method = ReMethodDescriptorProto(ctx, method_proto, parent=self)
            if not method.is_visible():
                continue
            method_sci_path: list[int] | None = None
            if ctx.out_sci is not None and sci_path is not None:
                method_sci_path = sci_path + [2, sci_method_idx]
            lines, inp = method.render(ctx, depth+1)
            if method_sci_path is not None:
                assert ctx.out_sci is not None
                ctx.out_sci.pending.append(
                    (method_sci_path, lines[0], closing_line(lines))
                )
            sci_method_idx += 1
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

        # --- Binary output side-channel (spec 0076) ---------------------------
        if outer_slot is not None:
            from google.protobuf.descriptor_pb2 import ServiceDescriptorProto as _SDP
            from .context import DescOut
            from .re_method import ReMethodDescriptorProto
            svc_out = _SDP()
            svc_out.CopyFrom(self.this)
            if svc_out.HasField('options') and ctx.target_syntax != "editions":
                svc_out.options.ClearField('features')
            svc_out.ClearField('method')
            for m in self.method:
                method_proto = cast(MethodDescriptorProto, m)
                method = ReMethodDescriptorProto(ctx, method_proto, parent=self)
                if not method.is_visible():
                    continue
                slot = DescOut()
                ctx.out_desc = slot
                method.render(ctx, depth + 1)
                ctx.out_desc = None
                if slot.out is not None:
                    assert isinstance(slot.out, MethodDescriptorProto)
                    svc_out.method.append(slot.out)
            ctx.out_desc = outer_slot
            ctx.out_desc.out = svc_out

        return out, inputs
    
