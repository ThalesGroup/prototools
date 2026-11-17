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
    """The extensions of an options type its pool holds: in an options
    pool, those of the rendering file's import closure (`use_options_pool`;
    a pool never looks a file up by extension number)."""
    return list(options_desc.file.pool.FindAllExtensions(options_desc))


def parse_options(
    ctx: Context,
    options_desc: Descriptor,
    cls: type[Message],
    data: bytes,
) -> Message:
    """Parse serialized options of type `options_desc`, with its class: the
    custom options of the rendering file's closure parse as the extensions
    they are, any other as an unknown field (spec 0429 S14)."""
    msg = cls()
    msg.ParseFromString(data)
    return msg


# The context's attributes an options pool sets (`_set_option_classes`).
_OPTION_ATTRS = (
    'options_pool',
    'eno_desc', 'eno_cls', 'evo_desc', 'evo_cls', 'fdo_desc', 'fdo_cls',
    'fio_desc', 'fio_cls', 'mso_desc', 'mso_cls', 'meo_desc', 'meo_cls',
    'ooo_desc', 'ooo_cls', 'svo_desc', 'svo_cls', 'ero_desc', 'ero_cls',
    'fts_desc', 'fts_cls',
)


def _closure(ctx: Context, name: str) -> frozenset[str]:
    """File `name`, its imports, theirs: what the pool loads for it; a
    file the input lacks left out."""
    cached = ctx.import_closures.get(name)
    if cached is not None:
        return cached
    seen: set[str] = set()
    todo = [name]
    while todo:
        n = todo.pop()
        if n in seen:
            continue
        try:
            fdp = ctx.pool_db.FindFileByName(n)
        except KeyError:
            continue
        seen.add(n)
        todo.extend(fdp.dependency)
    closure = frozenset(seen)
    ctx.import_closures[name] = closure
    return closure


def _declares_extensions(ctx: Context, name: str) -> bool:
    cached = ctx.extension_files.get(name)
    if cached is None:
        def nested(messages: Any) -> bool:
            return any(m.extension or nested(m.nested_type) for m in messages)
        fdp = ctx.pool_db.FindFileByName(name)
        cached = bool(fdp.extension) or nested(fdp.message_type)
        ctx.extension_files[name] = cached
    return cached


def use_options_pool(ctx: Context, file_name: str) -> None:
    """The options classes file `file_name` renders with (spec 0429 S14):
    from a pool holding the variant's descriptor.proto and the files of its
    import closure that declare extensions, whatever rendered before; one
    pool per such set of files."""
    key = frozenset(f for f in _closure(ctx, file_name) if _declares_extensions(ctx, f))
    attrs = ctx.options_pools.get(key)
    if attrs is None:
        pool = DescriptorPool(ctx.options_db)
        _set_option_classes(ctx, pool)
        for name in sorted(key):
            try:
                pool.FindFileByName(name)
            except (KeyError, TypeError):
                pass   # a file the pool refuses: its extensions unknown
        attrs = {a: getattr(ctx, a) for a in _OPTION_ATTRS}
        ctx.options_pools[key] = attrs
    for a, v in attrs.items():
        setattr(ctx, a, v)


def create_option_message_classes(ctx: Context) -> None:
    # The options types are those of the variant's descriptor.proto, in the
    # package that file declares: nothing is assumed from its path.
    # Phase 2 has refused a run whose input lacks it.
    pkg = ctx.pool_db.FindFileByName(ctx.variant_descriptor_proto).package
    ctx.options_package = pkg
    ctx.options_db = _MapsAsRepeatedDatabase(ctx.pool_db)
    ctx.options_pools = {}
    ctx.import_closures = {}
    ctx.extension_files = {}
    _set_option_classes(ctx, DescriptorPool(ctx.options_db))


def _set_option_classes(ctx: Context, pool: DescriptorPool) -> None:
    """The options types' descriptors and classes, from `pool`."""
    pkg = ctx.options_package
    ctx.options_pool = pool
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

    except TypeError as e:
        # The pool refused the variant's descriptor.proto: refused as a
        # corrupted one is, its reason under --debug.
        from .anomalies import debug_exception
        from .phases import DescriptorProtoUnbuildableError
        debug_exception(ctx.debug, f"'{ctx.variant_descriptor_proto}'", e)
        raise DescriptorProtoUnbuildableError from e
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

    # FeatureSet, whose extensions are the custom features (spec 0435). A
    # descriptor.proto older than editions has none.
    try:
        fts_desc = pool.FindMessageTypeByName(pkg + ".FeatureSet")
        ctx.fts_desc = fts_desc
        ctx.fts_cls = GetMessageClass(fts_desc)
    except KeyError:
        ctx.fts_desc = None
        ctx.fts_cls = None
