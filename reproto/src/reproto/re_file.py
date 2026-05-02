# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""ReFileDescriptorProto - File descriptor redescriptor."""

from __future__ import annotations

import json
from typing import Any, cast

from google.protobuf.descriptor import FileDescriptor
from google.protobuf.descriptor_pb2 import (
    DescriptorProto,
    Edition,
    EnumDescriptorProto,
    FieldDescriptorProto,
    FileDescriptorProto,
    ServiceDescriptorProto,
)
from google.protobuf.internal.containers import (
    RepeatedCompositeFieldContainer,
    RepeatedScalarFieldContainer,
)
from google.protobuf.message import Message


from .base import NodeBase
from .context import Context, Fqdn
from .fake_types import Prefix, Ref
from .globals import FILE
from .mappings import canonize_dependency
from .text import CODE, COMMENT, ORPHAN, Block, BlockLine


class ReFileDescriptorProto(NodeBase[FileDescriptorProto]):
    """Redescriptor for FileDescriptorProto."""

    @property
    def dependency(self) -> RepeatedScalarFieldContainer[str]:
        return self.this.dependency

    @property
    def edition(self) -> 'Edition.ValueType':
        return self.this.edition

    @property
    def enum_type(self) -> RepeatedCompositeFieldContainer:
        return self.this.enum_type

    @property
    def extension(self) -> RepeatedCompositeFieldContainer:
        return self.this.extension

    @property
    def message_type(self) -> RepeatedCompositeFieldContainer:
        return self.this.message_type

    @property
    def public_dependency(self) -> RepeatedScalarFieldContainer[int]:
        return self.this.public_dependency

    @property
    def package(self) -> str:
        return self.this.package

    @property
    def parent(self) -> None:
        assert self._parent is None
        return self._parent

    @parent.setter
    def parent(self, value: NodeBase[Any] | None) -> None:
        assert value is None, f"FileDescriptorProto parent must be None, got {type(value)}"
        self._parent = value

    @property
    def ref(self) -> Ref:
        return Ref(f'.{self.package}' if self.package else '')

    @property
    def service(self) -> RepeatedCompositeFieldContainer:
        return self.this.service

    @property
    def syntax(self) -> str:
        return self.this.syntax

    @property
    def weak_dependency(self) -> RepeatedScalarFieldContainer[int]:
        return self.this.weak_dependency

    @property
    def source_code_info(self):
        """Access to SourceCodeInfo for extracting original comments."""
        return self.this.source_code_info if self.this.HasField('source_code_info') else None

    @property
    def type_name(self) -> str:
        if self.package:
            return f'.{self.package}'
        else:
            return ''

    @classmethod
    def fqdn_from_ref(cls, ref: str) -> Fqdn:
        return Fqdn(f'{FILE}:{ref}')

    @classmethod
    def _register_in_context(cls, ctx: Context, fqdn: Fqdn, instance: NodeBase[Any]) -> None:
        """Register in the files collection."""
        assert isinstance(instance, cls)
        ctx.new_files[fqdn] = instance
    
    @classmethod
    def _requires_parent(cls) -> bool:
        """Files don't have parents."""
        return False
    
    def render_file_options(self, ctx: Context, depth: int = 0) -> Block:
        """
        Render file options (both builtin and custom extensions).

        Returns a Block containing option statements, or an empty Block if
        rendering fails or no options are set.
        """
        from .re_simple import ReExtensions, ReOptions

        out = Block()
        fio_desc = ctx.fio_desc
        FiOClass = ctx.fio_cls

        try:
            f_desc: FileDescriptor = ctx.pool.FindFileByName(self.name)

            fio_msg: Message = FiOClass()
            fio_msg.ParseFromString(f_desc.GetOptions().SerializeToString())

            # Emit file-level features overrides first (editions only; no-op otherwise).
            from .syntax import render_features_block
            if self.this.HasField('options') and self.this.options.HasField('features'):
                feat_block = render_features_block(
                    ctx, self.this.options.features, depth, inline=False)
                out.extend(feat_block)

            # Render builtin options
            blocks = ReOptions(fio_msg).render(ctx, False, depth)
            for block in blocks:
                out.extend(block)

            # Render custom extension options
            blocks, is_orp = ReExtensions(fio_msg).render(ctx, fio_desc, False, depth)
            for block in blocks:
                out.extend(block)
        except (KeyError, ValueError, TypeError, AttributeError) as e:
            from .anomalies import report
            out.append(report("A3", depth,
                               file=self.name, exc_type=type(e).__name__, exc_msg=str(e)))

        return out

    def render_source_code_info_comments(self, depth: int = 0) -> Block:
        """
        Extract and render comments from source_code_info.

        For now, we render file-level comments (those with empty or minimal paths).
        These are typically file headers and top-level documentation.

        File header comments are typically stored as detached comments on the
        syntax field (path=[12]).
        """
        out = Block()

        if not self.source_code_info:
            return out

        # Process each location in source_code_info
        for location in self.source_code_info.location:
            # File-level comments:
            # - path=[] for file itself
            # - path=[12] for syntax field (where file headers are often stored)
            if len(location.path) == 0 or (len(location.path) == 1 and location.path[0] == 12):
                # Leading comments before the element
                if location.leading_comments:
                    for line in location.leading_comments.strip().split('\n'):
                        out.append(BlockLine(line, depth, COMMENT))
                    out.append(BlockLine('', depth))

                # Leading detached comments (separated by blank lines)
                # These are typically file header comments
                for detached in location.leading_detached_comments:
                    for line in detached.strip().split('\n'):
                        out.append(BlockLine(line, depth, COMMENT))
                    out.append(BlockLine('', depth))

                # Trailing comments after the element
                if location.trailing_comments:
                    for line in location.trailing_comments.strip().split('\n'):
                        out.append(BlockLine(line, depth, COMMENT))
                    out.append(BlockLine('', depth))

        return out

    def _initialize_from_message(
        self,
        ctx: Context,
        message: FileDescriptorProto,
        go_root: str | None = None,
        **kwargs: Any,
    ) -> None:
        """Initialize file-specific attributes and build dependency graph."""
        # Lazy imports to avoid circular dependencies
        from .re_descriptor import ReDescriptorProto
        from .re_enum import ReEnumDescriptorProto
        from .re_field import ReFieldDescriptorProto
        from .re_service import ReServiceDescriptorProto
        
        self.parent = None
        self.prefix = Prefix(f'.{self.package}' if self.package else '')
        self.go_root = go_root

        # --- File dependencies ------------------------------------------------
        for dep in self.dependency:
            assert isinstance(dep, str)
            fdp = ReFileDescriptorProto.from_ref(ctx, Ref(dep))
            self.targets.add(fdp)
            # No 'contains' relation between a file and its dependencies

        # --- File services ----------------------------------------------------
        for s in self.service:
            service_proto = cast(ServiceDescriptorProto, s)
            service = ReServiceDescriptorProto(ctx, service_proto, parent=self)
            self.targets.add(service)
            self.contains.add(service)

        # --- File enums -------------------------------------------------------
        for e in self.enum_type:
            enum_proto = cast(EnumDescriptorProto, e)
            enum = ReEnumDescriptorProto(ctx, enum_proto, parent=self)
            self.targets.add(enum)
            self.contains.add(enum)

        # --- File messages ----------------------------------------------------
        for m in self.message_type:
            msg_proto = cast(DescriptorProto, m)
            msg = ReDescriptorProto(ctx, msg_proto, parent=self)
            self.targets.add(msg)
            self.contains.add(msg)

        # --- File extensions --------------------------------------------------
        for e in self.extension:
            extension_proto = cast(FieldDescriptorProto, e)
            extension = ReFieldDescriptorProto(ctx, extension_proto, parent=self)
            self.targets.add(extension)
            self.contains.add(extension)
            
            extendee = ReDescriptorProto.from_ref(ctx, Ref(extension.extendee))
            extension.targets.add(extendee)
            # No 'contains' relation between a file and an extendee
    
    def render(self, ctx: Context, depth: int = 0) -> tuple[Block, Block]:
        """Reconstruct a FileDescriptorProto as .proto."""
        from .re_descriptor import ReDescriptorProto
        from .re_enum import ReEnumDescriptorProto
        from .re_field import ReFieldDescriptorProto
        from .re_service import ReServiceDescriptorProto
        from .utils import short_ref
        
        assert isinstance(depth, int)

        from .syntax import fdp_syntax
        ctx.syntax = fdp_syntax(self.this)
        if not ctx.force_proto2_output and ctx.syntax in ("proto2", "proto3", "editions"):
            ctx.target_syntax = ctx.syntax
        else:
            ctx.target_syntax = "proto2"

        out = Block()
        inputs = Block()

        # --- File intro -------------------------------------------------------
        out.append(BlockLine(f'FILE NAME: {self.name}', depth, COMMENT))
        out.append(BlockLine('', depth))
        out.append_div_maybe(depth)

        # --- Source code info (original comments) -----------------------------
        sci_comments = self.render_source_code_info_comments(depth)
        if sci_comments:
            out.extend(sci_comments)
            out.append_div_maybe(depth)

        # Syntax / edition header
        from .anomalies import report
        if ctx.target_syntax == "editions":
            from .syntax import _edition_name
            edition_name = _edition_name(self.this.edition)
            out.append(BlockLine(f'edition = "{edition_name}";', depth))
        elif ctx.syntax != ctx.target_syntax:
            # A2: normal syntax downconversion (proto3 → proto2 via --force-proto2-output)
            # A1: editions → proto2 downconversion (--force-proto2-output)
            if ctx.syntax == "editions":
                out.append(report("A1", depth, file=self.name))
            else:
                out.append(report("A2", depth, file=self.name, syntax=ctx.syntax))
            out.append(BlockLine(f'syntax = "{ctx.target_syntax}";', depth))
        else:
            out.append(BlockLine(f'syntax = "{ctx.target_syntax}";', depth))
        out.append_div_maybe(depth)

        # Package
        if self.package != "":
            out.append(BlockLine(f'package {self.package};', depth))
            out.append(BlockLine('', depth))
        out.append_div_maybe(depth)

        # --- File options -----------------------------------------------------
        options_block = self.render_file_options(ctx, depth)
        if options_block:
            out.extend(options_block)
            out.append_div_maybe(depth)

        # --- File dependencies ------------------------------------------------
        for index, d in enumerate(self.dependency):
            assert isinstance(d, str)
            dependency = ReFileDescriptorProto.from_ref(ctx, Ref(d))
            if dependency.is_visible() or dependency.is_summoned:
                kind = CODE
            else:
                kind = ORPHAN

            # Determine import type (normal, public, or weak)
            from .syntax import allow_weak_import
            if index in self.public_dependency:
                import_cmd = 'import public'
            elif index in self.weak_dependency:
                if allow_weak_import(ctx):
                    import_cmd = 'import weak'
                else:
                    from .anomalies import report
                    out.append(report("A4", depth, file=self.name, dep=d))
                    import_cmd = 'import'
            else:
                import_cmd = 'import'

            # Format dependency name based on summoned status
            dep_name = (
                json.dumps(canonize_dependency(ctx, dependency.name))
                if dependency.is_summoned
                else dependency.fqdn
            )
            text = f'{import_cmd} {dep_name};'
            out.append(BlockLine(text, depth, kind))
        out.append_div_maybe(depth)

        # --- File services ----------------------------------------------------
        for s in self.service:
            service_proto = cast(ServiceDescriptorProto, s)
            service = ReServiceDescriptorProto(ctx, service_proto, parent=self)
            if not service.is_visible():
                continue
            lines, inp = service.render(ctx, depth)
            out.extend(lines)
            inputs.extend(inp)
        out.append_div_maybe(depth)

        # --- File enums -------------------------------------------------------
        for e in self.enum_type:
            enum_proto = cast(EnumDescriptorProto, e)
            enum = ReEnumDescriptorProto(ctx, enum_proto, parent=self)
            if not enum.is_reachable:
                continue
            out.extend(enum.render(ctx, depth))
        out.append_div_maybe(depth)

        # --- File messages ----------------------------------------------------
        for m in self.message_type:
            msg_proto = cast(DescriptorProto, m)
            message = ReDescriptorProto(ctx, msg_proto, parent=self)
            if not message.is_summoned or message.is_group:
                continue
            lines, inp = message.render(ctx)
            lines.insert(0, BlockLine(f'message {message.name} {{', depth))
            lines.append_div_maybe(depth)
            out.extend(lines)
            inputs.extend(inp)
        out.append_div_maybe(depth)

        # --- File extensions --------------------------------------------------
        from .syntax import allow_extend_block
        # Warn and skip extend blocks whose extendee is not legal in this syntax.
        # In proto3, only *Options extendees are allowed (custom options).
        extendee_short_names: list[str] = []
        for e in self.extension:
            extension_proto = cast(FieldDescriptorProto, e)
            if not allow_extend_block(ctx, extension_proto.extendee):
                from .anomalies import report
                out.append(report("A5", depth,
                                   file=self.name, extendee=extension_proto.extendee))
                continue
            fd = ReFieldDescriptorProto(ctx, extension_proto, parent=self)
            ref = short_ref(ctx, Fqdn(f'message:{fd.extendee}'), self)
            if ref not in extendee_short_names:
                extendee_short_names.append(ref)

        for ref in extendee_short_names:
            block = Block()
            is_orphan = True
            for e in self.extension:
                extension_proto = cast(FieldDescriptorProto, e)
                if not allow_extend_block(ctx, extension_proto.extendee):
                    continue
                fd = ReFieldDescriptorProto(ctx, extension_proto, parent=self)
                ref2 = short_ref(ctx, Fqdn(f'message:{fd.extendee}'), self)
                if ref2 != ref:
                    continue
                blk = fd.render(ctx, depth+1)
                if fd.is_summoned:
                    is_orphan = False
                else:
                    blk.abandon()
                block.extend(blk)
            block.insert(0, BlockLine(f'extend {ref} {{', depth,
                                      type = ORPHAN if is_orphan else CODE))
            block.append(BlockLine('}', level=depth,
                                   type = ORPHAN if is_orphan else CODE))
            out.extend(block)
        out.append_div_maybe(depth)

        # Remove any empty trailing lines from out
        while out and not out[-1].text:
            out.pop()
        return out, inputs
    
