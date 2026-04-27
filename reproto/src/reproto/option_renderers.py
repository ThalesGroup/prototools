# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import logging
from collections.abc import Callable, Sequence
from typing import Any

# Note: These types must be imported from the actual runtime backend (_upb)
# rather than from google.protobuf.internal.containers because:
# 1. Pattern matching requires the actual runtime types
# 2. The upb backend (used in protobuf 6.x) has different class names
# 3. Internal containers has RepeatedCompositeFieldContainer (different name)
from google.protobuf.descriptor import Descriptor, FieldDescriptor
from google.protobuf.descriptor_pb2 import (
    EnumOptions,
    FieldOptions,
    FileOptions,
    MessageOptions,
    MethodOptions,
    ServiceOptions,
)
from google.protobuf.message import Message


from .context import Context
from .simple_types import ReFieldDescriptor
from .text import (
    CODE,
    ORPHAN,
    Block,
)

logger = logging.getLogger(__name__)


### ############################################################################
### Non-Proto Decorators
### ############################################################################

## === FieldDescriptor decorator ===============================================


class ReEnumOptions:

    def __init__(self, options: EnumOptions) -> None:
        assert isinstance(options, Message)
        self.this = options

    @property
    def ListFields(self) -> Callable[[], Sequence[tuple[FieldDescriptor, Any]]]:
        return self.this.ListFields

    def render(
        self,
        ctx: Context,
        depth: int = 0,
        filter_out: list[str] = [],
    ) -> list[Block]:
        """Dump the options from an EnumOptions"""
        assert isinstance(depth, int)
        assert (isinstance(filter_out, list)
            and all(isinstance(option, str) for option in filter_out))
        texts: list[Block] = []

        # Let's dump explicitly set options (i.e. ListFields)
        for o, value in self.ListFields():
            option = ReFieldDescriptor(o)
            text, is_orphan =  option.dump_option(ctx, value, depth+1)
            if text:
                text[0].text = f'option {text[0].text}'
            if option.name in filter_out:
                for line in text:
                    line.text = '// ' + line.text
            texts.append(text)
        for text in texts:
            if text and not text[0].text.startswith('//'):
                text[-1].text += ';'
        return texts


# === MessageOptions decorator =================================================

class ReMessageOptions:

    def __init__(self, options: MessageOptions) -> None:
        assert isinstance(options, Message)
        self.this = options

    @property
    def ListFields(self) -> Callable[[], Sequence[tuple[FieldDescriptor, Any]]]:
        return self.this.ListFields

    def render(
        self,
        ctx: Context,
        depth: int = 0,
        filter_out: list[str] = [],
    ) -> list[Block]:
        """Dump the options from a MessageOptions"""
        assert isinstance(depth, int)
        assert (isinstance(filter_out, list)
            and all(isinstance(option, str) for option in filter_out))
        texts: list[Block] = []

        # Let's dump explicitly set options (i.e. ListFields)
        for o, value in self.ListFields():
            option = ReFieldDescriptor(o)
            text, is_orphan =  option.dump_option(ctx, value, depth+1)
            if text:
                text[0].text = f'option {text[0].text}'
            if option.name in filter_out:
                for line in text:
                    line.text = '// ' + line.text
            texts.append(text)
        for text in texts:
            if text:
                text[-1].text += ';'
        return texts


## === ServiceOptions decorator ================================================

