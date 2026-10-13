// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Descriptor structures without prost (spec 0415 S5).
//!
//! Plain structs for the parts of `descriptor.proto` the workspace reads or
//! writes without reflection: splitting a set, listing its type names,
//! building small descriptors in tests and tools. Their names and fields
//! are `prost-types`', so code moving off prost changes its imports and
//! little else.
//!
//! Decoding keeps the fields declared here and skips the rest; encoding
//! writes them in field-number order. Nothing here round-trips a descriptor
//! it did not build: a descriptor that must stay whole goes to a [`Pool`]
//! as bytes, and the pool keeps them ([`Pool::file_bytes`]).
//!
//! [`Pool`]: crate::Pool
//! [`Pool::file_bytes`]: crate::Pool::file_bytes

use std::fmt;

// ── Wire ────────────────────────────────────────────────────────────────────

/// Why bytes did not decode as the requested structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(String);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DecodeError {}

fn err<T>(msg: &str) -> Result<T, DecodeError> {
    Err(DecodeError(msg.to_string()))
}

/// One field of a message, as read off the wire.
enum Value<'a> {
    Varint(u64),
    Len(&'a [u8]),
}

/// The fields of one message, in wire order. Fixed-width fields and groups
/// are skipped (no structure here declares one); groups are skipped by
/// counting their nesting, so no input can exhaust the stack.
fn fields(buf: &[u8]) -> Result<Vec<(u32, Value<'_>)>, DecodeError> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut group_depth = 0usize;
    while pos < buf.len() {
        let Some(tag) = read_varint(buf, &mut pos) else {
            return err("malformed tag");
        };
        let (number, wire_type) = (tag >> 3, tag & 7);
        let Ok(number) = u32::try_from(number) else {
            return err("field number out of range");
        };
        if number == 0 {
            return err("field number 0");
        }
        let value = match wire_type {
            0 => match read_varint(buf, &mut pos) {
                Some(v) => Some(Value::Varint(v)),
                None => return err("malformed varint"),
            },
            1 | 5 => {
                let width = if wire_type == 1 { 8 } else { 4 };
                if buf.len() - pos < width {
                    return err("truncated fixed-width field");
                }
                pos += width;
                None
            }
            2 => {
                let Some(len) = read_varint(buf, &mut pos) else {
                    return err("malformed length");
                };
                if len > (buf.len() - pos) as u64 {
                    return err("length runs past the end");
                }
                let end = pos + len as usize;
                let payload = &buf[pos..end];
                pos = end;
                Some(Value::Len(payload))
            }
            3 => {
                group_depth += 1;
                None
            }
            4 => {
                let Some(d) = group_depth.checked_sub(1) else {
                    return err("end of a group that was never opened");
                };
                group_depth = d;
                None
            }
            _ => return err("invalid wire type"),
        };
        if let (0, Some(v)) = (group_depth, value) {
            out.push((number, v));
        }
    }
    if group_depth != 0 {
        return err("unterminated group");
    }
    Ok(out)
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for i in 0..10 {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn string(v: Value<'_>) -> Result<String, DecodeError> {
    match v {
        Value::Len(b) => match std::str::from_utf8(b) {
            Ok(s) => Ok(s.to_owned()),
            Err(_) => err("a string that is not UTF-8"),
        },
        Value::Varint(_) => err("a string field with the wrong wire type"),
    }
}

fn bytes(v: Value<'_>) -> Result<&[u8], DecodeError> {
    match v {
        Value::Len(b) => Ok(b),
        Value::Varint(_) => err("a message field with the wrong wire type"),
    }
}

fn int32(v: Value<'_>) -> Result<i32, DecodeError> {
    match v {
        // int32 is sign-extended to 64 bits on the wire; truncating is the
        // protobuf rule.
        Value::Varint(n) => Ok(n as i32),
        Value::Len(_) => err("an integer field with the wrong wire type"),
    }
}

fn boolean(v: Value<'_>) -> Result<bool, DecodeError> {
    match v {
        Value::Varint(n) => Ok(n != 0),
        Value::Len(_) => err("a bool field with the wrong wire type"),
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn put_tag(out: &mut Vec<u8>, number: u32, wire_type: u8) {
    put_varint(out, u64::from(number) << 3 | u64::from(wire_type));
}

fn put_bytes(out: &mut Vec<u8>, number: u32, b: &[u8]) {
    put_tag(out, number, 2);
    put_varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

fn put_str(out: &mut Vec<u8>, number: u32, s: &Option<String>) {
    if let Some(s) = s {
        put_bytes(out, number, s.as_bytes());
    }
}

fn put_int32(out: &mut Vec<u8>, number: u32, v: &Option<i32>) {
    if let Some(v) = *v {
        put_tag(out, number, 0);
        // Negative int32 values are sign-extended to ten bytes.
        put_varint(out, i64::from(v) as u64);
    }
}

fn put_bool(out: &mut Vec<u8>, number: u32, v: &Option<bool>) {
    if let Some(v) = *v {
        put_tag(out, number, 0);
        put_varint(out, u64::from(v));
    }
}

fn put_msg<M: Encode>(out: &mut Vec<u8>, number: u32, m: &M) {
    put_bytes(out, number, &m.encode_to_vec());
}

/// Encoding and decoding, as prost's `Message` spells them.
pub trait Encode: Sized {
    fn write(&self, out: &mut Vec<u8>);
    fn read(buf: &[u8]) -> Result<Self, DecodeError>;

    fn encode_to_vec(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    /// Append the encoding to `buf`. Infallible: the `Result` is prost's
    /// signature, so callers keep their `unwrap()`.
    fn encode(&self, buf: &mut Vec<u8>) -> Result<(), std::convert::Infallible> {
        self.write(buf);
        Ok(())
    }

    fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        Self::read(buf)
    }
}

/// `$name` gets accessors returning the field or its default, as prost's.
macro_rules! str_accessors {
    ($ty:ty { $($field:ident),* }) => {
        impl $ty {
            $(
                pub fn $field(&self) -> &str {
                    self.$field.as_deref().unwrap_or("")
                }
            )*
        }
    };
}

/// The serialized files of a `FileDescriptorSet`, as they are in it: for a
/// file that must reach a pool whole, which decoding into
/// [`FileDescriptorProto`] and re-encoding would not keep.
pub fn files_of_set(buf: &[u8]) -> Result<Vec<&[u8]>, DecodeError> {
    let mut out = Vec::new();
    for (n, v) in fields(buf)? {
        if n == 1 {
            out.push(bytes(v)?);
        }
    }
    Ok(out)
}

// ── Structures ──────────────────────────────────────────────────────────────

/// `google.protobuf.FileDescriptorSet`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileDescriptorSet {
    pub file: Vec<FileDescriptorProto>,
}

impl Encode for FileDescriptorSet {
    fn write(&self, out: &mut Vec<u8>) {
        for f in &self.file {
            put_msg(out, 1, f);
        }
    }

    fn read(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut m = FileDescriptorSet::default();
        for (n, v) in fields(buf)? {
            if n == 1 {
                m.file.push(FileDescriptorProto::read(bytes(v)?)?);
            }
        }
        Ok(m)
    }
}

/// `google.protobuf.FileDescriptorProto`, without services, options and
/// source info.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileDescriptorProto {
    pub name: Option<String>,
    pub package: Option<String>,
    pub dependency: Vec<String>,
    pub message_type: Vec<DescriptorProto>,
    pub enum_type: Vec<EnumDescriptorProto>,
    pub extension: Vec<FieldDescriptorProto>,
    pub syntax: Option<String>,
    pub edition: Option<i32>,
}

str_accessors!(FileDescriptorProto {
    name,
    package,
    syntax
});

impl Encode for FileDescriptorProto {
    fn write(&self, out: &mut Vec<u8>) {
        put_str(out, 1, &self.name);
        put_str(out, 2, &self.package);
        for d in &self.dependency {
            put_bytes(out, 3, d.as_bytes());
        }
        for m in &self.message_type {
            put_msg(out, 4, m);
        }
        for e in &self.enum_type {
            put_msg(out, 5, e);
        }
        for x in &self.extension {
            put_msg(out, 7, x);
        }
        put_str(out, 12, &self.syntax);
        put_int32(out, 14, &self.edition);
    }

    fn read(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut m = FileDescriptorProto::default();
        for (n, v) in fields(buf)? {
            match n {
                1 => m.name = Some(string(v)?),
                2 => m.package = Some(string(v)?),
                3 => m.dependency.push(string(v)?),
                4 => m.message_type.push(DescriptorProto::read(bytes(v)?)?),
                5 => m.enum_type.push(EnumDescriptorProto::read(bytes(v)?)?),
                7 => m.extension.push(FieldDescriptorProto::read(bytes(v)?)?),
                12 => m.syntax = Some(string(v)?),
                14 => m.edition = Some(int32(v)?),
                _ => {}
            }
        }
        Ok(m)
    }
}

/// `google.protobuf.DescriptorProto`, without reserved ranges and names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DescriptorProto {
    pub name: Option<String>,
    pub field: Vec<FieldDescriptorProto>,
    pub nested_type: Vec<DescriptorProto>,
    pub enum_type: Vec<EnumDescriptorProto>,
    pub extension_range: Vec<descriptor_proto::ExtensionRange>,
    pub extension: Vec<FieldDescriptorProto>,
    pub options: Option<MessageOptions>,
    pub oneof_decl: Vec<OneofDescriptorProto>,
}

str_accessors!(DescriptorProto { name });

impl Encode for DescriptorProto {
    fn write(&self, out: &mut Vec<u8>) {
        put_str(out, 1, &self.name);
        for f in &self.field {
            put_msg(out, 2, f);
        }
        for m in &self.nested_type {
            put_msg(out, 3, m);
        }
        for e in &self.enum_type {
            put_msg(out, 4, e);
        }
        for r in &self.extension_range {
            put_msg(out, 5, r);
        }
        for x in &self.extension {
            put_msg(out, 6, x);
        }
        if let Some(o) = &self.options {
            put_msg(out, 7, o);
        }
        for o in &self.oneof_decl {
            put_msg(out, 8, o);
        }
    }

    fn read(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut m = DescriptorProto::default();
        for (n, v) in fields(buf)? {
            match n {
                1 => m.name = Some(string(v)?),
                2 => m.field.push(FieldDescriptorProto::read(bytes(v)?)?),
                3 => m.nested_type.push(DescriptorProto::read(bytes(v)?)?),
                4 => m.enum_type.push(EnumDescriptorProto::read(bytes(v)?)?),
                5 => m
                    .extension_range
                    .push(descriptor_proto::ExtensionRange::read(bytes(v)?)?),
                6 => m.extension.push(FieldDescriptorProto::read(bytes(v)?)?),
                7 => m.options = Some(MessageOptions::read(bytes(v)?)?),
                8 => m.oneof_decl.push(OneofDescriptorProto::read(bytes(v)?)?),
                _ => {}
            }
        }
        Ok(m)
    }
}

pub mod descriptor_proto {
    use super::{fields, int32, put_int32, DecodeError, Encode};

    /// `google.protobuf.DescriptorProto.ExtensionRange`, without options.
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct ExtensionRange {
        pub start: Option<i32>,
        pub end: Option<i32>,
    }

    impl Encode for ExtensionRange {
        fn write(&self, out: &mut Vec<u8>) {
            put_int32(out, 1, &self.start);
            put_int32(out, 2, &self.end);
        }

        fn read(buf: &[u8]) -> Result<Self, DecodeError> {
            let mut m = ExtensionRange::default();
            for (n, v) in fields(buf)? {
                match n {
                    1 => m.start = Some(int32(v)?),
                    2 => m.end = Some(int32(v)?),
                    _ => {}
                }
            }
            Ok(m)
        }
    }
}

/// `google.protobuf.MessageOptions`: `message_set_wire_format` only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessageOptions {
    pub message_set_wire_format: Option<bool>,
}

impl Encode for MessageOptions {
    fn write(&self, out: &mut Vec<u8>) {
        put_bool(out, 1, &self.message_set_wire_format);
    }

    fn read(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut m = MessageOptions::default();
        for (n, v) in fields(buf)? {
            if n == 1 {
                m.message_set_wire_format = Some(boolean(v)?);
            }
        }
        Ok(m)
    }
}

/// `google.protobuf.FieldDescriptorProto`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldDescriptorProto {
    pub name: Option<String>,
    pub extendee: Option<String>,
    pub number: Option<i32>,
    pub label: Option<i32>,
    pub r#type: Option<i32>,
    pub type_name: Option<String>,
    pub default_value: Option<String>,
    pub options: Option<FieldOptions>,
    pub oneof_index: Option<i32>,
    pub json_name: Option<String>,
    pub proto3_optional: Option<bool>,
}

