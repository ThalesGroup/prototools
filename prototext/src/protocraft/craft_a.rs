// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Rust port of craft_a.py: fixture functions for the prototext test suite.

// ── test_empty ────────────────────────────────────────────────────────────────

fixture!(test_empty, super::knife_descriptor();
);

// ── test_FIELD_INVALID ────────────────────────────────────────────────────────

fixture!(test_FIELD_INVALID, super::knife_descriptor();
    message!("messageRp";
        raw!(b"\x07"),
    ),
    message!("messageRp";
        raw!(b"\x87\x80\x80\x00"),
    ),
    message!("messageRp";
        raw!(b"\x87\x80\x80\x00 -- Invalid wire-type"),
    ),
);

// ── test_garbage ──────────────────────────────────────────────────────────────

fixture!(test_garbage, super::knife_descriptor();
    message!("messageRp";
        raw!(b"\x80\x80\x80\x80\x80\x80"),
    ),
    message!("messageRp";
        raw!(b"\x86\x80\x80\x80\x80\x80"),
    ),
);

// ── test_n_overhanging_bytes ──────────────────────────────────────────────────

fixture!(test_n_overhanging_bytes, super::knife_descriptor();
    message!("messageRp";
        uint64!(Tag!(field_num: 1,  ohb: 17), 0),
        uint64!(Tag!(field: "uint64Rp", ohb: 2), 0),
    ),
);

// ── test_n_out_of_range ───────────────────────────────────────────────────────

fixture!(test_n_out_of_range, super::knife_descriptor();
    message!("messageRp";
        bytes_!(Tag!(field_num: 421), b" 421 is OK"),
        bytes_!(Tag!(field_num: 421), b" 421 is OK"),
        bytes_!(Tag!(field_num: 421), b" 421 is OK"),
    ),
    message!("messageRp";
        // Python: 1<<29-1 = 1<<(29-1) = 1<<28
        bytes_!(Tag!(field_num: 1 << 28), b" 1<<29-1 is OK "),
    ),
    message!("messageRp";
        bytes_!(Tag!(field_num: 1 << 29), b" 1<<29 is OOR "),
    ),
    message!("messageRp";
        bytes_!(Tag!(field_num: 1 << 32, ohb: 13), b" 1<<32 is OOR"),
    ),
    message!("messageRp";
        bytes_!(Tag!(field_num: 0, ohb: 13), b" zero is OOR"),
    ),
    message!("messageRp";
        bytes_!(Tag!(field_num: 1, ohb: 13), b" one is OK"),
    ),
    message!("messageRp";
        // Python: 1<<64 overflows u64; encode tag manually
        raw!(b"\x82\x80\x80\x80\x80\x80\x80\x80\x80\x10"),
        raw!(b"\x11"),
        raw!(b" 1<<64 is garbage"),
    ),
);

// ── test_wire_level ───────────────────────────────────────────────────────────

fixture!(test_wire_level, super::knife_descriptor();
    uint64!(1, 0),
    fixed64!(2, 0),
    fixed32!(2, 0),
    bytes_!(3, b""),
    // Group with deliberately mismatched end tag field number (start=4, end=44).
    group!(Tag!(field_num: 4, wire_type: 3) => Tag!(field_num: 44, wire_type: 4);
        uint64!(11, 0),
        fixed64!(12, 0),
        bytes_!(13, b""),
        uint32!(15, 0),
    ),
    uint32!(5, 0),
);

// ── test_proto2_level ─────────────────────────────────────────────────────────

fixture!(test_proto2_level, super::knife_descriptor();
    uint64!("uint64Rp", 0),
    fixed64!("fixed64Rp", 0),
    bytes_!("bytesRp", b""),
    group!(4;
        uint64!(11, 0),
    ),
    group!("group";
        message!("nested";
            fixed64!("fixed64Rp", 0),
            bytes_!("bytesRp", b""),
            uint32!("uint32Rp", 0),
        ),
    ),
    uint32!("uint32Rp", 0),
);

// ── test_VARINT_INVALID ───────────────────────────────────────────────────────

fixture!(test_VARINT_INVALID, super::knife_descriptor();
    message!("messageRp";
        uint64!("uint64Rp", RawData!(b"\x80")),
    ),
    uint64!("uint64Rp", 0),
);

// ── test_varint_overhanging_bytes ─────────────────────────────────────────────

fixture!(test_varint_overhanging_bytes, super::knife_descriptor();
    message!("messageRp";
        uint64!("uint64Rp", Integer!(unsigned: 42, ohb: 4)),
    ),
    uint64!("uint64Rp", 0),
);

// ── test_bool_out_of_range ────────────────────────────────────────────────────

fixture!(test_bool_out_of_range, super::knife_descriptor();
    message!("messageRp";
        bool_!("boolRp", false),
        bool_!("boolRp", true),
        raw!(b"\x80\x03\x02"),
        raw!(b"\x80\x03\x82\x00"),
    ),
    bool_!("boolRp", Integer!(unsigned: 1, ohb: 4)),
    uint32!("uint32Rp", 1),
);

// ── test_fixed64 ──────────────────────────────────────────────────────────────

fixture!(test_fixed64, super::knife_descriptor();
    double_!("doubleRp",   std::f64::consts::PI),
    fixed64!("fixed64Rp",  0xdeadbeefdeadbeef),
    sfixed64!("sfixed64Rp", 0xdeadbeefdeadbee),
    sfixed32!("sfixed32Rp", 0xdeadbee),
    fixed64!(1,             0xdeadbeefdeadbeef),
);

fixture!(test_fixed64_tag, super::knife_descriptor();
    // Fixed64(Tag(1), 0xdeadbeefdeadbeef) — raw field number 1
    fixed64!(1, 0xdeadbeefdeadbeef),
);

// ── test_FIELD_INVALID_LENGTH ─────────────────────────────────────────────────

fixture!(test_FIELD_INVALID_LENGTH, super::knife_descriptor();
    message!("messageRp";
        string!("stringRp", "hello1"),
        string!(Tag!(field: "stringRp", length_ohb: 3), "hello2"),
        // Valid string 'hello3' — raw bytes replace length+payload after the tag
        string!("stringRp", RawData!(b"\x06hello3")),
    ),
    message!("messageRp";
        // FIELD INVALID — raw bytes after tag: empty (no length byte)
        string!("stringRp", RawData!(b"")),
    ),
    message!("messageRp";
        // FIELD INVALID LENGTH — raw bytes after tag: incomplete varint
        string!("stringRp", RawData!(b"\x80")),
    ),
    message!("messageRp";
        // FIELD TRUNCATED — raw bytes after tag: length=1 but no payload
        string!("stringRp", RawData!(b"\x01")),
    ),
);

