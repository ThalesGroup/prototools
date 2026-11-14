# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""ReFieldDescriptorProto - Field descriptor redescriptor."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from google.protobuf.descriptor_pb2 import DescriptorProto, FieldDescriptorProto, FileDescriptorProto
from google.protobuf.message import Message

from .base import NodeBase
from .context import Context, Fqdn

if TYPE_CHECKING:
    from .re_descriptor import ReDescriptorProto
    from .re_file import ReFileDescriptorProto
from .fake_types import Ref
from .feature_resolution import ResolvedFeatures, resolve_features
from .globals import FIELD, type_names
from .option_messages import parse_options
from .text import Block, BlockLine


def _get_file_and_msg(
    node: 'ReFieldDescriptorProto',
) -> 'tuple[ReFileDescriptorProto, ReDescriptorProto | None]':
    """Return (file_node, message_node_or_None) for a field node."""
    from .re_descriptor import ReDescriptorProto
    from .re_file import ReFileDescriptorProto
    from .utils import get_file_node
    parent = node.parent
    if isinstance(parent, ReFileDescriptorProto):
        return parent, None
    assert isinstance(parent, ReDescriptorProto)
    return get_file_node(parent), parent


def _message_chain(node: 'ReDescriptorProto | None') -> list[DescriptorProto]:
    """The messages enclosing a field of `node`, outermost first, `node`'s
    own last; empty for a top-level extension (node None)."""
    from .re_descriptor import ReDescriptorProto
    chain: list[DescriptorProto] = []
    current: object = node
    while isinstance(current, ReDescriptorProto):
        chain.append(current.this)
        current = current._parent
    chain.reverse()
    return chain


def _resolve_field_features(
    ctx: Context,
    fdp: FileDescriptorProto,
    messages: list[DescriptorProto],
    field_proto: FieldDescriptorProto,
) -> ResolvedFeatures | None:
    """Resolve ResolvedFeatures for a field.  Returns None for proto2/proto3.

    Features flow from the file through every enclosing message, outermost
    first (`messages`, from `_message_chain`), to the field, as protoc and
    upb resolve them (spec 0424 S1).
    """
    # Use the FDP's own edition field to detect editions files — ctx.syntax is
    # set per-file at render time and is not reliable during pool construction.
    if not fdp.HasField('edition'):
        return None
    file_fs = fdp.options.features if fdp.options.HasField('features') else None
    msg_fss = [m.options.features if m.options.HasField('features') else None
               for m in messages]
    field_fs = field_proto.options.features if field_proto.options.HasField('features') else None
    return resolve_features(
        ctx.edition_defaults, fdp.edition, file_fs, *msg_fss, field_fs)


