# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""ReEnumDescriptorProto - Enum descriptor redescriptor."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from google.protobuf.descriptor_pb2 import (
    EnumDescriptorProto,
    EnumValueDescriptorProto,
)
from google.protobuf.internal.containers import (
    RepeatedCompositeFieldContainer,
    RepeatedScalarFieldContainer,
)

from .base import NodeBase
from .context import Context, Fqdn
from .fake_types import Ref
from .globals import ENUM
from .text import Block, BlockLine

# Import types only for type checking to avoid circular imports
if TYPE_CHECKING:
    from .re_descriptor import ReDescriptorProto
    from .re_field import ReFieldDescriptorProto
    from .re_file import ReFileDescriptorProto

from .scalar import Scalar

# === ReservedName helper =====================================================

class ReReservedName:
    def __init__(self, name: str) -> None:
        assert isinstance(name, str)
        self.this = name

    @property
    def name(self) -> str:
        return self.this


    def render(self, ctx: Context, depth: int = 0) -> Block:
        """Render a reserved name statement."""
        assert isinstance(depth, int)
        out = Block()
        if ctx.target_syntax == "editions":
            out.append(BlockLine(f'reserved {self.name};', depth))
        else:
            out.append(BlockLine(f'reserved {Scalar(self.name)};', depth))
        return out
    
# === ReservedRange helper ====================================================

EnumReservedRange = EnumDescriptorProto.EnumReservedRange
class ReReservedRange:
    def __init__(self, range: EnumReservedRange) -> None:
        assert isinstance(range, EnumReservedRange)
        self.this = range

    @property
    def end(self) -> int:
        return self.this.end  # inclusive

    @property
    def start(self) -> int:
        return self.this.start  # inclusive


    def render(self, depth: int = 0) -> Block:
        """Render a ReservedRange in proto2 syntax"""
        assert isinstance(depth, int)
        out = Block()

        if self.start == self.end:
            out.append(BlockLine(
                f'reserved {self.start};', depth))
        else:
            out.append(BlockLine(
                f'reserved {self.start} '
                f'to {self.end};', depth))
        return out
    

# === ReEnumDescriptorProto decorator =========================================

class ReEnumDescriptorProto(NodeBase[EnumDescriptorProto]):
    """Redescriptor for EnumDescriptorProto."""

    @property
    def parent(
        self
    ) -> ReFieldDescriptorProto | ReFileDescriptorProto | ReDescriptorProto:
        from .re_descriptor import ReDescriptorProto
        from .re_field import ReFieldDescriptorProto
        from .re_file import ReFileDescriptorProto

        assert isinstance(
            self._parent,
            (ReFieldDescriptorProto, ReFileDescriptorProto, ReDescriptorProto)
        )
        return self._parent

    @parent.setter
    def parent(
        self,
        value: NodeBase[Any] | None
    ) -> None:
        from .re_descriptor import ReDescriptorProto
        from .re_field import ReFieldDescriptorProto
        from .re_file import ReFileDescriptorProto

        assert value is None or isinstance(
            value,
            (ReFieldDescriptorProto, ReFileDescriptorProto, ReDescriptorProto)
        ), f"Expected ReFieldDescriptorProto | ReFileDescriptorProto | ReDescriptorProto, got {type(value)}"
        self._parent = value

    @property
    def reserved_name(self) -> RepeatedScalarFieldContainer[str]:
        return self.this.reserved_name

    @property
    def reserved_range(
        self) -> RepeatedCompositeFieldContainer[EnumReservedRange]:
        return self.this.reserved_range

    @property
    def value(self) -> RepeatedCompositeFieldContainer:
        return self.this.value

    @classmethod
    def fqdn_from_ref(cls, ref: Ref) -> Fqdn:
        return Fqdn(f'{ENUM}:{str(ref)}')

    # --- Initialization ------------------------------------------------------

    def _initialize_from_message(
        self,
        ctx: Context,
        message: EnumDescriptorProto,
        **kwargs: Any,
    ) -> None:
        """Initialize enum-specific attributes.

        Args:
            ctx: Build context
            message: The EnumDescriptorProto to initialize from
            **kwargs: Currently unused. The 'parent' argument is already
                    extracted and set by the base class (base.py:191-222).
                    Kept for future extensibility.
        """

    # --- Rendering -----------------------------------------------------------

    def render(self, ctx: Context, depth: int = 0) -> Block:
        """Reconstruct enum as .proto."""
        from .re_enum_value import ReEnumValueDescriptorProto

        out = Block()

        # --- Enum options ----------------------------------------------------
        # Emit features.X = Y overrides first (editions only; no-op otherwise).
        from .syntax import render_features_block
        if self.this.HasField('options') and self.this.options.HasField('features'):
            feat_block = render_features_block(
                ctx, self.this.options.features, depth + 1, inline=False)
            for line in feat_block:
                out.append(line)
        option_blocks = self.render_options(
            ctx=ctx,
            options_descriptor=ctx.eno_desc,
            options_class=ctx.eno_cls,
            depth=depth,
        )
        for block in option_blocks:
            out.extend(block)
        out.append_div_maybe(depth)

        # --- Enum values -----------------------------------------------------
        for v in self.value:
            assert isinstance(v, EnumValueDescriptorProto)
            value = ReEnumValueDescriptorProto(v, self)
            out.extend(value.render(ctx, depth+1))

        # --- Enum reserved ranges --------------------------------------------
        for r in self.reserved_range:
            assert isinstance(r, EnumReservedRange)
            range = ReReservedRange(r)
            out.extend(range.render(depth+1))
        out.append_div_maybe(depth)

        # --- Enum reserved names =--------------------------------------------
        for n in self.reserved_name:
            assert isinstance(n, str)
            name = ReReservedName(n)
            out.extend(name.render(ctx, depth+1))
        out.append_div_maybe(depth)

        # --- Enum outro ------------------------------------------------------
        out.append(BlockLine('}', depth))
        out.append_div_maybe(depth)

        # --- Enum intro ------------------------------------------------------
        out.insert(0, BlockLine(f'enum {self.name} {{', depth))

        return out