// ── test_INVALID_PACKED_RECORDS ───────────────────────────────────────────────

fixture!(test_INVALID_PACKED_RECORDS, super::knife_descriptor();
    packed_varints!("int32Pk", [
        Integer!(unsigned: 23),
        Integer!(unsigned: 24),
        Integer!(unsigned: 35),
    ]),
    packed_float!("floatPk", [
        0.0f32,
        0.0f32,
        0.0f32,
    ]),
    // Float(Tag('floatPk', length=5), [0., 1., pi]) — 3 floats = 12 bytes, truncated to 5
    packed_float!(Tag!(field: "floatPk", length: 5), [
        0.0f32,
        1.0f32,
        std::f32::consts::PI,
    ]),
);

// ── test_records_overhung_count ───────────────────────────────────────────────

fixture!(test_records_overhung_count, super::knife_descriptor();
    packed_varints!("int32Pk", [
        Integer!(unsigned: 23),
        Integer!(unsigned: 24),
        Integer!(unsigned: 35),
    ]),
    packed_varints!("int32Pk", [
        Integer!(unsigned: 23, ohb: 2),
        Integer!(unsigned: 24),
        Integer!(unsigned: 35, ohb: 3),
    ]),
);

// ── test_INVALID_STRING ───────────────────────────────────────────────────────

fixture!(test_INVALID_STRING, super::knife_descriptor();
    message!("messageRp";
        string!("stringRp", "Hello, world!"),
    ),
    message!("messageRp";
        bytes_!("stringRp", b"A vicious hello\xc0\x00"),
    ),
    message!("messageRp";
        bytes_!("stringRp", b"Another vicious hello\xf8\x80\x80\x80"),
    ),
    message!("messageRp";
        bytes_!("stringRp", b"And another one\xf8\x80\x80\x00"),
    ),
    message!("messageRp";
        // String(Tag('stringRp', ohb=97), ...) — tag with 97 overhanging bytes
        bytes_!(Tag!(field: "stringRp", ohb: 97), b"One more (\xf8\x80\x80\x00) + overhanging bytes"),
    ),
);

// ── test_message_no_schema ────────────────────────────────────────────────────

fixture!(test_message_no_schema, super::knife_descriptor();
    message!(3;
        bytes_!(1, b"no schema for these bytes."),
    ),
    message!(3;
        raw!(b"no schema for these bytes and no header."),
    ),
);

// ── test_group_no_schema ──────────────────────────────────────────────────────

fixture!(test_group_no_schema, super::knife_descriptor();
    group!("grouprp";
        uint64!("uint64Op", 1),
        uint64!("uint64Op", 2),
        uint64!("uint64Op", 3),
        uint64!("uint64Op", Integer!(unsigned: 4, ohb: 3)),
        bytes_!(1, b"no schema for these bytes."),
    ),
    group!("group";
        uint64!(1, 1),
        uint64!(1, 2),
        uint64!(1, 3),
        uint64!(1, Integer!(unsigned: 4, ohb: 3)),
    ),
);

// ── test_INVALID_GROUP_END ────────────────────────────────────────────────────

fixture!(test_INVALID_GROUP_END, super::knife_descriptor();
    uint64!("uint64Op", 1),
    uint64!("uint64Op", 2),
    uint64!("uint64Op", 3),
    uint64!("uint64Op", Integer!(unsigned: 4, ohb: 3)),
    raw!(b"\x04"),
    string!("stringRp", "Bogus END_GROUP just above"),
);

// ── test_INVALID_FIXED64 ──────────────────────────────────────────────────────

fixture!(test_INVALID_FIXED64, super::knife_descriptor();
    double_!("doublePk", std::f64::consts::PI),
    // Tag('doublePk', length=7): wire type 1, only 7 of 8 payload bytes
    double_!(Tag!(field: "doublePk", length: 7), std::f64::consts::PI),
);

// ── test_INVALID_FIXED32 ──────────────────────────────────────────────────────

fixture!(test_INVALID_FIXED32, super::knife_descriptor();
    float_!("floatRp", std::f32::consts::PI),
    // Tag('floatRp', length=2): wire type 5, only 2 of 4 payload bytes
    float_!(Tag!(field: "floatRp", length: 2), std::f32::consts::PI),
);

// ── test_open_ended_group ─────────────────────────────────────────────────────

fixture!(test_open_ended_group, super::knife_descriptor();
    group!("groupop";
        uint64!("uint64Op", 0),
    ),
    raw!(b"\xf3\x01\x90\x08\x00\xf4\x01"),
    raw!(b"\xf3\x01\x90\x08\x00"),
);

// ── test_group_with_overhang ──────────────────────────────────────────────────

fixture!(test_group_with_overhang, super::knife_descriptor();
    group!("groupop";
        uint64!("uint64Op", 0),
    ),
    raw!(b"\xf3\x01\x90\x08\x00\xf4\x01"),
    raw!(b"\xf3\x81\x00\x90\x08\x00\xf4\x01"),
    raw!(b"\xf3\x81\x00\x90\x08\x00\xf4\x81\x00"),
    raw!(b"\xf3\x01\x90\x08\x00\xf4\x81\x00"),
    raw!(b"\xf3\x01\x90\x08\x00\xf4\x81\x80"),
);

// ── test_proto2_primitive_types ───────────────────────────────────────────────

