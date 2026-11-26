# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The two members test_blob_root_and_defaults.py builds with Python's
protobuf (4da0f2d6), by its own `_fdp` and `_members`, each written as a
bare FileDescriptorProto: OUT/first.pb, OUT/second.pb. The tests assemble
the blob from them as its `_write_blob` did. Run with a Python that has
protobuf.

Usage: python3 -I -B test_blob_root_and_defaults.py OUT
"""
import sys
from pathlib import Path

from google.protobuf import descriptor_pb2


def _fdp(name, package, message):
    fdp = descriptor_pb2.FileDescriptorProto()
    fdp.name = name
    fdp.syntax = "proto3"
    fdp.package = package
    msg = fdp.message_type.add()
    msg.name = message
    field = msg.field.add()
    field.name = "value"
    field.number = 1
    field.type = descriptor_pb2.FieldDescriptorProto.TYPE_STRING
    field.label = descriptor_pb2.FieldDescriptorProto.LABEL_OPTIONAL
    return fdp


out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
first, second = _fdp("first.proto", "one", "First"), _fdp("extra/second.proto", "two", "Second")
(out / "first.pb").write_bytes(first.SerializeToString())
(out / "second.pb").write_bytes(second.SerializeToString())
print("ok")
