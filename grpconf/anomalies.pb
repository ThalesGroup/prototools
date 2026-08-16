#@ prototext: protoc
# Type: google.protobuf.FileDescriptorProto

# ============================================================================
# Every anomaly prototext-core can report, in one document.
#
# This file is the fixture itself, not a script that builds one: `#@ prototext`
# is a text format, and both `prototext` and `protolens` recognize it by its
# first thirteen bytes rather than by the file's extension.  So it is named
# `.pb`, opened directly, and edited by hand.
#
#     protolens --descriptor-set <set> --type google.protobuf.FileDescriptorProto \
#         grpconf/anomalies.pb
#
# Lines starting with `#` are dropped by the encoder and never reach the wire,
# so they are invisible in protolens.  The explanations the audience sees are
# the string field VALUES below.
#
# Read top to bottom: the legal-but-unusual encodings come first, the outright
# malformed bytes last.
#
# TWO RULES SHAPE THE LAYOUT.
#
# Every example is embedded in its own submessage, so that a reader who folds
# the whole document is left with a neat column of `message_type { ... }` rows
# rather than a mixture of subtrees and dangling scalars.  Section 6 has to do
# this anyway -- a malformed byte consumes the rest of its enclosing region --
# and the rest of the document simply follows suit.  Each submessage is
# introduced by a top-level `name` line that carries its heading: a folded node
# shows no preview of its contents, so a heading written INSIDE the submessage
# disappears with it, and one written beside it does not.  `name` is
# FileDescriptorProto's own field 1; it is singular, and a singular field
# repeated on the wire is rendered once per occurrence.
#
# And wherever an anomaly has a canonical counterpart, the two are written side
# by side in the same submessage, ALWAYS IN THAT ORDER: the unusual line first,
# the same value spelled the ordinary way under it.  Press `w` on either and the
# difference is in the bytes, not in the text.
#
# Sections are numbered `1.a.`, `1.b.`, ... in the headings, and
# `anomalies.script` walks them in that order, one anomaly per step.
# ============================================================================


# ---------------------------------------------------------------------------
# 1. Legal bytes that no canonical writer would produce.
#
# Every parser in the world accepts these fields and recovers exactly the
# values they claim.  What is unusual is how the bytes spell them: a varint may
# carry trailing 0x80 padding bytes and still decode to the same number, so a
# tag, a length prefix and a value can each be written longer than they need to
# be.  protolens reports the padding as `tag_ohb` / `len_ohb` / `val_ohb`
# ("overhang bytes") and colors it as non-canonical rather than invalid.
# ---------------------------------------------------------------------------

name: "1.a. Legal, not canonical: a TAG padded to 3 bytes."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  reserved_name: "this line's tag is padded"  #@ repeated string = 10; tag_ohb: 2
  reserved_name: "this line's tag is not"  #@ repeated string = 10
}

name: "1.b. Legal, not canonical: a LENGTH prefix padded to 4 bytes."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  reserved_name: "this line's length prefix is padded"  #@ repeated string = 10; len_ohb: 3
  reserved_name: "this line's length prefix is not"  #@ repeated string = 10
}

name: "1.c. Legal, not canonical: a VALUE padded to 5 bytes."  #@ string = 1
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


# ---------------------------------------------------------------------------
# 2. Values that survive a round trip but not a re-encode.
#
# Here two producers disagree and neither is wrong.  A negative int32 is
# specified to travel as a ten-byte varint (sign-extended to 64 bits), but some
# writers truncate it to five bytes; the value is identical, the bytes are not.
# And IEEE 754 has 2^52 distinct NaN payloads while protobuf text has one
# spelling, `nan`, so a re-encode would silently pick the canonical one.
# ---------------------------------------------------------------------------

name: "2.a. -1 written in five bytes instead of the specified ten."  #@ string = 1
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

name: "2.b. A NaN whose payload bits are not the canonical NaN's."  #@ string = 1
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


# ---------------------------------------------------------------------------
# 3. A newer producer, an older schema.
#
# The everyday case, and nothing is broken.  An enum value this descriptor set
# has no name for is kept as its number and flagged `ENUM_UNKNOWN`; a field
# number the schema does not declare at all is rendered by its WIRE TYPE, which
# is all the bytes themselves can tell us.
# ---------------------------------------------------------------------------

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

name: "3.b. Below: four fields this schema does not declare, by wire type."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  200: 42  #@ varint
  201: 0x400921fb54442d18  #@ fixed64
  202: 0x40490fdb  #@ fixed32
  203: "an undeclared payload that is not text: \377\376"  #@ bytes
}