fixture!(test_proto2_primitive_types, super::knife_descriptor();
    double_!("doubleRp", -1.7976931348623157e+308),
    double_!("doubleRp",  1.7976931348623157e+308),
    double_!("doubleRp",  f64::NAN),
    double_!("doubleRp",  f64::INFINITY),
    double_!("doubleRp",  f64::NEG_INFINITY),
    double_!("doubleRp",  5e-324),
    double_!("doubleRp", -5e-324),
    fixed64!("fixed64Rp", 0),
    fixed64!("fixed64Rp", 18446744073709551615),
    sfixed64!("sfixed64Rp", -9223372036854775808),
    sfixed64!("sfixed64Rp",  9223372036854775807),
    float_!("floatRp", -3.4028235e+38),
    float_!("floatRp",  3.4028235e+38),
    float_!("floatRp",  f32::NAN),
    float_!("floatRp",  f32::INFINITY),
    float_!("floatRp",  f32::NEG_INFINITY),
    float_!("floatRp",  1e-45),
    float_!("floatRp", -1e-45),
    fixed32!("fixed32Rp", 0),
    fixed32!("fixed32Rp", 4294967295),
    sfixed32!("sfixed32Rp", -2147483648),
    sfixed32!("sfixed32Rp",  2147483647),
    int64!("int64Rp", -9223372036854775808),
    int64!("int64Rp", 9223372036854775807),
    uint64!("uint64Rp", 0),
    uint64!("uint64Rp", 18446744073709551615),
    int32!("int32Rp", i32::MIN),
    int32!("int32Rp",  i32::MAX),
    bool_!("boolRp", false),
    bool_!("boolRp", true),
    uint32!("uint32Rp", 0),
    uint32!("uint32Rp", u32::MAX),
    int32!("enumRp", i32::MIN),
    int32!("enumRp",  i32::MAX),
    sint32!("sint32Rp", -2147483648),
    sint32!("sint32Rp",  2147483647),
    sint64!("sint64Rp", -9223372036854775808),
    sint64!("sint64Rp",  9223372036854775807),
    string!("stringRp", ""),
    bytes_!("bytesRp", b""),
);

// ── test_neg_int32_truncated ──────────────────────────────────────────────────

fixture!(test_neg_int32_truncated, super::knife_descriptor();
    // int32Rp = field 45, tag = (45<<3)|0 = 360 = \xe8\x02
    // enumRp  = field 54, tag = (54<<3)|0 = 432 = \xb0\x03
    raw!(b"\xe8\x02\x80\x80\x80\x80\x08"),          // int32Rp -2147483648 truncated 5-byte
    int32!("int32Rp", i32::MIN),               // int32Rp -2147483648 spec-correct 10-byte
    raw!(b"\xe8\x02\xff\xff\xff\xff\x0f"),            // int32Rp -1 truncated 5-byte
    int32!("int32Rp", -1),                        // int32Rp -1 spec-correct
    raw!(b"\xb0\x03\x80\x80\x80\x80\x08"),           // enumRp -2147483648 truncated 5-byte
    int32!("enumRp", i32::MIN),                // enumRp -2147483648 spec-correct
    // int32Pk packed: [1, -1(trunc), -2147483648(trunc), -1(spec), 2]
    packed_varints!("int32Pk", [
        Integer!(unsigned: 1),
        Integer!(unsigned: 0xFFFFFFFF),
        Integer!(unsigned: 0x80000000),
        Integer!(signed: -1),
        Integer!(unsigned: 2),
    ]),
    // enumPk packed: [3, -1(trunc), -2147483648(spec), -2147483648(trunc)]
    packed_varints!("enumPk", [
        Integer!(unsigned: 3),
        Integer!(unsigned: 0xFFFFFFFF),
        Integer!(signed: -2147483648),
        Integer!(unsigned: 0x80000000),
    ]),
);

// ── test_varint_required ──────────────────────────────────────────────────────

fixture!(test_varint_required, super::knife_rq_descriptor();
    uint64!(Tag!(field_num: 1, wire_type: 0), 0),
    int64!("int64Rq",     Integer!(unsigned: 0, ohb: 3)),
    uint64!("uint64Rq",   0),
    int32!("int32Rq",     0),
    bool_!("boolRq",      false),
    uint32!("uint32Rq",   0),
    enum_!("int32Rq",     0),
    sint32!("sint32Rq",   0),
    sint64!("sint64Rq",   0),
);

// ── test_varint_optional ──────────────────────────────────────────────────────

fixture!(test_varint_optional, super::knife_descriptor();
    uint64!(Tag!(field_num: 1, wire_type: 0), 0),
    int64!("int64Op",     0),
    uint64!("uint64Op",   0),
    int32!("int32Op",     0),
    bool_!("boolOp",      false),
    uint32!("uint32Op",   0),
    enum_!("int32Op",     0),
    sint32!("sint32Op",   0),
    sint64!("sint64Op",   0),
);

// ── test_varint_repeated ──────────────────────────────────────────────────────

fixture!(test_varint_repeated, super::knife_descriptor();
    uint64!(Tag!(field_num: 1, wire_type: 0), 1),
    int64!("int64Op",     2),
    uint64!("uint64Op", 3),
    int32!("int32Op",     4),
    bool_!("boolOp",      true),
    uint32!("uint32Op",   6),
    enum_!("int32Op",     7),
    sint32!("sint32Op",   8),
    sint64!("sint64Op",   9),
);

// ── test_varint_packed ────────────────────────────────────────────────────────

fixture!(test_varint_packed, super::knife_descriptor();
    raw!(b"\x82\x06\x00"),
    packed_varints!("int64Pk", []),
    packed_varints!("int64Pk", [Integer!(unsigned: 4)]),
    packed_varints!("int64Pk", [
        Integer!(unsigned: 1, ohb: 3),
        Integer!(unsigned: 2),
        Integer!(unsigned: 3),
        Integer!(unsigned: 4),
    ]),
    // UInt64(Tag(1, type=0), [1, 2, 3, 4]) — field 1, wire type 0 (not packed)
    packed_varints!(Tag!(field_num: 1, wire_type: 0), [
        Integer!(unsigned: 1),
        Integer!(unsigned: 2),
        Integer!(unsigned: 3),
        Integer!(unsigned: 4),
    ]),
    packed_varints!("uint64Pk", [
        Integer!(unsigned: 1),
        Integer!(unsigned: 2),
        Integer!(unsigned: 3),
        Integer!(unsigned: 4),
    ]),
    packed_varints!("int32Pk", [
        Integer!(unsigned: 1),
        Integer!(unsigned: 2),
        Integer!(unsigned: 3),
        Integer!(unsigned: 4),
    ]),
    packed_varints!("int32Pk", [
        Integer!(unsigned: 1 << 32),
        Integer!(unsigned: 2),
        Integer!(unsigned: 3),
        Integer!(unsigned: 4),
    ]),
    packed_varints!("boolPk", [
        Integer!(unsigned: 1),
        Integer!(unsigned: 1),
        Integer!(unsigned: 1),
        Integer!(unsigned: 1),
    ]),
    packed_varints!("uint32Pk", [
        Integer!(unsigned: 1),
        Integer!(unsigned: 2),
        Integer!(unsigned: 3),
        Integer!(unsigned: 4),
    ]),
    // Enum('int32Pk', [1, 2, 3, 4]) — field 85 (int32Pk, not enumPk)
    packed_varints!("int32Pk", [
        Integer!(unsigned: 1),
        Integer!(unsigned: 2),
        Integer!(unsigned: 3),
        Integer!(unsigned: 4),
    ]),
    packed_sint32!("sint32Pk", [1, 2, 3, 4]),
    packed_sint64!("sint64Pk", [1, 2, 3, 4]),
);

