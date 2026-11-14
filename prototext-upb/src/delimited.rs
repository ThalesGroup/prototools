// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! protoc's text of unknown fields, re-framed once their declarations are
//! known (spec 0426).
//!
//! protoc, without a schema, prints a group, a length-delimited message,
//! and a string whose bytes happen to read as a message alike: `N { … }`.
//! [`text_format::parse`](crate::text_format::parse) frames each one as a
//! group. Once every file of an input is read, [`Declarations`] holds what
//! each one declares, and [`correct`] re-frames, in a file read from text,
//! each group by number as its declaration says, and warns where none
//! settles it. Everything it does not change is copied byte for byte.
//!
//! Only for bytes [`text_format::parse`](crate::text_format::parse)
//! produced: a group in a binary file is a group, whatever is declared.

use std::collections::HashMap;
use std::fmt;

use crate::descriptor_access::{
    feature_set_message_encoding as encoding, field_descriptor_proto_type as ty, DescriptorProto,
    FieldDescriptorProto, FileDescriptorProto,
};
use crate::{DynamicMessage, Kind, MessageError, Pool};

/// What a declaration says of a field's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declared {
    /// A message of `type_name` (fully qualified, without the leading
    /// dot), framed as a group when `delimited`.
    Message { delimited: bool, type_name: String },
    /// A string or bytes.
    Bytes,
    /// Any other scalar.
    Scalar,
}

/// The fields and extensions an input's files declare.
#[derive(Debug, Clone, Default)]
pub struct Declarations {
    /// Message type → field number → declaration.
    messages: HashMap<String, HashMap<u32, Declared>>,
    /// (extendee, field number) → declaration.
    extensions: HashMap<(String, u32), Declared>,
}

const EDITIONS: &[u8] = b"editions";

impl Declarations {
    pub fn new() -> Declarations {
        Declarations::default()
    }

    /// Add what one serialized `FileDescriptorProto` declares. `pool` has
    /// a `descriptor.proto`, to read it with.
    pub fn add_file(&mut self, pool: &Pool, fdp: &[u8]) -> Result<(), MessageError> {
        let def = pool
            .get_message_by_name("google.protobuf.FileDescriptorProto")
            .ok_or_else(|| MessageError("the pool has no descriptor.proto".into()))?;
        let message = DynamicMessage::decode(&def, fdp)?;
        let file = FileDescriptorProto(message.as_ref());
        // The file's default, which a field's own features override.
        let file_delimited = file
            .options()
            .and_then(|o| o.features())
            .filter(|f| f.has_message_encoding())
            .is_some_and(|f| f.message_encoding() == encoding::DELIMITED);
        let ctx = Context {
            editions: file.syntax() == EDITIONS,
            file_delimited,
        };
        let package = String::from_utf8_lossy(file.package()).into_owned();
        for ext in file.extension() {
            self.add_extension(&ext, &ctx);
        }
        for m in file.message_type() {
            self.add_message(&m, &package, &ctx);
        }
        Ok(())
    }

    fn add_message(&mut self, m: &DescriptorProto<'_>, scope: &str, ctx: &Context) {
        let name = join(scope, &String::from_utf8_lossy(m.name()));
        let fields: HashMap<u32, Declared> = m
            .field()
            .iter()
            .map(|f| (f.number() as u32, declared(f, ctx)))
            .collect();
        self.messages.insert(name.clone(), fields);
        for ext in m.extension() {
            self.add_extension(&ext, ctx);
        }
        for nested in m.nested_type() {
            self.add_message(&nested, &name, ctx);
        }
    }

    fn add_extension(&mut self, f: &FieldDescriptorProto<'_>, ctx: &Context) {
        let extendee = strip_dot(f.extendee());
        self.extensions
            .insert((extendee, f.number() as u32), declared(f, ctx));
    }

    /// What is declared of field `number` of `message_type`, by the input
    /// or by `pool`: one of its fields, else one of its extensions.
    pub fn field(&self, pool: &Pool, message_type: &str, number: u32) -> Option<Declared> {
        if let Some(d) = self.messages.get(message_type).and_then(|m| m.get(&number)) {
            return Some(d.clone());
        }
        let def = pool.get_message_by_name(message_type);
        if let Some(f) = def.as_ref().and_then(|m| m.get_field(number)) {
            return Some(from_pool(&f));
        }
        if let Some(d) = self.extensions.get(&(message_type.to_string(), number)) {
            return Some(d.clone());
        }
        def.and_then(|m| m.get_extension(number))
            .map(|f| from_pool(&f))
    }
}

