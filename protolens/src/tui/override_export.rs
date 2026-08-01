// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0156 G6c/G6d: turning a subtree into a descriptor's field list.
//!
//! One function with one caller (`:export --descriptor-*`), in a file of
//! its own because a four-tier walk that reads a subtree in order to
//! *describe* it has nothing in common with the splice that rewrites one.

use super::*;

use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};
use prost_reflect::Cardinality;

impl App {
    /// Builds the `ResolvedField` list for `export --descriptor-*` (spec
    /// 0156 G6c): one entry per distinct field_number among `idx`'s
    /// direct live-tree children, typed via `own_field_override` (tiers
    /// 1–2), the child's schema field (tier 3), or a wire-type-derived
    /// primitive guess (tier 4). `Err` only for an untyped
    /// `WT_START_GROUP` field, or an override target that doesn't
    /// resolve in `ctx.pool()`.
    pub(super) fn resolve_export_fields(
        &self,
        idx: usize,
    ) -> Result<Vec<export_descriptor::ResolvedField>, String> {
        use prototext_core::helpers::{WT_I32, WT_I64, WT_LEN, WT_START_GROUP, WT_VARINT};

        // Group direct children by field_number, keyed off the first
        // child seen for each number (G6c).
        struct Group {
            field_number: u32,
            first_child: usize,
            count: usize,
        }
        let mut groups: Vec<Group> = Vec::new();
        let mut child = self.first_child(idx);
        while let Some(c) = child {
            let field_number = self.tree[c].span.field_number;
            match groups.iter_mut().find(|g| g.field_number == field_number) {
                Some(g) => g.count += 1,
                None => groups.push(Group {
                    field_number,
                    first_child: c,
                    count: 1,
                }),
            }
            child = self.next_sibling(c);
        }

        let mut fields = Vec::with_capacity(groups.len());
        for g in groups {
            let child = g.first_child;
            let name = export_descriptor::synthetic_field_name(&self.field_name_for(child));
            let parent_field_desc = self.parent_field(child);
            let is_repeated = parent_field_desc
                .as_ref()
                .map(|f| f.cardinality() == Cardinality::Repeated)
                .unwrap_or(false)
                || g.count > 1;
            let label = if is_repeated {
                Label::Repeated
            } else {
                Label::Optional
            };

            let (r#type, type_name, referenced_file) =
                match self.own_field_override(idx, g.field_number.into()) {
                    // Tiers 1-2: an active PathField/FqdnField override.
                    Some(None) => (Type::Bytes, None, None),
                    Some(Some(fqdn)) => {
                        if let Some(desc) = self.ctx.pool().get_message_by_name(&fqdn) {
                            (
                                Type::Message,
                                Some(format!(".{fqdn}")),
                                Some(desc.parent_file()),
                            )
                        } else if let Some(desc) = self.ctx.pool().get_enum_by_name(&fqdn) {
                            (
                                Type::Enum,
                                Some(format!(".{fqdn}")),
                                Some(desc.parent_file()),
                            )
                        } else {
                            return Err(format!(
                                "export --descriptor: override target '{fqdn}' not found"
                            ));
                        }
                    }
                    // Tier 3: the child's own schema field (`natural_type`'s
                    // resolution, kept as a `Kind` here so the message/enum
                    // descriptor's own file is available for G6d's closure).
                    None => match parent_field_desc.map(|f| f.kind()) {
                        Some(prost_reflect::Kind::Message(desc)) => (
                            Type::Message,
                            Some(format!(".{}", desc.full_name())),
                            Some(desc.parent_file()),
                        ),
                        Some(prost_reflect::Kind::Enum(desc)) => (
                            Type::Enum,
                            Some(format!(".{}", desc.full_name())),
                            Some(desc.parent_file()),
                        ),
                        Some(prost_reflect::Kind::Double) => (Type::Double, None, None),
                        Some(prost_reflect::Kind::Float) => (Type::Float, None, None),
                        Some(prost_reflect::Kind::Int32) => (Type::Int32, None, None),
                        Some(prost_reflect::Kind::Int64) => (Type::Int64, None, None),
                        Some(prost_reflect::Kind::Uint32) => (Type::Uint32, None, None),
                        Some(prost_reflect::Kind::Uint64) => (Type::Uint64, None, None),
                        Some(prost_reflect::Kind::Sint32) => (Type::Sint32, None, None),
                        Some(prost_reflect::Kind::Sint64) => (Type::Sint64, None, None),
                        Some(prost_reflect::Kind::Fixed32) => (Type::Fixed32, None, None),
                        Some(prost_reflect::Kind::Fixed64) => (Type::Fixed64, None, None),
                        Some(prost_reflect::Kind::Sfixed32) => (Type::Sfixed32, None, None),
                        Some(prost_reflect::Kind::Sfixed64) => (Type::Sfixed64, None, None),
                        Some(prost_reflect::Kind::Bool) => (Type::Bool, None, None),
                        Some(prost_reflect::Kind::String) => (Type::String, None, None),
                        Some(prost_reflect::Kind::Bytes) => (Type::Bytes, None, None),
                        // Tier 4: no override, no schema — guess from the
                        // (first) child's own effective wire type (a
                        // packed-record child's effective wire type is its
                        // reconstructed WT_LEN framing).
                        None => {
                            let span = &self.tree[child].span;
                            let wire_type = if span.packed_record_start != NO_PACKED_RECORD {
                                WT_LEN
                            } else {
                                u32::from(span.wire_type)
                            };
                            match wire_type {
                                WT_VARINT => (Type::Int64, None, None),
                                WT_I32 => (Type::Fixed32, None, None),
                                WT_I64 => (Type::Fixed64, None, None),
                                WT_LEN => (Type::Bytes, None, None),
                                WT_START_GROUP => {
                                    return Err(format!(
                                        "export --descriptor: field {} is an untyped \
                                         group — not supported",
                                        g.field_number
                                    ));
                                }
                                _ => (Type::Bytes, None, None),
                            }
                        }
                    },
                };

            fields.push(export_descriptor::ResolvedField {
                number: g.field_number,
                name,
                label,
                r#type,
                type_name,
                referenced_file,
            });
        }

        Ok(fields)
    }
}