// ── test_titi ─────────────────────────────────────────────────────────────────

fixture!(test_titi, super::knife_descriptor();
    message!(Tag!(field: "messageRp", length: 10);
        string!("stringOp", "Some pamling here..."),
    ),
);

// ── test_doubly_nested ────────────────────────────────────────────────────────

fixture!(test_doubly_nested, super::knife_descriptor();
    message!("messageRp";
        message!(Tag!(field: "messageRp", length: 100);
            string!("stringOp", "Some padding here..."),
        ),
    ),
    message!("messageRp";
        message!(Tag!(field: "messageRp", length: 10);
            string!("stringOp", "Some padding here..."),
        ),
    ),
);

// ── test_2 ────────────────────────────────────────────────────────────────────

fixture!(test_2, super::knife_descriptor();
    message!("messageRp";
        raw!(b"\x82\x02\x00"),
        raw!(b"\x82\x02\x04\x00\x00\x00\x00"),
    ),
);

// ── test_2b ───────────────────────────────────────────────────────────────────

fixture!(test_2b, super::knife_descriptor();
    message!("messageRp";
        // Bytes(Tag('bytesRp', length=7, length_ohb=1), b'') — bytesRp=52
        message!(Tag!(field: "bytesRp", length: 7, length_ohb: 1);
        ),
    ),
    bytes_!("bytesRp", b"\0\0\0\0"),
);

// ── test_2c ───────────────────────────────────────────────────────────────────

fixture!(test_2c, super::knife_descriptor();
    message!("messageRp";
        uint64!("uint64Rp", RawData!(b"\x80")),
    ),
    bytes_!("bytesRp", b"hello"),
);

// ── test_3 ────────────────────────────────────────────────────────────────────

fixture!(test_3, super::knife_descriptor();
    uint64!("uint64Op", 0),
    uint64!("uint64Op", 1),
    sint64!("sint64Op", 2),
    uint64!("uint64Op", 3),
);

// ── simple ────────────────────────────────────────────────────────────────────

fixture!(simple, super::schema_simple_descriptor();
    message!("payload";
        string!("nested_payload", "Hello!"),
    ),
);

// ── overhang ──────────────────────────────────────────────────────────────────

fixture!(overhang, super::schema_overhang_descriptor();
    uint64!("noncanonical", Integer!(unsigned: 42, ohb: 3)),
);

// ── interleaved ───────────────────────────────────────────────────────────────

fixture!(interleaved, super::schema_interleaved_descriptor();
    int64!("foo",  1),
    int64!("bar", -1),
    int64!("bar", -2),
    int64!("foo",  2),
    int64!("foo",  3),
    int64!("bar", -3),
);

// ── hidden ────────────────────────────────────────────────────────────────────

fixture!(hidden, super::schema_hidden_descriptor();
    string!("foo",    "On n'a"),
    string!("hidden", "PRESQUE"),
    string!("hidden", "rien"),
    string!("bar",    "à cacher :)"),
);

// ── fdp1 / fdp2 ───────────────────────────────────────────────────────────────

fixture!(fdp1, super::fdp_descriptor();
    string!("name", "simple/fdp1.proto"),
);

fixture!(fdp2, super::fdp_descriptor();
    string!("name", "simple/fdp2.proto"),
);

// ── fdp_complex ───────────────────────────────────────────────────────────────

fixture!(fdp_complex, super::fdp_descriptor();
    string!("name",    "test/complex.proto"),
    string!("package", "test.complex"),
    string!("syntax",  "proto3"),
    message!("message_type";
        string!("name", "ComplexMessage"),
        message!("field";
            string!("name",   "id"),
            uint32!("number", 1),
            uint32!("type",   5),
        ),
        message!("field";
            string!("name",   "name"),
            uint32!("number", 2),
            uint32!("type",   9),
        ),
    ),
    message!("enum_type";
        string!("name", "Status"),
        message!("value";
            string!("name",   "UNKNOWN"),
            uint32!("number", 0),
        ),
        message!("value";
            string!("name",   "ACTIVE"),
            uint32!("number", 1),
        ),
    ),
);

// ── fdp_enum_nested ───────────────────────────────────────────────────────────

fixture!(fdp_enum_nested, super::fdp_descriptor();
    string!("name", "test/nested_enum.proto"),
    message!("message_type";
        string!("name", "Msg"),
        message!("field";
            string!("name",   "val"),
            uint32!("number", 1),
            uint32!("label",  1),
            uint32!("type",   9),
        ),
    ),
);

// ── fdp_service ───────────────────────────────────────────────────────────────

fixture!(fdp_service, super::fdp_descriptor();
    string!("name",    "test/service.proto"),
    string!("package", "test.service"),
    string!("syntax",  "proto3"),
    message!("message_type";
        string!("name", "Request"),
        message!("field";
            string!("name",   "query"),
            uint32!("number", 1),
            uint32!("type",   9),
        ),
    ),
    message!("message_type";
        string!("name", "Response"),
        message!("field";
            string!("name",   "result"),
            uint32!("number", 1),
            uint32!("type",   9),
        ),
    ),
    message!("service";
        string!("name", "SearchService"),
        message!("method";
            string!("name",        "Search"),
            string!("input_type",  "Request"),
            string!("output_type", "Response"),
        ),
    ),
);

// ── mpm_complex_message ───────────────────────────────────────────────────────

fixture!(mpm_complex_message, super::knife_descriptor();
    uint64!("uint64Rp", 12345),
    string!("stringRp", "Complex MPM test message"),
    float_!("floatRp", std::f32::consts::PI),
    bool_!("boolRp", true),
    bytes_!("bytesRp", b"Binary data for MPM testing"),
    message!("messageRp";
        uint32!("uint32Rp", 42),
        string!("stringRp", "Nested message in MPM"),
        group!(10;
            uint64!(1, 999),
            string!(2, "Group inside nested message"),
        ),
    ),
);

// ── mpm_edge_cases ────────────────────────────────────────────────────────────

fixture!(mpm_edge_cases, super::knife_descriptor();
    uint64!(1, 0),
    uint64!(2, (1u64 << 63) - 1),
    fixed64!(3, 0x123456789ABCDEF0),
    sint64!(4, -12345),
    double_!(5, f64::INFINITY),
    double_!(6, f64::NEG_INFINITY),
    double_!(7, f64::NAN),
    bytes_!(8, b""),
    string!(9, ""),
    group!(10;
    ),
);

