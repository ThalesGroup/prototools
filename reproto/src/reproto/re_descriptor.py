# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""ReDescriptorProto - Message descriptor redescriptor."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, cast

if TYPE_CHECKING:
    from .re_file import ReFileDescriptorProto

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
from .globals import MESSAGE
from .re_field import _resolve_field_features
from .text import CODE, COMMENT, ORPHAN, Block, BlockLine


def _render_extend_blocks(
    ctx: Context,
    owner: 'ReDescriptorProto | ReFileDescriptorProto',
    extensions: RepeatedCompositeFieldContainer,
    depth: int,
    anomaly_code: str,
    track_orphans: bool,
) -> Block:
    """Render grouped extend blocks for a file or message owner.

    Iterates extensions twice: first to collect distinct extendee short names
    (preserving encounter order), then to emit one extend { } block per extendee.

    Args:
        ctx: Rendering context.
        owner: The file or message node that owns the extension fields.
        extensions: The repeated extension field descriptors to render.
        depth: Indentation depth for the extend { } brackets.
        anomaly_code: Anomaly code for illegal extend blocks ("A5" or "B1").
        track_orphans: When True, check fd.is_summoned and mark unsummoned
                       fields and brackets as ORPHAN. When False, emit CODE.
    """
    from .anomalies import report
    from .re_field import ReFieldDescriptorProto
    from .syntax import allow_extend_block
    from .utils import short_ref

    out = Block()

    # Pass 1: collect distinct extendee short refs in encounter order.
    extendee_refs: list[str] = []
    for e in extensions:
        extension_proto = cast(FieldDescriptorProto, e)
        if not allow_extend_block(ctx, extension_proto.extendee):
            out.append(report(anomaly_code, depth,
                              msg=getattr(owner, 'name', ''),
                              file=getattr(owner, 'name', ''),
                              extendee=extension_proto.extendee))
            continue
        fd = ReFieldDescriptorProto(ctx, extension_proto, parent=owner)
        ref = str(short_ref(ctx, Fqdn(f'message:{fd.extendee}'), owner))
        if ref not in extendee_refs:
            extendee_refs.append(ref)

    # Pass 2: emit one extend { } block per extendee.
    for ref in extendee_refs:
        block = Block()
        is_orphan = True
        for e in extensions:
            extension_proto = cast(FieldDescriptorProto, e)
            if not allow_extend_block(ctx, extension_proto.extendee):
                continue
            fd = ReFieldDescriptorProto(ctx, extension_proto, parent=owner)
            if str(short_ref(ctx, Fqdn(f'message:{fd.extendee}'), owner)) != ref:
                continue
            blk = fd.render(ctx, depth + 1)
            if track_orphans:
                if fd.is_summoned:
                    is_orphan = False
                else:
                    blk.abandon()
            else:
                is_orphan = False
            block.extend(blk)
        bracket_type = ORPHAN if (track_orphans and is_orphan) else CODE
        block.insert(0, BlockLine(f'extend {ref} {{', depth, type=bracket_type))
        block.append(BlockLine('}', level=depth, type=bracket_type))
        out.extend(block)

    return out


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
    def parent(self, value: NodeBase[Any]) -> None:
        self._parent = value

    @property
    def reserved_range(self) -> RepeatedCompositeFieldContainer:
        return self.this.reserved_range

    def render_extensions(self, ctx: Context, depth: int = 0) -> Block:
        """Render message extensions grouped by extendee."""
        return _render_extend_blocks(
            ctx=ctx,
            owner=self,
            extensions=self.extension,
            depth=depth,
            anomaly_code='B1',
            track_orphans=False,
        )

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

        # Extension ranges (proto2 only)
        from .syntax import allow_extension_ranges
        if not allow_extension_ranges(ctx):
            for r in self.extension_range:
                range_proto = cast(DescriptorProto.ExtensionRange, r)
                from .anomalies import report
                out.append(report("B2", depth,
                                   msg=self.name,
                                   start=range_proto.start, end=range_proto.end))
        else:
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
        for name in self.this.reserved_name:
            if ctx.target_syntax == "editions":
                out.append(BlockLine(f'reserved {name};', depth))
            else:
                out.append(BlockLine(f'reserved "{name}";', depth))

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
        **kwargs: Any,
    ) -> None:
        """Initialize message-specific attributes and build dependency graph."""
        # Lazy imports
        from .re_enum import ReEnumDescriptorProto
        from .re_field import ReFieldDescriptorProto
        
        # Message extensions
        for e in self.extension:
            extension_proto = cast(FieldDescriptorProto, e)
            extension = ReFieldDescriptorProto(ctx, extension_proto, parent=self)
            self.targets.add(extension)

        # Message enums
        for e in self.enum_type:
            enum_proto = cast(EnumDescriptorProto, e)
            enum = ReEnumDescriptorProto(ctx, enum_proto, parent=self)
            self.targets.add(enum)

        # Message nested messages
        for n in self.nested_type:
            nested_proto = cast(DescriptorProto, n)
            nested = ReDescriptorProto(ctx, nested_proto, parent=self)
            self.targets.add(nested)

        # Message fields
        for f in self.field:
            field_proto = cast(FieldDescriptorProto, f)
            fd = ReFieldDescriptorProto(ctx, field_proto, parent=self)
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
        from .utils import get_file_node

        assert isinstance(depth, int)
        out = Block()
        inputs = Block()

        fdp = get_file_node(self)

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
        from .syntax import allow_message_set_wire_format, render_features_block
        msf_exclude: set[str] | None = None
        if not allow_message_set_wire_format(ctx):
            msf_exclude = {'message_set_wire_format'}
            if (self.this.HasField('options')
                    and self.this.options.HasField('message_set_wire_format')
                    and self.this.options.message_set_wire_format):
                from .anomalies import report
                out.append(report("B3", depth+1, msg=self.name))
        # Emit features.X = Y overrides first (editions only; no-op otherwise).
        if self.this.HasField('options') and self.this.options.HasField('features'):
            feat_block = render_features_block(
                ctx, self.this.options.features, depth + 1, inline=False)
            for line in feat_block:
                out.append(line)
        option_blocks = self.render_options(
            ctx=ctx,
            options_descriptor=ctx.mso_desc,
            options_class=ctx.mso_cls,
            composite=False,
            depth=depth,
            exclude=msf_exclude,
        )
        for block in option_blocks:
            out.extend(block)
        if option_blocks or (self.this.HasField('options') and self.this.options.HasField('features') and ctx.target_syntax == "editions"):
            out.append_div_maybe(depth)

        # --- Message fields and oneofs (in declaration order) -----------------
        # Pre-compute synthetic oneof indices (spec 0019 §11).
        # A synthetic oneof is created by protoc for every proto3 `optional` field.
        # It must be suppressed; its sole member is rendered at message level.
        from .syntax import is_synthetic_oneof
        synthetic_oneof_indices: set[int] = set()
        for idx, oneof in enumerate(self.oneof_decl):
            members = [f for f in self.field
                       if f.HasField("oneof_index") and f.oneof_index == idx]
            # For editions: resolve features for the single member (if any).
            member_features = (
                _resolve_field_features(ctx, fdp.this, self.this,
                                        cast(FieldDescriptorProto, members[0]))
                if members else None
            )
            if is_synthetic_oneof(ctx, oneof.name, members, features=member_features):
                synthetic_oneof_indices.add(idx)

        # Track which real oneofs we've already rendered
        is_done: list[bool] = [False] * len(self.oneof_decl)

        for f in self.field:
            field_proto = cast(FieldDescriptorProto, f)
            fd = ReFieldDescriptorProto(ctx, field_proto, parent=self)

            # Non-oneof field (including synthetic-oneof members): render directly
            if (not fd.HasField("oneof_index")
                    or fd.oneof_index in synthetic_oneof_indices):
                field = fd.render(ctx, depth+1)
                if not fd.is_summoned:
                    field.abandon()
                out.extend(field)
                continue

            # Real oneof field: render the entire oneof block on first encounter
            if is_done[fd.oneof_index]:
                continue
            is_done[fd.oneof_index] = True

            # Render oneof block
            oneof = self.oneof_decl[fd.oneof_index]
            block = Block()
            is_orphan = True

            for f2 in self.field:
                if (f2.HasField("oneof_index")
                    and f2.oneof_index == fd.oneof_index):
                    field_proto2 = cast(FieldDescriptorProto, f2)
                    fd2 = ReFieldDescriptorProto(ctx, field_proto2, parent=self)
                    field = fd2.render(ctx, depth+2, is_oneof=True)
                    if fd2.is_summoned:
                        is_orphan = False
                    else:
                        field.abandon()
                    block.extend(field)

            # Emit oneof features overrides at the top of the oneof block.
            oneof_feat_block = Block()
            if oneof.HasField('options') and oneof.options.HasField('features'):
                from .syntax import render_features_block
                oneof_feat_block = render_features_block(
                    ctx, oneof.options.features, depth + 2, inline=False)
            block.insert(0, BlockLine(f'oneof {oneof.name} {{', depth+1,
                         type=ORPHAN if is_orphan else CODE))
            for feat_line in reversed(list(oneof_feat_block)):
                block.insert(1, feat_line)
            block.append(BlockLine('}', level=depth+1,
                         type=ORPHAN if is_orphan else CODE))
            out.extend(block)
        out.append_div_maybe(depth)

        # --- Message nested messages ------------------------------------------
        for n in self.nested_type:
            nested_proto = cast(DescriptorProto, n)
            nested = ReDescriptorProto(ctx, nested_proto, parent=self)
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
            enum = ReEnumDescriptorProto(ctx, enum_proto, parent=self)
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
