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
from .context import Context, Fqdn, descriptor_rerender_muted
from .source_info import SourceCodeInfoMixin, closing_line
from .fake_types import Ref
from .globals import MESSAGE
from .re_field import _message_chain, _resolve_field_features
from .text import CODE, ORPHAN, Block, BlockLine
from .utils import option_scope, scope_name


def _render_extend_blocks(
    ctx: Context,
    owner: 'ReDescriptorProto | ReFileDescriptorProto',
    extensions: RepeatedCompositeFieldContainer,
    depth: int,
    anomaly_code: str,
    track_orphans: bool,
    sci_path: list[int] | None = None,
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
        sci_path: The owner message's own SourceCodeInfo path (spec 0141),
            used as the prefix for its extension fields' own paths. Ignored
            (an empty prefix is used instead) when owner is a file, since
            file-level extension fields are already top-level. None if
            source-info synthesis is off.
    """
    from .anomalies import report
    from .re_field import ReFieldDescriptorProto
    from .re_file import ReFileDescriptorProto
    from .syntax import allow_extend_block
    from .names import Mode
    from .utils import scope_name, type_ref

    out = Block()

    # --- SourceCodeInfo path index (spec 0141) -----------------------------
    # File-level extensions (field 7 of FileDescriptorProto): a running
    # counter over is_summoned entries, matching the file-level binary
    # side-channel's own `if not fd.is_summoned: continue` filter.
    # Message-level (nested) extensions (field 6 of DescriptorProto): the
    # message-level binary side-channel is unconditional, so the raw
    # enumerate() position is used instead.
    is_file_owner = isinstance(owner, ReFileDescriptorProto)
    ext_field_num = 7 if is_file_owner else 6
    ext_paths: dict[int, list[int]] = {}
    if ctx.out_sci is not None and (is_file_owner or sci_path is not None):
        prefix = sci_path if sci_path is not None else []
        counter = 0
        for e in extensions:
            extension_proto = cast(FieldDescriptorProto, e)
            if is_file_owner:
                fd = ReFieldDescriptorProto(ctx, extension_proto, parent=owner)
                if not fd.is_summoned:
                    continue
            ext_paths[id(extension_proto)] = prefix + [ext_field_num, counter]
            counter += 1

    # The extensions by extendee, as written, in encounter order: one
    # extend { } block each. An extendee is looked up among every symbol of
    # the scope the extension is declared in (spec 0434 S3).
    scope = scope_name(owner)
    by_extendee: dict[str, list[tuple[FieldDescriptorProto, ReFieldDescriptorProto]]] = {}
    for e in extensions:
        extension_proto = cast(FieldDescriptorProto, e)
        if not allow_extend_block(ctx, extension_proto.extendee):
            out.append(report(anomaly_code, depth,
                              msg=getattr(owner, 'name', ''),
                              file=getattr(owner, 'name', ''),
                              extendee=extension_proto.extendee))
            continue
        fd = ReFieldDescriptorProto(ctx, extension_proto, parent=owner)
        ref = type_ref(ctx, fd.extendee, scope, Mode.ALL)
        by_extendee.setdefault(ref, []).append((extension_proto, fd))

    for ref, members in by_extendee.items():
        block = Block()
        is_orphan = True
        for extension_proto, fd in members:
            blk = fd.render(ctx, depth + 1)
            ext_sci_path = ext_paths.get(id(extension_proto))
            if track_orphans:
                if fd.is_summoned:
                    is_orphan = False
                    if ext_sci_path is not None:
                        assert ctx.out_sci is not None
                        ctx.out_sci.pending.append(
                            (ext_sci_path, blk[0], closing_line(blk))
                        )
                else:
                    blk.abandon()
            else:
                is_orphan = False
                if ext_sci_path is not None:
                    assert ctx.out_sci is not None
                    ctx.out_sci.pending.append(
                        (ext_sci_path, blk[0], closing_line(blk))
                    )
            block.extend(blk)
        # A block with no code line left is an orphan whole: protoc refuses
        # an empty extend block (spec 0459 S1).
        no_code = not any(line.type == CODE and line.text for line in block.lines)
        bracket_type = ORPHAN if (track_orphans and is_orphan) or no_code else CODE
        block.insert(0, BlockLine(f'extend {ref} {{', depth, type=bracket_type))
        block.append(BlockLine('}', level=depth, type=bracket_type))
        out.extend(block)

    return out


class ReDescriptorProto(SourceCodeInfoMixin, NodeBase[DescriptorProto]):
    """Redescriptor for DescriptorProto (message descriptors)."""

    # Fields stripped from the FDP before pool_db.Add() because their
    # type_name was not resolvable (spec 0087).  Populated by phase 3.
    # Typed as list[Any] here to avoid a circular import with phases.py;
    # elements are StrippedField instances at runtime.
    stripped_fields: list[Any]

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
    def is_synthetic_group(self) -> bool:
        """A group made for an editions DELIMITED field under
        --force-proto2-output: the graph's alone, not the pool's (protoback's
        `synthetic_group`)."""
        return getattr(self, '_is_synthetic_group', False)

    @is_synthetic_group.setter
    def is_synthetic_group(self, value: bool) -> None:
        self._is_synthetic_group = value

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

    def render_extensions(
        self, ctx: Context, depth: int = 0, sci_path: list[int] | None = None,
    ) -> Block:
        """Render message extensions grouped by extendee."""
        return _render_extend_blocks(
            ctx=ctx,
            owner=self,
            extensions=self.extension,
            depth=depth,
            anomaly_code='B1',
            track_orphans=False,
            sci_path=sci_path,
        )

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
                # An extension range's options are named as its message's.
                out.extend(range_obj.render(ctx, depth, scope=option_scope(self)))

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
            self.contains.add(extension)

            extendee = ReDescriptorProto.from_ref(ctx, Ref(extension.extendee))
            extension.targets.add(extendee)

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

        # Editions DELIMITED → proto2 group: assign synthetic group names and
        # register group descriptors in the pool before processing fields.
        # This mirrors what happens for native proto2 TYPE_GROUP fields, where
        # the group type is a distinct nested_type entry with its own FQDN.
        # We only do this under --force-proto2-output (target_syntax == "proto2")
        # for editions files.
        _delimited_groups: dict[int, tuple[str, str]] = {}  # field.number → (synthetic FQDN, group name)
        # Use the FDP's own edition field to detect editions files — ctx.syntax
        # is set per-file at render time (re_file.py) and is not yet reliable
        # during pool construction.
        from .re_file import ReFileDescriptorProto as _ReFile
        _node: NodeBase[Any] = self
        while not isinstance(_node, _ReFile):
            assert _node._parent is not None
            _node = _node._parent
        _is_editions_file = _node.this.HasField('edition')
        if _is_editions_file and ctx.force_proto2_output:
            from .feature_resolution import MESSAGE_ENCODING_DELIMITED

            # Collect names already in scope: nested message and enum names.
            occupied: set[str] = set()
            for n in self.nested_type:
                occupied.add(cast(DescriptorProto, n).name)
            for e in self.enum_type:
                occupied.add(cast(EnumDescriptorProto, e).name)

            for f in self.field:
                field_proto = cast(FieldDescriptorProto, f)
                if field_proto.type != FieldDescriptorProto.TYPE_MESSAGE:
                    continue
                field_features = _resolve_field_features(
                    ctx, _node.this, _message_chain(self), field_proto)
                if (field_features is None
                        or field_features.message_encoding != MESSAGE_ENCODING_DELIMITED):
                    continue

                # Derive group name: CamelCase of field name, disambiguated.
                raw = ''.join(
                    part.capitalize() for part in field_proto.name.split('_') if part
                )
                candidate = raw
                suffix = 2
                while candidate in occupied:
                    candidate = f'{raw}{suffix}'
                    suffix += 1
                occupied.add(candidate)

                # Synthetic FQDN for the group type.
                synthetic_fqdn = Fqdn(f'{self.prefix}.{candidate}')
                assert ctx.find_node(
                    ReDescriptorProto.fqdn_from_ref(Ref(synthetic_fqdn))
                ) is None, (
                    f"Synthetic group FQDN {synthetic_fqdn!r} already in pool — "
                    f"input descriptor is malformed"
                )

                # Resolve the shared Inner descriptor (already registered).
                inner = ReDescriptorProto.from_ref(ctx, Ref(field_proto.type_name))

                # Register a stub under the synthetic FQDN.
                grp = ReDescriptorProto(ctx, Ref(synthetic_fqdn))
                # Set prefix so _initialize_from_message scopes child FQDNs correctly.
                from .fake_types import Prefix as _Prefix2
                grp.prefix = _Prefix2(synthetic_fqdn)
                # Set _this so that property accessors (self.extension etc.) work
                # inside _initialize_from_message.
                grp._this = inner.this
                # Set _parent so child FQDNs are scoped to the group, not Inner.
                grp._parent = inner._parent
                # Populate the group descriptor from the same DescriptorProto as Inner.
                # Safety: _initialize_from_message creates new Re* wrappers for each
                # child (registered under the group's FQDN scope), so the group and
                # Inner subtrees are independent Re* object graphs.  The only sharing
                # is of the read-only underlying DescriptorProto/_this objects.
                grp._initialize_from_message(ctx, inner.this)
                grp.is_group = True
                grp.is_synthetic_group = True

                # Store (synthetic FQDN, group name) keyed by field number.
                _delimited_groups[field_proto.number] = (synthetic_fqdn, candidate)

        # Message fields
        for f in self.field:
            field_proto = cast(FieldDescriptorProto, f)
            fd = ReFieldDescriptorProto(ctx, field_proto, parent=self)
            # For editions DELIMITED fields under --force-proto2-output, attach
            # the synthetic group descriptor and name so rendering uses the group path.
            if field_proto.number in _delimited_groups:
                group_fqdn, group_name = _delimited_groups[field_proto.number]
                grp_desc = ctx.find_node(
                    ReDescriptorProto.fqdn_from_ref(Ref(group_fqdn)))
                assert isinstance(grp_desc, ReDescriptorProto)
                fd._editions_group_descriptor = grp_desc
                fd._editions_group_name = group_name
                # The synthetic group must be in the field's targets so the
                # reachability propagation in phases.py marks it reachable/summoned.
                fd.targets.add(grp_desc)
            self.targets.add(fd)
            self.contains.add(fd)
    
    @descriptor_rerender_muted
    def render(
        self,
        ctx: Context,
        depth: int = 0,
        force: bool = False,
        sci_path: list[int] | None = None,
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

        See docs/reproto/INSIGHTS.md for detailed investigation and proof.

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
            sci_path: This message's own SourceCodeInfo path (spec 0141),
                as computed by the caller; used as the prefix for nested
                messages' own paths. None if source-info synthesis is off.

        Returns:
            Tuple of (output_block, inputs_block) containing rendered proto
        """
        from .re_enum import ReEnumDescriptorProto
        from .re_field import ReFieldDescriptorProto
        from .utils import get_file_node

        assert isinstance(depth, int)
        # The children's text rendered with no slot, once; the descriptor
        # built after it, from muted re-renders (spec 0429 S11).
        outer_slot, ctx.out_desc = ctx.out_desc, None
        out = Block()
        inputs = Block()

        fdp = get_file_node(self)

        # --- Message comments -------------------------------------------------
        comments_block = self.render_message_comments(depth+1)
        if comments_block:
            out.extend(comments_block)
            out.append_div_maybe(depth)

        # --- Message extensions -----------------------------------------------
        extensions_block = self.render_extensions(ctx, depth+1, sci_path)
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
                ctx, self.this.options.features, depth + 1, inline=False,
                scope=option_scope(self))
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

        # --- Orphan fields (stripped unresolvable types, spec 0087) -----------
        for sf in self.stripped_fields:
            out.append(BlockLine(
                f'{sf.label} {sf.type_name} {sf.name} = {sf.number};',
                depth + 1, ORPHAN,
            ))
        if self.stripped_fields:
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
                _resolve_field_features(ctx, fdp.this, _message_chain(self),
                                        cast(FieldDescriptorProto, members[0]))
                if members else None
            )
            if is_synthetic_oneof(ctx, oneof.name, members, features=member_features):
                synthetic_oneof_indices.add(idx)

        # Track which real oneofs we've already rendered
        is_done: list[bool] = [False] * len(self.oneof_decl)

        # f_idx (spec 0141): raw enumerate() position over self.field. The
        # binary side-channel's own field-reconstruction loop (below) is
        # unconditional, so this matches the resulting msg_out.field index
        # regardless of oneof grouping/summoned status.
        for f_idx, f in enumerate(self.field):
            field_proto = cast(FieldDescriptorProto, f)
            fd = ReFieldDescriptorProto(ctx, field_proto, parent=self)

            # Non-oneof field (including synthetic-oneof members): render directly
            if (not field_proto.HasField("oneof_index")
                    or fd.oneof_index in synthetic_oneof_indices):
                field_sci_path: list[int] | None = None
                if ctx.out_sci is not None and sci_path is not None and fd.is_summoned:
                    field_sci_path = sci_path + [2, f_idx]
                field = fd.render(ctx, depth+1)
                if field_sci_path is not None:
                    assert ctx.out_sci is not None
                    ctx.out_sci.pending.append(
                        (field_sci_path, field[0], closing_line(field))
                    )
                if not force and not fd.is_summoned:
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

            for f2_idx, f2 in enumerate(self.field):
                if (f2.HasField("oneof_index")
                    and f2.oneof_index == fd.oneof_index):
                    field_proto2 = cast(FieldDescriptorProto, f2)
                    fd2 = ReFieldDescriptorProto(ctx, field_proto2, parent=self)
                    field2_sci_path: list[int] | None = None
                    if (ctx.out_sci is not None and sci_path is not None
                            and fd2.is_summoned):
                        field2_sci_path = sci_path + [2, f2_idx]
                    field = fd2.render(ctx, depth+2, is_oneof=True)
                    if field2_sci_path is not None:
                        assert ctx.out_sci is not None
                        ctx.out_sci.pending.append(
                            (field2_sci_path, field[0], closing_line(field))
                        )
                    if fd2.is_summoned:
                        is_orphan = False
                    else:
                        field.abandon()
                    block.extend(field)

            # Emit oneof features overrides at the top of the oneof block,
            # then its other options (spec 0424 S1).
            oneof_feat_block = Block()
            if oneof.HasField('options') and oneof.options.HasField('features'):
                from .syntax import render_features_block
                # A oneof's options are named from its message.
                oneof_feat_block = render_features_block(
                    ctx, oneof.options.features, depth + 2, inline=False,
                    scope=scope_name(self))
            if oneof.HasField('options'):
                from .option_messages import parse_options
                from .option_rendering import render_options_from_message
                oneof_opts = parse_options(
                    ctx, ctx.ooo_desc, ctx.ooo_cls,
                    oneof.options.SerializeToString())
                for opt_block in render_options_from_message(
                        ctx, oneof_opts, ctx.ooo_desc, depth=depth + 2,
                        scope=scope_name(self)):
                    oneof_feat_block.extend(opt_block)
            block.insert(0, BlockLine(f'oneof {oneof.name} {{', depth+1,
                         type=ORPHAN if is_orphan else CODE))
            for feat_line in reversed(list(oneof_feat_block)):
                block.insert(1, feat_line)
            block.append(BlockLine('}', level=depth+1,
                         type=ORPHAN if is_orphan else CODE))
            out.extend(block)
        out.append_div_maybe(depth)

        # --- Message nested messages ------------------------------------------
        # Path index (spec 0141) is the raw enumerate() position: unlike file-
        # level messages, the binary side-channel below re-accumulates *every*
        # self.nested_type entry unconditionally (see the nested-types loop
        # further down), so no is_group/is_map_entry/is_summoned filtering is
        # needed to keep indices aligned with the output nested_type list.
        for n_idx, n in enumerate(self.nested_type):
            nested_proto = cast(DescriptorProto, n)
            nested = ReDescriptorProto(ctx, nested_proto, parent=self)
            # Skip groups (rendered inline) and map entries (rendered as map<K,V>)
            if nested.is_group or nested.is_map_entry:
                continue
            nested_sci_path: list[int] | None = None
            if ctx.out_sci is not None and nested.is_summoned and sci_path is not None:
                nested_sci_path = sci_path + [3, n_idx]
            text, inp = nested.render(ctx, depth+1, sci_path=nested_sci_path)
            from .syntax import visibility_prefix
            text.insert(0, BlockLine(
                f'{visibility_prefix(ctx, nested.this)}message {nested.name} {{', depth+1))
            if text[1].text == '}':  # body is empty: collapse to one line
                text[0].postpend('}')
                text.pop(1)
            if nested_sci_path is not None:
                assert ctx.out_sci is not None
                ctx.out_sci.pending.append(
                    (nested_sci_path, text[0], closing_line(text))
                )
            if not nested.is_summoned:
                text.abandon()
            else:
                inputs.extend(inp)
            out.extend(text)
        out.append_div_maybe(depth)

        # --- Message enums ----------------------------------------------------
        # e_idx (spec 0141): raw enumerate() position, matching the binary
        # side-channel's own unconditional enum-reconstruction loop (below).
        for e_idx, e in enumerate(self.enum_type):
            enum_proto = cast(EnumDescriptorProto, e)
            enum = ReEnumDescriptorProto(ctx, enum_proto, parent=self)
            enum_sci_path: list[int] | None = None
            if ctx.out_sci is not None and sci_path is not None:
                enum_sci_path = sci_path + [4, e_idx]
            lines = enum.render(ctx, depth+1, sci_path=enum_sci_path)
            if enum_sci_path is not None:
                assert ctx.out_sci is not None
                ctx.out_sci.pending.append(
                    (enum_sci_path, lines[0], closing_line(lines))
                )
            out.extend(lines)
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

        # --- Binary output side-channel (spec 0076) ---------------------------
        if outer_slot is not None:
            from google.protobuf.descriptor_pb2 import (
                DescriptorProto as _DP,
                FieldDescriptorProto as _FieldDP,
            )
            from .context import DescOut
            from .re_enum import ReEnumDescriptorProto as _ReEnum
            from .re_field import ReFieldDescriptorProto as _ReField
            msg_out = _DP()
            msg_out.CopyFrom(self.this)
            if ctx.target_syntax != "editions":
                msg_out.ClearField('visibility')
            # options.features: clear if not editions target
            if msg_out.HasField('options') and ctx.target_syntax != "editions":
                msg_out.options.ClearField('features')
            # oneofs: fix options.features in-place (already copied)
            for oo in msg_out.oneof_decl:
                if oo.HasField('options') and ctx.target_syntax != "editions":
                    oo.options.ClearField('features')
            # children: re-accumulate via render() to respect summoning/pruning
            # (oneof_decl is kept from CopyFrom — features already fixed above)
            msg_out.ClearField('field')
            msg_out.ClearField('nested_type')
            msg_out.ClearField('enum_type')
            msg_out.ClearField('extension')
            # fields
            for f in self.field:
                field_proto = cast(_FieldDP, f)
                fd = _ReField(ctx, field_proto, parent=self)
                slot = DescOut()
                ctx.out_desc = slot
                fd.render(ctx, depth + 1)
                ctx.out_desc = None
                if slot.out is not None:
                    assert isinstance(slot.out, _FieldDP)
                    msg_out.field.append(slot.out)
            # Written as proto2, a proto3 message loses its synthetic oneofs
            # (its fields left them above), the real ones renumbered (spec
            # 0429 S13).
            if ctx.target_syntax != "proto3":
                kept = [i for i in range(len(self.oneof_decl))
                        if i not in synthetic_oneof_indices]
                if len(kept) != len(self.oneof_decl):
                    renumber = {old: new for new, old in enumerate(kept)}
                    oneofs = [msg_out.oneof_decl[i] for i in kept]
                    msg_out.ClearField('oneof_decl')
                    msg_out.oneof_decl.extend(oneofs)
                    for out_field in msg_out.field:
                        if out_field.HasField('oneof_index'):
                            out_field.oneof_index = renumber[out_field.oneof_index]
            # nested types (including map entries)
            for n in self.nested_type:
                nested_proto = cast(_DP, n)
                nested = ReDescriptorProto(ctx, nested_proto, parent=self)
                slot = DescOut()
                ctx.out_desc = slot
                nested.render(ctx, depth + 1)
                ctx.out_desc = None
                if slot.out is not None:
                    assert isinstance(slot.out, _DP)
                    msg_out.nested_type.append(slot.out)
            # enums
            for e in self.enum_type:
                enum_proto = cast(EnumDescriptorProto, e)
                enum = _ReEnum(ctx, enum_proto, parent=self)
                slot = DescOut()
                ctx.out_desc = slot
                enum.render(ctx, depth + 1)
                ctx.out_desc = None
                if slot.out is not None:
                    assert isinstance(slot.out, EnumDescriptorProto)
                    msg_out.enum_type.append(slot.out)
            # extensions
            for e in self.extension:
                ext_proto = cast(_FieldDP, e)
                fd = _ReField(ctx, ext_proto, parent=self)
                slot = DescOut()
                ctx.out_desc = slot
                fd.render(ctx, depth + 1)
                ctx.out_desc = None
                if slot.out is not None:
                    assert isinstance(slot.out, _FieldDP)
                    msg_out.extension.append(slot.out)
            ctx.out_desc = outer_slot
            ctx.out_desc.out = msg_out

        return out, inputs