// ── canonical fixtures ────────────────────────────────────────────────────────

fixture!(canonical_scalars, super::knife_descriptor();
    double_!("doubleOp",   std::f64::consts::E),
    float_!("floatOp",     std::f32::consts::PI),
    int64!("int64Op", -123456789),
    uint64!("uint64Op", 18446744073709551615),
    int32!("int32Op",      42),
    fixed64!("fixed64Op",  987654321),
    fixed32!("fixed32Op",  123456),
    bool_!("boolOp",       true),
    uint32!("uint32Op",    999),
    sfixed32!("sfixed32Op", -999),
    sfixed64!("sfixed64Op", -123456789),
    sint32!("sint32Op",    -42),
    sint64!("sint64Op",    123456789),
);

fixture!(canonical_strings, super::knife_descriptor();
    string!("stringOp",  "Hello, World!"),
    bytes_!("bytesOp",   b"\x00\x01\x02\x03\x04"),
    string!("stringRp",  "First"),
    string!("stringRp",  "Second"),
    string!("stringRp",  "Third"),
    bytes_!("bytesRp",   b"alpha"),
    bytes_!("bytesRp",   b"beta"),
);

fixture!(canonical_repeated, super::knife_descriptor();
    double_!("doubleRp", 1.1),
    double_!("doubleRp", 2.2),
    double_!("doubleRp", 3.3),
    int32!("int32Rp", 1),
    int32!("int32Rp", 2),
    int32!("int32Rp", 3),
    string!("stringRp", "a"),
    string!("stringRp", "b"),
    string!("stringRp", "c"),
);

fixture!(canonical_nested, super::knife_descriptor();
    int32!("int32Op", 100),
    message!("messageOp";
        int32!("int32Op", 200),
        string!("stringOp", "nested"),
    ),
    message!("messageRp";
        string!("stringOp", "first nested"),
        uint32!("uint32Op", 1),
    ),
    message!("messageRp";
        string!("stringOp", "second nested"),
        uint32!("uint32Op", 2),
    ),
);

fixture!(canonical_groups, super::knife_descriptor();
    int32!("int32Op", 42),
    group!(30;
        uint64!(130, 111),
    ),
    group!(50;
        uint64!(150, 10),
    ),
    group!(50;
        uint64!(150, 20),
    ),
);

fixture!(canonical_mixed, super::knife_descriptor();
    int64!("int64Op", 9999),
    string!("stringOp", "test"),
    message!("messageOp";
        double_!("doubleOp", 1.23e-10),
        bool_!("boolOp", false),
    ),
    bytes_!("bytesOp", b"data"),
    float_!("floatRp", 1.0),
    float_!("floatRp", 2.0),
);

fixture!(canonical_empty, super::knife_descriptor();
);

fixture!(canonical_single, super::knife_descriptor();
    int32!("int32Op", 1),
);

fixture!(canonical_unknown, super::knife_descriptor();
    int32!("int32Op", 42),
    uint32!("uint32Op", 100),
    int64!(999, 123456),
    bytes_!(1000, b"binary\x00\xff\xfe data"),
);

// ── test_groups_messages_known ────────────────────────────────────────────────

fixture!(test_groups_messages_known, super::knife_descriptor();
    int32!("int32Op", 7),
    group!("groupop";
        uint64!("uint64Op", 42),
        message!("messageOp";
            string!("stringOp", "inside groupop"),
        ),
    ),
    message!("messageOp";
        int32!("int32Op", 11),
        group!("groupop";
            uint64!("uint64Op", 99),
        ),
    ),
    group!("grouprp";
        uint64!("uint64Op", 1),
        message!("messageOp";
            string!("stringOp", "first grouprp"),
        ),
    ),
    group!("grouprp";
        uint64!("uint64Op", 2),
    ),
    message!("messageRp";
        uint32!("uint32Op", 100),
        group!("groupop";
            uint64!("uint64Op", 77),
        ),
    ),
    message!("messageRp";
        string!("stringOp", "second messagerp"),
    ),
);

// ── test_groups_messages_unknown ──────────────────────────────────────────────

fixture!(test_groups_messages_unknown, super::knife_descriptor();
    int32!("int32Op", 3),
    group!("groupop";
        uint64!("uint64Op", 10),
        uint64!(9000, 99),
    ),
    message!("messageOp";
        string!("stringOp", "in messageop"),
        bytes_!(9001, b"unknown bytes inside msg"),
    ),
    group!(999;
        uint64!(1, 7),
        string!(2, "in unknown group"),
    ),
    bytes_!(1000, b"unknown message-like bytes"),
    message!(421;
        uint32!(42, 55),
        group!(999;
            uint64!(1, 8),
        ),
    ),
);

// ── test_groups_messages_mismatch ─────────────────────────────────────────────

fixture!(test_groups_messages_mismatch, super::knife_descriptor();
    group!("messageOp";
        string!(1, "group posing as message"),
        uint64!(2, 42),
    ),
    bytes_!("groupop", b"\x82\x08\x00"),
    bytes_!("uint64Op", b"\x01\x02\x03"),
    message!("messageRp";
        string!("stringOp", "clean nested"),
    ),
);

// ── number serialization fixtures ────────────────────────────────────────────

fixture!(num_double_zero,      super::knife_descriptor(); double_!("doubleOp",  0.0));
fixture!(num_double_one,       super::knife_descriptor(); double_!("doubleOp",  1.0));
fixture!(num_double_neg_one,   super::knife_descriptor(); double_!("doubleOp", -1.0));
fixture!(num_double_pi,        super::knife_descriptor(); double_!("doubleOp",  std::f64::consts::PI));
fixture!(num_double_one_third, super::knife_descriptor(); double_!("doubleOp",  1.0 / 3.0));
fixture!(num_double_1e100,     super::knife_descriptor(); double_!("doubleOp",  1e100));
fixture!(num_double_1_23e_4,   super::knife_descriptor(); double_!("doubleOp",  1.23e-4));
fixture!(num_double_9_9e_5,    super::knife_descriptor(); double_!("doubleOp",  9.9e-5));
fixture!(num_double_1e_100,    super::knife_descriptor(); double_!("doubleOp",  1e-100));
fixture!(num_double_inf,       super::knife_descriptor(); double_!("doubleOp",  f64::INFINITY));
fixture!(num_double_neg_inf,   super::knife_descriptor(); double_!("doubleOp",  f64::NEG_INFINITY));
fixture!(num_double_nan,       super::knife_descriptor(); double_!("doubleOp",  f64::NAN));
fixture!(num_double_neg_zero,  super::knife_descriptor(); double_!("doubleOp", -0.0));

