#!/usr/bin/env python3
"""Inspect descriptor files for the 5 remaining open items."""
import sys
from google.protobuf import descriptor_pb2
from google.protobuf.descriptor_pb2 import (
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    FeatureSet, FieldOptions
)

MOCKUP = "/home/experiment/code/oss-prototools/docs/mockup"

def load(path):
    fds = descriptor_pb2.FileDescriptorSet()
    with open(path, "rb") as f:
        fds.ParseFromString(f.read())
    return fds.file

def header(title):
    print()
    print("=" * 60)
    print(title)
    print("=" * 60)

# ----------------------------------------------------------------
# OPEN ITEM 1: import weak in editions (f20)
# ----------------------------------------------------------------
header("OPEN ITEM 1: import weak in editions (f20)")
files = load(f"{MOCKUP}/f20.pb")
for fdp in files:
    print(f"  file: {fdp.name!r}")
    print(f"    syntax: {fdp.syntax!r}")
    print(f"    dependency: {list(fdp.dependency)}")
    print(f"    weak_dependency (indices): {list(fdp.weak_dependency)}")
    for i in fdp.weak_dependency:
        print(f"      -> weak: {fdp.dependency[i]!r}")

# ----------------------------------------------------------------
# OPEN ITEM 2: message_set_wire_format in editions (f21)
# ----------------------------------------------------------------
header("OPEN ITEM 2: message_set_wire_format in editions (f21)")
files = load(f"{MOCKUP}/f21.pb")
for fdp in files:
    print(f"  file: {fdp.name!r}  syntax={fdp.syntax!r}  edition={fdp.edition}")
    for msg in fdp.message_type:
        opts = msg.options
        print(f"    message: {msg.name!r}")
        print(f"      message_set_wire_format HasField: {opts.HasField('message_set_wire_format')}")
        print(f"      message_set_wire_format value:    {opts.message_set_wire_format}")
        if opts.HasField("features"):
            print(f"      options.features: {opts.features}")
        print(f"      extension_range: {list(msg.extension_range)}")

# ----------------------------------------------------------------
# OPEN ITEM 3: deprecated_legacy_json_field_conflicts (f22, f22b)
# ----------------------------------------------------------------
header("OPEN ITEM 3: deprecated_legacy_json_field_conflicts (f22, f22b)")
for fname, pb in [("f22 (proto2)", f"{MOCKUP}/f22.pb"), ("f22b (proto3)", f"{MOCKUP}/f22b.pb")]:
    print(f"\n  --- {fname} ---")
    files = load(pb)
    for fdp in files:
        print(f"  file: {fdp.name!r}  syntax={fdp.syntax!r}")
        for enum in fdp.enum_type:
            opts = enum.options
            has = opts.HasField("deprecated_legacy_json_field_conflicts") if hasattr(opts, "deprecated_legacy_json_field_conflicts") else "N/A"
            val = opts.deprecated_legacy_json_field_conflicts if hasattr(opts, "deprecated_legacy_json_field_conflicts") else "N/A"
            print(f"    enum {enum.name!r}: HasField={has}  value={val}")
        for msg in fdp.message_type:
            opts = msg.options
            has = opts.HasField("deprecated_legacy_json_field_conflicts") if hasattr(opts, "deprecated_legacy_json_field_conflicts") else "N/A"
            val = opts.deprecated_legacy_json_field_conflicts if hasattr(opts, "deprecated_legacy_json_field_conflicts") else "N/A"
            print(f"    message {msg.name!r}: HasField={has}  value={val}")

# ----------------------------------------------------------------
# OPEN ITEM 4: DescriptorProto.visibility (check f01-f04 for any set values)
# ----------------------------------------------------------------
header("OPEN ITEM 4: DescriptorProto.visibility field")
# Check the descriptor proto itself for what VISIBILITY enum values exist
vd = DescriptorProto.DESCRIPTOR.fields_by_name.get("visibility")
if vd:
    print(f"  visibility field type: {vd.type}  message_type: {vd.message_type}")
    print(f"  visibility field number: {vd.number}")
    enum_type = vd.enum_type
    if enum_type:
        print(f"  Visibility enum values:")
        for v in enum_type.values:
            print(f"    {v.name} = {v.number}")
else:
    print("  visibility field NOT found in DescriptorProto")

# Check proto2 and proto3 files
for fname, pb in [("f01 (proto2)", f"{MOCKUP}/f01.pb"),
                   ("f03 (proto3)", f"{MOCKUP}/f03.pb"),
                   ("f04 (editions)", f"{MOCKUP}/f04.pb")]:
    print(f"\n  --- {fname} ---")
    files = load(pb)
    for fdp in files:
        for msg in fdp.message_type:
            vis = msg.visibility
            print(f"    message {msg.name!r}: visibility = {vis} ({DescriptorProto.Visibility.Name(vis) if vis else 'unset/default'})")
        for enum in fdp.enum_type:
            from google.protobuf.descriptor_pb2 import EnumDescriptorProto
            vis = enum.visibility
            print(f"    enum {enum.name!r}: visibility = {vis} ({EnumDescriptorProto.Visibility.Name(vis) if vis else 'unset/default'})")

# ----------------------------------------------------------------
# OPEN ITEM 5: Extension range end value semantics (f23)
# ----------------------------------------------------------------
header("OPEN ITEM 5: Extension range end value semantics (f23)")
files = load(f"{MOCKUP}/f23.pb")
for fdp in files:
    print(f"  file: {fdp.name!r}")
    for msg in fdp.message_type:
        print(f"  message: {msg.name!r}")
        for er in msg.extension_range:
            print(f"    extension_range: start={er.start}  end={er.end}  (raw)")
        # Also check reserved_range for comparison
        print(f"  Source had: 100 to 199, 1000 to 1999, 2000 to max")
        print(f"  max in proto2 = 536870912 (0x20000000) = 2^29")
        print(f"  'to max' in source — what does protoc store?")
