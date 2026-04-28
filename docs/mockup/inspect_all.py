#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
# SPDX-License-Identifier: MIT
#
# Master inspection script: reads all compiled .pb descriptors and prints
# the raw descriptor state used to produce docs/proto2-proto3-findings.md.
#
# Run from inside the mockup/ directory (or adjust MOCKUP path):
#   cd docs/mockup && python3 inspect_all.py 2>&1 | tee inspect_all.log
#
# The output is intentionally verbose: every HasField check and every field
# value that matters for the rendering spec is printed explicitly.

import sys
from google.protobuf import descriptor_pb2
from google.protobuf.descriptor_pb2 import (
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    EnumDescriptorProto, FeatureSet,
)

MOCKUP = "."


def load(pb_path):
    fds = descriptor_pb2.FileDescriptorSet()
    with open(pb_path, "rb") as f:
        fds.ParseFromString(f.read())
    return fds.file


def hdr(title):
    print()
    print("=" * 70)
    print(title)
    print("=" * 70)


def sub(title):
    print()
    print(f"  --- {title} ---")


# -----------------------------------------------------------------------
# Part I: syntax / edition fields
# -----------------------------------------------------------------------
hdr("Part I: FileDescriptorProto.syntax and .edition")
for tag, pb in [
    ("f01 proto2-explicit", "f01.pb"),
    ("f02 proto2-implicit", "f02.pb"),
    ("f03 proto3",          "f03.pb"),
    ("f04 editions",        "f04.pb"),
]:
    for fdp in load(pb):
        print(f"  [{tag}] {fdp.name!r}: syntax={fdp.syntax!r}  edition={fdp.edition}")

# -----------------------------------------------------------------------
# Part II / III: field labels and synthetic oneofs
# -----------------------------------------------------------------------
hdr("Parts II & III: Field labels + synthetic oneofs (f05, f06, f10)")
for tag, pb in [("f05 proto2", "f05.pb"), ("f06 proto3", "f06.pb"), ("f10 synthetic-oneof", "f10.pb")]:
    sub(tag)
    for fdp in load(pb):
        for msg in fdp.message_type:
            print(f"    message: {msg.name!r}")
            for oneof in msg.oneof_decl:
                print(f"      oneof: {oneof.name!r}")
            for fld in msg.field:
                p3opt = fld.proto3_optional
                label = FieldDescriptorProto.Label.Name(fld.label)
                oneof_idx = fld.oneof_index if fld.HasField("oneof_index") else None
                print(f"      field {fld.name!r}: label={label}  "
                      f"proto3_optional={p3opt}  oneof_index={oneof_idx}")

# -----------------------------------------------------------------------
# Part IV: packed option
# -----------------------------------------------------------------------
hdr("Part IV: packed option (f07 proto2, f08 proto3)")
for tag, pb in [("f07 proto2", "f07.pb"), ("f08 proto3", "f08.pb")]:
    sub(tag)
    for fdp in load(pb):
        for msg in fdp.message_type:
            for fld in msg.field:
                has = fld.options.HasField("packed")
                val = fld.options.packed if has else "—"
                print(f"      {fld.name!r}: HasField(packed)={has}  value={val}")

# -----------------------------------------------------------------------
# Part V: json_name
# -----------------------------------------------------------------------
hdr("Part V: json_name (f09)")
for fdp in load("f09.pb"):
    for msg in fdp.message_type:
        for fld in msg.field:
            has = fld.HasField("json_name")
            print(f"    {fld.name!r}: HasField(json_name)={has}  json_name={fld.json_name!r}")

# -----------------------------------------------------------------------
# Part VI: default values
# -----------------------------------------------------------------------
hdr("Part VI: default values (f11 proto2)")
for fdp in load("f11.pb"):
    for msg in fdp.message_type:
        for fld in msg.field:
            has = fld.HasField("default_value")
            print(f"    {fld.name!r}: HasField(default_value)={has}  value={fld.default_value!r}")

# -----------------------------------------------------------------------
# Part VII: extensions and extension ranges
# -----------------------------------------------------------------------
hdr("Part VII: extensions + extension ranges (f12 proto2)")
for fdp in load("f12.pb"):
    print(f"  file: {fdp.name!r}")
    print(f"  file-level extensions: {len(fdp.extension)}")
    for ext in fdp.extension:
        print(f"    ext {ext.name!r}: extendee={ext.extendee!r}  number={ext.number}")
    for msg in fdp.message_type:
        print(f"  message: {msg.name!r}  extension_range={list(msg.extension_range)}")
        for ext in msg.extension:
            print(f"    ext {ext.name!r}: extendee={ext.extendee!r}  number={ext.number}")

