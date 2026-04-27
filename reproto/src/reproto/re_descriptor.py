# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""ReDescriptorProto - Message descriptor redescriptor."""

from __future__ import annotations

from typing import cast

from google.protobuf.descriptor import Descriptor
from google.protobuf.descriptor_pb2 import (
    DescriptorProto,
    EnumDescriptorProto,
    FieldDescriptorProto,
)
from google.protobuf.internal.containers import RepeatedCompositeFieldContainer


from .base import NodeBase
from .context import Context, Fqdn
from .fake_types import Ref
from .globals import MESSAGE, label_names
from .text import CODE, COMMENT, ORPHAN, Block, BlockLine


class ReDescriptorProto(NodeBase[DescriptorProto]):
    """Redescriptor for DescriptorProto (message descriptors)."""
    
    @property
    def DESCRIPTOR(self) -> Descriptor:
        return self.this.DESCRIPTOR

    @property
    def enum_type(self) -> RepeatedCompositeFieldContainer:
        return self.this.enum_type

    @property
    def extension_range(self) -> RepeatedCompositeFieldContainer:
        return self.this.extension_range

    @property
    def extension(self) -> RepeatedCompositeFieldContainer:
        return self.this.extension

    @property
    def field(self) -> RepeatedCompositeFieldContainer:
        return self.this.field

    @property
    def is_group(self) -> bool:
        return getattr(self, '_is_group', False)

    @is_group.setter
    def is_group(self, value: bool) -> None:
        self._is_group = value

    @property
    def is_map_entry(self) -> bool:
        """Check if this message is a map entry (synthetic message from map<K,V>)."""
        return self.this.options.map_entry if self.this.HasField('options') else False

    @property
    def nested_type(self) -> RepeatedCompositeFieldContainer:
        return self.this.nested_type

    @property
    def oneof_decl(self) -> RepeatedCompositeFieldContainer:
        return self.this.oneof_decl

    @property
    def parent(self):
        from .re_file import ReFileDescriptorProto
        assert isinstance(self._parent, (ReFileDescriptorProto, ReDescriptorProto))
        return self._parent

    @parent.setter
    def parent(self, value) -> None:
        self._parent = value

    @property
    def reserved_range(self) -> RepeatedCompositeFieldContainer:
        return self.this.reserved_range

    def render_extensions(self, ctx: Context, depth: int = 0) -> Block:
        """
        Render message extensions grouped by extendee.

        Returns a Block containing extend statements, or an empty Block if
        no extensions are defined.
        """
        from .re_field import ReFieldDescriptorProto

        out = Block()

        # Group extensions by extendee short name
        extendee_short_names: list[str] = []
        for e in self.extension:
            extension_proto = cast(FieldDescriptorProto, e)
            fd: ReFieldDescriptorProto = ReFieldDescriptorProto(ctx, extension_proto, parent=self)  # type: ignore[assignment]
            short_name = fd.short_type_name(ctx, fd.extendee)
            if short_name not in extendee_short_names:
                extendee_short_names.append(short_name)

        # Render extend blocks for each extendee
        for short_name in extendee_short_names:
            out.append(BlockLine(f'extend {short_name} {{', depth))
            for e in self.extension:
                extension_proto = cast(FieldDescriptorProto, e)
                fd: ReFieldDescriptorProto = ReFieldDescriptorProto(ctx, extension_proto, parent=self)  # type: ignore[assignment]
                name = fd.short_type_name(ctx, fd.extendee)
                if name != short_name:
                    continue
                out.append(BlockLine(f'{label_names[fd.label]} '
                                f'{fd.short_type_name(ctx)} '
                                f'{fd.name} = {fd.number};', depth+1))
            out.append(BlockLine('}', level=depth))

        return out

    def render_oneofs(self, ctx: Context, depth: int = 0) -> Block:
        """
        Render oneof blocks with their fields.

        Returns a Block containing oneof statements, or an empty Block if
        no oneofs are defined.
        """
        from .re_field import ReFieldDescriptorProto

        out = Block()

        # Track which oneofs we've already rendered
        is_done: list[bool] = [False] * len(self.oneof_decl)

        for f in self.field:
            field_proto = cast(FieldDescriptorProto, f)
            fd: ReFieldDescriptorProto = ReFieldDescriptorProto(ctx, field_proto, parent=self)  # type: ignore[assignment]

            # Skip fields not in a oneof
            if not fd.HasField("oneof_index"):
                continue

            # Skip if we've already rendered this oneof
            if is_done[fd.oneof_index]:
                continue
            else:
                is_done[fd.oneof_index] = True

            # Render the oneof block
            oneof = self.oneof_decl[fd.oneof_index]
            block = Block()
            is_orphan = True

            for f in self.field:
                if (f.HasField("oneof_index")
                    and f.oneof_index == fd.oneof_index):
                    field_proto2 = cast(FieldDescriptorProto, f)
                    fd2: ReFieldDescriptorProto = ReFieldDescriptorProto(ctx, field_proto2, parent=self)  # type: ignore[assignment]
                    field = fd2.render(ctx, depth+1, is_oneof=True)
                    if fd2.is_summoned:
                        is_orphan = False
                    else:
                        field.abandon()
                    block.extend(field)

            block.insert(0, BlockLine(f'oneof {oneof.name} {{', depth,
                         type=ORPHAN if is_orphan else CODE))
            block.append(BlockLine('}', level=depth,
                         type=ORPHAN if is_orphan else CODE))
            out.extend(block)

        return out

    def render_message_comments(self, depth: int = 0) -> Block:
        """
        Extract and render message-level comments from source_code_info.

        Returns a Block containing comment lines, or an empty Block if
        no comments are found.
        """
        from .re_file import ReFileDescriptorProto

        out = Block()

        # Find the root file to access source_code_info
        current = self
        while current.parent is not None:
            parent = current.parent
            if isinstance(parent, ReFileDescriptorProto):
                file = parent
                break
            current = parent
        else:
            # No file parent found
            return out

        if not file.source_code_info:
            return out

        # Calculate the path to this message in the descriptor tree
        # For top-level messages: [4, index]
        # For nested messages: [4, parent_index, 3, nested_index, 3, ...]
        path = self._calculate_source_code_info_path()
        if not path:
            return out

        # Find matching location in source_code_info
        for location in file.source_code_info.location:
            if list(location.path) == path:
                # Leading comments before the message
                if location.leading_comments:
                    for line in location.leading_comments.strip().split('\n'):
                        out.append(BlockLine(line, depth, COMMENT))

                # Leading detached comments
                for detached in location.leading_detached_comments:
                    for line in detached.strip().split('\n'):
                        out.append(BlockLine(line, depth, COMMENT))

                # Trailing comments after the message declaration
                if location.trailing_comments:
                    for line in location.trailing_comments.strip().split('\n'):
                        out.append(BlockLine(line, depth, COMMENT))

                break

        return out

    def _calculate_source_code_info_path(self) -> list[int] | None:
        """
        Calculate the source_code_info path for this message.

        Returns:
            List of field numbers representing the path, or None if unable to calculate.
        """
        from .re_file import ReFileDescriptorProto

        # Build path from root to this message
        path_segments = []
        current = self

        while current.parent is not None:
            parent = current.parent

            if isinstance(parent, ReFileDescriptorProto):
                # Top-level message: find index in file.message_type
                try:
                    index = next(i for i, m in enumerate(parent.message_type)
                                if m.name == current.name)
                    path_segments.insert(0, [4, index])  # 4 = message_type field
                except StopIteration:
                    return None
                break
            elif isinstance(parent, ReDescriptorProto):
                # Nested message: find index in parent.nested_type
                try:
                    index = next(i for i, m in enumerate(parent.nested_type)
                                if m.name == current.name)
                    path_segments.insert(0, [3, index])  # 3 = nested_type field
                except StopIteration:
                    return None

            current = parent

        # Flatten the path segments
        flat_path = []
        for segment in path_segments:
            flat_path.extend(segment)

        return flat_path if flat_path else None

    def render_reserved(self, ctx: Context, depth: int = 0) -> Block:
        """
        Render reserved ranges and names.

        Returns a Block containing reserved statements, or an empty Block if
        no reservations are defined.
        """
        from .re_simple import ReExtensionRange, ReReservedRange

        out = Block()

        # Extension ranges
        for r in self.extension_range:
            range_proto = cast(DescriptorProto.ExtensionRange, r)
            range_obj = ReExtensionRange(range_proto)
            out.extend(range_obj.render(ctx, depth))

        # Reserved ranges
        for r in self.reserved_range:
            range_proto = cast(DescriptorProto.ReservedRange, r)
            range_obj = ReReservedRange(range_proto)
            out.extend(range_obj.render(depth))

        # Reserved names
        if self.this.reserved_name:
            # Group all reserved names into a single statement
            names = ', '.join(f'"{name}"' for name in self.this.reserved_name)
            out.append(BlockLine(f'reserved {names};', depth))

        return out

    def get_package(self) -> str:
        """Get the full package path for this message."""
        from .re_file import ReFileDescriptorProto
        
        parent = self.parent
        if isinstance(parent, ReFileDescriptorProto):
            if not parent.package:
                return ''
            else:
                return f'.{parent.package}'
        parent_package = parent.get_package()
        return f'{parent_package}.{parent.name}'

    @classmethod
    def fqdn_from_ref(cls, ref: Ref) -> Fqdn:
        return Fqdn(f'{MESSAGE}:{str(ref)}')
    
    def _initialize_from_message(
        self,
        ctx: Context,
        message: DescriptorProto,
        **kwargs
    ) -> None:
        """Initialize message-specific attributes and build dependency graph."""
        # Lazy imports
        from .re_enum import ReEnumDescriptorProto
        from .re_field import ReFieldDescriptorProto
        
        # Message extensions
        for e in self.extension:
            extension_proto = cast(FieldDescriptorProto, e)
            extension: ReFieldDescriptorProto = ReFieldDescriptorProto(ctx, extension_proto, parent=self)  # type: ignore[assignment]
            self.targets.add(extension)

        # Message enums
        for e in self.enum_type:
            enum_proto = cast(EnumDescriptorProto, e)
            enum: ReEnumDescriptorProto = ReEnumDescriptorProto(ctx, enum_proto, parent=self)  # type: ignore[assignment]
            self.targets.add(enum)

        # Message nested messages
        for n in self.nested_type:
            nested_proto = cast(DescriptorProto, n)
            nested: ReDescriptorProto = ReDescriptorProto(ctx, nested_proto, parent=self)  # type: ignore[assignment]
            self.targets.add(nested)

        # Message fields
        for f in self.field:
            field_proto = cast(FieldDescriptorProto, f)
            fd: ReFieldDescriptorProto = ReFieldDescriptorProto(ctx, field_proto, parent=self)  # type: ignore[assignment]
            self.targets.add(fd)
            self.contains.add(fd)
    
    def render(
        self,
        ctx: Context,
        depth: int = 0,
        force: bool = False
    ) -> tuple[Block, Block]:
        """
        Reconstruct message as .proto.

        Element Ordering Strategy
        -------------------------
        This method renders message elements in a STATIC order that matches
        protoc's internal descriptor structure:

        1. Comments (from source_code_info, if available)
        2. Extension blocks (extend OtherMessage { ... })
        3. Message options
        4. Fields and oneofs (interleaved by field number)
        5. Nested messages
        6. Nested enums
        7. Reserved ranges and names

        Rationale:
        - Protoc ALWAYS groups elements by type in descriptors, regardless of
          source order (all fields together, all nested_type together, etc.)
        - This means ANY source ordering compiles to the SAME descriptor
          (modulo filename and source_code_info)
        - Using a static order that matches protoc's structure ensures perfect
          roundtripping without needing to reconstruct arbitrary source orders
        - We DO preserve relative order within each category (e.g., nested_type[0]
          stays before nested_type[1])

        See INSIGHTS.md for detailed investigation and proof.

        Special Cases:
        - Map entries (synthetic nested_type) are filtered out and rendered as
          map<K,V> syntax
        - Groups (TYPE_GROUP fields) are rendered inline with fields, not in
          the nested messages section
        - Oneofs must keep their fields together in a single oneof { } block

        Args:
            ctx: Rendering context
            depth: Indentation depth
            force: Force rendering even if node is not reachable

        Returns:
            Tuple of (output_block, inputs_block) containing rendered proto
        """
        from .re_enum import ReEnumDescriptorProto
        from .re_field import ReFieldDescriptorProto

        assert isinstance(depth, int)
        out = Block()
        inputs = Block()

        # --- Message comments -------------------------------------------------
        comments_block = self.render_message_comments(depth+1)
        if comments_block:
            out.extend(comments_block)
            out.append_div_maybe(depth)

        # --- Message extensions -----------------------------------------------
        extensions_block = self.render_extensions(ctx, depth+1)
        if extensions_block:
            out.extend(extensions_block)
            out.append_div_maybe(depth)

        # --- Message options --------------------------------------------------
        option_blocks = self.render_options(
            ctx=ctx,
            options_descriptor=ctx.mso_desc,
            options_class=ctx.mso_cls,
            composite=False,
            depth=depth
        )
        for block in option_blocks:
            out.extend(block)
        if option_blocks:
            out.append_div_maybe(depth)

        # --- Message fields and oneofs (in declaration order) -----------------
        # Track which oneofs we've already rendered
        is_done: list[bool] = [False] * len(self.oneof_decl)

        for f in self.field:
            field_proto = cast(FieldDescriptorProto, f)
            fd: ReFieldDescriptorProto = ReFieldDescriptorProto(ctx, field_proto, parent=self)  # type: ignore[assignment]

            # Non-oneof field: render directly
            if not fd.HasField("oneof_index"):
                field = fd.render(ctx, depth+1)
                if not fd.is_summoned:
                    field.abandon()
                out.extend(field)
                continue

            # Oneof field: render the entire oneof block on first encounter
            if is_done[fd.oneof_index]:
                continue
            else:
                is_done[fd.oneof_index] = True

            # Render oneof block
            oneof = self.oneof_decl[fd.oneof_index]
            block = Block()
            is_orphan = True

            for f2 in self.field:
                if (f2.HasField("oneof_index")
                    and f2.oneof_index == fd.oneof_index):
                    field_proto2 = cast(FieldDescriptorProto, f2)
                    fd2: ReFieldDescriptorProto = ReFieldDescriptorProto(ctx, field_proto2, parent=self)  # type: ignore[assignment]
                    field = fd2.render(ctx, depth+2, is_oneof=True)
                    if fd2.is_summoned:
                        is_orphan = False
                    else:
                        field.abandon()
                    block.extend(field)

            block.insert(0, BlockLine(f'oneof {oneof.name} {{', depth+1,
                         type=ORPHAN if is_orphan else CODE))
            block.append(BlockLine('}', level=depth+1,
                         type=ORPHAN if is_orphan else CODE))
            out.extend(block)
        out.append_div_maybe(depth)

        # --- Message nested messages ------------------------------------------
        for n in self.nested_type:
            nested_proto = cast(DescriptorProto, n)
            nested: ReDescriptorProto = ReDescriptorProto(ctx, nested_proto, parent=self)  # type: ignore[assignment]
            # Skip: unsummoned messages, groups (rendered inline), and map entries (rendered as map<K,V>)
            if not nested.is_summoned or nested.is_group or nested.is_map_entry:
                continue
            text, inp = nested.render(ctx, depth+1)
            text.insert(0, BlockLine(f'message {nested.name} {{', depth+1))
            out.extend(text)
            inputs.extend(inp)
        out.append_div_maybe(depth)

        # --- Message enums ----------------------------------------------------
        for e in self.enum_type:
            enum_proto = cast(EnumDescriptorProto, e)
            enum: ReEnumDescriptorProto = ReEnumDescriptorProto(ctx, enum_proto, parent=self)  # type: ignore[assignment]
            out.extend(enum.render(ctx, depth+1))
        out.append_div_maybe(depth)

        # --- Message reserved (extension ranges, reserved ranges, reserved names)
        reserved_block = self.render_reserved(ctx, depth+1)
        if reserved_block:
            out.extend(reserved_block)
            out.append_div_maybe(depth)

        # --- Message outro ----------------------------------------------------
        while len(out) > 1 and not out[-1].text:
            out.pop()
        out.append(BlockLine('}', depth))
        out.append_div_maybe(depth)

        return out, inputs
