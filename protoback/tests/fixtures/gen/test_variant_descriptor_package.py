# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The input of test_variant_descriptor_package.py, made by its own
`_alt_descriptor` and `_set` (4da0f2d6), copied verbatim but for the output
path: OUT/in/set.pb. Its descriptor.proto is the Python protobuf's embedded
one (descriptor_pb2.DESCRIPTOR.serialized_pb), so the bytes depend on that
release. Run with a Python that has protobuf.

Usage: python3 -I -B test_variant_descriptor_package.py OUT
"""
import sys
from pathlib import Path

import google.protobuf
from google.protobuf import descriptor_pb2
from google.protobuf.descriptor_pb2 import (
    FieldDescriptorProto,
    FileDescriptorProto,
    FileDescriptorSet,
)

_NAME = "alt/descriptor.proto"
_PACKAGE = "altpb"


def _alt_descriptor() -> FileDescriptorProto:
    fdp = FileDescriptorProto()
    fdp.ParseFromString(descriptor_pb2.DESCRIPTOR.serialized_pb)
    fdp.name = _NAME
    fdp.package = _PACKAGE

    def walk(messages) -> None:
        for m in messages:
            for f in m.field:
                if f.type_name.startswith(".google.protobuf."):
                    f.type_name = f".{_PACKAGE}." + f.type_name[len(".google.protobuf."):]
            walk(m.nested_type)

    walk(fdp.message_type)
    return fdp


def _set(tmp: Path) -> Path:
    use = FileDescriptorProto(name="use.proto", package="use", syntax="proto2",
                              dependency=[_NAME])
    m = use.message_type.add(name="M")
    field = m.field.add(name="s", number=1, type=FieldDescriptorProto.TYPE_STRING,
                        label=FieldDescriptorProto.LABEL_OPTIONAL)
    field.options.deprecated = True

    ext3 = FileDescriptorProto(name="ext3.proto", package="ext3", syntax="proto3",
                               dependency=[_NAME])
    ext3.extension.add(name="sensitivity", number=50001,
                       type=FieldDescriptorProto.TYPE_INT32,
                       label=FieldDescriptorProto.LABEL_OPTIONAL,
                       extendee=f".{_PACKAGE}.FieldOptions")

    root = tmp / "in"
    root.mkdir(parents=True)
    fds = FileDescriptorSet()
    fds.file.extend([_alt_descriptor(), use, ext3])
    pb = root / "set.pb"
    pb.write_bytes(fds.SerializeToString())
    return pb


print("protobuf", google.protobuf.__version__, "->", _set(Path(sys.argv[1])))