# -----------------------------------------------------------------------
# Part VIII: groups
# -----------------------------------------------------------------------
hdr("Part VIII: groups (f13 proto2)")
for fdp in load("f13.pb"):
    for msg in fdp.message_type:
        for fld in msg.field:
            type_name = FieldDescriptorProto.Type.Name(fld.type)
            if fld.type == FieldDescriptorProto.TYPE_GROUP:
                print(f"    GROUP field: name={fld.name!r}  type_name={fld.type_name!r}  type={type_name}")
        for nested in msg.nested_type:
            print(f"    nested_type (group body): {nested.name!r}")

# -----------------------------------------------------------------------
# Part IX: weak imports
# -----------------------------------------------------------------------
hdr("Part IX: weak imports (f14 proto2 with --include_imports)")
for fdp in load("f14_full.pb"):
    print(f"  file: {fdp.name!r}")
    print(f"    dependency:       {list(fdp.dependency)}")
    print(f"    weak_dependency:  {list(fdp.weak_dependency)}")
    for i in fdp.weak_dependency:
        print(f"      -> weak dep: {fdp.dependency[i]!r}")

# -----------------------------------------------------------------------
# Part X: enum open/closed
# -----------------------------------------------------------------------
hdr("Part X: enum open/closed (f15 proto2, f15b proto3)")
for tag, pb in [("f15 proto2", "f15.pb"), ("f15b proto3", "f15b.pb")]:
    sub(tag)
    for fdp in load(pb):
        for enum in fdp.enum_type:
            opts = enum.options
            print(f"    enum {enum.name!r}: options={opts}")

# -----------------------------------------------------------------------
# Part XI: FieldOptions ctype/jstype/deprecated/weak
# -----------------------------------------------------------------------
hdr("Part XI: FieldOptions ctype/jstype/deprecated/weak (f16, f16b)")
for tag, pb in [("f16 proto2", "f16.pb"), ("f16b proto3", "f16b.pb")]:
    sub(tag)
    for fdp in load(pb):
        for msg in fdp.message_type:
            for fld in msg.field:
                opts = fld.options
                items = []
                for fname in ("ctype", "jstype", "deprecated", "weak"):
                    if opts.HasField(fname):
                        items.append(f"{fname}={getattr(opts, fname)}")
                if items:
                    print(f"    {fld.name!r}: {', '.join(items)}")

# -----------------------------------------------------------------------
# Part XII: MessageOptions
# -----------------------------------------------------------------------
hdr("Part XII: MessageOptions (f17 proto2, f17b proto3)")
for tag, pb in [("f17 proto2", "f17.pb"), ("f17b proto3", "f17b.pb")]:
    sub(tag)
    for fdp in load(pb):
        for msg in fdp.message_type:
            opts = msg.options
            items = []
            for oname in ("message_set_wire_format", "no_standard_descriptor_accessor", "deprecated"):
                if opts.HasField(oname):
                    items.append(f"{oname}={getattr(opts, oname)}")
            print(f"    {msg.name!r}: {items or '(no options)'}")

# -----------------------------------------------------------------------
# Part XIII: edition 2023 features
# -----------------------------------------------------------------------
hdr("Part XIII: Edition 2023 features (f19)")
for fdp in load("f19.pb"):
    print(f"  syntax={fdp.syntax!r}  edition={fdp.edition}")
    if fdp.options.HasField("features"):
        print(f"  file-level features: {fdp.options.features}")
    for enum in fdp.enum_type:
        opts = enum.options
        has_feat = opts.HasField("features")
        print(f"  enum {enum.name!r}: HasField(features)={has_feat}", end="")
        if has_feat:
            print(f"  -> {opts.features}", end="")
        print()
    for msg in fdp.message_type:
        for fld in msg.field:
            opts = fld.options
            has_feat = opts.HasField("features")
            label = FieldDescriptorProto.Label.Name(fld.label)
            packed_has = opts.HasField("packed")
            print(f"  field {fld.name!r}: label={label}  HasField(packed)={packed_has}  "
                  f"HasField(features)={has_feat}", end="")
            if has_feat:
                print(f"  -> {opts.features}", end="")
            print()
