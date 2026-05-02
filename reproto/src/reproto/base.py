# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Base classes for redescriptor pattern - unified NodeBase implementation."""

from __future__ import annotations

import logging
from abc import ABC, abstractmethod
from collections.abc import Callable
from typing import Any, Generic, Protocol, Self, TypeVar, runtime_checkable

from google.protobuf.descriptor import Descriptor, FieldDescriptor
from google.protobuf.internal.extension_dict import _ExtensionDict
from google.protobuf.message import Message

from .context import Context, Fqdn
from .fake_types import Prefix, Ref, parse_fqdn
from .globals import ENUM, MESSAGE, METHOD, SERVICE
from .text import CODE, ORPHAN, Block, BlockLine

logger = logging.getLogger(__name__)

MessageT = TypeVar('MessageT', bound=Message)


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


class NodeBase(Generic[MessageT], ABC):
    """
    Unified base class for all redescriptor nodes in the descriptor graph.

    Combines registry pattern, protobuf message delegation, and graph structure
    in a single clean hierarchy. Eliminates the complexity of multiple mixins.

    Benefits over the previous mixin approach:
    - All attributes declared at class level (no type: ignore needed in most cases)
    - Type checkers understand the full structure
    - No confusing multiple inheritance
    - Simpler mental model
    """

    # === Type declarations for all attributes ===
    # Declaring everything here allows type checkers to understand the structure

    ctx: Context
    fqdn: Fqdn
    prefix: Prefix

    # Graph attributes
    is_pruned: bool
    is_reachable: bool
    is_summoned: bool
    contains: set['NodeBase[Any]']
    targets: set['NodeBase[Any]']
    seeder: 'NodeBase[Any] | None'

    # Protobuf message
    _this: MessageT | None
    _parent: 'NodeBase[Any] | None'

    # === Construction with Registry Pattern ===

    def __new__(
        cls,
        ctx: Context,
        message_or_ref: MessageT | Ref,
        **kwargs: Any
    ) -> Self:
        """
        Registry pattern implementation.

        Returns existing instance if already created, otherwise creates new.
        All attributes are properly typed, eliminating type: ignore comments.
        """
        # Determine FQDN from input
        parent = kwargs.get('parent')

        if isinstance(message_or_ref, str):
            # Ref case
            if parent is None:
                fqdn = cls.fqdn_from_ref(message_or_ref)
            else:
                # Some types need parent prefix for FQDN construction
                ref = Ref(f'{parent.prefix}.{message_or_ref}')
                fqdn = cls.fqdn_from_ref(ref)
        elif isinstance(message_or_ref, Message):
            # Message case - extract name for FQDN
            msg_name = getattr(message_or_ref, 'name')
            if parent is None:
                # File descriptors use name directly
                ref = Ref(msg_name)
            else:
                ref = Ref(f'{parent.prefix}.{msg_name}')
            fqdn = cls.fqdn_from_ref(ref)
        else:
            raise AssertionError(f"Invalid type for message_or_ref: {type(message_or_ref)}")

        # Check if instance already exists in registry
        instance = ctx.find_node(fqdn)
        if instance is None:
            # Create a new instance
            instance = super().__new__(cls)

            # Initialize all attributes (NO type: ignore needed!)
            instance.ctx = ctx
            instance.fqdn = fqdn
            instance.is_pruned = False
            instance.is_reachable = False
            instance.is_summoned = False
            instance.contains = set()
            instance.targets = set()
            instance._this = None
            instance._parent = None
            instance.seeder = None

            # Register in context
            ctx.new_nodes[fqdn] = instance  # type: ignore[assignment]
            cls._register_in_context(ctx, fqdn, instance)
        else:
            # Verify type matches
            if not isinstance(instance, cls):
                raise AssertionError(
                    f"Expected {cls.__name__}, got {type(instance).__name__}"
                )

        return instance

    def __init__(
        self,
        ctx: Context,
        message_or_ref: MessageT | Ref,
        **kwargs: Any
    ) -> None:
        """
        Initialize node from message or create stub from ref.

        If message_or_ref is a Ref (str), creates a stub instance.
        If message_or_ref is a Message, fully initializes the instance.
        """
        # Already initialized? (registry returned existing instance)
        if self._this is not None:
            return

        parent = kwargs.get('parent')

        # Case 1: Creating from reference only (stub instance)
        if isinstance(message_or_ref, str):
            if parent is not None:
                raise AssertionError("Parent must be None when creating from ref")
            return

        # Case 2: First-time initialization with actual message
        if not isinstance(message_or_ref, Message):
            raise AssertionError(
                f"Expected Message, got {type(message_or_ref)}"
            )

        # Validate parent requirement
        if self._requires_parent():
            if parent is None:
                raise AssertionError(
                    f"Parent required for {type(self).__name__}"
                )

        # Store message and parent
        self._this = message_or_ref
        self._parent = parent

        # Set prefix if we have a parent
        if parent is not None:
            msg_name = getattr(message_or_ref, 'name')
            self.prefix = Prefix(f'{parent.prefix}.{msg_name}')

        # Call subclass-specific initialization
        self._initialize_from_message(ctx, message_or_ref, **kwargs)

    # === Properties (delegation to underlying protobuf message) ===

    @property
    def this(self) -> MessageT:
        """
        The underlying protobuf message.

        Clean! Type system knows _this is MessageT | None,
        and assertion narrows it to MessageT.
        """
        if self._this is None:
            raise AssertionError(f"Node {self.fqdn} not initialized with message")
        if not isinstance(self._this, Message):
            raise AssertionError(f"Node {self.fqdn} _this is not a Message")
        return self._this

    @this.setter
    def this(self, value: MessageT) -> None:
        self._this = value

    @property
    def name(self) -> str:
        """Delegate to proto message name."""
        # All descriptor messages have 'name', but it's not in Message base class
        return self.this.name  # type: ignore[attr-defined]

    @property
    def options(self) -> OptionsMessage:
        """Delegate to proto message options."""
        # All descriptor messages have options
        return self.this.options  # type: ignore[return-value]

    @property
    def HasField(self) -> Callable[..., bool]:
        """Delegate to proto message HasField method."""
        return self.this.HasField

    @property
    def parent(self) -> 'NodeBase[Any] | None':
        """Parent node in the descriptor tree."""
        return self._parent

    @parent.setter
    def parent(self, value: 'NodeBase[Any]') -> None:
        """Set parent node."""
        self._parent = value

    # === Utility Methods ===

    def is_present(self) -> bool:
        """Check if this node has an associated protobuf message."""
        return self._this is not None

    def is_visible(self) -> bool:
        """Check if this node is visible (present and reachable)."""
        return self.is_reachable

    @classmethod
    def from_ref(cls, ctx: Context, ref: Ref) -> Self:
        """
        Create or retrieve an instance from a reference.

        Clean! No cast needed.
        """
        fqdn = cls.fqdn_from_ref(ref)
        instance = ctx.find_node(fqdn)

        if instance is not None:
            if not isinstance(instance, cls):
                raise AssertionError(
                    f"Expected {cls.__name__}, got {type(instance).__name__}"
                )
            return instance

        # Instance does not exist, create stub
        return cls(ctx, ref)

    # === Options Rendering ===
    # (Moved from ProtoMessageDelegateMixin - implementation unchanged)

    def render_options_from_message(
        self,
        ctx: Context,
        opts_msg: Message,
        options_descriptor: Descriptor,
        composite: bool = False,
        depth: int = 0,
        exclude: set[str] | None = None,
    ) -> list[Block]:
        """
        Render options from an already-parsed options message.

        This helper method contains the core rendering logic used by
        both render_options() and by classes like ReEnumValueDescriptorProto
        that need to render options from a pre-parsed message.

        Args:
            ctx: Build context
            opts_msg: Already-parsed options message
            options_descriptor: Descriptor for the options type
            composite: True for inline with commas, False for
                      standalone with 'option' keyword
            depth: Indentation depth
            exclude: Set of built-in field names to skip (e.g. {"packed"}).
                     The caller is responsible for rendering excluded fields.

        Returns:
            List of text Blocks containing rendered options
        """
        from .re_simple import ReFieldDescriptor

        _exclude: set[str] = exclude if exclude is not None else set()
        blocks: list[Block] = []

        # === Render built-in options ===
        # Use ListFields() to get only fields that are set
        for fd_desc, val in opts_msg.ListFields():
            # Skip extension fields - handled separately
            if fd_desc.is_extension:
                continue
            # Skip fields explicitly excluded by the caller
            if fd_desc.name in _exclude:
                continue

            opt = ReFieldDescriptor(fd_desc)

            if fd_desc.label == FieldDescriptor.LABEL_REPEATED:
                # val is already the list/repeated container
                for v in val:
                    block, is_orp = opt.dump_option(ctx, v, depth)
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
                # Singular field
                block, is_orp = opt.dump_option(ctx, val, depth)
                if not block:  # nothing rendered
                    continue
                if not composite:
                    block.prepend('option ')
                    block.postpend(';')
                else:
                    block.postpend(',')
                block.set_type(ORPHAN if is_orp else CODE)
                blocks.append(block)

        # === Render extension options ===
        # Sort by extension number for reproducibility
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
        self,
        option_blocks: Block,
        prolog: BlockLine,
        depth: int,
    ) -> Block:
        """
        Format options in composite/inline style with brackets.

        This helper formats option blocks that were rendered with
        composite=True, wrapping them in brackets and choosing the
        appropriate format based on option count and orphan status.

        The formatted output does NOT include a trailing semicolon
        - the caller should add it as appropriate for the context.

        Args:
            option_blocks: Block containing rendered option lines
            prolog: The BlockLine with the declaration (e.g.,
                   "VALUE = 0" or "string field = 1")
            depth: Indentation depth

        Returns:
            Formatted Block ready for semicolon addition

        Format patterns:
            - No options: prolog only
            - All orphaned: prolog + orphaned brackets + empty line
            - One option: prolog with inline brackets
            - Multiple options: prolog + multi-line brackets
        """
        # Remove trailing comma from last CODE line
        is_orphan = True
        for i in range(len(option_blocks) - 1, -1, -1):
            if option_blocks[i].type == CODE:
                # Found at least one non-orphan option
                is_orphan = False
                # Remove trailing comma from last option
                option_blocks[i].text = option_blocks[i].text[:-1]
                break

        # Format based on option count and orphan status
        if len(option_blocks) == 0:
            # No options at all
            result = Block([prolog])
        elif is_orphan:
            # All options orphaned - brackets marked as ORPHAN
            # (lines remain for context)
            option_blocks.insert(0, BlockLine('[', depth, type=ORPHAN))
            option_blocks.insert(0, prolog)
            option_blocks.append(BlockLine(']', depth, type=ORPHAN))
            option_blocks.append(BlockLine('', depth))
            result = option_blocks
        elif len(option_blocks) == 1:
            # Single option - use inline format
            prolog.postpend(f' [{option_blocks[0].text}]')
            result = Block([prolog])
        else:
            # Multiple options - use multi-line format
            prolog.postpend(' [')
            option_blocks.insert(0, prolog)
            option_blocks.append(BlockLine(']', depth))
            result = option_blocks

        return result

    def render_options(
        self,
        ctx: Context,
        options_descriptor: Descriptor,
        options_class: type[Message],
        composite: bool = False,
        depth: int = 0,
        exclude: set[str] | None = None,
    ) -> list[Block]:
        """
        Generic options renderer for all descriptor types.

        Handles both built-in options and extension options.

        Args:
            ctx: Build context
            options_descriptor: Descriptor for the options type
                               (e.g., ctx.eno_desc, ctx.mso_desc, ctx.fdo_desc)
            options_class: Message class for creating options instances
                          (e.g., ctx.eno_cls, ctx.mso_cls, ctx.fdo_cls)
            composite: True for inline options with commas, False for standalone
                      option statements with 'option' keyword and semicolon
            depth: Indentation depth

        Returns:
            List of text Blocks containing rendered options
        """
        blocks: list[Block] = []

        # Access fqdn attribute (now guaranteed to exist)
        if not self.fqdn:
            logger.warning(
                "render_options called on object without fqdn attribute"
            )
            return blocks

        # Extract prefix and type name from FQDN
        # FQDN format: '<prefix>:<leading_dot><package>.<name>'
        # Example: 'enum:.google.protobuf.MyEnum'
        if ':' not in self.fqdn:
            logger.warning(f"Invalid FQDN format: {self.fqdn}")
            return blocks

        prefix, ref = parse_fqdn(self.fqdn)

        # Strip the leading dot to get the full type name for pool lookup
        # parse_fqdn returns ref with leading dot (e.g., '.google.protobuf.Timestamp')
        # Pool lookup needs it without the dot (e.g., 'google.protobuf.Timestamp')
        full_type_name = ref.lstrip('.')

        # Find descriptor in pool based on prefix type
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
                    f"for pool lookup in {self.fqdn}"
                )
                return blocks

            # Create fresh options instance and parse from descriptor
            opts_msg = options_class()
            opts_msg.ParseFromString(
                desc.GetOptions().SerializeToString()
            )

            # Use helper method to render the options
            blocks = self.render_options_from_message(
                ctx=ctx,
                opts_msg=opts_msg,
                options_descriptor=options_descriptor,
                composite=composite,
                depth=depth + 1,
                exclude=exclude,
            )

        except (KeyError, ValueError, TypeError, AttributeError) as e:
            logger.warning(
                f"Could not find descriptor for {full_type_name} "
                f"in pool: {e}"
            )

        return blocks

    # === Abstract Methods (hooks for subclasses) ===

    @classmethod
    @abstractmethod
    def fqdn_from_ref(cls, ref: Ref) -> Fqdn:
        """Convert a reference to a fully qualified descriptor name."""
        pass

    @classmethod
    def _register_in_context(
        cls,
        ctx: Context,
        fqdn: Fqdn,
        instance: 'NodeBase[Any]',
    ) -> None:
        """
        Hook for subclass-specific context registration.
        Override to register in additional context collections.
        """
        pass

    @classmethod
    def _requires_parent(cls) -> bool:
        """
        Whether this descriptor type requires a parent.
        Override for descriptors that don't need parents (e.g., files).
        """
        return True

    @abstractmethod
    def _initialize_from_message(
        self,
        ctx: Context,
        message: MessageT,
        **kwargs: Any
    ) -> None:
        """
        Hook for subclass-specific initialization from a message.
        Override to set class-specific attributes and build dependency graph.
        """
        pass


# === Backward Compatibility Aliases ===
# Migration complete - all Re*DescriptorProto classes now use NodeBase directly

Node = NodeBase  # Alias for convenience
