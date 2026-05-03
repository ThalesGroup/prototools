// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Protocraft: a test-only library for constructing protobuf wire bytes
//! programmatically, including non-canonical and malformed encodings.

// ── Constructor macros ─────────────────────────────────────────────────────────
//
// These macros mirror Python's constructor syntax:
//   Tag!(field: 44, ohb: 2)      — like Python's Tag('uint64Rp', ohb=2)
//   Integer!(value: 42, ohb: 4)  — like Python's Integer(42, ohb=4)
//   RawData!(b"\x80")            — like Python's RawData(b'\x80')
//   Message!(desc: d)            — like Python's Message(schema=d)
//   Message!()                   — like Python's Message()
//
// All fields not mentioned take their value from the DEFAULT const.

#[cfg(test)]
macro_rules! Tag {
    ($($key:ident : $val:expr),* $(,)?) => {
        #[allow(clippy::needless_update)]
        { $crate::protocraft::Tag { $($key: $val,)* ..$crate::protocraft::Tag::DEFAULT } }
    };
}

#[cfg(test)]
macro_rules! Integer {
    ($($field:ident : $val:expr),* $(,)?) => {
        #[allow(clippy::needless_update)]
        { $crate::protocraft::Integer { $($field: $val,)* ..$crate::protocraft::Integer::DEFAULT } }
    }
}

#[cfg(test)]
macro_rules! RawData {
    ($val:expr) => {
        $crate::protocraft::RawData($val)
    };
}

#[cfg(test)]
#[allow(unused_macros)]
macro_rules! Message {
    (desc: $desc:expr) => {
        $crate::protocraft::Message::with_schema($desc)
    };
    () => {
        $crate::protocraft::Message::new()
    };
}

// ── fixture! / msg_fields! macros ─────────────────────────────────────────────
//
// fixture!(name, descriptor_expr; entries...) expands to:
//   pub fn name() -> Vec<u8> { ... }
//
// msg_fields! is an internal tt-muncher that processes field entries.
// Each entry is a macro call: uint64!(...), string!(...), message!(...), etc.
//
// msg_fields! entry forms:
//   uint64!(field, val)            — varint, wire type 0
//   int64!(field, val)
//   uint32!(field, val)
//   int32!(field, val)
//   bool_!(field, val)
//   enum_!(field, val)
//   sint32!(field, val)            — zigzag-encodes val (i32)
//   sint64!(field, val)            — zigzag-encodes val (i64)
//   fixed64!(field, val)           — wire type 1, val: u64
//   sfixed64!(field, val)          — wire type 1, val: i64
//   double_!(field, val)           — wire type 1, val: f64
//   fixed32!(field, val)           — wire type 5, val: u32
//   sfixed32!(field, val)          — wire type 5, val: i32
//   float_!(field, val)            — wire type 5, val: f32
//   string!(field, val)            — wire type 2, val: &str or RawData
//   bytes_!(field, val)            — wire type 2, val: &[u8] or RawData
//   message!(field; entries...)    — nested message, wire type 2
//   group!(field; entries...)      — group, wire types 3/4
//   group!(start => end; entries...)  — mismatched group tags
//   packed_varints!(field, [...])  — packed varint field
//   packed_sint32!(field, [...])
//   packed_sint64!(field, [...])
//   packed_float!(field, [...])    — packed f32
//   packed_double!(field, [...])   — packed f64
//   packed_fixed32!(field, [...])  — packed u32
//   packed_sfixed32!(field, [...]) — packed i32
//   packed_fixed64!(field, [...])  — packed u64
//   packed_sfixed64!(field, [...]) — packed i64
//   raw!(bytes)                    — verbatim bytes

