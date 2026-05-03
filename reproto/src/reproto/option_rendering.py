# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Protocols and functions for rendering protobuf options into .proto text."""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any, Protocol, runtime_checkable

from google.protobuf.descriptor import Descriptor, FieldDescriptor
from google.protobuf.internal.extension_dict import _ExtensionDict
from google.protobuf.message import Message

from .context import Context
from .globals import ENUM, MESSAGE, METHOD, SERVICE
from .fake_types import parse_fqdn
from .text import CODE, ORPHAN, Block, BlockLine

if TYPE_CHECKING:
    from .base import NodeBase

logger = logging.getLogger(__name__)


# Protocol for messages that have common descriptor attributes
@runtime_checkable
class DescriptorMessage(Protocol):
    """Protocol for protobuf descriptor messages with common attributes."""
    name: str

    def HasField(self, field_name: str) -> bool: ...

    @property
    def options(self) -> Message: ...


@runtime_checkable
class OptionsMessage(Protocol):
    """
    Protocol for all *Options message types (FileOptions, MessageOptions, etc.).

    All protobuf options classes inherit from Message and share this interface,
    allowing generic rendering of both built-in options and extension options.
    """

    @property
    def DESCRIPTOR(self) -> Descriptor:
        """The message type descriptor for this options message."""
        ...

    def ListFields(self) -> list[tuple[FieldDescriptor, Any]]:
        """
        Return a list of (FieldDescriptor, value) tuples for all explicitly set fields.

        Only includes fields that have been set (non-default values in proto2/editions,
        or explicitly set in proto3). Extension fields are included.
        """
        ...

    def HasExtension(self, extension: FieldDescriptor) -> bool:
        """Check if an extension field is set on this options message."""
        ...

    @property
    def Extensions(self) -> _ExtensionDict[Any]:
        """
        Access extension fields by descriptor.

        Usage: options.Extensions[extension_descriptor]
        Returns the value of the extension field.
        """
        ...

    def ParseFromString(self, data: bytes) -> None:
        """Parse serialized protobuf data into this message."""
        ...

    def SerializeToString(self) -> bytes:
        """Serialize this options message to a binary string."""
        ...


def render_options_from_message(
    ctx: Context,
    opts_msg: Message,
    options_descriptor: Descriptor,
    composite: bool = False,
    depth: int = 0,
    exclude: set[str] | None = None,
) -> list[Block]:
    """Render options from an already-parsed options message.

    Args:
        ctx: Build context
        opts_msg: Already-parsed options message
        options_descriptor: Descriptor for the options type
        composite: True for inline with commas, False for standalone with 'option' keyword
        depth: Indentation depth
        exclude: Set of built-in field names to skip (e.g. {"packed"}).

    Returns:
        List of text Blocks containing rendered options
    """
    from .re_simple import ReFieldDescriptor

    _exclude: set[str] = exclude if exclude is not None else set()
    blocks: list[Block] = []

    for fd_desc, val in opts_msg.ListFields():
        if fd_desc.is_extension:
            continue
        if fd_desc.name in _exclude:
            continue

        opt = ReFieldDescriptor(fd_desc)

        if fd_desc.label == FieldDescriptor.LABEL_REPEATED:
            for v in val:
                block, is_orp = opt.dump_option(ctx, v, depth)
                if not block:
                    continue
                if not composite:
                    block.prepend('option ')
                    block.postpend(';')
                else:
                    block.postpend(',')
                block.set_type(ORPHAN if is_orp else CODE)
                blocks.append(block)
        else:
            block, is_orp = opt.dump_option(ctx, val, depth)
            if not block:
                continue
            if not composite:
                block.prepend('option ')
                block.postpend(';')
            else:
                block.postpend(',')
            block.set_type(ORPHAN if is_orp else CODE)
            blocks.append(block)

    for ext_desc in sorted(
        ctx.pool.FindAllExtensions(options_descriptor),
        key=lambda d: d.number
    ):
        opt = ReFieldDescriptor(ext_desc)
        val = opts_msg.Extensions[ext_desc]

        if ext_desc.label == FieldDescriptor.LABEL_REPEATED:
            for v in val:
                block, is_orp = opt.dump_option(ctx, v, depth, True)
                if not composite:
                    block.prepend('option ')
                    block.postpend(';')
                else:
                    block.postpend(',')
                block.set_type(ORPHAN if is_orp else CODE)
                blocks.append(block)
        else:
            if opts_msg.HasExtension(ext_desc):
                block, is_orp = opt.dump_option(ctx, val, depth, True)
                if not composite:
                    block.prepend('option ')
                    block.postpend(';')
                else:
                    block.postpend(',')
                block.set_type(ORPHAN if is_orp else CODE)
                blocks.append(block)

    return blocks


def format_composite_options(
    node: 'NodeBase[Any]',
    option_blocks: Block,
    prolog: BlockLine,
    depth: int,
) -> Block:
    """Format options in composite/inline style with brackets."""
    is_orphan = True
    for i in range(len(option_blocks) - 1, -1, -1):
        if option_blocks[i].type == CODE:
            is_orphan = False
            option_blocks[i].text = option_blocks[i].text[:-1]
            break

    if len(option_blocks) == 0:
        result = Block([prolog])
    elif is_orphan:
        option_blocks.insert(0, BlockLine('[', depth, type=ORPHAN))
        option_blocks.insert(0, prolog)
        option_blocks.append(BlockLine(']', depth, type=ORPHAN))
        option_blocks.append(BlockLine('', depth))
        result = option_blocks
    elif len(option_blocks) == 1:
        prolog.postpend(f' [{option_blocks[0].text}]')
        result = Block([prolog])
    else:
        prolog.postpend(' [')
        option_blocks.insert(0, prolog)
        option_blocks.append(BlockLine(']', depth))
        result = option_blocks

    return result


def render_options(
    node: 'NodeBase[Any]',
    ctx: Context,
    options_descriptor: Descriptor,
    options_class: type[Message],
    composite: bool = False,
    depth: int = 0,
    exclude: set[str] | None = None,
) -> list[Block]:
    """Generic options renderer for all descriptor types."""
    blocks: list[Block] = []

    if not node.fqdn:
        logger.warning("render_options called on object without fqdn attribute")
        return blocks

    if ':' not in node.fqdn:
        logger.warning(f"Invalid FQDN format: {node.fqdn}")
        return blocks

    prefix, ref = parse_fqdn(node.fqdn)
    full_type_name = ref.lstrip('.')

    try:
        if prefix == ENUM:
            desc = ctx.pool.FindEnumTypeByName(full_type_name)
        elif prefix == MESSAGE:
            desc = ctx.pool.FindMessageTypeByName(full_type_name)
        elif prefix == SERVICE:
            desc = ctx.pool.FindServiceByName(full_type_name)
        elif prefix == METHOD:
            desc = ctx.pool.FindMethodByName(full_type_name)
        else:
            logger.warning(
                f"Unsupported descriptor prefix '{prefix}' "
                f"for pool lookup in {node.fqdn}"
            )
            return blocks

        opts_msg = options_class()
        opts_msg.ParseFromString(desc.GetOptions().SerializeToString())

        blocks = render_options_from_message(
            ctx=ctx,
            opts_msg=opts_msg,
            options_descriptor=options_descriptor,
            composite=composite,
            depth=depth + 1,
            exclude=exclude,
        )

    except (KeyError, ValueError, TypeError, AttributeError) as e:
        logger.warning(
            f"Could not find descriptor for {full_type_name} in pool: {e}"
        )

    return blocks
