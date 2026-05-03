// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// ── Parsed annotation ──────────────────────────────────────────────────────────

/// Annotation fields derived from the `#@ …` comment on a line.
#[derive(Default)]
pub(crate) struct Ann<'a> {
    pub(crate) wire_type: &'a str, // v2: "varint", "bytes", "group", "fixed64", "fixed32", or INVALID_*
    pub(crate) field_number: Option<u64>,
    pub(crate) field_type: &'a str, // "int32", "string", "MyMsg", "enum", …
    pub(crate) is_packed: bool,
    pub(crate) tag_overhang_count: Option<u64>,
    pub(crate) value_overhang_count: Option<u64>,
    pub(crate) length_overhang_count: Option<u64>,
    pub(crate) missing_bytes_count: Option<u64>,
    pub(crate) mismatched_group_end: Option<u64>,
    pub(crate) open_ended_group: bool,
    pub(crate) end_tag_overhang_count: Option<u64>,
    pub(crate) records_overhung_count: Vec<u64>,
    pub(crate) neg_int32_truncated: bool, // proto2_neg_int32_truncated: true
    pub(crate) records_neg_int32_truncated: Vec<bool>, // records_neg_int32_truncated: [true, false, …]
    /// For packed ENUM fields: raw i32 numeric values extracted from `EnumType([n1, n2])`.
    /// Used by `encode_packed_array_line` instead of parsing the LHS symbolic-name list.
    pub(crate) enum_packed_values: Vec<i64>,
    /// For scalar ENUM fields: the raw i32 extracted from `EnumType(N)` in the annotation.
    /// Overrides the LHS value token for encoding.
    pub(crate) enum_scalar_value: Option<i64>,
    /// Non-canonical NaN bit pattern from `nan_bits: 0x…` annotation modifier.
    /// Applies to float (32-bit, stored as u64) and double (64-bit) fields.
    pub(crate) nan_bits: Option<u64>,
    /// Number of elements in a per-line packed wire record (`pack_size: N`).
    /// `None` means this is not the first element of a packed record.
    pub(crate) pack_size: Option<usize>,
    /// Per-element varint OHB for a per-line packed element (`ohb: N`).
    pub(crate) elem_ohb: Option<u64>,
    /// True when this per-line packed element is a truncated-negative int32/enum (`neg`).
    pub(crate) elem_neg_trunc: bool,
}

// ── Annotation parser ─────────────────────────────────────────────────────────

#[inline]
pub(super) fn parse_u64_str(s: &str) -> u64 {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        u64::from_str_radix(&s[2..], 16).unwrap_or(0)
    } else {
        s.parse::<u64>().unwrap_or(0)
    }
}

/// Parse the annotation string (everything after `# ` on a line) into `Ann`.
///
/// v2 annotation format:
///   unknown_field_ann := wire_type [";" modifier (";" modifier)*]
///   known_field_ann   := ["group" ";"] field_decl [";" modifier (";" modifier)*]
pub(crate) fn parse_annotation(ann_str: &str) -> Ann<'_> {
    let mut ann = Ann::default();

    for raw_token in ann_str.split(';') {
        let token = raw_token.trim();
        if token.is_empty() {
            continue;
        }

        if let Some(colon) = token.find(':') {
            // Modifier "name: value" — only if the part before ':' has no spaces.
            let before = &token[..colon];
            if !before.contains(' ') {
                let name = before.trim();
                let value = token[colon + 1..].trim();
                match name {
                    "tag_ohb" => ann.tag_overhang_count = Some(parse_u64_str(value)),
                    "val_ohb" => ann.value_overhang_count = Some(parse_u64_str(value)),
                    "len_ohb" => ann.length_overhang_count = Some(parse_u64_str(value)),
                    "MISSING" => ann.missing_bytes_count = Some(parse_u64_str(value)),
                    "END_MISMATCH" => ann.mismatched_group_end = Some(parse_u64_str(value)),
                    "etag_ohb" => ann.end_tag_overhang_count = Some(parse_u64_str(value)),
                    "packed_ohb" => {
                        // value is "[1, 2, 3]"
                        let inner = value.trim_start_matches('[').trim_end_matches(']');
                        ann.records_overhung_count = inner.split(',').map(parse_u64_str).collect();
                    }
                    "packed_truncated_neg" => {
                        // value is "[0, 1, 0, …]"
                        let inner = value.trim_start_matches('[').trim_end_matches(']');
                        ann.records_neg_int32_truncated =
                            inner.split(',').map(|s| s.trim() == "1").collect();
                    }
                    "nan_bits" => ann.nan_bits = Some(parse_u64_str(value)),
                    "pack_size" => ann.pack_size = Some(parse_u64_str(value) as usize),
                    "ohb" => ann.elem_ohb = Some(parse_u64_str(value)),
                    _ => {}
                }
                continue;
            }
        }

        // No colon (or colon is inside a field_decl with spaces before it).
        if token.contains('=') {
            // Field declaration: "int32 = 5", "repeated string [packed=true] = 3", etc.
            parse_field_decl_into(token, &mut ann);
        } else {
            // Bare wire-type name or bare flag modifier (no colon, no '=').
            match token {
                "OPEN_GROUP" => {
                    ann.open_ended_group = true;
                }
                "truncated_neg" => {
                    ann.neg_int32_truncated = true;
                }
                "neg" => {
                    ann.elem_neg_trunc = true;
                }
                // Flags that don't affect binary encoding — ignore.
                "TAG_OOR" | "ETAG_OOR" | "TYPE_MISMATCH" | "ENUM_UNKNOWN" => {}
                // Everything else is a wire-type name (lowercase valid or ALLCAPS invalid).
                _ => {
                    ann.wire_type = token;
                }
            }
        }
    }

    ann
}

