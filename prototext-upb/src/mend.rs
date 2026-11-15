//! A file the pool refuses, reduced to what still resolves (spec 0427 S2).
//!
//! An input's file may import a file the input lacks, or use a type
//! nobody declares: upb refuses it whole, and its declarations with it.
//! [`mend`] removes, at the byte level, the records that do not resolve
//! (imports not in the pool, fields, extensions and methods whose types
//! are not, extensions whose extendee is not), and copies everything else
//! as it is, options and features included. The mended file serves the
//! pool's declarations only: reproto mends and reports for itself.

use std::collections::HashSet;

use crate::Pool;

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

/// One record of a message: its field number, wire type, payload (the
/// length-delimited content, or the varint's value as `value`), and all
/// its bytes.
struct Record<'a> {
    number: u64,
    wire_type: u64,
    payload: &'a [u8],
    value: u64,
    bytes: &'a [u8],
}

/// The records of `b`, or `None` when it is not well formed.
fn records(b: &[u8]) -> Option<Vec<Record<'_>>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < b.len() {
        let start = pos;
        let tag = read_varint(b, &mut pos)?;
        let (number, wire_type) = (tag >> 3, tag & 7);
        let (mut payload, mut value): (&[u8], u64) = (&[], 0);
        match wire_type {
            0 => value = read_varint(b, &mut pos)?,
            1 => pos = pos.checked_add(8).filter(|&p| p <= b.len())?,
            5 => pos = pos.checked_add(4).filter(|&p| p <= b.len())?,
            2 => {
                let len = usize::try_from(read_varint(b, &mut pos)?).ok()?;
                let end = pos.checked_add(len).filter(|&e| e <= b.len())?;
                payload = &b[pos..end];
                pos = end;
            }
            // Groups do not occur in a descriptor's own fields; their
            // options' content is copied whole, inside a length-delimited
            // record.
            _ => return None,
        }
        out.push(Record {
            number,
            wire_type,
            payload,
            value,
            bytes: &b[start..pos],
        });
    }
    Some(out)
}

fn put_len(out: &mut Vec<u8>, number: u64, payload: &[u8]) {
    put_varint(out, (number << 3) | 2);
    put_varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

/// The field of `b` numbered `number`, as text (a name), if any.
fn string_field(b: &[u8], number: u64) -> Option<String> {
    records(b)?
        .into_iter()
        .rev()
        .find(|r| r.number == number && r.wire_type == 2)
        .map(|r| String::from_utf8_lossy(r.payload).into_owned())
}

struct Mender<'a> {
    pool: &'a Pool,
    /// The file's own types, fully qualified, without the leading dot.
    own: HashSet<String>,
}

impl Mender<'_> {
    fn resolves(&self, type_name: &str) -> bool {
        let name = type_name.trim_start_matches('.');
        self.own.contains(name)
            || self.pool.get_message_by_name(name).is_some()
            || self.pool.get_enum_by_name(name).is_some()
    }

    /// A field or an extension (`FieldDescriptorProto`): whether its type
    /// and its extendee resolve.
    fn field_resolves(&self, f: &[u8]) -> bool {
        let type_ok = string_field(f, 6).is_none_or(|t| self.resolves(&t));
        let extendee_ok = string_field(f, 2).is_none_or(|t| self.resolves(&t));
        type_ok && extendee_ok
    }

    /// A message (`DescriptorProto`), its unresolvable fields and
    /// extensions removed, nested messages alike.
    fn message(&self, b: &[u8]) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(b.len());
        for r in records(b)? {
            match (r.number, r.wire_type) {
                (2 | 6, 2) if !self.field_resolves(r.payload) => {}
                (3, 2) => put_len(&mut out, 3, &self.message(r.payload)?),
                _ => out.extend_from_slice(r.bytes),
            }
        }
        Some(out)
    }

    /// A service (`ServiceDescriptorProto`), its methods with an
    /// unresolvable input or output type removed.
    fn service(&self, b: &[u8]) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(b.len());
        for r in records(b)? {
            let unresolvable = r.number == 2
                && r.wire_type == 2
                && [2, 3]
                    .iter()
                    .any(|&n| string_field(r.payload, n).is_some_and(|t| !self.resolves(&t)));
            if !unresolvable {
                out.extend_from_slice(r.bytes);
            }
        }
        Some(out)
    }
}

/// The types `b`, a message of scope `scope`, declares, nested ones
/// included.
fn collect_types(b: &[u8], scope: &str, own: &mut HashSet<String>) -> Option<()> {
    let name = string_field(b, 1)?;
    let full = if scope.is_empty() {
        name
    } else {
        format!("{scope}.{name}")
    };
    for r in records(b)? {
        match (r.number, r.wire_type) {
            (3, 2) => collect_types(r.payload, &full, own)?,
            (4, 2) => {
                if let Some(e) = string_field(r.payload, 1) {
                    own.insert(format!("{full}.{e}"));
                }
            }
            _ => {}
        }
    }
    own.insert(full);
    Some(())
}

/// `fdp`, a serialized `FileDescriptorProto`, with what does not resolve
/// in `pool` removed: its imports not in `pool` (`public_dependency` and
/// `weak_dependency` renumbered), then the fields, extensions and methods
/// whose types are neither in `pool` nor in the file. `None` when `fdp` is
/// not well formed.
pub(crate) fn mend(pool: &Pool, fdp: &[u8]) -> Option<Vec<u8>> {
    let top = records(fdp)?;
    let package = string_field(fdp, 2).unwrap_or_default();
    let mut own = HashSet::new();
    for r in &top {
        match (r.number, r.wire_type) {
            (4, 2) => collect_types(r.payload, &package, &mut own)?,
            (5, 2) => {
                if let Some(e) = string_field(r.payload, 1) {
                    own.insert(if package.is_empty() {
                        e
                    } else {
                        format!("{package}.{e}")
                    });
                }
            }
            _ => {}
        }
    }
    let mender = Mender { pool, own };
    // Each import's new index, `None` when it goes.
    let mut new_index = Vec::new();
    let mut kept = 0u64;
    for r in top.iter().filter(|r| r.number == 3 && r.wire_type == 2) {
        let name = String::from_utf8_lossy(r.payload);
        if pool.file(&name).is_some() {
            new_index.push(Some(kept));
            kept += 1;
        } else {
            new_index.push(None);
        }
    }
    let renumber = |i: u64| new_index.get(i as usize).copied().flatten();
    let mut out = Vec::with_capacity(fdp.len());
    let mut import = 0usize;
    for r in &top {
        match (r.number, r.wire_type) {
            (3, 2) => {
                if new_index[import].is_some() {
                    out.extend_from_slice(r.bytes);
                }
                import += 1;
            }
            (10 | 11, 0) => {
                if let Some(i) = renumber(r.value) {
                    put_varint(&mut out, r.number << 3);
                    put_varint(&mut out, i);
                }
            }
            (10 | 11, 2) => {
                // Packed.
                let mut pos = 0;
                let mut packed = Vec::new();
                while pos < r.payload.len() {
                    if let Some(i) = renumber(read_varint(r.payload, &mut pos)?) {
                        put_varint(&mut packed, i);
                    }
                }
                if !packed.is_empty() {
                    put_len(&mut out, r.number, &packed);
                }
            }
            (4, 2) => put_len(&mut out, 4, &mender.message(r.payload)?),
            (6, 2) => put_len(&mut out, 6, &mender.service(r.payload)?),
            (7, 2) if !mender.field_resolves(r.payload) => {}
            _ => out.extend_from_slice(r.bytes),
        }
    }
    Some(out)
}
