# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import logging

from google.protobuf.descriptor import Descriptor, FieldDescriptor
from google.protobuf.message import Message


from .context import Context
from .simple_types import ReFieldDescriptor
from .text import (
    CODE,
    ORPHAN,
    Block,
)

logger = logging.getLogger(__name__)


# === Options decorator (generic) =============================================

class ReOptions:
    def __init__(self, options: Message) -> None:
        assert isinstance(options, Message)
        self.this = options

    def render(
        self,
        ctx: Context,
        composite: bool,
        lev: int,
    ) -> list[Block]:
        assert isinstance(lev, int)
        blocks: list[Block] = list()

        # Use ListFields() to get only fields that are actually set (non-default)
        # This works across proto2, proto3, and editions syntax
        # ListFields() returns: list of (FieldDescriptor, value) tuples
        # - For message fields: only included if HasField() would return true
        # - For singular primitives: only if set in proto2, or non-zero in proto3
        # - For repeated fields: only if contains at least one element

        for fd_desc, val in self.this.ListFields():
            # Skip extension fields - they're handled separately via FindAllExtensions()
            if fd_desc.is_extension:
                continue

            opt = ReFieldDescriptor(fd_desc)

            if fd_desc.label == FieldDescriptor.LABEL_REPEATED:
                # val is already the list/repeated container
                for v in val:
                    block, is_orp = opt.dump_option(ctx, v, lev)
                    if not block:  # nothing rendered
                        continue
                    if not composite:
                        block.prepend('option ')
                        block.postpend(';')
                    else:
                        block.postpend(',')
                    block.set_type(ORPHAN if is_orp else CODE)
                    blocks.append(block)

            else:
                # Singular field (val is already the value)
                block, is_orp = opt.dump_option(ctx, val, lev)
                if not block:  # nothing rendered
                    continue
                if not composite:
                    block.prepend('option ')
                    block.postpend(';')
                else:
                    block.postpend(',')
                block.set_type(ORPHAN if is_orp else CODE)
                blocks.append(block)

        return blocks
    

# === Extensions decorator (generic) =====================================

class ReExtensions:
    def __init__(self, options: Message) -> None:
        assert isinstance(options, Message)
        self.this = options

    def render(
        self,
        ctx: Context,
        options_desc: Descriptor,
        composite: bool,
        lev: int,
    ) -> tuple[list[Block], bool]:
        assert isinstance(lev, int)
        blocks: list[Block] = list()
        is_orphan = True

        # Sort by extension number for reproductibility
        for ext_desc in sorted(ctx.pool.FindAllExtensions(options_desc),
                               key=lambda d: d.number):
            opt = ReFieldDescriptor(ext_desc)
            val = self.this.Extensions[ext_desc]

            if ext_desc.label == FieldDescriptor.LABEL_REPEATED:
                for v in val:
                    block, is_orp = opt.dump_option(
                        ctx, v, lev, True)
                    is_orphan &= is_orp
                    if not composite:
                        block.prepend('option ')
                        block.postpend(';')
                    else:
                        block.postpend(',')
                    block.set_type(ORPHAN if is_orp else CODE)
                    blocks.append(block)
            else:
                if self.this.HasExtension(ext_desc):
                    block, is_orp = opt.dump_option(
                        ctx, val, lev, True)
                    is_orphan &= is_orp
                    if not composite:
                        block.prepend('option ')
                        block.postpend(';')
                    else:
                        block.postpend(',')
                    block.set_type(ORPHAN if is_orp else CODE)
                    blocks.append(block)
        return blocks, is_orphan