class ReFieldDescriptorProto(NodeBase[FieldDescriptorProto]):
    """Redescriptor for FieldDescriptorProto."""

    # Set by re_descriptor._initialize_from_message under --force-proto2-output
    # when the field uses editions DELIMITED message encoding and must be rendered
    # as a proto2 group.
    _editions_group_descriptor: 'ReDescriptorProto | None'
    _editions_group_name: str | None

    @property
    def extendee(self) -> str:
        return self.this.extendee

    @property
    def label(self) -> int:
        return self.this.label

    @property
    def number(self) -> int:
        return self.this.number

    @property
    def oneof_index(self) -> int:
        return self.this.oneof_index

    @property
    def parent(self):
        from .re_descriptor import ReDescriptorProto
        from .re_file import ReFileDescriptorProto
        
        assert isinstance(self._parent, (ReDescriptorProto, ReFileDescriptorProto))
        return self._parent

    @parent.setter
    def parent(self, value: NodeBase[Any]) -> None:
        self._parent = value

    @property
    def type(self) -> int:
        return self.this.type

    @property
    def type_name(self) -> str:
        return self.this.type_name

    def short_type_name(self, ctx: Context, name: str = '') -> str:
        from .utils import shorten_type_name
        return shorten_type_name(ctx, self, name)

    @classmethod
    def fqdn_from_ref(cls, ref: Ref) -> Fqdn:
        return Fqdn(f'{FIELD}:{str(ref)}')
    
    def _initialize_from_message(
        self,
        ctx: Context,
        message: FieldDescriptorProto,
        **kwargs: Any,
    ) -> None:
        """Initialize field-specific attributes and resolve type references."""
        # Lazy imports
        from .re_descriptor import ReDescriptorProto
        from .re_enum import ReEnumDescriptorProto
        
        match self.type:
            case FieldDescriptorProto.TYPE_GROUP:
                grp = ReDescriptorProto.from_ref(ctx, Ref(self.type_name))
                from .syntax import allow_groups
                if allow_groups(ctx):
                    grp.is_group = True
                self.targets.add(grp)
                self.type_descriptor = grp
            case FieldDescriptorProto.TYPE_MESSAGE:
                msg = ReDescriptorProto.from_ref(ctx, Ref(self.type_name))
                self.targets.add(msg)
                self.type_descriptor = msg
                # Editions DELIMITED under --force-proto2-output: use the
                # synthetic group descriptor registered by re_descriptor.py.
                # _editions_group_descriptor is set on the field by the naming
                # pass before _initialize_from_message is called.
                grp = getattr(self, '_editions_group_descriptor', None)
                if grp is not None:
                    self.targets.add(grp)
                    self.type_descriptor = grp
            case FieldDescriptorProto.TYPE_ENUM:
                enum = ReEnumDescriptorProto.from_ref(ctx, Ref(self.type_name))
                self.targets.add(enum)
                self.type_descriptor = enum
            case _:
                self.type_descriptor = self.type

    def _map_field_string(
        self, ctx: Context, depth: int
    ) -> tuple[str, Block | None]:
        """Return (declaration_string, anomaly_block_or_None) for a map field.

        The declaration string does not include a trailing semicolon; the
        caller's shared options-rendering path (and out.postpend(';')) handles
        that.  If the map entry is malformed, returns an empty string and a
        Block containing the anomaly report and a repeated-message fallback
        line (the caller should extend out with that block and return early).
        """
        from .re_descriptor import ReDescriptorProto

        map_entry_msg = self.type_descriptor
        assert isinstance(map_entry_msg, ReDescriptorProto)
        assert map_entry_msg.is_map_entry, "Expected map entry message"

        key_field = None
        value_field = None
        for field in map_entry_msg.field:
            if field.number == 1:
                key_field = field
            elif field.number == 2:
                value_field = field

        if key_field is None or value_field is None:
            from .anomalies import report
            from .utils import short_ref
            found = [f.number for f in map_entry_msg.field]
            anomaly = Block()
            anomaly.append(report("C1", depth,
                                   field=self.name, entry=map_entry_msg.name, found=found))
            ref = short_ref(ctx, self.type_descriptor, self.parent)
            anomaly.append(BlockLine(f'repeated {ref} {self.name} = {self.number};', depth))
            return '', anomaly

        key_type = self._get_field_type_name(ctx, key_field)
        value_type = self._get_field_type_name(ctx, value_field)
        return f'map<{key_type}, {value_type}> {self.name} = {self.number}', None

    def _get_field_type_name(self, ctx: Context, field: FieldDescriptorProto) -> str:
        """Get the type name for a field (used in map<K,V> rendering)."""
        from .re_descriptor import ReDescriptorProto
        from .re_enum import ReEnumDescriptorProto
        from .utils import short_ref

        # Handle complex types that need reference resolution
        if field.type == FieldDescriptorProto.TYPE_MESSAGE:
            # For message types, get reference and shorten relative to current scope
            msg = ReDescriptorProto.from_ref(ctx, Ref(field.type_name))
            return short_ref(ctx, msg, self.parent)
        elif field.type == FieldDescriptorProto.TYPE_ENUM:
            # For enum types, get reference and shorten relative to current scope
            enum = ReEnumDescriptorProto.from_ref(ctx, Ref(field.type_name))
            return short_ref(ctx, enum, self.parent)
        elif field.type in type_names:
            # Primitive types: use name from globals (double, float, int32, etc.)
            return type_names[field.type]
        else:
            return f'<unknown type {field.type}>'

    def _render_default_value(self, depth: int = 0) -> Block:
        """
        Render default value for proto2 fields.

        Returns a Block containing the default value option line, or empty Block if no default.
        Default values are rendered as: default = <value>,
        """
        from .scalar import Scalar

        out = Block()

        # Check if field has a default value set
        # Note: We only check HasField, not the value itself, because empty strings/bytes
        # are valid default values that must be rendered
        if not self.this.HasField('default_value'):
            return out

        # For enum types, default_value contains the enum value name (not FQDN)
        # For other types, it contains the literal value as a string
        is_enum = (self.type == FieldDescriptorProto.TYPE_ENUM)

        # Parse the default value based on field type
        if is_enum:
            # Enum: use value as-is (it's the enum value name)
            scalar = Scalar(self.this.default_value, is_enum=True)
        elif self.type == FieldDescriptorProto.TYPE_STRING:
            # String: use value as-is (Scalar will quote it)
            scalar = Scalar(self.this.default_value)
        elif self.type == FieldDescriptorProto.TYPE_BYTES:
            # Bytes: default_value contains C-style octal escapes like "\\000\\377"
            # We need to decode these escape sequences to get actual bytes
            import codecs
            decoded_str = codecs.decode(self.this.default_value, 'unicode-escape')
            scalar = Scalar(decoded_str.encode('latin1'))
        elif self.type == FieldDescriptorProto.TYPE_BOOL:
            # Bool: parse "true"/"false" to boolean
            scalar = Scalar(self.this.default_value.lower() == 'true')
        elif self.type in (FieldDescriptorProto.TYPE_FLOAT, FieldDescriptorProto.TYPE_DOUBLE):
            # Float/Double: parse as float
            scalar = Scalar(float(self.this.default_value))
        else:
            # Integer types: parse as int
            scalar = Scalar(int(self.this.default_value))

        out.append(BlockLine(f'default = {scalar},', depth))
        return out

    def render(self, ctx: Context, depth: int = 0, is_oneof: bool = False) -> Block:
        """Reconstruct field as .proto."""
        from .syntax import allow_groups
        from .utils import short_ref

        assert isinstance(depth, int)
        assert isinstance(is_oneof, bool)

        out = Block()

        # --- Map field detection ----------------------------------------------
        # Map fields are TYPE_MESSAGE + LABEL_REPEATED where the target message
        # has map_entry = true.  We build the declaration string here and fall
        # through to the shared options-rendering block below so that field
        # options (including custom extensions) are not silently dropped.
        is_map_field = False
        map_string = ''
        if (self.type == FieldDescriptorProto.TYPE_MESSAGE and
                self.label == FieldDescriptorProto.LABEL_REPEATED):
            from .re_descriptor import ReDescriptorProto
            if (isinstance(self.type_descriptor, ReDescriptorProto)
                    and self.type_descriptor.is_present()
                    and self.type_descriptor.is_map_entry):
                is_map_field = True
                map_string, anomaly = self._map_field_string(ctx, depth)
                if anomaly is not None:
                    # Malformed map entry: emit anomaly + fallback and bail out
                    out.extend(anomaly)
                    return out

        # Resolve per-element features (None for proto2/proto3 files).
        file_node, msg_node = _get_file_and_msg(self)
        field_features = _resolve_field_features(
            ctx,
            file_node.this,
            _message_chain(msg_node),
            self.this,
        )

        string = ''
        if is_map_field:
            string = map_string

        if not is_map_field:
            # --- Field label (aka cardinality) --------------------------------
            from .syntax import field_label
            from google.protobuf.descriptor_pb2 import FieldDescriptorProto as _FDP
            if (ctx.target_syntax == "proto3"
                    and not is_oneof
                    and self.this.label == _FDP.LABEL_REQUIRED):
                from .anomalies import report
                out.append(report("C3", depth, name=self.name))
            label_str = field_label(ctx, self.this, is_oneof, features=field_features)
            string += label_str

            # --- Field type and name ------------------------------------------
            _is_group_field = (
                self.type == FieldDescriptorProto.TYPE_GROUP
                or getattr(self, '_editions_group_descriptor', None) is not None
            )
            if not _is_group_field or not allow_groups(ctx, features=field_features):
                if self.type == FieldDescriptorProto.TYPE_GROUP:
                    from .anomalies import report
                    out.append(report("C2", depth, name=self.name))
                ref = short_ref(ctx, self.type_descriptor, self.parent)
                string += f'{ref} {self.name}'
            else:
                editions_grp_name = getattr(self, '_editions_group_name', None)
                ref = editions_grp_name if editions_grp_name is not None else short_ref(ctx, self.type_descriptor, self)
                string += f'group {ref}'

            # --- Field number -------------------------------------------------
            string += f' = {self.number}'

        # --- Field options ----------------------------------------------------
        # Initialize here so the binary block below can safely reference them
        # even if the try block exits early via an exception.
        is_packable: bool = False
        has_packed: bool = False
        effective_packed: bool = False
        try:
            # Build a dynamic FieldOptions instance from the FDP's own options bytes.
            # We do not use a pool lookup (FindExtensionByNumber / FindFieldByName)
            # because for extension fields the pool lookup fails when the defining
            # file was not successfully added to the pool (e.g. missing transitive
            # deps).  The FDP options bytes are the canonical source of truth and
            # always match what GetOptions() would return, so going via the pool
            # is both unnecessary and fragile.
            fo_msg: Message = parse_options(
                ctx, ctx.fdo_desc, ctx.fdo_cls, self.this.options.SerializeToString())


            # Build options block: default value + json_name + field options
            opt_block = Block()
            prolog = BlockLine(string, depth)

            # Add default_value first (proto2 only — proto3 forbids explicit defaults).
            # Note: default_value is a direct field of FieldDescriptorProto,
            # NOT a field inside the FieldOptions message. We handle it separately
            # and combine it with FieldOptions in the composite format [default = x, ...].
            from .syntax import should_render_default
            if should_render_default(ctx, self.this, features=field_features):
                default_block = self._render_default_value(depth+1)
                opt_block.extend(default_block)
            elif ctx.target_syntax == "proto3" and self.this.HasField('default_value'):
                from .anomalies import report
                opt_block.append(report("C4", depth+1, name=self.name))

            # Add json_name only when it differs from the auto-derived camelCase
            # (spec 0019 §4/§16 — syntax-independent).
            from .syntax import should_render_json_name
            if should_render_json_name(self.this):
                opt_block.append(BlockLine(f'json_name = "{self.this.json_name}",', depth+1))

            # Compute packed annotation syntax-awaredly (spec 0016).
            # fo_msg is kept read-only; packed is excluded from generic rendering.
            from .syntax import packed_option
            has_packed = (self.this.HasField('options')
                          and self.this.options.HasField('packed'))
            # GetMessageClass returns a dynamic C-extension class (module=None, name='FieldOptions').
            # isinstance() against descriptor_pb2.FieldOptions always returns False for this class,
            # so we assert structural identity instead.
            assert type(fo_msg).__name__ == 'FieldOptions' and type(fo_msg).__module__ is None, (
                f"Expected dynamic FieldOptions from GetMessageClass, got {type(fo_msg)}"
            )
            # Compute effective_packed: reflects the actual wire-level packing,
            # including syntax defaults (proto3 implicit default is packed).
            is_repeated = self.this.label == FieldDescriptorProto.LABEL_REPEATED
            if has_packed:
                effective_packed = getattr(fo_msg, 'packed')
            elif ctx.syntax == "proto3":
                effective_packed = True
            else:
                effective_packed = False
            # Packed is only meaningful for repeated scalar (numeric) fields.
            # String, bytes, and message fields are not packable.
            from .globals import PACKABLE_TYPES
            is_packable = is_repeated and self.this.type in PACKABLE_TYPES
            # Under --force-proto2-output for editions files, protoc does not set
            # options.packed — it uses features.repeated_field_encoding instead.
            # Derive effective_packed from resolved features when available.
            if field_features is not None and not has_packed and is_packable:
                from .feature_resolution import REPEATED_FIELD_ENCODING_PACKED
                effective_packed = (
                    field_features.repeated_field_encoding == REPEATED_FIELD_ENCODING_PACKED
                )
            # Only call packed_option for packable fields (or fields with an
            # explicit packed option — which protoc would have rejected if invalid,
            # so we trust it and pass through).
            packed_str = packed_option(ctx, has_packed, effective_packed, features=field_features) if is_packable or has_packed else None
            if packed_str is not None:
                opt_block.append(BlockLine(f'packed = {packed_str},', depth + 1))

            # Emit features.X = Y overrides (editions only; no-op for proto2/proto3).
            from .syntax import render_features_block
            if self.this.HasField('options') and self.this.options.HasField('features'):
                feat_block = render_features_block(
                    ctx, self.this.options.features, depth + 1, inline=True)
                opt_block.extend(feat_block)

            # Render field options using inherited method (packed and json_name excluded).
            # json_name is a field of FieldDescriptorProto, not FieldOptions — handled above.
            option_blocks = self.render_options_from_message(
                ctx=ctx,
                opts_msg=fo_msg,
                options_descriptor=ctx.fdo_desc,
                composite=True,
                depth=depth+1,
                exclude={"packed", "json_name"},
            )
            for block in option_blocks:
                opt_block.extend(block)

            # Format composite options using helper
            formatted = self.format_composite_options(
                option_blocks=opt_block,
                prolog=prolog,
                depth=depth,
            )
            out.extend(formatted)
        except (KeyError, ValueError, TypeError, AttributeError) as e:
            from .anomalies import report
            out.append(report("C5", depth, name=self.name, file=ctx.current_file,
                               exc_type=type(e).__name__, exc_msg=str(e)))
            out.append(BlockLine(string, depth))

        # --- Field group definition (only for fields of type group) -----------
        _is_group_field = (
            self.type == FieldDescriptorProto.TYPE_GROUP
            or getattr(self, '_editions_group_descriptor', None) is not None
        )
        if is_map_field or not _is_group_field or not allow_groups(ctx, features=field_features):
            out.postpend(';')
        else:
            # Groups definitions must be inlined.
            # For editions DELIMITED fields the group descriptor is stored
            # directly on self.type_descriptor (set by _initialize_from_message).
            from .re_descriptor import ReDescriptorProto

            out.postpend(' {')
            editions_grp_desc = getattr(self, '_editions_group_descriptor', None)
            re_desc: ReDescriptorProto = (
                editions_grp_desc
                if editions_grp_desc is not None
                else ReDescriptorProto.from_ref(ctx, Ref(self.type_name))
            )
            block, _ = re_desc.render(ctx, depth, force=True)
            out.extend(block)

        if any(t for t in self.targets if not t.is_summoned):
            out.abandon()

        # --- Binary output side-channel (spec 0076) ---------------------------
        if ctx.out_desc is not None:
            from google.protobuf.descriptor_pb2 import FieldDescriptorProto as _FDP
            from .syntax import field_label_enum, should_render_default
            outer_slot = ctx.out_desc
            field_out = _FDP()
            field_out.CopyFrom(self.this)
            # type_name / extendee: canonize via variant namespace rules
            # (spec 0086), unless --keep-descriptor-path suppresses
            # namespace rewriting entirely (spec 0159 — matches the
            # existing precedent in utils.py's shorten_type_name/
            # parse_fqdn for the text-render path)
            if not ctx.keep_variant_descriptor:
                from .mappings import apply_variant_namespace
                from .fake_types import Ref as _Ref
                if field_out.type_name:
                    field_out.type_name = str(apply_variant_namespace(ctx, _Ref(field_out.type_name)))
                if field_out.extendee:
                    field_out.extendee = str(apply_variant_namespace(ctx, _Ref(field_out.extendee)))
            # label: translation-aware (also handles map field → LABEL_REPEATED)
            if is_map_field:
                field_out.label = _FDP.LABEL_REPEATED
            else:
                field_out.label = field_label_enum(
                    ctx, self.this, is_oneof, features=field_features)
            # type: for editions DELIMITED -> proto2 group substitution
            editions_grp_desc = getattr(self, '_editions_group_descriptor', None)
            if editions_grp_desc is not None and ctx.target_syntax != "editions":
                field_out.type = _FDP.TYPE_GROUP
                field_out.type_name = f'.{editions_grp_desc.prefix}'.rstrip('.')
            # packed
            if is_packable and (has_packed or effective_packed != (ctx.syntax == "proto3")):
                field_out.options.packed = effective_packed
            # default value: clear if not renderable
            if is_map_field or not should_render_default(
                    ctx, self.this, features=field_features):
                field_out.ClearField('default_value')
            # options.features: clear if not editions target
            if field_out.HasField('options') and ctx.target_syntax != "editions":
                field_out.options.ClearField('features')
            ctx.out_desc = outer_slot
            ctx.out_desc.out = field_out

        return out