struct Context {
    editions: bool,
    file_delimited: bool,
}

fn join(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_string()
    } else {
        format!("{scope}.{name}")
    }
}

fn strip_dot(name: &[u8]) -> String {
    let name = String::from_utf8_lossy(name);
    name.strip_prefix('.').unwrap_or(&name).to_string()
}

/// A field's declaration. A message is delimited when it is a group, or in
/// editions when its `message_encoding`, its own or else the file's, is
/// `DELIMITED`.
fn declared(f: &FieldDescriptorProto<'_>, ctx: &Context) -> Declared {
    match f.r#type() {
        ty::TYPE_GROUP => Declared::Message {
            delimited: true,
            type_name: strip_dot(f.type_name()),
        },
        ty::TYPE_MESSAGE => Declared::Message {
            delimited: ctx.editions
                && f.options()
                    .and_then(|o| o.features())
                    .filter(|s| s.has_message_encoding())
                    .map_or(ctx.file_delimited, |s| {
                        s.message_encoding() == encoding::DELIMITED
                    }),
            type_name: strip_dot(f.type_name()),
        },
        ty::TYPE_STRING | ty::TYPE_BYTES => Declared::Bytes,
        _ => Declared::Scalar,
    }
}

fn from_pool(f: &crate::Field) -> Declared {
    match f.kind() {
        Kind::Message(m) => Declared::Message {
            delimited: f.is_group(),
            type_name: m.full_name().to_string(),
        },
        Kind::String | Kind::Bytes => Declared::Bytes,
        _ => Declared::Scalar,
    }
}

// ── The correction ──────────────────────────────────────────────────────────

/// What [`correct`] could not settle from the declarations (spec 0426 S4):
/// field `number` of a message of type `message_type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// W1: declared nowhere; read as length-delimited.
    Undeclared { message_type: String, number: u32 },
    /// W2: a string or bytes protoc printed as a message; rebuilt taking
    /// its sub-fields for groups.
    StringAsMessage { message_type: String, number: u32 },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::Undeclared {
                message_type,
                number,
            } => write!(
                f,
                "field {number} of {message_type} is declared nowhere in the input: \
                 read as length-delimited"
            )?,
            Warning::StringAsMessage {
                message_type,
                number,
            } => write!(
                f,
                "field {number} of {message_type} is a string protoc printed as a \
                 message: rebuilt taking its sub-fields for groups"
            )?,
        }
        f.write_str(" (decode the text with the files that declare the options to avoid it)")
    }
}

/// What [`correct`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corrected {
    pub bytes: Vec<u8>,
    pub warnings: Vec<Warning>,
}

/// `fdp`, a serialized `FileDescriptorProto` that
/// [`text_format::parse`](crate::text_format::parse) produced, with each
/// group by number re-framed as `decls` (or `pool`) declares it (spec 0426
/// S2, S3). What is not changed is copied byte for byte; bytes that are not
/// a well-formed message are copied whole.
pub fn correct(pool: &Pool, fdp: &[u8], decls: &Declarations) -> Corrected {
    let mut c = Corrector {
        pool,
        decls,
        warnings: Vec::new(),
    };
    let bytes = c
        .message(fdp, Some("google.protobuf.FileDescriptorProto"))
        .unwrap_or_else(|| fdp.to_vec());
    Corrected {
        bytes,
        warnings: c.warnings,
    }
}

struct Corrector<'a> {
    pool: &'a Pool,
    decls: &'a Declarations,
    warnings: Vec<Warning>,
}