str_accessors!(FieldDescriptorProto {
    name,
    extendee,
    type_name,
    default_value,
    json_name
});

impl FieldDescriptorProto {
    pub fn number(&self) -> i32 {
        self.number.unwrap_or(0)
    }
}

impl Encode for FieldDescriptorProto {
    fn write(&self, out: &mut Vec<u8>) {
        put_str(out, 1, &self.name);
        put_str(out, 2, &self.extendee);
        put_int32(out, 3, &self.number);
        put_int32(out, 4, &self.label);
        put_int32(out, 5, &self.r#type);
        put_str(out, 6, &self.type_name);
        put_str(out, 7, &self.default_value);
        if let Some(o) = &self.options {
            put_msg(out, 8, o);
        }
        put_int32(out, 9, &self.oneof_index);
        put_str(out, 10, &self.json_name);
        put_bool(out, 17, &self.proto3_optional);
    }

    fn read(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut m = FieldDescriptorProto::default();
        for (n, v) in fields(buf)? {
            match n {
                1 => m.name = Some(string(v)?),
                2 => m.extendee = Some(string(v)?),
                3 => m.number = Some(int32(v)?),
                4 => m.label = Some(int32(v)?),
                5 => m.r#type = Some(int32(v)?),
                6 => m.type_name = Some(string(v)?),
                7 => m.default_value = Some(string(v)?),
                8 => m.options = Some(FieldOptions::read(bytes(v)?)?),
                9 => m.oneof_index = Some(int32(v)?),
                10 => m.json_name = Some(string(v)?),
                17 => m.proto3_optional = Some(boolean(v)?),
                _ => {}
            }
        }
        Ok(m)
    }
}

pub mod field_descriptor_proto {
    /// `FieldDescriptorProto.Type`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    #[repr(i32)]
    pub enum Type {
        Double = 1,
        Float = 2,
        Int64 = 3,
        Uint64 = 4,
        Int32 = 5,
        Fixed64 = 6,
        Fixed32 = 7,
        Bool = 8,
        String = 9,
        Group = 10,
        Message = 11,
        Bytes = 12,
        Uint32 = 13,
        Enum = 14,
        Sfixed32 = 15,
        Sfixed64 = 16,
        Sint32 = 17,
        Sint64 = 18,
    }

