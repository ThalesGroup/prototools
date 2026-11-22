// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The rule set `fdp`, version 1 (spec 0453 S4).
//!
//! - **R1.** A field's `json_name` is dropped where it equals what protoc
//!   derives from the field's name.
//! - **R2.** Fields are re-encoded in field-number order, recursively in
//!   the fields `descriptor.proto` declares as messages; the occurrences of
//!   one number keep their order.
//!
//! Both read the stock `descriptor.proto`, whatever schema the repository
//! renders with: a canonical form depends on the rule set's version alone.
//! Bytes that don't parse as wire format are left as they are.

use git_seriate::plugins::Canonicalizer;
use git_seriate::Error;
use prototext_core::schema::{Kind, Message};

/// The rule set's name and version, as history commits record them.
pub const RULE_SET: (&str, u32) = ("fdp", 1);

const FIELD_DESCRIPTOR: &str = "google.protobuf.FieldDescriptorProto";
const FIELD_NAME: u64 = 1;
const FIELD_JSON_NAME: u64 = 10;

pub struct Fdp {
    root: Message,
}

impl Fdp {
    pub fn new() -> Result<Self, Error> {
        let schema = prototext_core::parse_schema(prototext::EMBEDDED_DESCRIPTOR, crate::FDP)
            .map_err(|e| Error::Refused(format!("the stock descriptor.proto: {e}")))?;
        let root = schema.root_descriptor().ok_or_else(|| {
            Error::Bug("the stock descriptor.proto has no FileDescriptorProto".into())
        })?;
        Ok(Fdp { root })
    }
}

impl Canonicalizer for Fdp {
    fn rule_sets(&self) -> Vec<(String, u32)> {
        vec![(RULE_SET.0.to_string(), RULE_SET.1)]
    }

    fn canon(&self, _path: &str, item: &[u8]) -> Vec<u8> {
        canonical(item, &self.root).unwrap_or_else(|| item.to_vec())
    }
}

/// One field as read off the wire: its number, its tag's bytes, and its
/// value: a length-delimited payload, or the other wire types' bytes.
struct Field<'a> {
    number: u64,
    tag: &'a [u8],
    value: Value<'a>,
}

enum Value<'a> {
    Payload(&'a [u8]),
    Raw(&'a [u8]),
}

/// `bytes`, a `message` on the wire, with R1 and R2 applied; `None` when
/// it doesn't parse.
fn canonical(bytes: &[u8], message: &Message) -> Option<Vec<u8>> {
    let mut fields = parse(bytes)?;

    // R1.
    if message.full_name() == FIELD_DESCRIPTOR {
        let text = |n: u64| {
            fields.iter().rev().find_map(|f| match f.value {
                Value::Payload(p) if f.number == n => std::str::from_utf8(p).ok(),
                _ => None,
            })
        };
        if let (Some(name), Some(json)) = (text(FIELD_NAME), text(FIELD_JSON_NAME)) {
            if json == prototext_core::helpers::json_name(name) {
                fields.retain(|f| f.number != FIELD_JSON_NAME);
            }
        }
    }

    // R2, entering the declared messages.
    let mut out_fields: Vec<(u64, Vec<u8>)> = Vec::with_capacity(fields.len());
    for f in &fields {
        let mut encoded = f.tag.to_vec();
        match f.value {
            Value::Raw(raw) => encoded.extend_from_slice(raw),
            Value::Payload(payload) => {
                let child = u32::try_from(f.number)
                    .ok()
                    .and_then(|n| message.fields().find(|field| field.number() == n))
                    .and_then(|field| match field.kind() {
                        Kind::Message(m) => Some(m),
                        _ => None,
                    });
                let payload = match child {
                    Some(m) => canonical(payload, &m)?,
                    None => payload.to_vec(),
                };
                encode_varint(payload.len() as u64, &mut encoded);
                encoded.extend_from_slice(&payload);
            }
        }
        out_fields.push((f.number, encoded));
    }
    out_fields.sort_by_key(|&(number, _)| number);
    Some(out_fields.into_iter().flat_map(|(_, b)| b).collect())
}

fn parse(bytes: &[u8]) -> Option<Vec<Field<'_>>> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let start = pos;
        let (tag, n) = decode_varint(&bytes[pos..])?;
        pos += n;
        let tag_bytes = &bytes[start..pos];
        let value_start = pos;
        let value = match tag & 7 {
            0 => {
                pos += decode_varint(&bytes[pos..])?.1;
                Value::Raw(&bytes[value_start..pos])
            }
            1 => {
                pos = pos.checked_add(8).filter(|&p| p <= bytes.len())?;
                Value::Raw(&bytes[value_start..pos])
            }
            5 => {
                pos = pos.checked_add(4).filter(|&p| p <= bytes.len())?;
                Value::Raw(&bytes[value_start..pos])
            }
            2 => {
                let (len, n) = decode_varint(&bytes[pos..])?;
                let begin = pos + n;
                let end = begin.checked_add(usize::try_from(len).ok()?)?;
                if end > bytes.len() {
                    return None;
                }
                pos = end;
                Value::Payload(&bytes[begin..end])
            }
            // Groups: none in descriptor.proto; left whole.
            _ => return None,
        };
        fields.push(Field {
            number: tag >> 3,
            tag: tag_bytes,
            value,
        });
    }
    Some(fields)
}

