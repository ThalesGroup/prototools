#@ prototext: protoc
# Type: google.protobuf.FileDescriptorSet

# ============================================================================
# Every anomaly prototext-core can report, in one document.
#
# Opened with:
#
#     protolens --descriptor-set $PROTOTEXT_WKT_SET \
#         --type google.protobuf.FileDescriptorSet \
#         anomalies.pb
#
# Ordered from most to least interesting for a security audience, so
# the presenter can exit early at any natural break.
#
# Structure: every example lives in its own FileDescriptorProto (field 1
# of FileDescriptorSet), identified by its `name` field.
#
# Where an anomaly has a canonical counterpart, the two sit side by side
# in the SAME FDP: unusual line first, canonical line under it.
# ============================================================================


# ---------------------------------------------------------------------------
# 1. Shadowed scalar. (Most interesting for a security audience.)
#
# The schema declares `name` singular (field 1, string).  The wire carries
# it twice.  Standard decoders apply last-write-wins: the first value is
# silently overwritten and never reaches the application.  prototext shows
# both: the second occurrence gets `repeated_singular`, the first occurrence
# — already overwritten — gets `shadowed_scalar`.  The shadowed value is
# invisible to every standard SDK and to protoc --decode.
# ---------------------------------------------------------------------------

file {  #@ repeated FileDescriptorProto = 1
  name: "1. A singular field written twice: one value shadows the other."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    name: "I am the shadowed value — no standard SDK sees me"  #@ string = 1; repeated_singular
    name: "I am the surviving value — last write wins"  #@ string = 1
  }
}


# ---------------------------------------------------------------------------
# 2. Legal bytes that no canonical writer would produce.
#
# A varint may carry trailing 0x80 padding bytes and still decode correctly.
# Tags, length prefixes, and values can each be written longer than needed.
# prototext reports this as tag_ohb / len_ohb / val_ohb
# ("OverHanging Bytes") and colors it non-canonical.
# Deliberate padding is a fingerprint, or a covert channel in the encoding.
# ---------------------------------------------------------------------------

file {  #@ repeated FileDescriptorProto = 1
  name: "2.a. Legal, not canonical: a TAG padded to 3 bytes."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    reserved_name: "this line's tag is padded"  #@ repeated string = 10; tag_ohb: 2
    reserved_name: "this line's tag is not"  #@ repeated string = 10
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "2.b. Legal, not canonical: a LENGTH prefix padded to 4 bytes."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    reserved_name: "this line's length prefix is padded"  #@ repeated string = 10; len_ohb: 3
    reserved_name: "this line's length prefix is not"  #@ repeated string = 10
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "2.c. Legal, not canonical: a VALUE padded to 5 bytes."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    field {  #@ repeated FieldDescriptorProto = 2
      name: "padded"  #@ string = 1
      number: 5  #@ int32 = 3; val_ohb: 4
    }
    field {  #@ repeated FieldDescriptorProto = 2
      name: "canonical"  #@ string = 1
      number: 5  #@ int32 = 3
    }
  }
}


# ---------------------------------------------------------------------------
# 3. A newer producer, an older schema.
#
# Nothing is broken — the bytes are valid.  But the schema in hand has
# no name for them, so prototext shows what the bytes themselves say.
# ---------------------------------------------------------------------------

file {  #@ repeated FileDescriptorProto = 1
  name: "3.a. An enum value this schema has no name for."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    field {  #@ repeated FieldDescriptorProto = 2
      name: "unnamed"  #@ string = 1
      label: 99  #@ Label(99) = 4
    }
    field {  #@ repeated FieldDescriptorProto = 2
      name: "named"  #@ string = 1
      label: LABEL_OPTIONAL  #@ Label(1) = 4
    }
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "3.b. Below: four fields this schema does not declare, by wire type."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    200: 42  #@ varint
    201: 0x400921fb54442d18  #@ fixed64
    202: 0x40490fdb  #@ fixed32
    203: "an undeclared payload that is not text: \377\376"  #@ bytes
  }
}


# ---------------------------------------------------------------------------
# 4. A length prefix that claims more bytes than are there.
#
# protoc --decode fails on this entirely.  prototext decodes as far as
# possible, annotates the truncation boundary, and moves on.
# In log forensics, truncation typically means the process was killed
# mid-write.
# ---------------------------------------------------------------------------

file {  #@ repeated FileDescriptorProto = 1
  name: "4. A length prefix that claims more bytes than are there."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    field {  #@ repeated FieldDescriptorProto = 2; TRUNCATED_MESSAGE; MISSING: 3
      1: "ab"  #@ TRUNCATED_BYTES; MISSING: 2
    }
  }
}


# ---------------------------------------------------------------------------
# 5. The blob and the descriptor set disagree.
# ---------------------------------------------------------------------------

