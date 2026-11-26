# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""The descriptors test_feature_resolution.py builds with Python's protobuf
(4da0f2d6), by its own code, copied verbatim but for the output paths,
each a bare FileDescriptorProto:
  OUT/no_featureset.pb   test_T6_empty_variant_returns_empty_table
  OUT/synthetic.pb       test_T7_synthetic_descriptor (`_make_synthetic_fdp`)
Run with a Python that has protobuf.

Usage: python3 -I -B test_feature_resolution.py OUT
"""
import sys
from pathlib import Path

from google.protobuf.descriptor_pb2 import (
    DescriptorProto,
    Edition,
    EnumDescriptorProto,
    EnumValueDescriptorProto,
    FieldDescriptorProto,
    FileDescriptorProto,
)


def _make_synthetic_fdp(feature_defs):
    fdp = FileDescriptorProto()
    fdp.name = "synthetic_descriptor.proto"
    fdp.syntax = "proto2"
    fset = DescriptorProto()
    fset.name = "FeatureSet"
    for field_num, (field_name, defaults) in enumerate(feature_defs.items(), start=1):
        enum = EnumDescriptorProto()
        enum_name = field_name.upper()
        enum.name = enum_name
        v0 = EnumValueDescriptorProto()
        v0.name = f"{enum_name}_UNKNOWN"
        v0.number = 0
        v1 = EnumValueDescriptorProto()
        v1.name = "FIRST"
        v1.number = 1
        v2 = EnumValueDescriptorProto()
        v2.name = "SECOND"
        v2.number = 2
        enum.value.extend([v0, v1, v2])
        fset.enum_type.append(enum)
        f = FieldDescriptorProto()
        f.name = field_name
        f.number = field_num
        f.label = FieldDescriptorProto.LABEL_OPTIONAL
        f.type = FieldDescriptorProto.TYPE_ENUM
        f.type_name = f".FeatureSet.{enum_name}"
        for edition_val, value_name in defaults:
            ed_def = f.options.edition_defaults.add()
            ed_def.edition = edition_val
            ed_def.value = value_name
        fset.field.append(f)
    fdp.message_type.append(fset)
    return fdp


out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
empty_fdp = FileDescriptorProto()
empty_fdp.name = "no_featureset.proto"
(out / "no_featureset.pb").write_bytes(empty_fdp.SerializeToString())
fdp = _make_synthetic_fdp({
    "field_presence": [(Edition.EDITION_LEGACY, "FIRST"), (Edition.EDITION_2023, "SECOND")],
})
(out / "synthetic.pb").write_bytes(fdp.SerializeToString())
print("ok")
