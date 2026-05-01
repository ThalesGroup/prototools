# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""ReFieldDescriptorProto - Field descriptor redescriptor."""

from __future__ import annotations

from typing import cast

from google.protobuf.descriptor import FieldDescriptor
from google.protobuf.descriptor_pb2 import FieldDescriptorProto, FieldOptions
from google.protobuf.message import Message

from .base import NodeBase
from .context import Context, Fqdn
from .fake_types import Ref, parse_fqdn
from .globals import FIELD, type_names
from .text import Block, BlockLine


class ReFieldDescriptorProto(NodeBase[FieldDescriptorProto]):
    """Redescriptor for FieldDescriptorProto."""

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
    def parent(self, value) -> None:
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
        **kwargs
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
            case FieldDescriptorProto.TYPE_ENUM:
                enum = ReEnumDescriptorProto.from_ref(ctx, Ref(self.type_name))
                self.targets.add(enum)
                self.type_descriptor = enum
            case _:
                self.type_descriptor = self.type

    def _render_map_field(self, ctx: Context, depth: int) -> Block:
        """Render a map field as map<K,V> syntax."""
        from .re_descriptor import ReDescriptorProto

        out = Block()

        # Get the map entry message
        map_entry_msg = self.type_descriptor
        assert isinstance(map_entry_msg, ReDescriptorProto)
        assert map_entry_msg.is_map_entry, "Expected map entry message"

        # Map entry messages have exactly 2 fields: key (field 1) and value (field 2)
        # Extract key and value types
        key_field = None
        value_field = None

        for field in map_entry_msg.field:
            if field.number == 1:
                key_field = field
            elif field.number == 2:
                value_field = field

        # Validate canonical map entry structure
        if key_field is None or value_field is None:
            from .anomalies import report
            found = [f.number for f in map_entry_msg.field]
            out.append(report("C1", depth,
                               field=self.name, entry=map_entry_msg.name, found=found))
            # Fallback: render as repeated message instead of map
            from .utils import short_ref
            ref = short_ref(ctx, self.type_descriptor, self.parent)
            string = f'repeated {ref} {self.name} = {self.number};'
            out.append(BlockLine(string, depth))
            return out

        # Get type names for key and value
        # For primitive types, use the type name directly
        # For message/enum types, use short_ref to get the type name
        key_type = self._get_field_type_name(ctx, key_field)
        value_type = self._get_field_type_name(ctx, value_field)

        # Render as: map<key_type, value_type> field_name = number;
        string = f'map<{key_type}, {value_type}> {self.name} = {self.number};'
        out.append(BlockLine(string, depth))

        return out

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
        from .utils import short_ref

        assert isinstance(depth, int)
        assert isinstance(is_oneof, bool)

        out = Block()

        # --- Map field detection ----------------------------------------------
        # Map fields are:
        # - TYPE_MESSAGE with LABEL_REPEATED
        # - The message type has options.map_entry = true
        # - Rendered as: map<key_type, value_type> field_name = number;
        if (self.type == FieldDescriptorProto.TYPE_MESSAGE and
            self.label == FieldDescriptorProto.LABEL_REPEATED):
            # Check if the message is a map entry
            from .re_descriptor import ReDescriptorProto
            if isinstance(self.type_descriptor, ReDescriptorProto) and self.type_descriptor.is_map_entry:
                # Render map field
                out.extend(self._render_map_field(ctx, depth))
                return out

        string = ''

        # --- Field label (aka cardinality) ------------------------------------
        from .syntax import field_label
        from google.protobuf.descriptor_pb2 import FieldDescriptorProto as _FDP
        if (ctx.target_syntax == "proto3"
                and not is_oneof
                and self.this.label == _FDP.LABEL_REQUIRED):
            from .anomalies import report
            out.append(report("C3", depth, name=self.name))
        label_str = field_label(ctx, self.this, is_oneof)
        string += label_str

        # --- Field type and name ----------------------------------------------
        from .syntax import allow_groups
        if self.type != FieldDescriptorProto.TYPE_GROUP or not allow_groups(ctx):
            if self.type == FieldDescriptorProto.TYPE_GROUP:
                from .anomalies import report
                out.append(report("C2", depth, name=self.name))
            ref = short_ref(ctx, self.type_descriptor, self.parent)
            string += f'{ref} {self.name}'
        else:
            ref = short_ref(ctx, self.type_descriptor, self)
            string += f'group {ref}'

        # --- Field number -----------------------------------------------------
        string += f' = {self.number}'

        # --- Field options ----------------------------------------------------
        try:
            # Look up the target field descriptor
            # Extension fields and regular fields use different lookup methods
            if self.this.HasField('extendee'):
                # Extension field: look up by extendee and number
                extendee_desc = ctx.pool.FindMessageTypeByName(self.extendee[1:])  # Strip leading '.'
                field_desc: FieldDescriptor = ctx.pool.FindExtensionByNumber(extendee_desc, self.number)
            else:
                # Regular field: look up by full name
                # fqdn format: "fdsc:.package.MessageName.fieldName"
                # FindFieldByName expects: "package.MessageName.fieldName" (no leading dot)
                prefix, ref = parse_fqdn(self.fqdn)
                if prefix != FIELD:
                    raise ValueError(f"Expected field FQDN, got: {self.fqdn}")
                # parse_fqdn returns ref with leading dot (e.g., '.package.MessageName.fieldName')
                # Strip the dot for pool lookup
                full_type_name = ref.lstrip('.')
                field_desc: FieldDescriptor = ctx.pool.FindFieldByName(full_type_name)

            # Create a dynamic FieldOptions instance and parse serialized data
            fo_msg: Message = ctx.fdo_cls()
            fo_msg.ParseFromString(field_desc.GetOptions().SerializeToString())


            # Build options block: default value + json_name + field options
            opt_block = Block()
            prolog = BlockLine(string, depth)

            # Add default_value first (proto2 only — proto3 forbids explicit defaults).
            # Note: default_value is a direct field of FieldDescriptorProto,
            # NOT a field inside the FieldOptions message. We handle it separately
            # and combine it with FieldOptions in the composite format [default = x, ...].
            from .syntax import should_render_default
            if should_render_default(ctx, self.this):
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
            effective_packed = cast(FieldOptions, fo_msg).packed
            packed_str = packed_option(ctx, has_packed, effective_packed)
            if packed_str is not None:
                opt_block.append(BlockLine(f'packed = {packed_str},', depth + 1))

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
            out.append(report("C5", depth, name=self.name,
                               exc_type=type(e).__name__, exc_msg=str(e)))
            out.append(BlockLine(string, depth))

        # --- Field group definition (only for fields of type group) -----------
        if self.type != FieldDescriptorProto.TYPE_GROUP or not allow_groups(ctx):
            out.postpend(';')
        else:
            # Groups definitions must be inlined
            from .re_descriptor import ReDescriptorProto
            
            out.postpend(' {')
            re_desc = ReDescriptorProto.from_ref(ctx, Ref(self.type_name))
            block, _ = re_desc.render(ctx, depth, force=True)
            out.extend(block)

        if any(t for t in self.targets if not t.is_summoned):
            out.abandon()
        
        return out
