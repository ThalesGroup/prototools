// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

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

use crate::wire::{put_len, put_tag, put_varint, read_varint, records, LEN, VARINT};
use crate::Pool;

/// The field of `b` numbered `number`, as text (a name), if any.
fn string_field(b: &[u8], number: u32) -> Option<String> {
    records(b)?
        .into_iter()
        .rev()
        .find(|r| r.number == number && r.wire_type == LEN)
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
                (2 | 6, LEN) if !self.field_resolves(r.payload) => {}
                (3, LEN) => put_len(&mut out, 3, &self.message(r.payload)?),
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
                && r.wire_type == LEN
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
            (3, LEN) => collect_types(r.payload, &full, own)?,
            (4, LEN) => {
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
            (4, LEN) => collect_types(r.payload, &package, &mut own)?,
            (5, LEN) => {
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
    for r in top.iter().filter(|r| r.number == 3 && r.wire_type == LEN) {
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
            (3, LEN) => {
                if new_index[import].is_some() {
                    out.extend_from_slice(r.bytes);
                }
                import += 1;
            }
            (10 | 11, VARINT) => {
                if let Some(i) = renumber(r.value) {
                    put_tag(&mut out, r.number, VARINT);
                    put_varint(&mut out, i);
                }
            }
            (10 | 11, LEN) => {
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
            (4, LEN) => put_len(&mut out, 4, &mender.message(r.payload)?),
            (6, LEN) => put_len(&mut out, 6, &mender.service(r.payload)?),
            (7, LEN) if !mender.field_resolves(r.payload) => {}
            _ => out.extend_from_slice(r.bytes),
        }
    }
    Some(out)
}
