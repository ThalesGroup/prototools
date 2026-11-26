# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""The descriptor sets test_anomalies.py builds with Python's protobuf
(4da0f2d6), each test's construction copied verbatim, written under OUT
with the names the tests gave them (`_write_fds`): weak_dep.pb,
weak_main.pb (A4); a5_helper.pb, a5_main.pb (A5); a5_regression.pb (A5
regression); b1_helper.pb, b1_main.pb (B1); b2.pb; b3.pb; c1.pb; c2.pb;
c3.pb; c4.pb. Run with a Python that has protobuf.

Usage: python3 -I -B test_anomalies.py OUT
"""
import sys
from pathlib import Path

from google.protobuf.descriptor_pb2 import (
    FieldDescriptorProto,
    FileDescriptorProto,
    FileDescriptorSet,
    MessageOptions,
)

pb_dir = Path(sys.argv[1])
pb_dir.mkdir(parents=True, exist_ok=True)


def _write_fds(path, *fdps):
    fds = FileDescriptorSet()
    for fdp in fdps:
        fds.file.append(fdp)
    path.write_bytes(fds.SerializeToString())
    return path


def _minimal_proto3(name):
    fdp = FileDescriptorProto()
    fdp.name = name
    fdp.syntax = "proto3"
    return fdp


def _minimal_proto2(name):
    fdp = FileDescriptorProto()
    fdp.name = name
    fdp.syntax = "proto2"
    return fdp


# A4
dep = _minimal_proto3("weak_dep.proto")
dep_msg = dep.message_type.add()
dep_msg.name = "Dep"
dep_field = dep_msg.field.add()
dep_field.name = "value"
dep_field.number = 1
dep_field.label = FieldDescriptorProto.LABEL_OPTIONAL
dep_field.type = FieldDescriptorProto.TYPE_INT32
main = _minimal_proto3("weak_main.proto")
main.dependency.append("weak_dep.proto")
main.weak_dependency.append(0)
msg = main.message_type.add()
msg.name = "Main"
f = msg.field.add()
f.name = "value"
f.number = 1
f.label = FieldDescriptorProto.LABEL_OPTIONAL
f.type = FieldDescriptorProto.TYPE_INT32
_write_fds(pb_dir / "weak_dep.pb", dep)
_write_fds(pb_dir / "weak_main.pb", main)

# A5
helper = _minimal_proto2("extendable_helper.proto")
extendable = helper.message_type.add()
extendable.name = "Extendable"
er = extendable.extension_range.add()
er.start = 100
er.end = 200
main = _minimal_proto3("a5_main.proto")
main.dependency.append("extendable_helper.proto")
ext_field = main.extension.add()
ext_field.name = "my_ext"
ext_field.number = 100
ext_field.label = FieldDescriptorProto.LABEL_OPTIONAL
ext_field.type = FieldDescriptorProto.TYPE_INT32
ext_field.extendee = ".Extendable"
msg = main.message_type.add()
msg.name = "Main"
_write_fds(pb_dir / "a5_helper.pb", helper)
_write_fds(pb_dir / "a5_main.pb", main)

# A5 regression
main = _minimal_proto3("a5_regression.proto")
ext_field = main.extension.add()
ext_field.name = "my_method_opt"
ext_field.number = 50000
ext_field.label = FieldDescriptorProto.LABEL_OPTIONAL
ext_field.type = FieldDescriptorProto.TYPE_BOOL
ext_field.extendee = ".proto2.MethodOptions"
_write_fds(pb_dir / "a5_regression.pb", main)

# B1
helper = _minimal_proto2("b1_helper.proto")
extendable = helper.message_type.add()
extendable.name = "Extendable"
er = extendable.extension_range.add()
er.start = 100
er.end = 200
main = _minimal_proto3("b1_main.proto")
main.dependency.append("b1_helper.proto")
outer = main.message_type.add()
outer.name = "Outer"
regular = outer.field.add()
regular.name = "value"
regular.number = 1
regular.label = FieldDescriptorProto.LABEL_OPTIONAL
regular.type = FieldDescriptorProto.TYPE_INT32
ext_field = outer.extension.add()
ext_field.name = "nested_ext"
ext_field.number = 101
ext_field.label = FieldDescriptorProto.LABEL_OPTIONAL
ext_field.type = FieldDescriptorProto.TYPE_INT32
ext_field.extendee = ".Extendable"
_write_fds(pb_dir / "b1_helper.pb", helper)
_write_fds(pb_dir / "b1_main.pb", main)

# B2
main = _minimal_proto3("b2_main.proto")
msg = main.message_type.add()
msg.name = "WithRange"
field = msg.field.add()
field.name = "value"
field.number = 1
field.label = FieldDescriptorProto.LABEL_OPTIONAL
field.type = FieldDescriptorProto.TYPE_INT32
er = msg.extension_range.add()
er.start = 100
er.end = 200
_write_fds(pb_dir / "b2.pb", main)

# B3
main = _minimal_proto3("b3_main.proto")
msg = main.message_type.add()
msg.name = "MsgSet"
msg.options.CopyFrom(MessageOptions())
msg.options.message_set_wire_format = True
_write_fds(pb_dir / "b3.pb", main)

# C1
main = _minimal_proto3("c1_main.proto")
outer = main.message_type.add()
outer.name = "Outer"
entry = outer.nested_type.add()
entry.name = "BadMapEntry"
entry.options.CopyFrom(MessageOptions())
entry.options.map_entry = True
key_field = entry.field.add()
key_field.name = "key"
key_field.number = 1
key_field.label = FieldDescriptorProto.LABEL_OPTIONAL
key_field.type = FieldDescriptorProto.TYPE_STRING
map_field = outer.field.add()
map_field.name = "bad_map"
map_field.number = 1
map_field.label = FieldDescriptorProto.LABEL_REPEATED
map_field.type = FieldDescriptorProto.TYPE_MESSAGE
map_field.type_name = ".Outer.BadMapEntry"
_write_fds(pb_dir / "c1.pb", main)

# C2
main = _minimal_proto3("c2_main.proto")
msg = main.message_type.add()
msg.name = "Outer"
grp = main.message_type.add()
grp.name = "Mygroup"
grp_field = grp.field.add()
grp_field.name = "val"
grp_field.number = 1
grp_field.label = FieldDescriptorProto.LABEL_OPTIONAL
grp_field.type = FieldDescriptorProto.TYPE_INT32
field = msg.field.add()
field.name = "mygroup"
field.number = 1
field.label = FieldDescriptorProto.LABEL_OPTIONAL
field.type = FieldDescriptorProto.TYPE_GROUP
field.type_name = ".Mygroup"
_write_fds(pb_dir / "c2.pb", main)

# C3
main = _minimal_proto3("c3_main.proto")
msg = main.message_type.add()
msg.name = "Msg"
field = msg.field.add()
field.name = "mandatory"
field.number = 1
field.label = FieldDescriptorProto.LABEL_REQUIRED
field.type = FieldDescriptorProto.TYPE_STRING
_write_fds(pb_dir / "c3.pb", main)

# C4
main = _minimal_proto3("c4_main.proto")
msg = main.message_type.add()
msg.name = "Msg"
field = msg.field.add()
field.name = "greeting"
field.number = 1
field.label = FieldDescriptorProto.LABEL_OPTIONAL
field.type = FieldDescriptorProto.TYPE_STRING
field.default_value = "hello"
_write_fds(pb_dir / "c4.pb", main)
print("ok")