file {  #@ repeated FileDescriptorProto = 1
  name: "5.a. The schema says this field is a string; the wire says varint."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    1: 7  #@ varint; TYPE_MISMATCH
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "5.b. Declared a string, but the payload is not valid UTF-8."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    10: "\377\376"  #@ INVALID_STRING
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "5.c. Declared packed int32, but the payload does not decode."  #@ string = 1
  source_code_info {  #@ SourceCodeInfo = 9
    location {  #@ repeated Location = 1
      1: "\001\002\200"  #@ INVALID_PACKED_RECORDS
    }
  }
}


# ---------------------------------------------------------------------------
# 6. Values that survive a round trip but not a re-encode.
# ---------------------------------------------------------------------------

file {  #@ repeated FileDescriptorProto = 1
  name: "6.a. -1 written in five bytes instead of the specified ten."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    field {  #@ repeated FieldDescriptorProto = 2
      name: "truncated"  #@ string = 1
      number: -1  #@ int32 = 3; truncated_neg
    }
    field {  #@ repeated FieldDescriptorProto = 2
      name: "canonical"  #@ string = 1
      number: -1  #@ int32 = 3
    }
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "6.b. A NaN whose payload bits are not the canonical NaN's."  #@ string = 1
  options {  #@ FileOptions = 8
    uninterpreted_option {  #@ repeated UninterpretedOption = 999
      identifier_value: "unusual"  #@ string = 3
      double_value: nan  #@ double = 6; nan_bits: 0x7ff8000000000001
    }
    uninterpreted_option {  #@ repeated UninterpretedOption = 999
      identifier_value: "canonical"  #@ string = 3
      double_value: nan  #@ double = 6
    }
  }
}


# ---------------------------------------------------------------------------
# 7. A packed repeated field with per-element anomalies.
# ---------------------------------------------------------------------------

file {  #@ repeated FileDescriptorProto = 1
  name: "7. Two packed runs: three text lines each, one wire record each."  #@ string = 1
  source_code_info {  #@ SourceCodeInfo = 9
    location {  #@ repeated Location = 1
      path: 4  #@ repeated int32 [packed=true] = 1; pack_size: 3
      path: 0  #@ repeated int32 [packed=true] = 1; ohb: 2
      path: -1  #@ repeated int32 [packed=true] = 1; neg
      span: 4  #@ repeated int32 [packed=true] = 2; pack_size: 3
      span: 0  #@ repeated int32 [packed=true] = 2
      span: -1  #@ repeated int32 [packed=true] = 2
    }
  }
}


# ---------------------------------------------------------------------------
# 8. Malformed wire bytes.
#
# Each of these stops the scan dead inside its submessage; the parent
# resumes at the next tag.  From here on the bytes are unreadable rather
# than merely unusual.
# ---------------------------------------------------------------------------

file {  #@ repeated FileDescriptorProto = 1
  name: "8.a. A varint with no terminating byte."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    3: "\200\200"  #@ INVALID_VARINT
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "8.b. A length prefix that is itself an unterminated varint."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    4: "\377\377\377\377\377\377\377\377\377\377"  #@ INVALID_LEN
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "8.c. A 64-bit field with only three bytes behind it."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    5: "\001\002\003"  #@ INVALID_FIXED64
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "8.d. A 32-bit field with only two bytes behind it."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    6: "\001\002"  #@ INVALID_FIXED32
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "8.e. Wire type 6: no such thing. Nothing after it can be found."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    0: "&\030\t"  #@ INVALID_TAG_TYPE
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "8.f. Field number 0 is out of range: field numbers start at 1."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    0: 5  #@ varint; TAG_OOR
  }
}


# ---------------------------------------------------------------------------
# 9. Groups. (proto2 legacy; rarely seen, included for completeness.)
# ---------------------------------------------------------------------------

file {  #@ repeated FileDescriptorProto = 1
  name: "9.a. A group closed by a padded tag, then the same group canonically."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    100 {  #@ group; etag_ohb: 2
      1: 5  #@ varint
    }
    100 {  #@ group
      1: 5  #@ varint
    }
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "9.b. Opened as field 100, closed as field 101."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    100 {  #@ group; END_MISMATCH: 101
      1: 5  #@ varint
    }
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "9.c. Closed with a field number no field may have."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    100 {  #@ group; END_MISMATCH: 536870912
      1: 5  #@ varint
    }
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "9.d. A group that is never closed."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    100 {  #@ group; OPEN_GROUP
      1: 5  #@ varint
    }
  }
}

file {  #@ repeated FileDescriptorProto = 1
  name: "9.e. An END_GROUP tag that closes nothing."  #@ string = 1
  message_type {  #@ repeated DescriptorProto = 4
    100: "\010\001"  #@ INVALID_GROUP_END
  }
}
