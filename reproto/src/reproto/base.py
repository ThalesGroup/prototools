# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Base classes for redescriptor pattern - unified NodeBase implementation."""

from __future__ import annotations

import logging
from abc import ABC, abstractmethod
from typing import Any, Generic, Self, TypeVar

from google.protobuf.descriptor import Descriptor
from google.protobuf.message import Message

from .context import Context, Fqdn
from .fake_types import Prefix, Ref
from .option_rendering import (
    DescriptorMessage,
    OptionsMessage,
    format_composite_options,
    render_options,
    render_options_from_message,
)
from .text import Block, BlockLine

__all__ = [
    "NodeBase",
    "Node",
    "DescriptorMessage",
    "OptionsMessage",
    "format_composite_options",
    "render_options",
    "render_options_from_message",
]

logger = logging.getLogger(__name__)

MessageT = TypeVar('MessageT', bound=Message)


class NodeBase(Generic[MessageT], ABC):
    """Base class for all redescriptor nodes in the descriptor graph.

    Combines registry pattern, protobuf message delegation, and graph structure
    in a single hierarchy.
    """

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
        """Return existing instance from registry if already created, otherwise create new."""
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
            # Stripped pruned-duplicate imports (spec 0053); file nodes only.
            from .re_file import ReFileDescriptorProto as _ReFile
            if isinstance(instance, _ReFile):
                instance.stripped_dependencies = []
                instance.stripped_public_dependencies = []

            # Register in context
            ctx.new_nodes[fqdn] = instance
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
        """The underlying protobuf message."""
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
        # All descriptor messages have 'name', but it's not in Message base class
        return self.this.name  # type: ignore[attr-defined]

    @property
    def options(self) -> OptionsMessage:
        # All descriptor messages have options
        return self.this.options  # type: ignore[return-value]

    @property
    def parent(self) -> 'NodeBase[Any] | None':
        return self._parent

    @parent.setter
    def parent(self, value: 'NodeBase[Any]') -> None:
        self._parent = value

    # === Utility Methods ===

    def is_present(self) -> bool:
        return self._this is not None

    def is_visible(self) -> bool:
        return self.is_reachable

    @classmethod
    def from_ref(cls, ctx: Context, ref: Ref) -> Self:
        """Create or retrieve an instance from a reference."""
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

    def render_options_from_message(
        self,
        ctx: Context,
        opts_msg: Message,
        options_descriptor: Descriptor,
        composite: bool = False,
        depth: int = 0,
        exclude: set[str] | None = None,
    ) -> list[Block]:
        return render_options_from_message(
            ctx, opts_msg, options_descriptor, composite, depth, exclude
        )

    def format_composite_options(
        self,
        option_blocks: Block,
        prolog: BlockLine,
        depth: int,
    ) -> Block:
        return format_composite_options(self, option_blocks, prolog, depth)

    def render_options(
        self,
        ctx: Context,
        options_descriptor: Descriptor,
        options_class: type[Message],
        composite: bool = False,
        depth: int = 0,
        exclude: set[str] | None = None,
    ) -> list[Block]:
        return render_options(
            self, ctx, options_descriptor, options_class, composite, depth, exclude
        )

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