fn read_varint(b: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*pos)?;
        *pos += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_len(out: &mut Vec<u8>, number: u32, payload: &[u8]) {
    put_varint(out, (u64::from(number) << 3) | 2);
    put_varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

/// The body of group `number`, from `pos` (after its start tag): its bytes,
/// the position after its end tag, and whether a non-empty group is nested
/// in it, at any depth.
fn group_body(b: &[u8], pos: usize, number: u32) -> Option<(&[u8], usize, bool)> {
    let start = pos;
    let mut pos = pos;
    let mut nested = false;
    loop {
        let tag_at = pos;
        let tag = read_varint(b, &mut pos)?;
        let (n, wire_type) = (u32::try_from(tag >> 3).ok()?, tag & 7);
        match wire_type {
            0 => {
                read_varint(b, &mut pos)?;
            }
            1 => pos = pos.checked_add(8).filter(|&p| p <= b.len())?,
            5 => pos = pos.checked_add(4).filter(|&p| p <= b.len())?,
            2 => {
                let len = usize::try_from(read_varint(b, &mut pos)?).ok()?;
                pos = pos.checked_add(len).filter(|&e| e <= b.len())?;
            }
            3 => {
                let (inner, next, deeper) = group_body(b, pos, n)?;
                nested |= !inner.is_empty() || deeper;
                pos = next;
            }
            4 if n == number => return Some((&b[start..tag_at], pos, nested)),
            _ => return None,
        }
    }
}

impl Corrector<'_> {
    /// The records of a message of type `ty` (unknown when `None`),
    /// corrected; `None` when the bytes are not a well-formed message.
    fn message(&mut self, b: &[u8], ty: Option<&str>) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(b.len());
        let mut pos = 0;
        while pos < b.len() {
            pos = self.record(b, pos, ty, &mut out)?;
        }
        Some(out)
    }

    /// The record at `pos`, corrected, appended to `out`; the position
    /// after it.
    fn record(
        &mut self,
        b: &[u8],
        pos: usize,
        ty: Option<&str>,
        out: &mut Vec<u8>,
    ) -> Option<usize> {
        let start = pos;
        let mut pos = pos;
        let tag = read_varint(b, &mut pos)?;
        let (number, wire_type) = (u32::try_from(tag >> 3).ok()?, tag & 7);
        if number == 0 {
            return None;
        }
        let declared = ty.and_then(|t| self.decls.field(self.pool, t, number));
        match wire_type {
            0 => {
                read_varint(b, &mut pos)?;
            }
            1 => pos = pos.checked_add(8).filter(|&p| p <= b.len())?,
            5 => pos = pos.checked_add(4).filter(|&p| p <= b.len())?,
            2 => {
                let len_at = pos;
                let len = usize::try_from(read_varint(b, &mut pos)?).ok()?;
                let end = pos.checked_add(len).filter(|&e| e <= b.len())?;
                if let Some(Declared::Message { type_name, .. }) = &declared {
                    // A message given as `N: "…"` is as protoc read it; its
                    // content may still hold groups by number to re-frame.
                    let payload = &b[pos..end];
                    if let Some(inner) = self.message(payload, Some(type_name)) {
                        if inner != payload {
                            out.extend_from_slice(&b[start..len_at]);
                            put_varint(out, inner.len() as u64);
                            out.extend(inner);
                            return Some(end);
                        }
                    }
                }
                pos = end;
            }
            3 => {
                let (body, end, nested) = group_body(b, pos, number)?;
                let message_type = ty.unwrap_or_default().to_string();
                match declared {
                    Some(Declared::Message {
                        delimited: true,
                        type_name,
                    }) => {
                        let inner = self.message(body, Some(&type_name))?;
                        out.extend_from_slice(&b[start..pos]);
                        out.extend(inner);
                        out.extend_from_slice(&b[pos + body.len()..end]);
                    }
                    Some(Declared::Message {
                        delimited: false,
                        type_name,
                    }) => {
                        let inner = self.message(body, Some(&type_name))?;
                        put_len(out, number, &inner);
                    }
                    Some(Declared::Bytes) => {
                        if nested {
                            self.warnings.push(Warning::StringAsMessage {
                                message_type,
                                number,
                            });
                        }
                        put_len(out, number, body);
                    }
                    Some(Declared::Scalar) => out.extend_from_slice(&b[start..end]),
                    None => {
                        if ty.is_some() {
                            self.warnings.push(Warning::Undeclared {
                                message_type,
                                number,
                            });
                        }
                        let inner = self.message(body, None)?;
                        put_len(out, number, &inner);
                    }
                }
                return Some(end);
            }
            _ => return None,
        }
        out.extend_from_slice(&b[start..pos]);
        Some(pos)
    }
}