fn decode_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, &b) in bytes.iter().enumerate().take(10) {
        value |= u64::from(b & 0x7f) << (7 * i);
        if b < 0x80 {
            return Some((value, i + 1));
        }
    }
    None
}

fn encode_varint(mut v: u64, out: &mut Vec<u8>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn field(number: u64, wire: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        encode_varint(number << 3 | wire, &mut out);
        if wire == 2 {
            encode_varint(payload.len() as u64, &mut out);
        }
        out.extend_from_slice(payload);
        out
    }

    fn len(number: u64, payload: &[u8]) -> Vec<u8> {
        field(number, 2, payload)
    }

    fn varint(number: u64, value: u64) -> Vec<u8> {
        let mut v = Vec::new();
        encode_varint(value, &mut v);
        field(number, 0, &v)
    }

    /// A FieldDescriptorProto: name, number, json_name.
    fn fdp_field(name: &str, number: u64, json: Option<&str>) -> Vec<u8> {
        let mut f = len(1, name.as_bytes());
        f.extend(varint(3, number));
        if let Some(j) = json {
            f.extend(len(10, j.as_bytes()));
        }
        f
    }

    /// A file with one message holding `fields`, each its bytes.
    fn file(fields: &[Vec<u8>], message_first: bool) -> Vec<u8> {
        let mut message = len(1, b"M");
        for f in fields {
            message.extend(len(2, f));
        }
        let (name, package, mt) = (len(1, b"a.proto"), len(2, b"p"), len(4, &message));
        if message_first {
            [mt, name, package].concat()
        } else {
            [name, package, mt].concat()
        }
    }

    #[test]
    fn r1_drops_a_default_json_name_and_keeps_another() {
        let rules = Fdp::new().unwrap();
        let with = file(&[fdp_field("foo_bar", 1, Some("fooBar"))], false);
        let without = file(&[fdp_field("foo_bar", 1, None)], false);
        assert_eq!(rules.canon("a.pb", &with), without);
        let custom = file(&[fdp_field("foo_bar", 1, Some("custom"))], false);
        assert_eq!(rules.canon("a.pb", &custom), custom);
    }

    #[test]
    fn r2_orders_fields_by_number_recursively_and_keeps_repeated_order() {
        let rules = Fdp::new().unwrap();
        let a = fdp_field("a", 1, None);
        let b = fdp_field("b", 2, None);
        // The message first (Go's shape), its repeated fields b then a.
        let go = file(&[b.clone(), a.clone()], true);
        let protoc = file(&[b.clone(), a.clone()], false);
        assert_eq!(
            rules.canon("a.pb", &go),
            protoc,
            "the order of one number is kept"
        );
        assert_eq!(
            rules.canon("a.pb", &protoc),
            protoc,
            "protoc's own is unchanged"
        );
        // A field of FieldDescriptorProto out of order, nested two deep.
        let mut late_name = varint(3, 1);
        late_name.extend(len(1, b"x"));
        let nested = file(&[late_name], false);
        assert_eq!(
            rules.canon("a.pb", &nested),
            file(&[fdp_field("x", 1, None)], false)
        );
    }

    #[test]
    fn canon_is_idempotent_and_leaves_garbage_alone() {
        let rules = Fdp::new().unwrap();
        let x = file(
            &[
                fdp_field("foo_bar", 2, Some("fooBar")),
                fdp_field("a", 1, None),
            ],
            true,
        );
        let once = rules.canon("a.pb", &x);
        assert_eq!(rules.canon("a.pb", &once), once);
        let garbage = vec![0x0a, 0x7f, 0x01];
        assert_eq!(rules.canon("a.pb", &garbage), garbage);
    }
}
