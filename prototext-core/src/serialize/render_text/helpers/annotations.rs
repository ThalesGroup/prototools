// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use prost_reflect::{Cardinality, Kind};

use super::super::FieldOrExt;
use super::output::{write_dec_i32, write_dec_u64};

// ── Shared annotation helpers ─────────────────────────────────────────────────

/// Push the three standard tag-level anomaly modifiers into an annotation.
/// Call this after pushing the field declaration or wire-type name.
#[inline]
pub(in super::super) fn push_tag_modifiers(
    aw: &mut AnnWriter,
    out: &mut Vec<u8>,
    tag_ohb: Option<u64>,
    tag_oor: bool,
    len_ohb: Option<u64>,
) {
    if let Some(v) = tag_ohb {
        aw.push_u64_mod(out, b"tag_ohb: ", v);
    }
    if tag_oor {
        aw.push(out, b"TAG_OOR");
    }
    if let Some(v) = len_ohb {
        aw.push_u64_mod(out, b"len_ohb: ", v);
    }
}

// ── Annotation helpers ────────────────────────────────────────────────────────

/// Return the protobuf type name string for a field's Kind.
#[inline]
pub(in super::super) fn proto_type_str(kind: &Kind) -> &'static str {
    match kind {
        Kind::Double => "double",
        Kind::Float => "float",
        Kind::Int64 => "int64",
        Kind::Uint64 => "uint64",
        Kind::Int32 => "int32",
        Kind::Fixed64 => "fixed64",
        Kind::Fixed32 => "fixed32",
        Kind::Bool => "bool",
        Kind::String => "string",
        Kind::Message(_) => "message", // groups also appear as Message in prost-reflect
        Kind::Bytes => "bytes",
        Kind::Uint32 => "uint32",
        Kind::Enum(_) => "enum",
        Kind::Sfixed32 => "sfixed32",
        Kind::Sfixed64 => "sfixed64",
        Kind::Sint32 => "sint32",
        Kind::Sint64 => "sint64",
    }
}

/// Build the `"[repeated |required ]type[ [packed=true]] = N"` field declaration string.
/// v2 format: `optional` omitted as default; no trailing `;`.
/// Used only by `render_group_field` for post-hoc splice insertion.
pub(in super::super) fn field_decl(
    field_number: u64,
    field_schema: Option<&FieldOrExt>,
) -> Option<String> {
    let fi = field_schema?;
    // v2: `optional` is the default — omit it; emit `repeated` / `required` explicitly.
    let label_prefix = match fi.cardinality() {
        Cardinality::Required => "required ",
        Cardinality::Repeated => "repeated ",
        Cardinality::Optional => "",
    };
    let kind = fi.kind();
    let type_str = proto_type_str(&kind);
    let type_display = match &kind {
        Kind::Message(msg_desc) => msg_desc.name().to_string(),
        Kind::Enum(enum_desc) => enum_desc.name().to_string(),
        _ => type_str.to_string(),
    };
    let packed = if fi.is_packed() { " [packed=true]" } else { "" };
    // v2: no trailing `;`
    Some(format!(
        "{}{}{} = {}",
        label_prefix, type_display, packed, field_number
    ))
}

/// Tracks whether we've started writing an annotation on the current line.
///
/// First part writes `"  # "` prefix; subsequent parts write `"; "` separator.
/// All annotation parts are written directly into the output buffer — no heap allocation.
///
/// v2 annotation format: tokens separated by `"; "`, NO trailing `";"`.
pub(in super::super) struct AnnWriter {
    started: bool,
    /// When true, the opening prefix is `"#@ "` instead of `"  #@ "`.
    no_leading_spaces: bool,
}

impl AnnWriter {
    #[inline]
    pub(in super::super) fn new() -> Self {
        Self {
            started: false,
            no_leading_spaces: false,
        }
    }

    /// Like `new()`, but the opening `#@` prefix is written without the two
    /// leading spaces (for lines that carry no value token before the annotation).
    #[inline]
    pub(in super::super) fn new_no_leading_spaces() -> Self {
        Self {
            started: false,
            no_leading_spaces: true,
        }
    }