# ---------------------------------------------------------------------------
# 4. A packed repeated field.
#
# `path` is `repeated int32 [packed = true]`, so all three elements share one
# tag and one length prefix: three text lines, ONE wire record.  `pack_size: 3`
# marks where the record begins and how many elements it holds -- press `w` in
# protolens to see the single run of bytes underneath.  The per-element
# anomalies are spelled `ohb` and `neg`, the packed-run equivalents of
# `val_ohb` and `truncated_neg` above.
#
# `span` is a second packed int32 field carrying the same three values
# canonically.  One run of bytes against the other, same numbers -- and the
# canonical run is the LONGER of the two, twelve bytes against nine: dropping
# the padding saves two, and sign-extending the -1 costs five.
#
# The two runs are the twin of the rule above, so they share one submessage --
# and therefore one heading, which is why this section's heading is the only
# one carrying no letter.  `anomalies.script` letters the two runs 4.a. and
# 4.b. as it steps onto each.
# ---------------------------------------------------------------------------

name: "4. Two runs below: three text lines each, one wire record each."  #@ string = 1
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


# ---------------------------------------------------------------------------
# 5. The blob and the descriptor set disagree.
#
# Everything above was well-formed protobuf.  From here on the bytes and the
# schema contradict each other, and protolens says which of the two is at
# fault.  These are still parseable -- the scan continues past them.
# ---------------------------------------------------------------------------

name: "5.a. The schema says this field is a string; the wire says varint."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  1: 7  #@ varint; TYPE_MISMATCH
}

name: "5.b. Declared a string, but the payload is not valid UTF-8."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  10: "\377\376"  #@ INVALID_STRING
}

name: "5.c. Declared packed int32, but the payload does not decode."  #@ string = 1
source_code_info {  #@ SourceCodeInfo = 9
  location {  #@ repeated Location = 1
    1: "\001\002\200"  #@ INVALID_PACKED_RECORDS
  }
}


# ---------------------------------------------------------------------------
# 6. Malformed wire bytes.
#
# Each of these stops the scan dead: once a varint has no terminator or a wire
# type has no meaning, there is no way to find the next tag.  The decoder gives
# up on the REST OF THE ENCLOSING REGION -- which is why each of them gets a
# `message_type` submessage to itself and nothing may follow it there.  A
# length prefix bounds the damage, so the parent's scan resumes at the next tag
# and the document can hold as many of these as it likes.
#
# The `MISSING` counts are not numbers chosen here; each is computed against
# what was actually left in the buffer.  6.a nests two of them: the schema
# names a message at field 2, so the cut field is descended into anyway (spec
# 0311), and the cut inside it lands on a declared string, which is not.
# ---------------------------------------------------------------------------

name: "6.a. A length prefix that claims more bytes than are there."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  field {  #@ repeated FieldDescriptorProto = 2; TRUNCATED_MESSAGE; MISSING: 3
    1: "ab"  #@ TRUNCATED_BYTES; MISSING: 2
  }
}

name: "6.b. A varint with no terminating byte."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  3: "\200\200"  #@ INVALID_VARINT
}

name: "6.c. A length prefix that is itself an unterminated varint."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  4: "\377\377\377\377\377\377\377\377\377\377"  #@ INVALID_LEN
}

name: "6.d. A 64-bit field with only three bytes behind it."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  5: "\001\002\003"  #@ INVALID_FIXED64
}

name: "6.e. A 32-bit field with only two bytes behind it."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  6: "\001\002"  #@ INVALID_FIXED32
}

name: "6.f. Wire type 6: no such thing. Nothing after it can be found."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  0: "&\030\t"  #@ INVALID_TAG_TYPE
}

name: "6.g. Field number 0 is out of range: field numbers start at 1."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  0: 5  #@ varint; TAG_OOR
}


# ---------------------------------------------------------------------------
# 7. Groups.
#
# proto2's groups predate submessages: instead of a length prefix, a group is
# opened by a START_GROUP tag and closed by an END_GROUP tag carrying the same
# field number.  Nothing declares them here -- this descriptor set has no
# groups at all -- so they arrive as undeclared fields, which is exactly how a
# modern parser meets one in the wild.
#
# Because the closing tag is a tag like any other, it can be padded
# (`etag_ohb`), carry the wrong field number (`END_MISMATCH`), carry an
# out-of-range one (`ETAG_OOR`), or never arrive at all (`OPEN_GROUP`).  An
# END_GROUP with no opener is `INVALID_GROUP_END`.  The last two end the scan,
# so nothing may follow them inside their submessage.
#
# All five use field number 100, the same one the canonical group in 7.a uses,
# so that the number is never what differs between one example and the next.
# ---------------------------------------------------------------------------

name: "7.a. A group closed by a padded tag, then the same group closed canonically."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  100 {  #@ group; etag_ohb: 2
    1: 5  #@ varint
  }
  100 {  #@ group
    1: 5  #@ varint
  }
}

name: "7.b. Opened as field 100, closed as field 101."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  100 {  #@ group; END_MISMATCH: 101
    1: 5  #@ varint
  }
}

name: "7.c. Closed with a field number no field may have."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  100 {  #@ group; END_MISMATCH: 536870912
    1: 5  #@ varint
  }
}

name: "7.d. A group that is never closed."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  100 {  #@ group; OPEN_GROUP
    1: 5  #@ varint
  }
}

name: "7.e. An END_GROUP tag that closes nothing."  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
  100: "\010\001"  #@ INVALID_GROUP_END
}