    /// `FieldDescriptorProto.Label`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    #[repr(i32)]
    pub enum Label {
        Optional = 1,
        Required = 2,
        Repeated = 3,
    }
}

/// `google.protobuf.FieldOptions`: `packed` and `deprecated` only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldOptions {
    pub packed: Option<bool>,
    pub deprecated: Option<bool>,
}

impl Encode for FieldOptions {
    fn write(&self, out: &mut Vec<u8>) {
        put_bool(out, 2, &self.packed);
        put_bool(out, 3, &self.deprecated);
    }

    fn read(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut m = FieldOptions::default();
        for (n, v) in fields(buf)? {
            match n {
                2 => m.packed = Some(boolean(v)?),
                3 => m.deprecated = Some(boolean(v)?),
                _ => {}
            }
        }
        Ok(m)
    }
}

/// `google.protobuf.OneofDescriptorProto`, without options.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OneofDescriptorProto {
    pub name: Option<String>,
}

str_accessors!(OneofDescriptorProto { name });

impl Encode for OneofDescriptorProto {
    fn write(&self, out: &mut Vec<u8>) {
        put_str(out, 1, &self.name);
    }

    fn read(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut m = OneofDescriptorProto::default();
        for (n, v) in fields(buf)? {
            if n == 1 {
                m.name = Some(string(v)?);
            }
        }
        Ok(m)
    }
}

