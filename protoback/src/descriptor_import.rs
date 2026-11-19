// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The import rule (spec 0447 S4), as reproto's `descriptor_import.py`.
//!
//! A file setting a built-in option field that its reader knows only from
//! the descriptor in use (a field of an options message the stock
//! descriptor lacks), and not importing that descriptor, gets the import:
//! protoc reads built-in options against the descriptor in its pool, its
//! compiled-in copy unless the file loads another.
//!
//! The fields set are read from each file's serialized descriptor, by
//! number.

use std::collections::{BTreeMap, BTreeSet};

use prototext_upb::descriptor_access::FileDescriptorProto;

/// The numbers of the fields of each message, by the message's name.
pub type Fields = BTreeMap<String, BTreeSet<u64>>;

#[derive(Clone, Copy)]
enum Kind {
    File,
    Msg,
    Field,
    Oneof,
    Enum,
    Value,
    Range,
    Service,
    Method,
}

/// What each kind of descriptor holds: its children, by field number.
fn children(kind: Kind) -> &'static [(u64, Kind)] {
    match kind {
        Kind::File => &[
            (4, Kind::Msg),
            (5, Kind::Enum),
            (6, Kind::Service),
            (7, Kind::Field),
        ],
        Kind::Msg => &[
            (2, Kind::Field),
            (6, Kind::Field),
            (3, Kind::Msg),
            (4, Kind::Enum),
            (5, Kind::Range),
            (8, Kind::Oneof),
        ],
        Kind::Enum => &[(2, Kind::Value)],
        Kind::Service => &[(2, Kind::Method)],
        _ => &[],
    }
}

/// The number of its options, and their message.
fn options(kind: Kind) -> (u64, &'static str) {
    match kind {
        Kind::File => (8, "FileOptions"),
        Kind::Msg => (7, "MessageOptions"),
        Kind::Field => (8, "FieldOptions"),
        Kind::Oneof => (2, "OneofOptions"),
        Kind::Enum => (3, "EnumOptions"),
        Kind::Value => (3, "EnumValueOptions"),
        Kind::Range => (3, "ExtensionRangeOptions"),
        Kind::Service => (3, "ServiceOptions"),
        Kind::Method => (4, "MethodOptions"),
    }
}

fn varint(buf: &[u8], i: &mut usize) -> Option<u64> {
    let (mut value, mut shift) = (0u64, 0u32);
    loop {
        let b = *buf.get(*i)?;
        *i += 1;
        value |= u64::from(b & 0x7F).checked_shl(shift)?;
        if b < 0x80 {
            return Some(value);
        }
        shift += 7;
    }
}

/// Each field of a serialized message: its number, and its bytes when
/// length-delimited. A malformed buffer gives the fields before the flaw.
fn fields(buf: &[u8]) -> Vec<(u64, Option<&[u8]>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let Some(tag) = varint(buf, &mut i) else {
            break;
        };
        let number = tag >> 3;
        match tag & 7 {
            0 => {
                if varint(buf, &mut i).is_none() {
                    break;
                }
                out.push((number, None));
            }
            1 | 5 => {
                i += if tag & 7 == 1 { 8 } else { 4 };
                if i > buf.len() {
                    break;
                }
                out.push((number, None));
            }
            2 => {
                let Some(n) = varint(buf, &mut i).and_then(|n| usize::try_from(n).ok()) else {
                    break;
                };
                let Some(body) = i.checked_add(n).and_then(|end| buf.get(i..end)) else {
                    break;
                };
                i += n;
                out.push((number, Some(body)));
            }
            _ => break, // groups: not in a descriptor's options
        }
    }
    out
}

/// The field numbers each options message has set in the file, at any
/// depth.
pub fn options_set(fdp: &[u8]) -> Fields {
    let mut out = Fields::new();
    let mut todo = vec![(Kind::File, fdp)];
    while let Some((kind, buf)) = todo.pop() {
        let (options_number, options_name) = options(kind);
        for (number, body) in fields(buf) {
            let Some(body) = body else { continue };
            if number == options_number {
                out.entry(options_name.to_string())
                    .or_default()
                    .extend(fields(body).into_iter().map(|(n, _)| n));
            } else if let Some((_, child)) = children(kind).iter().find(|(n, _)| *n == number) {
                todo.push((*child, body));
            }
        }
    }
    out
}

/// The numbers of the fields of each top-level message of `fdp`.
pub fn message_fields(fdp: &FileDescriptorProto<'_>) -> Fields {
    fdp.message_type()
        .iter()
        .map(|m| {
            (
                String::from_utf8_lossy(m.name()).into_owned(),
                m.field()
                    .iter()
                    .filter_map(|f| u64::try_from(f.number()).ok())
                    .collect(),
            )
        })
        .collect()
}

/// Whether the file (its serialized descriptor `fdp`) sets a field of an
/// options message that `descriptor` declares and `stock` lacks.
pub fn needs_import(fdp: &[u8], descriptor: &Fields, stock: &Fields) -> bool {
    let empty = BTreeSet::new();
    options_set(fdp).iter().any(|(message, numbers)| {
        let declared = descriptor.get(message).unwrap_or(&empty);
        let known = stock.get(message).unwrap_or(&empty);
        numbers
            .iter()
            .any(|n| declared.contains(n) && !known.contains(n))
    })
}

/// `fdp` with one more `dependency`, `name`, after the others: field 3
/// appended to the serialization, the rest untouched.
pub fn with_dependency(fdp: &[u8], name: &str) -> Vec<u8> {
    let mut out = fdp.to_vec();
    out.push(0x1a);
    let mut n = name.len();
    while n >= 0x80 {
        out.push(u8::try_from(n & 0x7F).unwrap_or(0) | 0x80);
        n >>= 7;
    }
    out.push(u8::try_from(n).unwrap_or(0));
    out.extend_from_slice(name.as_bytes());
    out
}