    /// Write the inter-part separator into `out`.
    ///
    /// First call: writes `"  #@ "` (or `"#@ "` for no-leading-spaces writers)
    /// to open the annotation.
    /// Subsequent calls: writes `"; "` to separate tokens (v2 format).
    #[inline]
    pub(in super::super) fn sep(&mut self, out: &mut Vec<u8>) {
        if self.started {
            out.extend_from_slice(b"; ");
        } else {
            if self.no_leading_spaces {
                out.extend_from_slice(b"#@ ");
            } else {
                out.extend_from_slice(b"  #@ ");
            }
            self.started = true;
        }
    }

    /// Push a raw annotation token (any byte slice, no trailing `;`).
    #[inline]
    pub(in super::super) fn push(&mut self, out: &mut Vec<u8>, s: &[u8]) {
        self.sep(out);
        out.extend_from_slice(s);
    }

    /// Push a wire-type or invalid-wire-type token (no trailing `;`).
    #[inline]
    pub(in super::super) fn push_wire(&mut self, out: &mut Vec<u8>, name: &str) {
        self.sep(out);
        out.extend_from_slice(name.as_bytes());
    }

    /// Push a `key: N` modifier (no trailing `;`).
    /// `key` must include the colon-space, e.g. `b"tag_ohb: "`.
    #[inline]
    pub(in super::super) fn push_u64_mod(&mut self, out: &mut Vec<u8>, key: &[u8], v: u64) {
        self.sep(out);
        out.extend_from_slice(key);
        write_dec_u64(v, out);
    }

    /// Push `"[repeated |required ]type[ [packed=true]] = N"` field-declaration
    /// directly into `out` (v2 format: `optional` is omitted as default label).
    ///
    /// For ENUM fields, `enum_raw` must be `Some(numeric_value)` and the
    /// annotation emits `EnumTypeName(numeric)` instead of just `EnumTypeName`.
    /// For packed ENUM fields, `enum_packed_nums` must be `Some(&[i32])` containing
    /// the raw numeric values; the annotation emits `EnumTypeName([n1, n2])`.
    ///
    /// No-op when `fs` is `None` (unknown field).
    /// Eliminates the `field_decl() -> Option<String>` allocation at all
    /// non-GROUP call sites.
    #[inline]
    pub(in super::super) fn push_field_decl(
        &mut self,
        out: &mut Vec<u8>,
        num: u64,
        fs: Option<&FieldOrExt>,
        enum_raw: Option<i32>,
        enum_packed_nums: Option<&[i32]>,
    ) {
        let Some(fi) = fs else { return };
        self.sep(out);
        // v2: `optional` is the default — omit it; emit `repeated` / `required` explicitly.
        match fi.cardinality() {
            Cardinality::Required => {
                out.extend_from_slice(b"required ");
            }
            Cardinality::Repeated => {
                out.extend_from_slice(b"repeated ");
            }
            Cardinality::Optional => {}
        }
        let kind = fi.kind();
        if let Kind::Enum(ref enum_desc) = kind {
            // ENUM: emit EnumTypeName(N) or EnumTypeName([n1, n2]) for packed
            let type_display = enum_desc.name();
            out.extend_from_slice(type_display.as_bytes());
            if let Some(nums) = enum_packed_nums {
                // Packed enum: EnumTypeName([n1, n2, ...])
                out.push(b'(');
                out.push(b'[');
                for (i, &n) in nums.iter().enumerate() {
                    if i > 0 {
                        out.extend_from_slice(b", ");
                    }
                    write_dec_i32(n, out);
                }
                out.push(b']');
                out.push(b')');
            } else if let Some(n) = enum_raw {
                // Scalar enum: EnumTypeName(N)
                out.push(b'(');
                write_dec_i32(n, out);
                out.push(b')');
            }
        } else {
            let type_display = match &kind {
                Kind::Message(msg_desc) => msg_desc.name().to_string(),
                _ => proto_type_str(&kind).to_string(),
            };
            out.extend_from_slice(type_display.as_bytes());
        }
        if fi.is_packed() {
            out.extend_from_slice(b" [packed=true]");
        }
        out.extend_from_slice(b" = ");
        write_dec_u64(num, out);
    }
}