/// `google.protobuf.EnumDescriptorProto`, without options and reserved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnumDescriptorProto {
    pub name: Option<String>,
    pub value: Vec<EnumValueDescriptorProto>,
}

str_accessors!(EnumDescriptorProto { name });

impl Encode for EnumDescriptorProto {
    fn write(&self, out: &mut Vec<u8>) {
        put_str(out, 1, &self.name);
        for v in &self.value {
            put_msg(out, 2, v);
        }
    }

    fn read(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut m = EnumDescriptorProto::default();
        for (n, v) in fields(buf)? {
            match n {
                1 => m.name = Some(string(v)?),
                2 => m.value.push(EnumValueDescriptorProto::read(bytes(v)?)?),
                _ => {}
            }
        }
        Ok(m)
    }
}

/// `google.protobuf.EnumValueDescriptorProto`, without options.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnumValueDescriptorProto {
    pub name: Option<String>,
    pub number: Option<i32>,
}

str_accessors!(EnumValueDescriptorProto { name });

impl Encode for EnumValueDescriptorProto {
    fn write(&self, out: &mut Vec<u8>) {
        put_str(out, 1, &self.name);
        put_int32(out, 2, &self.number);
    }

    fn read(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut m = EnumValueDescriptorProto::default();
        for (n, v) in fields(buf)? {
            match n {
                1 => m.name = Some(string(v)?),
                2 => m.number = Some(int32(v)?),
                _ => {}
            }
        }
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::field_descriptor_proto::{Label, Type};
    use super::*;

    fn sample() -> FileDescriptorSet {
        FileDescriptorSet {
            file: vec![FileDescriptorProto {
                name: Some("a.proto".into()),
                package: Some("pkg".into()),
                dependency: vec!["b.proto".into()],
                syntax: Some("proto2".into()),
                message_type: vec![DescriptorProto {
                    name: Some("M".into()),
                    field: vec![FieldDescriptorProto {
                        name: Some("f".into()),
                        number: Some(-1),
                        label: Some(Label::Repeated as i32),
                        r#type: Some(Type::Sint32 as i32),
                        options: Some(FieldOptions {
                            packed: Some(true),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }],
                    nested_type: vec![DescriptorProto {
                        name: Some("N".into()),
                        options: Some(MessageOptions {
                            message_set_wire_format: Some(true),
                        }),
                        ..Default::default()
                    }],
                    extension_range: vec![descriptor_proto::ExtensionRange {
                        start: Some(100),
                        end: Some(200),
                    }],
                    oneof_decl: vec![OneofDescriptorProto {
                        name: Some("o".into()),
                    }],
                    ..Default::default()
                }],
                enum_type: vec![EnumDescriptorProto {
                    name: Some("E".into()),
                    value: vec![EnumValueDescriptorProto {
                        name: Some("E_ZERO".into()),
                        number: Some(0),
                    }],
                }],
                edition: Some(1000),
                ..Default::default()
            }],
        }
    }

    #[test]
    fn what_is_encoded_decodes_the_same() {
        let set = sample();
        assert_eq!(
            FileDescriptorSet::decode(&set.encode_to_vec()).unwrap(),
            set
        );
    }

    #[test]
    fn a_negative_int32_takes_ten_bytes() {
        let f = FieldDescriptorProto {
            number: Some(-1),
            ..Default::default()
        };
        assert_eq!(f.encode_to_vec().len(), 1 + 10);
    }

    #[test]
    fn upb_reads_what_is_encoded() {
        let mut set = sample();
        // What upb refuses on its own: an import absent from the set, an
        // edition with a proto2 syntax, and a oneof with no field.
        set.file[0].dependency.clear();
        set.file[0].edition = None;
        set.file[0].message_type[0].oneof_decl.clear();
        set.file[0].message_type[0].field[0].number = Some(1);
        let pool = crate::Pool::new();
        let report = pool.load_set(&set.encode_to_vec());
        assert_eq!(report.loaded, ["a.proto"], "{report:?}");
        let m = pool.get_message_by_name("pkg.M").unwrap();
        assert!(m.get_field(1).unwrap().is_packed());
        assert!(pool
            .get_message_by_name("pkg.M.N")
            .unwrap()
            .is_message_set_wire_format());
    }

    #[test]
    fn malformed_bytes_are_an_error() {
        assert!(FileDescriptorSet::decode(b"\x0a\x05abc").is_err());
        assert!(FileDescriptorProto::decode(b"\x0a\x02\xff\xfe").is_err());
        assert!(FileDescriptorProto::decode(b"\x08\x01").is_err());
    }
}