/// Parse a v2 field-declaration segment into `ann`.
///
/// v2 forms (no `optional` label by default):
///   `type = N`                              — e.g. `int32 = 5`
///   `type [packed=true] = N`               — e.g. `int32 [packed=true] = 7`
///   `required type = N`                    — explicit required
///   `repeated type = N`                    — explicit repeated
///   `repeated type [packed=true] = N`      — explicit repeated packed
///
/// Enum forms (type token contains `(`):
///   `EnumType(numeric) = N`                — scalar enum
///   `repeated EnumType(numeric) = N`       — repeated scalar enum
///   `EnumType([n1, n2]) [packed=true] = N` — packed enum
pub(crate) fn parse_field_decl_into<'a>(token: &'a str, ann: &mut Ann<'a>) {
    // If the token contains '(', it's an enum annotation.
    // Extract the type name, the parenthesised content, and then parse the rest.
    if let Some(paren_pos) = token.find('(') {
        parse_enum_field_decl(token, paren_pos, ann);
        return;
    }

    // Non-enum: original whitespace-split parse.
    // Bounded to 5 tokens — avoids Vec allocation.
    let mut it = token.split_ascii_whitespace();
    let Some(first) = it.next() else { return };
    let Some(second) = it.next() else { return };

    if second == "=" {
        // "type = N"  (v2 default: no label)
        let Some(n) = it.next() else { return };
        ann.field_type = first;
        ann.is_packed = false;
        ann.field_number = Some(parse_u64_str(n));
        return;
    }

    if second == "[packed=true]" {
        // "type [packed=true] = N"  (v2 default label, packed)
        let Some(eq_tok) = it.next() else { return };
        if eq_tok != "=" {
            return;
        }
        let Some(n) = it.next() else { return };
        ann.field_type = first;
        ann.is_packed = true;
        ann.field_number = Some(parse_u64_str(n));
        return;
    }

    // "label type [packed=true] = N"  (required / repeated)
    // first = label, second = type
    let Some(tok3) = it.next() else { return };
    let (is_packed, number_tok) = if tok3 == "=" {
        let Some(n) = it.next() else { return };
        (false, n)
    } else {
        // tok3 is "[packed=true]"
        let Some(eq_tok) = it.next() else { return };
        if eq_tok != "=" {
            return;
        }
        let Some(n) = it.next() else { return };
        (tok3 == "[packed=true]", n)
    };
    ann.field_type = second;
    ann.is_packed = is_packed;
    ann.field_number = Some(parse_u64_str(number_tok));
}

/// Parse an enum field declaration where the type token contains `(`.
///
/// Forms:
///   `EnumType(N) = field_num`
///   `repeated EnumType(N) = field_num`
///   `EnumType([n1, n2]) [packed=true] = field_num`
///   `repeated EnumType([n1, n2]) [packed=true] = field_num`
fn parse_enum_field_decl(token: &str, paren_pos: usize, ann: &mut Ann<'_>) {
    // Find the closing ')' scanning from paren_pos.
    let Some(close_paren) = token[paren_pos..].find(')') else {
        return;
    };
    let close_paren = paren_pos + close_paren;
    let inner = &token[paren_pos + 1..close_paren]; // content between '(' and ')'

    // Everything before the '(' is "[label ]TypeName".
    let before_paren = token[..paren_pos].trim();
    // Everything after ')' is "[ [packed=true]] = N".
    let after_paren = token[close_paren + 1..].trim();

    // Detect label prefix.
    let _is_repeated = before_paren.starts_with("repeated ");

    // Set field_type to "enum" unconditionally — routes through the varint path.
    ann.field_type = "enum";

    // Detect packed vs scalar from the inner content.
    if inner.starts_with('[') {
        // Packed enum: inner = "n1, n2, ..."  (after stripping '[' already stripped by `starts_with`).
        // Actually inner = "[n1, n2]" — the brackets are inside the parens.
        let list_inner = inner.trim_start_matches('[').trim_end_matches(']');
        ann.is_packed = true;
        ann.enum_packed_values = list_inner
            .split(',')
            .map(|s| s.trim().parse::<i64>().unwrap_or(0))
            .collect();
        // Parse "= N" from after_paren (skipping optional "[packed=true]").
        let field_num_str = if let Some(eq_pos) = after_paren.rfind('=') {
            after_paren[eq_pos + 1..].trim()
        } else {
            return;
        };
        ann.field_number = Some(parse_u64_str(field_num_str));
    } else {
        // Scalar enum: inner = "N".
        let n: i64 = inner.trim().parse::<i64>().unwrap_or(0);
        ann.enum_scalar_value = Some(n);
        // Detect [packed=true] in after_paren.
        ann.is_packed = after_paren.contains("[packed=true]");
        // Parse "= N" from after_paren.
        let field_num_str = if let Some(eq_pos) = after_paren.rfind('=') {
            after_paren[eq_pos + 1..].trim()
        } else {
            return;
        };
        ann.field_number = Some(parse_u64_str(field_num_str));
    }
}
