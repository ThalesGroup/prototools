# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import sys
from typing import TYPE_CHECKING, Any

from google.protobuf.descriptor import Descriptor
from google.protobuf.descriptor_database import DescriptorDatabase
from google.protobuf.descriptor_pb2 import DescriptorProto, FileDescriptorProto
from google.protobuf.descriptor_pool import DescriptorPool
from google.protobuf.message import Message
from google.protobuf.message_factory import GetMessageClass

from .lib.warnings import cli_error
from .context import Context

if TYPE_CHECKING:
    from google.protobuf.internal.extension_dict import _ExtensionFieldDescriptor


class _MapsAsRepeatedDatabase(DescriptorDatabase):
    """`ctx.pool_db`, with every message's `map_entry` option cleared.

    Option values are parsed with classes from a pool over this database
    (spec 0424 S1). Python's maps iterate in hash order, and keep no trace
    of the order their entries came in; protoc writes the entries of a map
    in an option value in the order of the source, so reproto must print
    them in wire order to compile back to the same bytes. Without
    `map_entry`, a map field is the repeated message it is on the wire,
    its entries in order, and its text format is the map's own:
    `counts: { key: ... value: ... }`. Files are copied lazily, as the
    pool asks for them.
    """

    def __init__(self, db: DescriptorDatabase) -> None:
        super().__init__()
        self._db = db
        self._files: dict[str, FileDescriptorProto] = {}

    def _without_map_entry(self, fdp: FileDescriptorProto) -> FileDescriptorProto:
        copy = self._files.get(fdp.name)
        if copy is None:
            copy = FileDescriptorProto()
            copy.CopyFrom(fdp)

            def clear(messages: list[DescriptorProto]) -> None:
                for m in messages:
                    if m.options.HasField('map_entry'):
                        m.options.ClearField('map_entry')
                        if not m.options.ListFields():
                            m.ClearField('options')
                    clear(list(m.nested_type))

            clear(list(copy.message_type))
            self._files[fdp.name] = copy
        return copy

    def FindFileByName(self, name: str) -> FileDescriptorProto:
        return self._without_map_entry(self._db.FindFileByName(name))

    def FindFileContainingSymbol(self, symbol: str) -> FileDescriptorProto:
        return self._without_map_entry(self._db.FindFileContainingSymbol(symbol))

    def FindFileContainingExtension(
        self, extendee_name: str, extension_number: int,
    ) -> FileDescriptorProto | None:
        fdp = self._db.FindFileContainingExtension(extendee_name, extension_number)
        return None if fdp is None else self._without_map_entry(fdp)

    def FindAllExtensionNumbers(self, extendee_name: str) -> list[int]:
        return self._db.FindAllExtensionNumbers(extendee_name)


def option_extensions(
    ctx: Context, options_desc: Descriptor,
) -> list[_ExtensionFieldDescriptor[Message, Any]]:
    """The extensions of an options type, from that type's own pool.

    `ctx.options_pool` loads files lazily, and Python's
    `DescriptorDatabase` cannot find a file by extension number, so its
    `FindAllExtensions` knows only the files loaded so far. The files that
    declare the type's extensions are loaded first, as `ctx.pool`, where
    rendering has loaded every file, lists them.
    """
    pool = options_desc.file.pool
    if pool is ctx.options_pool:
        for ext in ctx.pool.FindAllExtensions(
                ctx.pool.FindMessageTypeByName(options_desc.full_name)):
            if ext.file.name not in ctx.options_pool_files:
                pool.FindFileByName(ext.file.name)
                ctx.options_pool_files.add(ext.file.name)
    return list(pool.FindAllExtensions(options_desc))


def parse_options(
    ctx: Context,
    options_desc: Descriptor,
    cls: type[Message],
    data: bytes,
) -> Message:
    """Parse serialized options of type `options_desc`, with its class.

    The files declaring the type's extensions are loaded first
    (`option_extensions`), so that the custom options parse as the
    extensions they are, as they would in `ctx.pool`, rather than as
    unknown fields.
    """
    option_extensions(ctx, options_desc)
    msg = cls()
    msg.ParseFromString(data)
    return msg


def create_option_message_classes(ctx: Context) -> None:
    # The options types are those of the variant's descriptor.proto, in the
    # package that file declares: nothing is assumed from its path.
    # Phase 2 has refused a run whose input lacks it.
    pkg = ctx.pool_db.FindFileByName(ctx.variant_descriptor_proto).package
    ctx.options_package = pkg
    ctx.options_pool = DescriptorPool(_MapsAsRepeatedDatabase(ctx.pool_db))
    ctx.options_pool_files = set()
    pool = ctx.options_pool
    try:
        # EnumOptions message class
        ctx.eno_desc = pool.FindMessageTypeByName(pkg + ".EnumOptions")
        ctx.eno_cls = GetMessageClass(ctx.eno_desc)

        # EnumValueOptions message class
        ctx.evo_desc = pool.FindMessageTypeByName(pkg + ".EnumValueOptions")
        ctx.evo_cls = GetMessageClass(ctx.evo_desc)

        # FieldOptions message class
        ctx.fdo_desc = pool.FindMessageTypeByName(pkg + ".FieldOptions")
        ctx.fdo_cls = GetMessageClass(ctx.fdo_desc)

        # FileOptions message class
        ctx.fio_desc = pool.FindMessageTypeByName(pkg + ".FileOptions")
        ctx.fio_cls = GetMessageClass(ctx.fio_desc)

        # MessageOptions message class
        ctx.mso_desc = pool.FindMessageTypeByName(pkg + ".MessageOptions")
        ctx.mso_cls = GetMessageClass(ctx.mso_desc)

        # MethodOptions message class
        ctx.meo_desc = pool.FindMessageTypeByName(pkg + ".MethodOptions")
        ctx.meo_cls = GetMessageClass(ctx.meo_desc)

        # OneofOptions message class
        ctx.ooo_desc = pool.FindMessageTypeByName(pkg + ".OneofOptions")
        ctx.ooo_cls = GetMessageClass(ctx.ooo_desc)

        # ServiceOptions message class
        ctx.svo_desc = pool.FindMessageTypeByName(pkg + ".ServiceOptions")
        ctx.svo_cls = GetMessageClass(ctx.svo_desc)

    except KeyError as e:
        cli_error(f"Failed to initialize option message classes: {e}")
        cli_error(f"The descriptor pool is missing required types from '{pkg}' package.")
        cli_error("Ensure descriptor.proto is properly loaded in the pool.")
        sys.exit(1)

    # ExtensionRangeOptions message class. The proto2 variant's
    # descriptor.proto predates it, and has no extension range options.
    try:
        ero_desc = pool.FindMessageTypeByName(pkg + ".ExtensionRangeOptions")
        ctx.ero_desc = ero_desc
        ctx.ero_cls = GetMessageClass(ero_desc)
    except KeyError:
        ctx.ero_desc = None
        ctx.ero_cls = None