fixture!(num_float_zero,      super::knife_descriptor(); float_!("floatOp",  0.0));
fixture!(num_float_one,       super::knife_descriptor(); float_!("floatOp",  1.0));
fixture!(num_float_neg_one,   super::knife_descriptor(); float_!("floatOp", -1.0));
fixture!(num_float_pi,        super::knife_descriptor(); float_!("floatOp",  std::f32::consts::PI));
fixture!(num_float_one_third, super::knife_descriptor(); float_!("floatOp",  1.0 / 3.0));
fixture!(num_float_0_1,       super::knife_descriptor(); float_!("floatOp",  0.1));
fixture!(num_float_inf,       super::knife_descriptor(); float_!("floatOp",  f32::INFINITY));
fixture!(num_float_neg_inf,   super::knife_descriptor(); float_!("floatOp",  f32::NEG_INFINITY));
fixture!(num_float_nan,       super::knife_descriptor(); float_!("floatOp",  f32::NAN));
fixture!(num_float_neg_zero,  super::knife_descriptor(); float_!("floatOp", -0.0));

fixture!(num_int32_zero,    super::knife_descriptor(); int32!("int32Op",  0));
fixture!(num_int32_42,      super::knife_descriptor(); int32!("int32Op",  42));
fixture!(num_int32_neg_one, super::knife_descriptor(); int32!("int32Op", -1));
fixture!(num_int32_max,     super::knife_descriptor(); int32!("int32Op",  2147483647));
fixture!(num_int32_min,     super::knife_descriptor(); int32!("int32Op", -2147483648));

fixture!(num_int64_zero,    super::knife_descriptor(); int64!("int64Op", 0));
fixture!(num_int64_neg_one, super::knife_descriptor(); int64!("int64Op", -1));
fixture!(num_int64_max,     super::knife_descriptor(); int64!("int64Op", 9223372036854775807));
fixture!(num_int64_min,     super::knife_descriptor(); int64!("int64Op", -9223372036854775808));

fixture!(num_uint32_zero, super::knife_descriptor(); uint32!("uint32Op", 0));
fixture!(num_uint32_42,   super::knife_descriptor(); uint32!("uint32Op", 42));
fixture!(num_uint32_max,  super::knife_descriptor(); uint32!("uint32Op", 0xFFFFFFFF));

fixture!(num_uint64_zero,   super::knife_descriptor(); uint64!("uint64Op", 0));
fixture!(num_uint64_2pow60, super::knife_descriptor(); uint64!("uint64Op", 1152921504606846976));
fixture!(num_uint64_max,    super::knife_descriptor(); uint64!("uint64Op", 18446744073709551615));

fixture!(num_sint32_zero,    super::knife_descriptor(); sint32!("sint32Op",  0));
fixture!(num_sint32_neg_one, super::knife_descriptor(); sint32!("sint32Op", -1));
fixture!(num_sint32_one,     super::knife_descriptor(); sint32!("sint32Op",  1));
fixture!(num_sint32_neg_128, super::knife_descriptor(); sint32!("sint32Op", -128));
fixture!(num_sint32_max,     super::knife_descriptor(); sint32!("sint32Op",  2147483647));
fixture!(num_sint32_min,     super::knife_descriptor(); sint32!("sint32Op", -2147483648));

fixture!(num_sint64_neg_one, super::knife_descriptor(); sint64!("sint64Op", -1));
fixture!(num_sint64_neg_128, super::knife_descriptor(); sint64!("sint64Op", -128));
fixture!(num_sint64_max,     super::knife_descriptor(); sint64!("sint64Op",  9223372036854775807));
fixture!(num_sint64_min,     super::knife_descriptor(); sint64!("sint64Op", -9223372036854775808));

fixture!(num_fixed32_zero,     super::knife_descriptor(); fixed32!("fixed32Op", 0));
fixture!(num_fixed32_deadbeef, super::knife_descriptor(); fixed32!("fixed32Op", 0xDEADBEEF));
fixture!(num_fixed32_max,      super::knife_descriptor(); fixed32!("fixed32Op", 0xFFFFFFFF));

fixture!(num_fixed64_zero,     super::knife_descriptor(); fixed64!("fixed64Op", 0));
fixture!(num_fixed64_deadbeef, super::knife_descriptor(); fixed64!("fixed64Op", 0xDEADBEEFCAFEBABE));
fixture!(num_fixed64_max,      super::knife_descriptor(); fixed64!("fixed64Op", 0xFFFFFFFFFFFFFFFF));

fixture!(num_sfixed32_zero,    super::knife_descriptor(); sfixed32!("sfixed32Op",  0));
fixture!(num_sfixed32_neg_one, super::knife_descriptor(); sfixed32!("sfixed32Op", -1));
fixture!(num_sfixed32_42,      super::knife_descriptor(); sfixed32!("sfixed32Op",  42));

fixture!(num_sfixed64_zero,    super::knife_descriptor(); sfixed64!("sfixed64Op",  0));
fixture!(num_sfixed64_neg_one, super::knife_descriptor(); sfixed64!("sfixed64Op", -1));
fixture!(num_sfixed64_42,      super::knife_descriptor(); sfixed64!("sfixed64Op",  42));

fixture!(num_bool_true,  super::knife_descriptor(); bool_!("boolOp", true));
fixture!(num_bool_false, super::knife_descriptor(); bool_!("boolOp", false));

fixture!(num_enum_zero,    super::knife_descriptor(); int32!("enumOp",  0));
fixture!(num_enum_one,     super::knife_descriptor(); int32!("enumOp",  1));
fixture!(num_enum_neg_one, super::knife_descriptor(); int32!("enumOp", -1));

fixture!(num_mismatch_int32_as_fixed32, super::knife_descriptor();
    fixed32!("int32Op", 0xDEADBEEF),
);

fixture!(num_mismatch_int32_as_fixed64, super::knife_descriptor();
    fixed64!("int32Op", 0xDEADBEEFCAFEBABE),
);

fixture!(num_mismatch_fixed32_as_varint, super::knife_descriptor();
    uint32!("fixed32Op", 0xDEAD),
);

fixture!(num_mismatch_double_as_fixed32, super::knife_descriptor();
    fixed32!("doubleOp", 0xDEADBEEF),
);