class ReServiceOptions:

    def __init__(self, options: ServiceOptions) -> None:
        assert isinstance(options, Message)
        self.this = options

    @property
    def ListFields(self) -> Callable[[], Sequence[tuple[FieldDescriptor, Any]]]:
        return self.this.ListFields

    def render(
        self,
        ctx: Context,
        depth: int = 0,
        filter_out: list[str] = [],
    ) -> list[Block]:
        """Dump the options from a ServiceOptions"""
        assert isinstance(depth, int)
        assert (isinstance(filter_out, list)
            and all(isinstance(option, str) for option in filter_out))
        texts: list[Block] = []

        # Let's dump explicitly set options (i.e. ListFields)
        for o, value in self.ListFields():
            option = ReFieldDescriptor(o)
            text, is_orphan =  option.dump_option(ctx, value, depth+1)
            if text:
                text[0].text = f'option {text[0].text}'
            if option.name in filter_out:
                for line in text:
                    line.text = '// ' + line.text
            texts.append(text)
        for text in texts:
            if text:
                text[-1].text += ';'
        return texts


# === MethodOptions decorator ==================================================

class ReMethodOptions:

    def __init__(self, options: MethodOptions) -> None:
        assert isinstance(options, Message  )
        self.this = options

    @property
    def ListFields(self) -> Callable[[], Sequence[tuple[FieldDescriptor, Any]]]:
        return self.this.ListFields

    def render(
        self,
        ctx: Context,
        depth: int = 0,
        filter_out: list[str] = [],
    ) -> list[Block]:
        """Dump the options from a ServiceOptions"""
        assert isinstance(depth, int)
        assert (isinstance(filter_out, list)
            and all(isinstance(option, str) for option in filter_out))
        texts: list[Block] = []

        # Let's dump explicitly set options (i.e. ListFields)
        for o, value in self.ListFields():
            option = ReFieldDescriptor(o)
            text, is_orphan =  option.dump_option(ctx, value, depth+1)
            if text:
                text[0].text = f'option {text[0].text}'
            if option.name in filter_out:
                for line in text:
                    line.text = '// ' + line.text
            texts.append(text)
        for text in texts:
            if text:
                text[-1].text += ';'
        return texts


# === FieldOptions decorator ===================================================

class ReFieldOptions:

    def __init__(self, options: FieldOptions) -> None:
        assert isinstance(options, Message)
        self.this = options

    @property
    def ListFields(self) -> Callable[[], Sequence[tuple[FieldDescriptor, Any]]]:
        return self.this.ListFields

    def render(
        self,
        ctx: Context,
        depth: int = 0,
        filter_out: list[str] = [],
    ) -> list[Block]:
        """Dump the options from a MessageOptions"""
        assert isinstance(depth, int)
        assert (isinstance(filter_out, list)
            and all(isinstance(option, str) for option in filter_out))
        texts: list[Block] = []

        # Let's dump explicitly set options (i.e. ListFields)
        for o, value in self.ListFields():
            option = ReFieldDescriptor(o)
            text, is_orphan =  option.dump_option(ctx, value, depth+1)
            if option.name in filter_out:
                for line in text:
                    line.text = '// ' + line.text
            texts.append(text)
        is_last = True
        for text in reversed(texts):
            if is_last:
                if text and not text[0].text.startswith('//'):
                    is_last = False
            elif text and not text[0].text.startswith('//'):
                text[-1].text += ','
        return texts


# === FileOptions decorator ====================================================

class ReFileOptions:
    def __init__(self, options: FileOptions) -> None:
        assert isinstance(options, Message)
        self.this = options

    @property
    def ListFields(self) -> Callable[[], Sequence[tuple[FieldDescriptor, Any]]]:
        return self.this.ListFields

    def render(
        self,
        ctx: Context,
        lev: int = 0,
        filter_out: list[str] = [],
    ) -> list[Block]:
        """Dump the options from a FileOptions"""
        assert isinstance(lev, int)
        blocks: list[Block] = list()

        for o, v in self.ListFields():
            option = ReFieldDescriptor(o)
            block, is_orphan =  option.dump_option(ctx, v, lev)
            block[0].text = f'option {block[0].text}'
            if is_orphan:
                block.set_type(ORPHAN)
            block[-1].text += ';'
            blocks.append(block)
        return blocks
    

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
    

# === Extensions decorator (generic)=======================================
_ExtensionFieldDescriptor = Any  # type alias

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
