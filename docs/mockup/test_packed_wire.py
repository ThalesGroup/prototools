#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
# SPDX-License-Identifier: MIT
#
# Empirically verify: does a proto3 repeated int32 with HasField("packed")==False
# actually produce PACKED wire encoding at runtime?
#
# Wire format primer:
#   Each field on the wire is: (field_number << 3) | wire_type
#   Wire type 0 = varint   (used by unpacked repeated integers: one tag per element)
#   Wire type 2 = length-delimited (used by packed repeated: one tag + length + all elements)
#
# Run: python3 test_packed_wire.py
#   (needs the generated f24_packed_wire_test_pb2.py on sys.path)

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from google.protobuf import descriptor_pb2
from f24_packed_wire_test_pb2 import PackedWireTest

VALUES = [1, 2, 3]

def decode_wire(raw: bytes):
    """Minimal wire-format decoder: yield (field_number, wire_type, payload)."""
    i = 0
    while i < len(raw):
        tag_byte, n = 0, 0
        while True:
            b = raw[i]; i += 1
            tag_byte |= (b & 0x7F) << n; n += 7
            if not (b & 0x80):
                break
        field_number = tag_byte >> 3
        wire_type    = tag_byte & 0x07
        if wire_type == 0:      # varint
            val, n = 0, 0
            while True:
                b = raw[i]; i += 1
                val |= (b & 0x7F) << n; n += 7
                if not (b & 0x80):
                    break
            yield field_number, wire_type, val
        elif wire_type == 2:    # length-delimited
            length, n = 0, 0
            while True:
                b = raw[i]; i += 1
                length |= (b & 0x7F) << n; n += 7
                if not (b & 0x80):
                    break
            payload = raw[i:i+length]; i += length
            yield field_number, wire_type, payload
        else:
            raise ValueError(f"unexpected wire_type={wire_type} at offset {i}")

def wire_type_name(wt):
    return {0: "varint (unpacked)", 2: "length-delimited (packed or message/string)"}.get(wt, str(wt))

# -----------------------------------------------------------------------
# Step 1: check HasField("packed") for each field in the descriptor
# -----------------------------------------------------------------------
print("=== Step 1: descriptor packed state ===")
fds = descriptor_pb2.FileDescriptorSet()
with open("f24.pb", "rb") as f:
    fds.ParseFromString(f.read())
fdp = fds.file[0]
print(f"syntax: {fdp.syntax!r}")
for msg in fdp.message_type:
    for fld in msg.field:
        opts = fld.options
        has = opts.HasField("packed")
        val = opts.packed if has else "—"
        print(f"  field {fld.name!r} (#{fld.number}): HasField(packed)={has}  value={val}")

# -----------------------------------------------------------------------
# Step 2: encode a message with [1, 2, 3] in each field, inspect wire bytes
# -----------------------------------------------------------------------
print()
print("=== Step 2: wire encoding of [1, 2, 3] ===")
msg = PackedWireTest()
msg.default_field.extend(VALUES)
msg.explicit_true.extend(VALUES)
msg.explicit_false.extend(VALUES)
raw = msg.SerializeToString()
print(f"raw bytes ({len(raw)}): {raw.hex()}")

print()
print("=== Step 3: wire-level field tags ===")
for field_num, wire_type, payload in decode_wire(raw):
    print(f"  field #{field_num}  wire_type={wire_type} ({wire_type_name(wire_type)})  "
          f"payload={payload!r}")

# -----------------------------------------------------------------------
# Step 3: summarise
# -----------------------------------------------------------------------
print()
print("=== Summary ===")
tags_seen = {}
for field_num, wire_type, payload in decode_wire(raw):
    tags_seen.setdefault(field_num, set()).add(wire_type)

field_names = {1: "default_field (no annotation)",
               2: "explicit_true  [packed=true]",
               3: "explicit_false [packed=false]"}
for fnum, wts in sorted(tags_seen.items()):
    for wt in wts:
        encoding = "PACKED" if wt == 2 else "UNPACKED"
        print(f"  field #{fnum} {field_names[fnum]}: {encoding} (wire_type={wt})")