fixture!(num_mismatch_float_as_fixed64, super::knife_descriptor();
    fixed64!("floatOp", 0xDEADBEEFCAFEBABE),
);

// ── EnumCollision fixtures ────────────────────────────────────────────────────

fixture!(enum_collision_float_kind, super::enum_collision_descriptor();
    enum_!("kind", 1),
);

fixture!(enum_collision_color_known, super::enum_collision_descriptor();
    enum_!("color", 1),
);

fixture!(enum_collision_color_unknown, super::enum_collision_descriptor();
    enum_!("unknown_color", 99),
);

// Non-packed repeated enum, but Python emits list → packed wire encoding.
fixture!(enum_collision_color_repeated, super::enum_collision_descriptor();
    packed_varints!("colors", [
        Integer!(unsigned: 0),
        Integer!(unsigned: 2),
    ]),
);

fixture!(enum_collision_color_packed, super::enum_collision_descriptor();
    packed_varints!("colors_pk", [
        Integer!(unsigned: 0),
        Integer!(unsigned: 1),
        Integer!(unsigned: 2),
    ]),
);

// F1 × C1: unknown enum value in a repeated (non-packed) field.
// Python emits list → packed wire encoding.
fixture!(enum_collision_color_unknown_repeated, super::enum_collision_descriptor();
    packed_varints!("colors", [
        Integer!(unsigned: 0),
        Integer!(unsigned: 99),
        Integer!(unsigned: 2),
    ]),
);

fixture!(enum_collision_packed_mixed, super::enum_collision_descriptor();
    packed_varints!("colors_pk", [
        Integer!(unsigned: 0),
        Integer!(unsigned: 99),
        Integer!(unsigned: 2),
    ]),
);

pub fn enum_collision_empty_packed() -> Vec<u8> {
    // Python: Enum('colors_pk', []) emits nothing for an empty packed list.
    Vec::new()
}

fixture!(enum_in_nested_message, super::enum_collision_descriptor();
    message!("nested";
        enum_!("color", 1),
        enum_!("unknown_color", 99),
    ),
);

fixture!(enum_in_group, super::enum_collision_descriptor();
    group!("enumgroup";
        enum_!("group_color", 2),
    ),
);

fixture!(string_escapes, super::knife_descriptor();
    string!("stringOp", "tab:\there\nnewline\\backslash\"quote"),
);

fixture!(string_escapes_bytes, super::knife_descriptor();
    bytes_!("bytesOp", (0u8..=255u8).collect::<Vec<_>>().as_slice()),
);

// ── test_string_wrong_len ─────────────────────────────────────────────────────
// string_with_len: explicit (wrong) length value — truncated and over-long cases.

fixture!(test_string_wrong_len, super::knife_descriptor();
    // canonical
    string!("stringOp", "hello"),
    // length declares 3 but payload is 5 bytes → truncated read
    string!(Tag!(field: "stringOp", length: 3), "hello"),
    // length declares 10 but payload is 5 bytes → over-long read
    string!(Tag!(field: "stringOp", length: 10), "hello"),
);

// ── test_packed_fixed64 ───────────────────────────────────────────────────────
// packed_fixed64 / packed_fixed64_with_len: canonical and truncated-length cases.

fixture!(test_packed_fixed64, super::knife_descriptor();
    // fixed64Pk: three fixed64 values
    packed_fixed64!("fixed64Pk", [
        0,
        0xdeadbeefdeadbeef,
        u64::MAX,
    ]),
    // Fixed64Pk(length=5): 3 elements = 24 bytes, declared length truncated to 5
    packed_fixed64!(Tag!(field: "fixed64Pk", length: 5), [
        0,
        1,
        2,
    ]),
);

// ── test_packed_double / test_packed_fixed32 / test_packed_sfixed ────────────

fixture!(test_packed_double, super::knife_descriptor();
    packed_double!("doublePk", [
        0.0,
        std::f64::consts::PI,
        f64::MAX,
    ]),
);

fixture!(test_packed_fixed32, super::knife_descriptor();
    packed_fixed32!("fixed32Pk", [
        0,
        0xdeadbeef,
        u32::MAX,
    ]),
);

fixture!(test_packed_sfixed32, super::knife_descriptor();
    packed_sfixed32!("sfixed32Pk", [
        i32::MIN,
        0,
        i32::MAX,
    ]),
);

fixture!(test_packed_sfixed64, super::knife_descriptor();
    packed_sfixed64!("sfixed64Pk", [
        i64::MIN,
        0,
        i64::MAX,
    ]),
);

// ── ALL_FIXTURES registry ─────────────────────────────────────────────────────

