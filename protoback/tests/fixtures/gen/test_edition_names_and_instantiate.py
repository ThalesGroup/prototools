# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The inputs test_edition_names.py and test_instantiate.py build with
Python's protobuf (4da0f2d6), by their own code, copied verbatim but for
the output paths:
  OUT/edition_names/ok.pb, b.pb    test_an_unnamed_edition_is_a_w6 (bare
                                   FileDescriptorProtos, b.pb with edition
                                   1003 appended)
  OUT/instantiate/inst.desc        `_schema`
  OUT/instantiate/own.desc         test_the_sets_own_descriptor_proto_is_used
Run with a Python that has protobuf (its own descriptor.proto is own.desc's).

Usage: python3 -I -B test_edition_names_and_instantiate.py OUT
"""
import sys
from pathlib import Path

import google.protobuf
from google.protobuf import descriptor_pb2

out = Path(sys.argv[1])


def _with_edition(f, edition):
    out = bytearray(f.SerializeToString())
    out.append(14 << 3)
    while edition >= 0x80:
        out.append((edition & 0x7F) | 0x80)
        edition >>= 7
    out.append(edition)
    return bytes(out)


inp = out / "edition_names"
inp.mkdir(parents=True)
named = descriptor_pb2.FileDescriptorProto(name="a/ok.proto", package="a", syntax="editions",
                                           edition=descriptor_pb2.EDITION_2024)
unnamed = descriptor_pb2.FileDescriptorProto(name="a/b.proto", package="a", syntax="editions")
unnamed.message_type.add(name="M")
(inp / "ok.pb").write_bytes(named.SerializeToString())
(inp / "b.pb").write_bytes(_with_edition(unnamed, 1003))


def _schema(tmp_path):
    f = descriptor_pb2.FileDescriptorProto()
    f.name = 'inst.proto'
    f.package = 'inst'
    f.syntax = 'proto2'
    m = f.message_type.add()
    m.name = 'M'
    entry = m.nested_type.add()
    entry.name = 'LabelsEntry'
    entry.options.map_entry = True
    for name, number in (('key', 1), ('value', 2)):
        e = entry.field.add()
        e.name, e.number = name, number
        e.label, e.type = e.LABEL_OPTIONAL, e.TYPE_STRING
    labels = m.field.add()
    labels.name, labels.number = 'labels', 1
    labels.label, labels.type = labels.LABEL_REPEATED, labels.TYPE_MESSAGE
    labels.type_name = '.inst.M.LabelsEntry'
    group = m.nested_type.add()
    group.name = 'G'
    x = group.field.add()
    x.name, x.number = 'x', 1
    x.label, x.type = x.LABEL_OPTIONAL, x.TYPE_INT32
    for name, number, label in (
        ('single', 2, descriptor_pb2.FieldDescriptorProto.LABEL_OPTIONAL),
        ('many', 3, descriptor_pb2.FieldDescriptorProto.LABEL_REPEATED),
    ):
        g = m.field.add()
        g.name, g.number, g.label = name, number, label
        g.type, g.type_name = g.TYPE_GROUP, '.inst.M.G'
    fds = descriptor_pb2.FileDescriptorSet()
    fds.file.append(f)
    path = tmp_path / 'inst.desc'
    path.write_bytes(fds.SerializeToString())
    return path


inst = out / "instantiate"
inst.mkdir(parents=True)
_schema(inst)
own = descriptor_pb2.FileDescriptorProto()
descriptor_pb2.DESCRIPTOR.CopyToProto(own)
edition = next(e for e in own.enum_type if e.name == 'Edition')
extra = edition.value.add()
extra.name, extra.number = 'EDITION_TEST_EXTRA', 123456
fds = descriptor_pb2.FileDescriptorSet()
fds.file.append(own)
(inst / 'own.desc').write_bytes(fds.SerializeToString())
print("protobuf", google.protobuf.__version__)
