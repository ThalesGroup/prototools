// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// ── Data structures ───────────────────────────────────────────────────────────

/// Lossless representation of a protobuf message.
/// Corresponds to `prototext.proto / Message`.
#[derive(Debug, Default, Clone)]
pub struct ProtoTextMessage {
    /// Repeated Field (proto field 1).
    pub fields: Vec<ProtoTextField>,
}

/// Lossless representation of one protobuf field.
/// Corresponds to `prototext.proto / Field`.
#[derive(Debug, Default, Clone)]
pub struct ProtoTextField {
    // ── Metadata ─────────────────────────────────────────────────────────────
    /// Annotation strings (proto field 1, repeated string o).
    pub annotations: Vec<String>,
    /// Field number from the wire (proto field 2, optional uint64 n).
    pub field_number: Option<u64>,

    // ── Content (exactly one variant set per field) ───────────────────────────
    pub content: ProtoTextContent,

    // ── Wire-level modifiers (fields 70–78) ───────────────────────────────────
    pub tag_overhang_count: Option<u64>,
    pub tag_is_out_of_range: bool,
    pub value_overhang_count: Option<u64>,
    pub length_overhang_count: Option<u64>,
    pub missing_bytes_count: Option<u64>,
    pub mismatched_group_end: Option<u64>,
    pub open_ended_group: bool,
    pub end_tag_overhang_count: Option<u64>,
    pub end_tag_is_out_of_range: bool,

    // ── Proto2 modifiers (fields 80–82) ───────────────────────────────────────
    pub proto2_has_type_mismatch: bool,
    pub records_overhung_count: Vec<u64>,
}

/// The content of a single prototext field.
/// See DESIGN.md / ProtoTextContent enum for the proto field-number mapping.
#[derive(Debug, Clone, Default)]
pub enum ProtoTextContent {
    #[default]
    Unset,
    // Wire-level (fields 10–24)
    WireVarint(u64),                  // 10
    WireFixed64(u64),                 // 11
    WireBytes(Vec<u8>),               // 12
    WireGroup(Box<ProtoTextMessage>), // 13
    WireFixed32(u32),                 // 15
    InvalidTagType(Vec<u8>),          // 16
    InvalidVarint(Vec<u8>),           // 17
    InvalidFixed64(Vec<u8>),          // 18
    InvalidBytesLength(Vec<u8>),      // 19
    TruncatedBytes(Vec<u8>),          // 20
    InvalidPackedRecords(Vec<u8>),    // 21
    InvalidString(Vec<u8>),           // 22
    InvalidGroupEnd(Vec<u8>),         // 23
    InvalidFixed32(Vec<u8>),          // 24
    // Proto2-level (fields 31–48)
    Double(f64),                       // 31
    Float(f32),                        // 32
    Int64(i64),                        // 33
    Uint64(u64),                       // 34
    Int32(i32),                        // 35
    PFixed64(u64),                     // 36  proto2 fixed64
    PFixed32(u32),                     // 37  proto2 fixed32
    Bool(bool),                        // 38
    StringVal(String),                 // 39
    Group(Box<ProtoTextMessage>),      // 40  proto2 group
    MessageVal(Box<ProtoTextMessage>), // 41
    BytesVal(Vec<u8>),                 // 42
    Uint32(u32),                       // 43
    Enum(i32),                         // 44
    Sfixed32(i32),                     // 45
    Sfixed64(i64),                     // 46
    Sint32(i32),                       // 47
    Sint64(i64),                       // 48
    // Packed repeated (fields 51–68)
    Doubles(Vec<f64>),   // 51
    Floats(Vec<f32>),    // 52
    Int64s(Vec<i64>),    // 53
    Uint64s(Vec<u64>),   // 54
    Int32s(Vec<i32>),    // 55
    Fixed64s(Vec<u64>),  // 56
    Fixed32s(Vec<u32>),  // 57
    Bools(Vec<bool>),    // 58
    Uint32s(Vec<u32>),   // 63
    Enums(Vec<i32>),     // 64
    Sfixed32s(Vec<i32>), // 65
    Sfixed64s(Vec<i64>), // 66
    Sint32s(Vec<i32>),   // 67
    Sint64s(Vec<i64>),   // 68
}