#[allow(clippy::type_complexity)]
pub static ALL_FIXTURES: &[(&str, fn() -> Vec<u8>)] = &[
    ("test_empty", test_empty),
    ("test_FIELD_INVALID", test_FIELD_INVALID),
    ("test_garbage", test_garbage),
    ("test_n_overhanging_bytes", test_n_overhanging_bytes),
    ("test_n_out_of_range", test_n_out_of_range),
    ("test_wire_level", test_wire_level),
    ("test_proto2_level", test_proto2_level),
    ("test_VARINT_INVALID", test_VARINT_INVALID),
    (
        "test_varint_overhanging_bytes",
        test_varint_overhanging_bytes,
    ),
    ("test_bool_out_of_range", test_bool_out_of_range),
    ("test_fixed64", test_fixed64),
    ("test_fixed64_tag", test_fixed64_tag),
    ("test_FIELD_INVALID_LENGTH", test_FIELD_INVALID_LENGTH),
    ("test_INVALID_PACKED_RECORDS", test_INVALID_PACKED_RECORDS),
    ("test_records_overhung_count", test_records_overhung_count),
    ("test_INVALID_STRING", test_INVALID_STRING),
    ("test_message_no_schema", test_message_no_schema),
    ("test_group_no_schema", test_group_no_schema),
    ("test_INVALID_GROUP_END", test_INVALID_GROUP_END),
    ("test_INVALID_FIXED64", test_INVALID_FIXED64),
    ("test_INVALID_FIXED32", test_INVALID_FIXED32),
    ("test_open_ended_group", test_open_ended_group),
    ("test_group_with_overhang", test_group_with_overhang),
    ("test_proto2_primitive_types", test_proto2_primitive_types),
    ("test_neg_int32_truncated", test_neg_int32_truncated),
    ("test_varint_required", test_varint_required),
    ("test_varint_optional", test_varint_optional),
    ("test_varint_repeated", test_varint_repeated),
    ("test_varint_packed", test_varint_packed),
    ("test_titi", test_titi),
    ("test_doubly_nested", test_doubly_nested),
    ("test_2", test_2),
    ("test_2b", test_2b),
    ("test_2c", test_2c),
    ("test_3", test_3),
    ("simple", simple),
    ("overhang", overhang),
    ("interleaved", interleaved),
    ("hidden", hidden),
    ("fdp1", fdp1),
    ("fdp2", fdp2),
    ("fdp_complex", fdp_complex),
    ("fdp_enum_nested", fdp_enum_nested),
    ("fdp_service", fdp_service),
    ("mpm_complex_message", mpm_complex_message),
    ("mpm_edge_cases", mpm_edge_cases),
    ("canonical_scalars", canonical_scalars),
    ("canonical_strings", canonical_strings),
    ("canonical_repeated", canonical_repeated),
    ("canonical_nested", canonical_nested),
    ("canonical_groups", canonical_groups),
    ("canonical_mixed", canonical_mixed),
    ("canonical_empty", canonical_empty),
    ("canonical_single", canonical_single),
    ("canonical_unknown", canonical_unknown),
    ("test_groups_messages_known", test_groups_messages_known),
    ("test_groups_messages_unknown", test_groups_messages_unknown),
    (
        "test_groups_messages_mismatch",
        test_groups_messages_mismatch,
    ),
    ("num_double_zero", num_double_zero),
    ("num_double_one", num_double_one),
    ("num_double_neg_one", num_double_neg_one),
    ("num_double_pi", num_double_pi),
    ("num_double_one_third", num_double_one_third),
    ("num_double_1e100", num_double_1e100),
    ("num_double_1_23e_4", num_double_1_23e_4),
    ("num_double_9_9e_5", num_double_9_9e_5),
    ("num_double_1e_100", num_double_1e_100),
    ("num_double_inf", num_double_inf),
    ("num_double_neg_inf", num_double_neg_inf),
    ("num_double_nan", num_double_nan),
    ("num_double_neg_zero", num_double_neg_zero),
    ("num_float_zero", num_float_zero),
    ("num_float_one", num_float_one),
    ("num_float_neg_one", num_float_neg_one),
    ("num_float_pi", num_float_pi),
    ("num_float_one_third", num_float_one_third),
    ("num_float_0_1", num_float_0_1),
    ("num_float_inf", num_float_inf),
    ("num_float_neg_inf", num_float_neg_inf),
    ("num_float_nan", num_float_nan),
    ("num_float_neg_zero", num_float_neg_zero),
    ("num_int32_zero", num_int32_zero),
    ("num_int32_42", num_int32_42),
    ("num_int32_neg_one", num_int32_neg_one),
    ("num_int32_max", num_int32_max),
    ("num_int32_min", num_int32_min),
    ("num_int64_zero", num_int64_zero),
    ("num_int64_neg_one", num_int64_neg_one),
    ("num_int64_max", num_int64_max),
    ("num_int64_min", num_int64_min),
    ("num_uint32_zero", num_uint32_zero),
    ("num_uint32_42", num_uint32_42),
    ("num_uint32_max", num_uint32_max),
    ("num_uint64_zero", num_uint64_zero),
    ("num_uint64_2pow60", num_uint64_2pow60),
    ("num_uint64_max", num_uint64_max),
    ("num_sint32_zero", num_sint32_zero),
    ("num_sint32_neg_one", num_sint32_neg_one),
    ("num_sint32_one", num_sint32_one),
    ("num_sint32_neg_128", num_sint32_neg_128),
    ("num_sint32_max", num_sint32_max),
    ("num_sint32_min", num_sint32_min),
    ("num_sint64_neg_one", num_sint64_neg_one),
    ("num_sint64_neg_128", num_sint64_neg_128),
    ("num_sint64_max", num_sint64_max),
    ("num_sint64_min", num_sint64_min),
    ("num_fixed32_zero", num_fixed32_zero),
    ("num_fixed32_deadbeef", num_fixed32_deadbeef),
    ("num_fixed32_max", num_fixed32_max),
    ("num_fixed64_zero", num_fixed64_zero),
    ("num_fixed64_deadbeef", num_fixed64_deadbeef),
    ("num_fixed64_max", num_fixed64_max),
    ("num_sfixed32_zero", num_sfixed32_zero),
    ("num_sfixed32_neg_one", num_sfixed32_neg_one),
    ("num_sfixed32_42", num_sfixed32_42),
    ("num_sfixed64_zero", num_sfixed64_zero),
    ("num_sfixed64_neg_one", num_sfixed64_neg_one),
    ("num_sfixed64_42", num_sfixed64_42),
    ("num_bool_true", num_bool_true),
    ("num_bool_false", num_bool_false),
    ("num_enum_zero", num_enum_zero),
    ("num_enum_one", num_enum_one),
    ("num_enum_neg_one", num_enum_neg_one),
    (
        "num_mismatch_int32_as_fixed32",
        num_mismatch_int32_as_fixed32,
    ),
    (
        "num_mismatch_int32_as_fixed64",
        num_mismatch_int32_as_fixed64,
    ),
    (
        "num_mismatch_fixed32_as_varint",
        num_mismatch_fixed32_as_varint,
    ),
    (
        "num_mismatch_double_as_fixed32",
        num_mismatch_double_as_fixed32,
    ),
    (
        "num_mismatch_float_as_fixed64",
        num_mismatch_float_as_fixed64,
    ),
    ("enum_collision_float_kind", enum_collision_float_kind),
    ("enum_collision_color_known", enum_collision_color_known),
    ("enum_collision_color_unknown", enum_collision_color_unknown),
    (
        "enum_collision_color_repeated",
        enum_collision_color_repeated,
    ),
    ("enum_collision_color_packed", enum_collision_color_packed),
    (
        "enum_collision_color_unknown_repeated",
        enum_collision_color_unknown_repeated,
    ),
    ("enum_collision_packed_mixed", enum_collision_packed_mixed),
    ("enum_collision_empty_packed", enum_collision_empty_packed),
    ("enum_in_nested_message", enum_in_nested_message),
    ("enum_in_group", enum_in_group),
    ("string_escapes", string_escapes),
    ("string_escapes_bytes", string_escapes_bytes),
    ("test_string_wrong_len", test_string_wrong_len),
    ("test_packed_fixed64", test_packed_fixed64),
    ("test_packed_double", test_packed_double),
    ("test_packed_fixed32", test_packed_fixed32),
    ("test_packed_sfixed32", test_packed_sfixed32),
    ("test_packed_sfixed64", test_packed_sfixed64),
];