#[cfg(test)]
macro_rules! msg_fields {
    ($m:ident,) => {};

    // ── Scalar varint entries ─────────────────────────────────────────────────

    ($m:ident, uint64!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.uint64($field, $val);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, int64!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.int64($field, $val);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, uint32!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.uint32($field, $val);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, int32!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.int32($field, $val);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, bool_!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.bool_($field, $val);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, enum_!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.enum_($field, $val);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, sint32!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.sint32($field, $val as i32);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, sint64!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.sint64($field, $val as i64);
        msg_fields!($m, $($rest)*);
    }};

    // ── Fixed-width entries ───────────────────────────────────────────────────

    ($m:ident, fixed64!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.fixed64($field, $val as u64);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, sfixed64!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.sfixed64($field, $val as i64);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, double_!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.double_($field, $val as f64);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, fixed32!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.fixed32($field, $val as u32);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, sfixed32!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.sfixed32($field, $val as i32);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, float_!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.float_($field, $val as f32);
        msg_fields!($m, $($rest)*);
    }};

    // ── Length-delimited scalar entries ───────────────────────────────────────

    ($m:ident, string!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.string($field, $val);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, bytes_!($field:expr, $val:expr), $($rest:tt)*) => {{
        $m.bytes_($field, $val);
        msg_fields!($m, $($rest)*);
    }};

    // ── Nested message ────────────────────────────────────────────────────────

    ($m:ident, message!($field:expr; $($nested:tt)*), $($rest:tt)*) => {{
        let (_fnum, _mdesc) = super::FieldSpec::resolve($field, $m.desc.as_ref());
        let mut _nm = if let Some(d) = _mdesc {
            $crate::protocraft::Message::with_schema(d)
        } else {
            $crate::protocraft::Message::new()
        };
        msg_fields!(_nm, $($nested)*);
        // Pass $field directly so that Tag { length, length_ohb } are preserved.
        $m.message($field, _nm);
        msg_fields!($m, $($rest)*);
    }};

    // ── Group — single field specifier (start wire_type=3, end wire_type=4) ──

    ($m:ident, group!($field:expr; $($nested:tt)*), $($rest:tt)*) => {{
        let (_fnum, _gdesc) = super::FieldSpec::resolve($field, $m.desc.as_ref());
        let mut _nm = if let Some(d) = _gdesc {
            $crate::protocraft::Message::with_schema(d)
        } else {
            $crate::protocraft::Message::new()
        };
        msg_fields!(_nm, $($nested)*);
        $m.group(
            $crate::protocraft::Tag { field_num: _fnum, wire_type: 3, ..$crate::protocraft::Tag::DEFAULT },
            $crate::protocraft::Tag { field_num: _fnum, wire_type: 4, ..$crate::protocraft::Tag::DEFAULT },
            _nm,
        );
        msg_fields!($m, $($rest)*);
    }};

    // ── Group — explicit start => end tags (mismatched field numbers / ohb) ──

    ($m:ident, group!($start:expr => $end:expr; $($nested:tt)*), $($rest:tt)*) => {{
        let mut _nm = $crate::protocraft::Message::new();
        msg_fields!(_nm, $($nested)*);
        $m.group($start, $end, _nm);
        msg_fields!($m, $($rest)*);
    }};

    // ── Raw bytes ─────────────────────────────────────────────────────────────

    ($m:ident, raw!($val:expr), $($rest:tt)*) => {{
        $m.raw($val);
        msg_fields!($m, $($rest)*);
    }};

    // ── Packed repeated field entries ─────────────────────────────────────────

    ($m:ident, packed_varints!($field:expr, [$($val:expr),* $(,)?]), $($rest:tt)*) => {{
        $m.packed_varints($field, &[$($val),*]);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, packed_sint32!($field:expr, [$($val:expr),* $(,)?]), $($rest:tt)*) => {{
        $m.packed_sint32($field, &[$($val),*]);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, packed_sint64!($field:expr, [$($val:expr),* $(,)?]), $($rest:tt)*) => {{
        $m.packed_sint64($field, &[$($val),*]);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, packed_float!($field:expr, [$($val:expr),* $(,)?]), $($rest:tt)*) => {{
        $m.packed_float($field, vec![$($val),*]);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, packed_double!($field:expr, [$($val:expr),* $(,)?]), $($rest:tt)*) => {{
        $m.packed_double($field, vec![$($val),*]);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, packed_fixed32!($field:expr, [$($val:expr),* $(,)?]), $($rest:tt)*) => {{
        $m.packed_fixed32($field, vec![$($val),*]);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, packed_sfixed32!($field:expr, [$($val:expr),* $(,)?]), $($rest:tt)*) => {{
        $m.packed_sfixed32($field, vec![$($val),*]);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, packed_fixed64!($field:expr, [$($val:expr),* $(,)?]), $($rest:tt)*) => {{
        $m.packed_fixed64($field, vec![$($val),*]);
        msg_fields!($m, $($rest)*);
    }};
    ($m:ident, packed_sfixed64!($field:expr, [$($val:expr),* $(,)?]), $($rest:tt)*) => {{
        $m.packed_sfixed64($field, vec![$($val),*]);
        msg_fields!($m, $($rest)*);
    }};

    // ── Trailing-comma-less last entry ────────────────────────────────────────

    ($m:ident, $($last:tt)+) => {
        msg_fields!($m, $($last)+,);
    };
}

#[cfg(test)]
macro_rules! fixture {
    ($name:ident, $schema:expr; $($fields:tt)*) => {
        #[allow(non_snake_case)]
        pub fn $name() -> Vec<u8> {
            let mut _m = $crate::protocraft::Message::with_schema($schema);
            msg_fields!(_m, $($fields)*);
            _m.build()
        }
    };
}

pub mod craft_a;

#[cfg(test)]
use prost_reflect::{DescriptorPool, Kind, MessageDescriptor};

// ── Core types ────────────────────────────────────────────────────────────────

/// A wire tag with field specifier, wire type, overhanging bytes, and optional
/// length/length_ohb overrides for length-delimited fields.
///
/// Field resolution at `into_field_tag()` time:
/// - `field != ""` → if all-digit, use as field number; otherwise name-lookup
/// - `field == ""` → use `field_num` directly
#[derive(Clone, Copy)]
pub struct Tag {
    pub field: &'static str,
    pub field_num: u64,
    pub wire_type: u8,
    pub ohb: u8,
    pub length: usize,
    pub length_ohb: u8,
}

impl Tag {
    /// `wire_type: u8::MAX` in DEFAULT means "use the caller's default wire type".
    pub const DEFAULT: Self = Self {
        field: "",
        field_num: 0,
        wire_type: u8::MAX,
        ohb: 0,
        length: usize::MAX,
        length_ohb: 0,
    };
}

/// An integer value with optional overhanging bytes (0 = canonical).
///
/// Exactly one of `unsigned`, `signed`, `zigzag` should be non-zero
/// (if more than one are non-zero, `into_bytes` panics).
/// If all are zero the result is a zero varint regardless of which was intended.
///
/// - `unsigned`: raw u64 bit pattern — encode as-is.
/// - `signed`:   i64 value — sign-extend to u64 (`short: false`) or truncate
///   to i32→u32→u64 (`short: true`).
/// - `zigzag`:   i64 value — zigzag-encode then store as u64.
/// - `short`:    only meaningful with `signed`; truncates to 32-bit encoding.
/// - `ohb`:      overhanging bytes appended after the varint.
#[derive(Clone, Copy)]
pub struct Integer {
    pub unsigned: u64,
    pub signed: i64,
    pub zigzag: i64,
    pub short: bool,
    pub ohb: u8,
}

impl Integer {
    pub const DEFAULT: Self = Self {
        unsigned: 0,
        signed: 0,
        zigzag: 0,
        short: false,
        ohb: 0,
    };
}

/// Raw bytes used as a field value, bypassing all encoding.
/// The tag (and length prefix for LEN fields) is still emitted normally.
#[derive(Clone, Copy)]
pub struct RawData<'a>(pub &'a [u8]);

// ── IntoFieldTag: unified field + descriptor resolution for builder methods ───
//
// Builder methods accept `impl IntoFieldTag` so that Tag, integers, and
// (in test builds) &str name-resolved arguments are accepted uniformly.

/// Resolves a field specifier to a `Tag`, using the message descriptor if needed.
pub trait IntoFieldTag {
    fn into_field_tag(
        self,
        default_wire_type: u8,
        #[cfg(test)] desc: Option<&MessageDescriptor>,
        #[cfg(not(test))] _desc: Option<&()>,
    ) -> Tag;
}

macro_rules! impl_into_field_tag_int {
    ($($t:ty),*) => {
        $(
            impl IntoFieldTag for $t {
                fn into_field_tag(
                    self,
                    default_wire_type: u8,
                    #[cfg(test)] _desc: Option<&MessageDescriptor>,
                    #[cfg(not(test))] _desc: Option<&()>,
                ) -> Tag {
                    Tag {
                        field: "",
                        field_num: self as u64,
                        wire_type: default_wire_type,
                        ..Tag::DEFAULT
                    }
                }
            }
        )*
    };
}

impl_into_field_tag_int!(u64, u32, u16, u8, i64, i32, i16, usize);

/// Resolve a field name or numeric string to a field number.
/// - all-digit string → parse as u64
/// - otherwise → look up by name in `desc`
#[cfg(test)]
fn resolve_field_name(name: &str, desc: Option<&MessageDescriptor>) -> u64 {
    if let Ok(n) = name.parse::<u64>() {
        return n;
    }
    let d = desc.unwrap_or_else(|| {
        panic!(
            "no descriptor bound on message, cannot resolve field '{}'",
            name
        )
    });
    d.get_field_by_name(name)
        .unwrap_or_else(|| panic!("field '{}' not found in descriptor '{}'", name, d.name()))
        .number() as u64
}

/// Resolve a `Tag`'s field specifier to a concrete field number.
/// - `field != ""`: resolve via `resolve_field_name`
/// - `field == ""`: use `field_num` directly.
#[cfg(test)]
fn resolve_tag_field(tag: &Tag, default_wire_type: u8, desc: Option<&MessageDescriptor>) -> Tag {
    let field_num = if tag.field.is_empty() {
        tag.field_num
    } else {
        resolve_field_name(tag.field, desc)
    };
    Tag {
        field: "",
        field_num,
        wire_type: if tag.wire_type == u8::MAX {
            default_wire_type
        } else {
            tag.wire_type
        },
        ..*tag
    }
}

impl IntoFieldTag for Tag {
    fn into_field_tag(
        self,
        default_wire_type: u8,
        #[cfg(test)] desc: Option<&MessageDescriptor>,
        #[cfg(not(test))] _desc: Option<&()>,
    ) -> Tag {
        #[cfg(test)]
        return resolve_tag_field(&self, default_wire_type, desc);
        #[cfg(not(test))]
        return self;
    }
}

#[cfg(test)]
impl IntoFieldTag for &str {
    fn into_field_tag(self, default_wire_type: u8, desc: Option<&MessageDescriptor>) -> Tag {
        Tag {
            field: "",
            field_num: resolve_field_name(self, desc),
            wire_type: default_wire_type,
            ..Tag::DEFAULT
        }
    }
}

// ── FieldSpec trait ───────────────────────────────────────────────────────────

/// Resolves a field specifier to `(field_number, nested_descriptor)`.
/// Used by schema-aware message/group builder arms.
#[cfg(test)]
pub trait FieldSpec {
    fn resolve(self, desc: Option<&MessageDescriptor>) -> (u64, Option<MessageDescriptor>);
}

#[cfg(test)]
impl FieldSpec for &str {
    fn resolve(self, desc: Option<&MessageDescriptor>) -> (u64, Option<MessageDescriptor>) {
        let d = desc.unwrap_or_else(|| panic!("no descriptor bound, cannot resolve '{}'", self));
        let fd = d
            .get_field_by_name(self)
            .unwrap_or_else(|| panic!("field '{}' not found in descriptor '{}'", self, d.name()));
        let nested = match fd.kind() {
            Kind::Message(m) => Some(m),
            _ => None,
        };
        (fd.number() as u64, nested)
    }
}

macro_rules! impl_field_spec_int {
    ($($t:ty),*) => {
        $(
            #[cfg(test)]
            impl FieldSpec for $t {
                fn resolve(self, _desc: Option<&MessageDescriptor>) -> (u64, Option<MessageDescriptor>) {
                    (self as u64, None)
                }
            }
        )*
    };
}

impl_field_spec_int!(u64, u32, u16, u8, i64, i32, i16, usize);

#[cfg(test)]
impl FieldSpec for Tag {
    fn resolve(self, desc: Option<&MessageDescriptor>) -> (u64, Option<MessageDescriptor>) {
        if !self.field.is_empty() {
            // Named field: delegate to &str impl to get nested descriptor too.
            self.field.resolve(desc)
        } else {
            (self.field_num, None)
        }
    }
}

// ── IntoBytes: encodes a field value to wire bytes ────────────────────────────

pub trait IntoBytes {
    fn into_bytes(self) -> Vec<u8>;
}

impl IntoBytes for u64 {
    fn into_bytes(self) -> Vec<u8> {
        encode_varint_ohb(self, 0)
    }
}
impl IntoBytes for i64 {
    fn into_bytes(self) -> Vec<u8> {
        encode_varint_ohb(self as u64, 0)
    }
}
impl IntoBytes for u32 {
    fn into_bytes(self) -> Vec<u8> {
        encode_varint_ohb(self as u64, 0)
    }
}
impl IntoBytes for i32 {
    fn into_bytes(self) -> Vec<u8> {
        encode_varint_ohb(self as i64 as u64, 0)
    }
}
impl IntoBytes for bool {
    fn into_bytes(self) -> Vec<u8> {
        encode_varint_ohb(self as u64, 0)
    }
}
impl IntoBytes for Integer {
    fn into_bytes(self) -> Vec<u8> {
        let nonzero_count =
            (self.unsigned != 0) as u8 + (self.signed != 0) as u8 + (self.zigzag != 0) as u8;
        assert!(
            nonzero_count <= 1,
            "Integer: at most one of unsigned/signed/zigzag may be non-zero"
        );
        let value = if self.signed != 0 {
            if self.short {
                self.signed as i32 as u32 as u64
            } else {
                self.signed as u64
            }
        } else if self.zigzag != 0 {
            let v = self.zigzag;
            ((v << 1) ^ (v >> 63)) as u64
        } else {
            self.unsigned
        };
        encode_varint_ohb(value, self.ohb)
    }
}
impl<'a> IntoBytes for RawData<'a> {
    fn into_bytes(self) -> Vec<u8> {
        self.0.to_vec()
    }
}

// ── Per-kind varint traits ────────────────────────────────────────────────────
//
// Each proto varint kind gets its own trait so that bare integer literals are
// unambiguous at call sites (e.g. `uint64!("f", 3)` infers `3` as `u64`).
// Each trait is implemented for the natural primitive, `Integer` (for ohb
// overrides), and `RawData` (escape-hatch for malformed-varint fixtures).

macro_rules! define_varint_trait {
    ($trait:ident, $prim:ty, $conv:expr) => {
        pub trait $trait {
            fn into_bytes(self) -> Vec<u8>;
        }
        impl $trait for $prim {
            fn into_bytes(self) -> Vec<u8> {
                ($conv)(self)
            }
        }
        impl $trait for Integer {
            fn into_bytes(self) -> Vec<u8> {
                IntoBytes::into_bytes(self)
            }
        }
        impl<'_a> $trait for RawData<'_a> {
            fn into_bytes(self) -> Vec<u8> {
                self.0.to_vec()
            }
        }
    };
}

define_varint_trait!(IntoInt32, i32, |v: i32| encode_varint_ohb(
    v as i64 as u64,
    0
));
define_varint_trait!(IntoInt64, i64, |v: i64| encode_varint_ohb(v as u64, 0));
define_varint_trait!(IntoUint32, u32, |v: u32| encode_varint_ohb(v as u64, 0));
define_varint_trait!(IntoUint64, u64, |v: u64| encode_varint_ohb(v, 0));
define_varint_trait!(IntoBool, bool, |v: bool| encode_varint_ohb(v as u64, 0));
define_varint_trait!(IntoEnum, i32, |v: i32| encode_varint_ohb(
    v as i64 as u64,
    0
));

// ── IntoStringPayload / IntoBytesPayload ──────────────────────────────────────
//
// `into_payload` returns `(bytes, raw)` where `raw = true` means the bytes
// replace the *entire* length-delimited encoding (length prefix + payload),
// and only the tag is prepended.  This mirrors Python's behaviour where
// `RawData` as a field value bypasses both length encoding and payload
// encoding, emitting only the raw bytes after the tag.

pub trait IntoStringPayload {
    fn into_payload(self) -> (Vec<u8>, bool);
}
impl IntoStringPayload for &str {
    fn into_payload(self) -> (Vec<u8>, bool) {
        (self.as_bytes().to_vec(), false)
    }
}
impl<'a> IntoStringPayload for RawData<'a> {
    fn into_payload(self) -> (Vec<u8>, bool) {
        (self.0.to_vec(), true)
    }
}

pub trait IntoBytesPayload {
    fn into_payload(self) -> (Vec<u8>, bool);
}
impl IntoBytesPayload for &[u8] {
    fn into_payload(self) -> (Vec<u8>, bool) {
        (self.to_vec(), false)
    }
}
impl<const N: usize> IntoBytesPayload for &[u8; N] {
    fn into_payload(self) -> (Vec<u8>, bool) {
        (self.to_vec(), false)
    }
}
impl<'a> IntoBytesPayload for RawData<'a> {
    fn into_payload(self) -> (Vec<u8>, bool) {
        (self.0.to_vec(), true)
    }
}

// ── Varint encoding ───────────────────────────────────────────────────────────

/// Encode a varint with `ohb` extra continuation bytes appended.
/// ohb=0 produces canonical minimal encoding.
pub fn encode_varint_ohb(value: u64, ohb: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(11);
    let mut v = value;
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            if ohb == 0 {
                out.push(byte);
                break;
            } else {
                out.push(byte | 0x80);
                // Emit ohb - 1 continuation bytes of 0x80, then one 0x00.
                out.extend(std::iter::repeat_n(0x80u8, (ohb - 1) as usize));
                out.push(0x00);
                break;
            }
        } else {
            out.push(byte | 0x80);
        }
    }
    out
}

/// Encode a wire tag: (field_number << 3) | wire_type, as a varint with
/// optional overhanging bytes.
pub fn encode_tag(field: u64, wire_type: u8, ohb: u8) -> Vec<u8> {
    let tag_value = (field << 3) | (wire_type as u64);
    encode_varint_ohb(tag_value, ohb)
}

// ── Scalar encoders ───────────────────────────────────────────────────────────

fn encode_sint32(v: i32) -> u64 {
    ((v << 1) ^ (v >> 31)) as u32 as u64
}

fn encode_sint64(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

// ── Descriptor helper functions ───────────────────────────────────────────────

/// Load a `MessageDescriptor` for `SwissArmyKnife` from the compiled schema.
#[cfg(test)]
pub fn knife_descriptor() -> MessageDescriptor {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/knife.pb"));
    let pool = DescriptorPool::decode(bytes.as_ref()).expect("knife.pb decode failed");
    pool.get_message_by_name("SwissArmyKnife")
        .expect("SwissArmyKnife not found in knife.pb")
}

/// Load a `MessageDescriptor` for `SwissArmyKnifeRq` from the compiled schema.
#[cfg(test)]
pub fn knife_rq_descriptor() -> MessageDescriptor {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/knife.pb"));
    let pool = DescriptorPool::decode(bytes.as_ref()).expect("knife.pb decode failed");
    pool.get_message_by_name("SwissArmyKnifeRq")
        .expect("SwissArmyKnifeRq not found in knife.pb")
}

/// Load a `MessageDescriptor` for `EnumCollision` from the compiled schema.
#[cfg(test)]
pub fn enum_collision_descriptor() -> MessageDescriptor {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/enum_collision.pb"));
    let pool = DescriptorPool::decode(bytes.as_ref()).expect("enum_collision.pb decode failed");
    pool.get_message_by_name("EnumCollision")
        .expect("EnumCollision not found in enum_collision.pb")
}

/// Load a `MessageDescriptor` for `SchemaSimple` from the compiled schema.
#[cfg(test)]
pub fn schema_simple_descriptor() -> MessageDescriptor {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/knife.pb"));
    let pool = DescriptorPool::decode(bytes.as_ref()).expect("knife.pb decode failed");
    pool.get_message_by_name("SchemaSimple")
        .expect("SchemaSimple not found in knife.pb")
}

/// Load a `MessageDescriptor` for `SchemaOverhang` from the compiled schema.
#[cfg(test)]
pub fn schema_overhang_descriptor() -> MessageDescriptor {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/knife.pb"));
    let pool = DescriptorPool::decode(bytes.as_ref()).expect("knife.pb decode failed");
    pool.get_message_by_name("SchemaOverhang")
        .expect("SchemaOverhang not found in knife.pb")
}

/// Load a `MessageDescriptor` for `SchemaInterleaved` from the compiled schema.
#[cfg(test)]
pub fn schema_interleaved_descriptor() -> MessageDescriptor {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/knife.pb"));
    let pool = DescriptorPool::decode(bytes.as_ref()).expect("knife.pb decode failed");
    pool.get_message_by_name("SchemaInterleaved")
        .expect("SchemaInterleaved not found in knife.pb")
}

/// Load a `MessageDescriptor` for `SchemaHidden` from the compiled schema.
#[cfg(test)]
pub fn schema_hidden_descriptor() -> MessageDescriptor {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/knife.pb"));
    let pool = DescriptorPool::decode(bytes.as_ref()).expect("knife.pb decode failed");
    pool.get_message_by_name("SchemaHidden")
        .expect("SchemaHidden not found in knife.pb")
}

/// Load a `MessageDescriptor` for `google.protobuf.FileDescriptorProto`.
#[cfg(test)]
pub fn fdp_descriptor() -> MessageDescriptor {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/descriptor.pb"));
    let pool = DescriptorPool::decode(bytes.as_ref()).expect("descriptor.pb decode failed");
    pool.get_message_by_name("google.protobuf.FileDescriptorProto")
        .expect("FileDescriptorProto not found in descriptor.pb")
}

// ── Message builder ───────────────────────────────────────────────────────────

/// A message builder accumulating wire bytes.
pub struct Message {
    buf: Vec<u8>,
    #[cfg(test)]
    pub desc: Option<MessageDescriptor>,
}

impl Default for Message {
    fn default() -> Self {
        Self::new()
    }
}

impl Message {
    pub fn new() -> Self {
        Message {
            buf: Vec::new(),
            #[cfg(test)]
            desc: None,
        }
    }

    /// Construct a schema-bound message; field names are resolved against `desc`.
    #[cfg(test)]
    pub fn with_schema(desc: MessageDescriptor) -> Self {
        Message {
            buf: Vec::new(),
            desc: Some(desc),
        }
    }

    /// Return the accumulated wire bytes.
    pub fn build(self) -> Vec<u8> {
        self.buf
    }

    /// Append raw bytes verbatim (no tag, no length prefix).
    pub fn raw(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Return a reference to the bound descriptor (test-only helper).
    #[cfg(test)]
    fn desc_ref(&self) -> Option<&MessageDescriptor> {
        self.desc.as_ref()
    }

    /// Non-test stub: always returns None.
    #[cfg(not(test))]
    #[allow(dead_code)]
    fn desc_ref(&self) -> Option<&()> {
        None
    }

    // ── Varint fields (wire type 0) ──────────────────────────────────────────

    fn add_varint_bytes(&mut self, tag: Tag, bytes: Vec<u8>) {
        self.buf
            .extend(encode_tag(tag.field_num, tag.wire_type, tag.ohb));
        self.buf.extend(bytes);
    }

    pub fn int32(&mut self, field: impl IntoFieldTag, value: impl IntoInt32) {
        self.add_varint_bytes(field.into_field_tag(0, self.desc_ref()), value.into_bytes());
    }

    pub fn int64(&mut self, field: impl IntoFieldTag, value: impl IntoInt64) {
        self.add_varint_bytes(field.into_field_tag(0, self.desc_ref()), value.into_bytes());
    }

    pub fn uint32(&mut self, field: impl IntoFieldTag, value: impl IntoUint32) {
        self.add_varint_bytes(field.into_field_tag(0, self.desc_ref()), value.into_bytes());
    }

    pub fn uint64(&mut self, field: impl IntoFieldTag, value: impl IntoUint64) {
        self.add_varint_bytes(field.into_field_tag(0, self.desc_ref()), value.into_bytes());
    }

    pub fn bool_(&mut self, field: impl IntoFieldTag, value: impl IntoBool) {
        self.add_varint_bytes(field.into_field_tag(0, self.desc_ref()), value.into_bytes());
    }

    pub fn enum_(&mut self, field: impl IntoFieldTag, value: impl IntoEnum) {
        self.add_varint_bytes(field.into_field_tag(0, self.desc_ref()), value.into_bytes());
    }

    pub fn sint32(&mut self, field: impl IntoFieldTag, value: i32) {
        let encoded = encode_sint32(value);
        self.add_varint_bytes(
            field.into_field_tag(0, self.desc_ref()),
            encode_varint_ohb(encoded, 0),
        );
    }

    pub fn sint64(&mut self, field: impl IntoFieldTag, value: i64) {
        let encoded = encode_sint64(value);
        self.add_varint_bytes(
            field.into_field_tag(0, self.desc_ref()),
            encode_varint_ohb(encoded, 0),
        );
    }

    // ── Fixed64 fields (wire type 1) ─────────────────────────────────────────

    fn add_fixed64_field(&mut self, tag: Tag, bytes: [u8; 8]) {
        self.buf
            .extend(encode_tag(tag.field_num, tag.wire_type, tag.ohb));
        let emit_len = tag.length.min(8);
        self.buf.extend_from_slice(&bytes[..emit_len]);
    }

    pub fn fixed64(&mut self, field: impl IntoFieldTag, value: u64) {
        self.add_fixed64_field(
            field.into_field_tag(1, self.desc_ref()),
            value.to_le_bytes(),
        );
    }

    pub fn sfixed64(&mut self, field: impl IntoFieldTag, value: i64) {
        self.add_fixed64_field(
            field.into_field_tag(1, self.desc_ref()),
            value.to_le_bytes(),
        );
    }

    pub fn double_(&mut self, field: impl IntoFieldTag, value: f64) {
        self.add_fixed64_field(
            field.into_field_tag(1, self.desc_ref()),
            value.to_le_bytes(),
        );
    }

    // ── Length-delimited fields (wire type 2) ────────────────────────────────

    fn add_len_field(&mut self, tag: Tag, payload: &[u8]) {
        self.buf
            .extend(encode_tag(tag.field_num, tag.wire_type, tag.ohb));
        let len = if tag.length == usize::MAX {
            payload.len()
        } else {
            tag.length
        };
        let len_ohb = tag.length_ohb;
        self.buf.extend(encode_varint_ohb(len as u64, len_ohb));
        let emit_len = len.min(payload.len());
        self.buf.extend_from_slice(&payload[..emit_len]);
    }

    pub fn bytes_(&mut self, field: impl IntoFieldTag, value: impl IntoBytesPayload) {
        let tag = field.into_field_tag(2, self.desc_ref());
        let (payload, raw) = value.into_payload();
        if raw {
            self.buf
                .extend(encode_tag(tag.field_num, tag.wire_type, tag.ohb));
            self.buf.extend_from_slice(&payload);
        } else {
            self.add_len_field(tag, &payload);
        }
    }

    pub fn string(&mut self, field: impl IntoFieldTag, value: impl IntoStringPayload) {
        let tag = field.into_field_tag(2, self.desc_ref());
        let (payload, raw) = value.into_payload();
        if raw {
            self.buf
                .extend(encode_tag(tag.field_num, tag.wire_type, tag.ohb));
            self.buf.extend_from_slice(&payload);
        } else {
            self.add_len_field(tag, &payload);
        }
    }

    /// Append a nested message (length-delimited).
    pub fn message(&mut self, field: impl IntoFieldTag, nested: Message) {
        let payload = nested.build();
        let tag = field.into_field_tag(2, self.desc_ref());
        self.add_len_field(tag, &payload);
    }

    // ── Group fields (wire type 3 + 4) ───────────────────────────────────────

    /// Append a group (start tag + contents + end tag).
    pub fn group(&mut self, start: impl IntoFieldTag, end: impl IntoFieldTag, nested: Message) {
        let start_tag = start.into_field_tag(3, self.desc_ref());
        let end_tag = end.into_field_tag(4, self.desc_ref());
        self.buf.extend(encode_tag(
            start_tag.field_num,
            start_tag.wire_type,
            start_tag.ohb,
        ));
        self.buf.extend(nested.build());
        self.buf.extend(encode_tag(
            end_tag.field_num,
            end_tag.wire_type,
            end_tag.ohb,
        ));
    }

    // ── Fixed32 fields (wire type 5) ─────────────────────────────────────────

    fn add_fixed32_field(&mut self, tag: Tag, bytes: [u8; 4]) {
        self.buf
            .extend(encode_tag(tag.field_num, tag.wire_type, tag.ohb));
        let emit_len = tag.length.min(4);
        self.buf.extend_from_slice(&bytes[..emit_len]);
    }

    pub fn fixed32(&mut self, field: impl IntoFieldTag, value: u32) {
        self.add_fixed32_field(
            field.into_field_tag(5, self.desc_ref()),
            value.to_le_bytes(),
        );
    }

    pub fn sfixed32(&mut self, field: impl IntoFieldTag, value: i32) {
        self.add_fixed32_field(
            field.into_field_tag(5, self.desc_ref()),
            value.to_le_bytes(),
        );
    }

    pub fn float_(&mut self, field: impl IntoFieldTag, value: f32) {
        self.add_fixed32_field(
            field.into_field_tag(5, self.desc_ref()),
            value.to_le_bytes(),
        );
    }

    // ── Packed repeated fields ───────────────────────────────────────────────

    /// Packed varints (int32, int64, uint32, uint64, bool, enum).
    pub fn packed_varints(&mut self, field: impl IntoFieldTag, values: &[Integer]) {
        let tag = field.into_field_tag(2, self.desc_ref());
        let mut payload = Vec::new();
        for v in values {
            payload.extend(IntoBytes::into_bytes(*v));
        }
        self.add_len_field(tag, &payload);
    }

    /// Packed sint32 (zigzag encoded).
    pub fn packed_sint32(&mut self, field: impl IntoFieldTag, values: &[i32]) {
        let integers: Vec<Integer> = values
            .iter()
            .map(|&v| Integer {
                unsigned: encode_sint32(v),
                ..Integer::DEFAULT
            })
            .collect();
        self.packed_varints(field, &integers);
    }

    /// Packed sint64 (zigzag encoded).
    pub fn packed_sint64(&mut self, field: impl IntoFieldTag, values: &[i64]) {
        let integers: Vec<Integer> = values
            .iter()
            .map(|&v| Integer {
                unsigned: encode_sint64(v),
                ..Integer::DEFAULT
            })
            .collect();
        self.packed_varints(field, &integers);
    }

    /// Packed float (wire type 2, fixed32 payload).
    pub fn packed_float(&mut self, field: impl IntoFieldTag, values: Vec<f32>) {
        let tag = field.into_field_tag(2, self.desc_ref());
        let payload: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.add_len_field(tag, &payload);
    }

    /// Packed double (wire type 2, fixed64 payload).
    pub fn packed_double(&mut self, field: impl IntoFieldTag, values: Vec<f64>) {
        let tag = field.into_field_tag(2, self.desc_ref());
        let payload: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.add_len_field(tag, &payload);
    }

    /// Packed fixed32 (wire type 2, u32 payload).
    pub fn packed_fixed32(&mut self, field: impl IntoFieldTag, values: Vec<u32>) {
        let tag = field.into_field_tag(2, self.desc_ref());
        let payload: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.add_len_field(tag, &payload);
    }

    /// Packed sfixed32 (wire type 2, i32 payload).
    pub fn packed_sfixed32(&mut self, field: impl IntoFieldTag, values: Vec<i32>) {
        let tag = field.into_field_tag(2, self.desc_ref());
        let payload: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.add_len_field(tag, &payload);
    }

    /// Packed fixed64 (wire type 2, u64 payload).
    pub fn packed_fixed64(&mut self, field: impl IntoFieldTag, values: Vec<u64>) {
        let tag = field.into_field_tag(2, self.desc_ref());
        let payload: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.add_len_field(tag, &payload);
    }

    /// Packed sfixed64 (wire type 2, i64 payload).
    pub fn packed_sfixed64(&mut self, field: impl IntoFieldTag, values: Vec<i64>) {
        let tag = field.into_field_tag(2, self.desc_ref());
        let payload: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.add_len_field(tag, &payload);
    }
}
